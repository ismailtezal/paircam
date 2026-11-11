//! The PairCam virtual camera, seen from the app.
//!
//! Windows loads the camera itself (`native/vcam`, a Media Foundation source DLL) into its Camera
//! Frame Server service whenever an app opens "PairCam". That service runs in session 0 as Local
//! Service, so the DLL fetches pictures from this process over a named pipe. This module is the
//! app's side of that arrangement:
//!
//! - [`VirtualCamera`] runs the pipe server, fits every decoded picture into the camera's fixed
//!   1920x1080 (letterboxed, never stretched) and reports whether any app is using the camera.
//! - [`register_with_windows`] makes the camera appear in apps while PairCam runs.
//! - [`is_installed`] says whether the camera DLL has been registered, which takes
//!   administrator rights once (`scripts/register-vcam.ps1`).
//!
//! The wire format is specified in `native/vcam/include/paircam_vcam.h`. The constants below
//! mirror it, and a unit test checks that they still match.

use std::ffi::c_void;
use std::mem::size_of;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, E_ACCESSDENIED, ERROR_ACCESS_DENIED, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED,
    ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree, REGDB_E_CLASSNOTREG, WAIT_OBJECT_0,
};
use windows::Win32::Media::MediaFoundation::{
    IMFAsyncCallback, IMFVirtualCamera, MF_VERSION, MFSTARTUP_LITE, MFShutdown, MFStartup,
    MFVirtualCameraAccess, MFVirtualCameraAccess_CurrentUser, MFVirtualCameraLifetime,
    MFVirtualCameraLifetime_Session, MFVirtualCameraType, MFVirtualCameraType_SoftwareCameraSource,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES,
    PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, REG_SZ, REG_VALUE_TYPE, RegCloseKey,
    RegOpenKeyExW, RegQueryValueExW,
};
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, INFINITE, OpenProcessToken, ResetEvent, SetEvent,
    WaitForMultipleObjects,
};
use windows::core::{GUID, HRESULT, HSTRING, Interface, PCWSTR, PWSTR, s, w};

use crate::decoder::{DecodeError, Nv12Frame, Nv12Planes, Nv12PlanesMut, Scaler};
use crate::{FrameSink, lock};

// -------------------------------------------------------------------------------------------------
// The contract with the DLL (native/vcam/include/paircam_vcam.h)
// -------------------------------------------------------------------------------------------------

/// The COM class of the camera's media source.
pub const CLSID: &str = "{81785F8A-A3B5-4F45-BC7B-F92794EB45BB}";
/// The name apps show for the camera. A product name, so never translated.
pub const FRIENDLY_NAME: &str = "PairCam";
/// The pipe the DLL connects to.
pub const PIPE_NAME: &str = r"\\.\pipe\PairCam.Camera";
/// The one picture size the camera offers. Every picture is fitted into it.
pub const WIDTH: u32 = 1920;
pub const HEIGHT: u32 = 1080;
/// The camera's frame rate. The DLL repeats the newest picture to keep this pace.
pub const FPS: u32 = 30;

const MAGIC: u32 = 0x4356_4350;
const VERSION: u16 = 1;
const TYPE_FRAME: u16 = 1;
const TYPE_IDLE: u16 = 2;
const TYPE_STATUS: u16 = 3;
const HEADER_SIZE: usize = 16;
const FRAME_INFO_SIZE: usize = 16;
const STATUS_SIZE: usize = 8;
const STATUS_MESSAGE_SIZE: usize = HEADER_SIZE + STATUS_SIZE;
const NV12_SIZE: usize = (WIDTH * HEIGHT * 3 / 2) as usize;
const LUMA_SIZE: usize = (WIDTH * HEIGHT) as usize;

/// Black in limited-range NV12.
const BLACK_LUMA: u8 = 16;
const NEUTRAL_CHROMA: u8 = 128;

/// A frame older than this is not sent to a camera that connects later: it would show a stale
/// picture until the DLL's own one-second timeout.
const FRESH_FOR: Duration = Duration::from_millis(500);
/// A consumer that does not take a frame within this time is considered hung and dropped.
const WRITE_TIMEOUT_MS: u32 = 2000;

mod wire {
    use super::*;

    pub(super) fn header(kind: u16, payload_len: u32) -> [u8; HEADER_SIZE] {
        let mut bytes = [0; HEADER_SIZE];
        bytes[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[4..6].copy_from_slice(&VERSION.to_le_bytes());
        bytes[6..8].copy_from_slice(&kind.to_le_bytes());
        bytes[8..12].copy_from_slice(&payload_len.to_le_bytes());
        bytes
    }

    /// The header and frame info that precede every picture's NV12 bytes.
    pub(super) fn frame_prefix(timestamp_us: u64) -> [u8; HEADER_SIZE + FRAME_INFO_SIZE] {
        let mut bytes = [0; HEADER_SIZE + FRAME_INFO_SIZE];
        bytes[..HEADER_SIZE]
            .copy_from_slice(&header(TYPE_FRAME, (FRAME_INFO_SIZE + NV12_SIZE) as u32));
        bytes[16..20].copy_from_slice(&WIDTH.to_le_bytes());
        bytes[20..24].copy_from_slice(&HEIGHT.to_le_bytes());
        bytes[24..32].copy_from_slice(&timestamp_us.to_le_bytes());
        bytes
    }

    pub(super) fn idle() -> [u8; HEADER_SIZE] {
        header(TYPE_IDLE, 0)
    }

    /// What the DLL reports about itself.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) struct Status {
        pub streaming: bool,
        /// The process hosting the DLL, normally the Frame Server.
        pub process_id: u32,
    }

    pub(super) fn parse_status(bytes: &[u8; STATUS_MESSAGE_SIZE]) -> Option<Status> {
        let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let valid = u32_at(0) == MAGIC
            && u16_at(4) == VERSION
            && u16_at(6) == TYPE_STATUS
            && u32_at(8) as usize == STATUS_SIZE;
        valid.then(|| Status {
            streaming: u32_at(16) != 0,
            process_id: u32_at(20),
        })
    }

    #[cfg(test)]
    pub(super) fn status(streaming: bool, process_id: u32) -> [u8; STATUS_MESSAGE_SIZE] {
        let mut bytes = [0; STATUS_MESSAGE_SIZE];
        bytes[..HEADER_SIZE].copy_from_slice(&header(TYPE_STATUS, STATUS_SIZE as u32));
        bytes[16..20].copy_from_slice(&u32::from(streaming).to_le_bytes());
        bytes[20..24].copy_from_slice(&process_id.to_le_bytes());
        bytes
    }
}

// -------------------------------------------------------------------------------------------------
// Fitting pictures into 1920x1080
// -------------------------------------------------------------------------------------------------

/// Where a picture lands inside the camera's frame. Every edge is even, as NV12 requires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// Fits a `source_width` x `source_height` picture into the camera frame, keeping its aspect
/// ratio: bars above and below a wider picture, at the sides of a taller one.
fn letterbox(source_width: u32, source_height: u32) -> Rect {
    let source_width = u64::from(source_width.max(1));
    let source_height = u64::from(source_height.max(1));
    let (target_width, target_height) = (u64::from(WIDTH), u64::from(HEIGHT));

    // Rounds a / b to the nearest even number in 2..=limit.
    let even = |a: u64, b: u64, limit: u64| ((a + b) / (2 * b) * 2).clamp(2, limit);

    let (width, height) = if source_width * target_height >= source_height * target_width {
        let height = even(source_height * target_width, source_width, target_height);
        (target_width, height)
    } else {
        let width = even(source_width * target_height, source_height, target_width);
        (width, target_height)
    };
    Rect {
        x: (((target_width - width) / 2) & !1) as u32,
        y: (((target_height - height) / 2) & !1) as u32,
        width: width as u32,
        height: height as u32,
    }
}

/// A decoded picture copied out of the decoder: tightly packed NV12 as
/// [`Nv12Frame::copy_packed`] lays it out.
struct Picture {
    width: u32,
    height: u32,
    timestamp_us: u64,
    data: Vec<u8>,
}

impl Picture {
    fn planes(&self) -> Nv12Planes<'_> {
        let luma = self.width as usize * self.height as usize;
        Nv12Planes {
            width: self.width,
            height: self.height,
            y: &self.data[..luma],
            y_stride: self.width as usize,
            uv: &self.data[luma..],
            uv_stride: self.width.div_ceil(2) as usize * 2,
        }
    }
}

/// Turns pictures of any size into camera frames.
struct Composer {
    // A scaler of its own: the preview's scaler converts to BGRA at another size.
    scaler: Scaler,
}

impl Composer {
    fn new() -> Self {
        Self {
            scaler: Scaler::new(),
        }
    }

    /// Fills `output` with `picture` fitted into 1920x1080. A picture that already has the
    /// camera's size is moved, not copied: `picture.data` and `output` swap buffers.
    fn compose(&mut self, picture: &mut Picture, output: &mut Vec<u8>) -> Result<(), DecodeError> {
        if picture.width == WIDTH && picture.height == HEIGHT && picture.data.len() == NV12_SIZE {
            std::mem::swap(output, &mut picture.data);
            return Ok(());
        }

        output.resize(NV12_SIZE, 0);
        let rect = letterbox(picture.width, picture.height);
        let (luma, chroma) = output.split_at_mut(LUMA_SIZE);
        if rect.width != WIDTH || rect.height != HEIGHT {
            luma.fill(BLACK_LUMA);
            chroma.fill(NEUTRAL_CHROMA);
        }

        let stride = WIDTH as usize;
        let y_offset = rect.y as usize * stride + rect.x as usize;
        let uv_offset = (rect.y / 2) as usize * stride + rect.x as usize;
        let target = Nv12PlanesMut {
            width: rect.width,
            height: rect.height,
            y: &mut luma[y_offset..],
            y_stride: stride,
            uv: &mut chroma[uv_offset..],
            uv_stride: stride,
        };
        self.scaler.nv12_to_nv12(picture.planes(), target)
    }
}

// -------------------------------------------------------------------------------------------------
// Small Win32 helpers
// -------------------------------------------------------------------------------------------------

/// A kernel handle closed on drop.
struct Handle(HANDLE);

// SAFETY: kernel handles may be used from any thread.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: the handle is owned and closed exactly once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

fn event(manual_reset: bool) -> windows::core::Result<Handle> {
    // SAFETY: plain event creation.
    unsafe { CreateEventW(None, manual_reset, false, None).map(Handle) }
}

fn is_pending(error: &windows::core::Error) -> bool {
    error.code() == ERROR_IO_PENDING.to_hresult()
}

/// A self-relative security descriptor allocated by Windows, freed on drop.
struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

// SAFETY: the descriptor is immutable after creation and only read by CreateNamedPipeW.
unsafe impl Send for SecurityDescriptor {}
unsafe impl Sync for SecurityDescriptor {}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe {
            LocalFree(Some(HLOCAL(self.0.0)));
        }
    }
}

/// The SID of the user this process runs as, as a string like `S-1-5-21-...`.
fn current_user_sid() -> windows::core::Result<String> {
    // SAFETY: the token handle is owned and closed; the buffer is sized by the first call and
    // TOKEN_USER is read from it while it lives.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let token = Handle(token);

        let mut size = 0u32;
        let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut size);
        // u64 elements keep the buffer aligned for TOKEN_USER.
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            size,
            &mut size,
        )?;
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);

        let mut text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text)?;
        let sid = text.to_string();
        LocalFree(Some(HLOCAL(text.0.cast())));
        sid.map_err(|_| windows::core::Error::from(ERROR_ACCESS_DENIED.to_hresult()))
    }
}

/// Who may connect to the pipe: the Frame Server's accounts and this user.
///
/// - `SY` Local System (Frame Server Monitor), `LS` Local Service (Frame Server): read and
///   write, which is all a client needs.
/// - `BA` Administrators and the current user: full control. The user needs it to create more
///   instances of the pipe for further connections.
///
/// `P` protects the list from inherited entries. Remote clients are refused separately with
/// PIPE_REJECT_REMOTE_CLIENTS.
fn pipe_security() -> windows::core::Result<SecurityDescriptor> {
    let sddl = format!(
        "D:P(A;;GRGW;;;SY)(A;;GRGW;;;LS)(A;;GA;;;BA)(A;;GA;;;{})",
        current_user_sid()?
    );
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: the string outlives the call; the result is owned by SecurityDescriptor.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            &HSTRING::from(sddl),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )?;
    }
    Ok(SecurityDescriptor(descriptor))
}

// -------------------------------------------------------------------------------------------------
// The pipe server
// -------------------------------------------------------------------------------------------------

/// Why the camera's pipe server could not start.
#[derive(Debug, thiserror::Error)]
pub enum PipeError {
    /// Another process (normally another PairCam) already serves the camera.
    #[error("another process already serves the PairCam camera")]
    AlreadyRunning,
    #[error("cannot create the camera pipe: {0}")]
    Os(#[from] windows::core::Error),
}

/// What the camera shows next.
#[derive(Clone)]
enum Content {
    Nothing,
    Frame {
        data: Arc<Vec<u8>>,
        timestamp_us: u64,
        published: Instant,
    },
    /// The phone stopped: the placeholder, right away.
    Idle,
}

struct Published {
    /// Increases with every change, so each connection sends each change once.
    sequence: u64,
    content: Content,
}

/// Buffers for decoded pictures waiting to be fitted into the camera frame.
#[derive(Default)]
struct Pending {
    /// The newest picture not yet composed. A newer one replaces it.
    picture: Option<Picture>,
    spare_inputs: Vec<Vec<u8>>,
}

type StreamingListener = Box<dyn Fn(bool) + Send>;

struct Inner {
    pipe_name: HSTRING,
    security: SecurityDescriptor,
    shutdown: Handle,
    shutting_down: AtomicBool,

    pending: Mutex<Pending>,
    pending_ready: Condvar,
    published: Mutex<Published>,
    spare_outputs: Mutex<Vec<Vec<u8>>>,

    /// Each connection's auto-reset event, set when new content is published.
    connections: Mutex<Vec<(u64, Arc<Handle>)>>,
    next_connection: AtomicU64,
    connected: AtomicUsize,
    streaming: AtomicUsize,
    listener: Mutex<Option<StreamingListener>>,

    connection_threads: Mutex<Vec<JoinHandle<()>>>,
}

impl Inner {
    fn create_instance(&self, first: bool) -> Result<Handle, PipeError> {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.security.0.0,
            bInheritHandle: false.into(),
        };
        let mut open_mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
        if first {
            // Fails if any other process already created the pipe, so a second PairCam (or an
            // impostor) cannot share or hijack the camera feed.
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        // SAFETY: the name and the descriptor outlive the call.
        let pipe = unsafe {
            CreateNamedPipeW(
                &self.pipe_name,
                FILE_FLAGS_AND_ATTRIBUTES(open_mode.0),
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                // Room for a whole frame, so a write rarely waits for the reader.
                (NV12_SIZE + HEADER_SIZE + FRAME_INFO_SIZE) as u32,
                4096,
                0,
                Some(&attributes),
            )
        };
        if pipe.is_invalid() {
            let error = windows::core::Error::from_win32();
            if first && error.code() == ERROR_ACCESS_DENIED.to_hresult() {
                return Err(PipeError::AlreadyRunning);
            }
            return Err(error.into());
        }
        Ok(Handle(pipe))
    }

    /// Hands a decoded picture to the composer thread. Cheap when nobody uses the camera.
    fn submit(&self, frame: &Nv12Frame<'_>) {
        if self.connected.load(Ordering::Acquire) == 0 || self.shutting_down.load(Ordering::Relaxed)
        {
            return;
        }
        let mut data = lock(&self.pending).spare_inputs.pop().unwrap_or_default();
        frame.copy_packed(&mut data);
        self.submit_picture(Picture {
            width: frame.width(),
            height: frame.height(),
            timestamp_us: frame.timestamp_us(),
            data,
        });
    }

    fn submit_picture(&self, picture: Picture) {
        let mut pending = lock(&self.pending);
        if let Some(older) = pending.picture.replace(picture) {
            pending.spare_inputs.push(older.data);
        }
        drop(pending);
        self.pending_ready.notify_one();
    }

    fn publish(&self, content: Content) {
        let replaced = {
            let mut published = lock(&self.published);
            published.sequence += 1;
            std::mem::replace(&mut published.content, content)
        };
        // Reuse the old frame's buffer once no connection is still sending it.
        if let Content::Frame { data, .. } = replaced
            && let Ok(buffer) = Arc::try_unwrap(data)
        {
            let mut spares = lock(&self.spare_outputs);
            if spares.len() < 2 {
                spares.push(buffer);
            }
        }
        for (_, event) in lock(&self.connections).iter() {
            // SAFETY: the event lives as long as its registration.
            unsafe {
                let _ = SetEvent(event.0);
            }
        }
    }

    fn idle(&self) {
        lock(&self.pending).picture = None;
        self.publish(Content::Idle);
    }

    fn change_streaming(&self, started: bool) {
        // Only the transitions between "nobody" and "somebody" matter to the interface.
        let transition = if started {
            self.streaming.fetch_add(1, Ordering::AcqRel) == 0
        } else {
            self.streaming.fetch_sub(1, Ordering::AcqRel) == 1
        };
        if transition && let Some(listener) = lock(&self.listener).as_ref() {
            listener(started);
        }
    }
}

/// Fits decoded pictures into camera frames, off the decoding thread.
fn compose_loop(inner: Arc<Inner>) {
    let mut composer = Composer::new();
    loop {
        let mut picture = {
            let mut pending = lock(&inner.pending);
            loop {
                if inner.shutting_down.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(picture) = pending.picture.take() {
                    break picture;
                }
                pending = inner
                    .pending_ready
                    .wait(pending)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };

        let mut output = lock(&inner.spare_outputs).pop().unwrap_or_default();
        match composer.compose(&mut picture, &mut output) {
            Ok(()) => inner.publish(Content::Frame {
                data: Arc::new(output),
                timestamp_us: picture.timestamp_us,
                published: Instant::now(),
            }),
            Err(error) => log::warn!("virtual camera: cannot scale a picture: {error}"),
        }

        let mut pending = lock(&inner.pending);
        if pending.spare_inputs.len() < 2 {
            pending.spare_inputs.push(picture.data);
        }
    }
}

/// Waits for camera DLLs to connect and serves each on a thread of its own.
fn accept_loop(inner: Arc<Inner>, mut instance: Handle) {
    let Ok(connected_event) = event(true) else {
        log::error!("virtual camera: cannot create an event");
        return;
    };

    loop {
        match wait_for_client(&inner, &instance, &connected_event) {
            Accept::Connected => {
                let next = loop {
                    match inner.create_instance(false) {
                        Ok(next) => break Some(next),
                        Err(error) => {
                            log::warn!("virtual camera: cannot create a pipe instance: {error}");
                            // SAFETY: waiting on our own event.
                            let wait =
                                unsafe { WaitForMultipleObjects(&[inner.shutdown.0], false, 1000) };
                            if wait == WAIT_OBJECT_0 {
                                break None;
                            }
                        }
                    }
                };
                let client = std::mem::replace(&mut instance, Handle(HANDLE::default()));
                let worker = inner.clone();
                let mut threads = lock(&inner.connection_threads);
                threads.retain(|thread| !thread.is_finished());
                threads.push(thread::spawn(move || serve(worker, client)));
                drop(threads);
                match next {
                    Some(next) => instance = next,
                    None => return,
                }
            }
            Accept::Retry => {
                // The client vanished between connecting and our wait: start over on a fresh
                // instance.
                // SAFETY: the instance is ours.
                unsafe {
                    let _ = DisconnectNamedPipe(instance.0);
                }
            }
            Accept::Shutdown => return,
        }
    }
}

enum Accept {
    Connected,
    Retry,
    Shutdown,
}

fn wait_for_client(inner: &Inner, instance: &Handle, connected_event: &Handle) -> Accept {
    // SAFETY: `overlapped` lives on this frame until the operation has completed or has been
    // cancelled and waited for.
    unsafe {
        let _ = ResetEvent(connected_event.0);
        let mut overlapped = OVERLAPPED {
            hEvent: connected_event.0,
            ..Default::default()
        };
        match ConnectNamedPipe(instance.0, Some(&mut overlapped)) {
            Ok(()) => return Accept::Connected,
            Err(error) if error.code() == ERROR_PIPE_CONNECTED.to_hresult() => {
                return Accept::Connected;
            }
            Err(error) if is_pending(&error) => {}
            Err(_) => return Accept::Retry,
        }

        let wait = WaitForMultipleObjects(&[inner.shutdown.0, connected_event.0], false, INFINITE);
        let mut transferred = 0;
        if wait == WAIT_OBJECT_0 {
            let _ = CancelIoEx(instance.0, Some(&overlapped));
            let _ = GetOverlappedResult(instance.0, &overlapped, &mut transferred, true);
            return Accept::Shutdown;
        }
        if GetOverlappedResult(instance.0, &overlapped, &mut transferred, false).is_ok() {
            Accept::Connected
        } else {
            Accept::Retry
        }
    }
}

/// Runs one connection from start to end.
fn serve(inner: Arc<Inner>, pipe: Handle) {
    let (Ok(frame_event), Ok(read_event), Ok(write_event)) =
        (event(false), event(true), event(true))
    else {
        return;
    };
    let frame_event = Arc::new(frame_event);
    let id = inner.next_connection.fetch_add(1, Ordering::Relaxed);
    lock(&inner.connections).push((id, frame_event.clone()));
    inner.connected.fetch_add(1, Ordering::AcqRel);

    let mut connection = Connection {
        inner: &inner,
        pipe: &pipe,
        frame_event: &frame_event,
        write_event: &write_event,
        read: Box::new(ReadState {
            overlapped: OVERLAPPED::default(),
            event: read_event,
            buffer: [0; STATUS_MESSAGE_SIZE],
            filled: 0,
            pending: false,
            empty_reads: 0,
        }),
        streaming: false,
        sent_sequence: 0,
    };
    connection.run();
    let was_streaming = connection.streaming;
    drop(connection);

    lock(&inner.connections).retain(|(other, _)| *other != id);
    inner.connected.fetch_sub(1, Ordering::AcqRel);
    if was_streaming {
        inner.change_streaming(false);
    }
    // SAFETY: the pipe is ours; no I/O is pending any more.
    unsafe {
        let _ = DisconnectNamedPipe(pipe.0);
    }
}

/// The pending status read. Boxed so its address stays fixed while Windows writes into it.
struct ReadState {
    overlapped: OVERLAPPED,
    event: Handle,
    buffer: [u8; STATUS_MESSAGE_SIZE],
    filled: usize,
    pending: bool,
    empty_reads: u32,
}

struct Connection<'a> {
    inner: &'a Inner,
    pipe: &'a Handle,
    frame_event: &'a Handle,
    write_event: &'a Handle,
    read: Box<ReadState>,
    streaming: bool,
    sent_sequence: u64,
}

impl Connection<'_> {
    fn run(&mut self) {
        // A camera that connects now gets the current picture only if it is recent.
        {
            let published = lock(&self.inner.published);
            let fresh = match &published.content {
                Content::Frame { published, .. } => published.elapsed() < FRESH_FOR,
                Content::Nothing | Content::Idle => false,
            };
            if !fresh {
                self.sent_sequence = published.sequence;
            }
        }

        let mut alive = self.start_read() && self.send_latest();
        while alive {
            let handles = [self.inner.shutdown.0, self.read.event.0, self.frame_event.0];
            // SAFETY: all three handles outlive the wait.
            let wait = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
            alive = match wait.0 {
                1 => self.finish_read() && self.start_read(),
                2 => self.send_latest(),
                _ => false, // shutdown, or a failed wait
            };
        }
        self.cancel_read();
    }

    fn start_read(&mut self) -> bool {
        let read = &mut *self.read;
        read.overlapped = OVERLAPPED {
            hEvent: read.event.0,
            ..Default::default()
        };
        // SAFETY: the buffer and OVERLAPPED are boxed and outlive the operation (cancel_read
        // waits for it before they are dropped).
        let result = unsafe {
            let _ = ResetEvent(read.event.0);
            ReadFile(
                self.pipe.0,
                Some(&mut read.buffer[read.filled..]),
                None,
                Some(&mut read.overlapped),
            )
        };
        // Completed or pending, the event is signalled when the data is there.
        read.pending = match result {
            Ok(()) => true,
            Err(error) => is_pending(&error),
        };
        read.pending
    }

    fn finish_read(&mut self) -> bool {
        let read = &mut *self.read;
        let mut transferred = 0;
        // SAFETY: the operation has completed (its event is set).
        let result =
            unsafe { GetOverlappedResult(self.pipe.0, &read.overlapped, &mut transferred, false) };
        read.pending = false;
        if result.is_err() {
            return false; // The DLL disconnected.
        }
        if transferred == 0 {
            read.empty_reads += 1;
            return read.empty_reads < 64;
        }
        read.empty_reads = 0;
        read.filled += transferred as usize;
        if read.filled < STATUS_MESSAGE_SIZE {
            return true;
        }
        read.filled = 0;

        let Some(status) = wire::parse_status(&read.buffer) else {
            log::warn!("virtual camera: unexpected message from the camera; disconnecting");
            return false;
        };
        if status.streaming != self.streaming {
            self.streaming = status.streaming;
            log::info!(
                "virtual camera: {} (process {})",
                if status.streaming {
                    "an app started using the camera"
                } else {
                    "an app stopped using the camera"
                },
                status.process_id
            );
            self.inner.change_streaming(status.streaming);
        }
        true
    }

    fn cancel_read(&mut self) {
        if self.read.pending {
            let mut transferred = 0;
            // SAFETY: cancel the read, then wait until Windows no longer touches the buffer.
            unsafe {
                let _ = CancelIoEx(self.pipe.0, Some(&self.read.overlapped));
                let _ =
                    GetOverlappedResult(self.pipe.0, &self.read.overlapped, &mut transferred, true);
            }
            self.read.pending = false;
        }
    }

    fn send_latest(&mut self) -> bool {
        let (sequence, content) = {
            let published = lock(&self.inner.published);
            (published.sequence, published.content.clone())
        };
        if sequence == self.sent_sequence {
            return true;
        }
        self.sent_sequence = sequence;
        match content {
            Content::Nothing => true,
            Content::Idle => self.write_all(&wire::idle()),
            Content::Frame {
                data, timestamp_us, ..
            } => self.write_all(&wire::frame_prefix(timestamp_us)) && self.write_all(&data),
        }
    }

    /// Writes all of `data`, or gives up on shutdown or when the camera stops reading.
    fn write_all(&mut self, mut data: &[u8]) -> bool {
        while !data.is_empty() {
            let mut overlapped = OVERLAPPED {
                hEvent: self.write_event.0,
                ..Default::default()
            };
            // SAFETY: `overlapped` and `data` stay alive until the write completed or was
            // cancelled and waited for.
            unsafe {
                let _ = ResetEvent(self.write_event.0);
                match WriteFile(self.pipe.0, Some(data), None, Some(&mut overlapped)) {
                    Ok(()) => {}
                    Err(error) if is_pending(&error) => {}
                    Err(_) => return false,
                }
                let handles = [self.inner.shutdown.0, self.write_event.0];
                let wait = WaitForMultipleObjects(&handles, false, WRITE_TIMEOUT_MS);
                let mut written = 0;
                if wait.0 != 1 {
                    let _ = CancelIoEx(self.pipe.0, Some(&overlapped));
                    let _ = GetOverlappedResult(self.pipe.0, &overlapped, &mut written, true);
                    if wait.0 != 0 {
                        log::warn!("virtual camera: a camera stopped reading; disconnecting");
                    }
                    return false;
                }
                if GetOverlappedResult(self.pipe.0, &overlapped, &mut written, false).is_err() {
                    return false;
                }
                data = &data[written as usize..];
            }
        }
        true
    }
}

/// The app's end of the virtual camera.
///
/// Create it once at startup and give [`VirtualCamera::sink`] to the engine with
/// `Engine::set_frame_sink`. It serves the camera until dropped.
pub struct VirtualCamera {
    inner: Arc<Inner>,
    threads: Vec<JoinHandle<()>>,
}

impl VirtualCamera {
    /// Starts serving the camera on its pipe.
    pub fn start() -> Result<Self, PipeError> {
        Self::start_on(PIPE_NAME)
    }

    fn start_on(pipe_name: &str) -> Result<Self, PipeError> {
        let inner = Arc::new(Inner {
            pipe_name: HSTRING::from(pipe_name),
            security: pipe_security()?,
            shutdown: event(true)?,
            shutting_down: AtomicBool::new(false),
            pending: Mutex::new(Pending::default()),
            pending_ready: Condvar::new(),
            published: Mutex::new(Published {
                sequence: 0,
                content: Content::Nothing,
            }),
            spare_outputs: Mutex::new(Vec::new()),
            connections: Mutex::new(Vec::new()),
            next_connection: AtomicU64::new(1),
            connected: AtomicUsize::new(0),
            streaming: AtomicUsize::new(0),
            listener: Mutex::new(None),
            connection_threads: Mutex::new(Vec::new()),
        });

        // The first instance is created here so that "already running" reaches the caller.
        let first = inner.create_instance(true)?;
        let accept = {
            let inner = inner.clone();
            thread::Builder::new()
                .name("paircam-vcam-accept".into())
                .spawn(move || accept_loop(inner, first))
                .map_err(|_| PipeError::Os(windows::core::Error::from_win32()))?
        };
        let compose = {
            let inner = inner.clone();
            thread::Builder::new()
                .name("paircam-vcam-compose".into())
                .spawn(move || compose_loop(inner))
                .map_err(|_| PipeError::Os(windows::core::Error::from_win32()))?
        };
        Ok(Self {
            inner,
            threads: vec![accept, compose],
        })
    }

    /// A handle for the engine. It never blocks the decoding thread: it copies the picture and
    /// returns, and does nothing at all while no app uses the camera.
    pub fn sink(&self) -> VirtualCameraSink {
        VirtualCameraSink {
            inner: self.inner.clone(),
        }
    }

    /// Whether any app is streaming from the camera right now ("in use").
    pub fn is_streaming(&self) -> bool {
        self.inner.streaming.load(Ordering::Acquire) > 0
    }

    /// Calls `listener` with `true` when the first app starts using the camera and with
    /// `false` when the last one stops. It runs on a camera thread and must return quickly.
    pub fn set_streaming_listener(&self, listener: impl Fn(bool) + Send + 'static) {
        *lock(&self.inner.listener) = Some(Box::new(listener));
    }
}

impl Drop for VirtualCamera {
    fn drop(&mut self) {
        self.inner.shutting_down.store(true, Ordering::Release);
        // SAFETY: our own event.
        unsafe {
            let _ = SetEvent(self.inner.shutdown.0);
        }
        // Take the lock so a composer between its check and its wait cannot miss the signal.
        drop(lock(&self.inner.pending));
        self.inner.pending_ready.notify_all();

        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
        let connections = std::mem::take(&mut *lock(&self.inner.connection_threads));
        for thread in connections {
            let _ = thread.join();
        }
    }
}

impl FrameSink for VirtualCamera {
    fn frame(&mut self, frame: &Nv12Frame<'_>) {
        self.inner.submit(frame);
    }

    fn stopped(&mut self) {
        self.inner.idle();
    }
}

/// The engine's handle to a [`VirtualCamera`]. Harmless after the camera is dropped.
pub struct VirtualCameraSink {
    inner: Arc<Inner>,
}

impl FrameSink for VirtualCameraSink {
    fn frame(&mut self, frame: &Nv12Frame<'_>) {
        self.inner.submit(frame);
    }

    fn stopped(&mut self) {
        self.inner.idle();
    }
}

// -------------------------------------------------------------------------------------------------
// Registration with Windows
// -------------------------------------------------------------------------------------------------

/// Why the camera could not be added to Windows.
#[derive(Debug, thiserror::Error)]
pub enum RegisterError {
    /// The camera DLL is not registered. The interface offers "Install camera", which runs
    /// `scripts/register-vcam.ps1` (once, with administrator rights).
    #[error("the PairCam camera component is not installed")]
    NotInstalled,
    /// Windows 10 and older have no virtual cameras.
    #[error("this version of Windows does not support virtual cameras")]
    Unsupported,
    /// Windows' camera service cannot read the DLL, usually because it is under a user
    /// profile instead of Program Files. Reinstalling the camera fixes it.
    #[error("the Windows camera service cannot read the PairCam camera component")]
    AccessDenied,
    #[error("Windows could not start the PairCam camera: {0}")]
    Failed(windows::core::Error),
}

impl RegisterError {
    fn from_windows(error: windows::core::Error) -> Self {
        let code = error.code();
        if code == REGDB_E_CLASSNOTREG {
            Self::NotInstalled
        } else if code == E_ACCESSDENIED {
            Self::AccessDenied
        } else {
            Self::Failed(error)
        }
    }
}

/// The virtual camera, visible to apps while this value lives. Dropping it removes the camera.
pub struct RegisteredCamera {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for RegisteredCamera {
    fn drop(&mut self) {
        // Closing the channel tells the owner thread to remove the camera.
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

type CreateVirtualCamera = unsafe extern "system" fn(
    MFVirtualCameraType,
    MFVirtualCameraLifetime,
    MFVirtualCameraAccess,
    PCWSTR,
    PCWSTR,
    *const GUID,
    u32,
    *mut *mut c_void,
) -> HRESULT;

/// Finds MFCreateVirtualCamera at run time, so the app still starts on systems without it.
fn create_virtual_camera_function() -> Option<CreateVirtualCamera> {
    // SAFETY: loading a system DLL by name and looking up a documented export; the signature
    // matches mfvirtualcamera.h.
    unsafe {
        let module = LoadLibraryW(w!("mfsensorgroup.dll")).ok()?;
        let function = GetProcAddress(module, s!("MFCreateVirtualCamera"))?;
        Some(std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            CreateVirtualCamera,
        >(function))
    }
}

/// Adds the PairCam camera to Windows for the signed-in user, for as long as the returned value
/// lives (it is also removed if the app exits without dropping it).
///
/// Media Foundation objects live on a thread of their own with a multithreaded COM apartment,
/// so this may be called from any thread, the interface thread included.
pub fn register_with_windows() -> Result<RegisteredCamera, RegisterError> {
    if !is_installed() {
        return Err(RegisterError::NotInstalled);
    }
    let create = create_virtual_camera_function().ok_or(RegisterError::Unsupported)?;

    let (result_sender, result_receiver) = mpsc::channel();
    let (stop, stopped) = mpsc::channel::<()>();
    let thread = thread::Builder::new()
        .name("paircam-vcam-owner".into())
        .spawn(move || own_camera(create, result_sender, stopped))
        .map_err(|_| RegisterError::Failed(windows::core::Error::from_win32()))?;

    match result_receiver.recv() {
        Ok(Ok(())) => Ok(RegisteredCamera {
            stop: Some(stop),
            thread: Some(thread),
        }),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => {
            let _ = thread.join();
            Err(RegisterError::Failed(windows::core::Error::from_win32()))
        }
    }
}

fn own_camera(
    create: CreateVirtualCamera,
    result: mpsc::Sender<Result<(), RegisterError>>,
    stopped: mpsc::Receiver<()>,
) {
    // SAFETY: COM and Media Foundation are initialised and torn down on this thread only, and
    // the camera object is released before MFShutdown.
    unsafe {
        let com = CoInitializeEx(None, COINIT_MULTITHREADED);
        if com.is_err() {
            let _ = result.send(Err(RegisterError::Failed(com.into())));
            return;
        }
        if let Err(error) = MFStartup(MF_VERSION, MFSTARTUP_LITE) {
            let _ = result.send(Err(RegisterError::Failed(error)));
            CoUninitialize();
            return;
        }

        let started = start_camera(create);
        match started {
            Ok(camera) => {
                let _ = result.send(Ok(()));
                // Wait until the RegisteredCamera is dropped (the channel closes).
                let _ = stopped.recv();
                // Remove, not Shutdown: Shutdown as well would shut the source down twice and
                // keep the camera from being removed (noted in VCamSample).
                if let Err(error) = camera.Remove() {
                    log::warn!("virtual camera: removing it failed: {error}");
                }
                drop(camera);
            }
            Err(error) => {
                let _ = result.send(Err(error));
            }
        }

        let _ = MFShutdown();
        CoUninitialize();
    }
}

/// # Safety
/// Call on a thread with COM and Media Foundation started.
unsafe fn start_camera(create: CreateVirtualCamera) -> Result<IMFVirtualCamera, RegisterError> {
    let name = HSTRING::from(FRIENDLY_NAME);
    let clsid = HSTRING::from(CLSID);
    let mut raw: *mut c_void = std::ptr::null_mut();
    // SAFETY: valid strings, no categories, and an out pointer for the interface.
    let camera = unsafe {
        create(
            MFVirtualCameraType_SoftwareCameraSource,
            MFVirtualCameraLifetime_Session,
            MFVirtualCameraAccess_CurrentUser,
            PCWSTR(name.as_ptr()),
            PCWSTR(clsid.as_ptr()),
            std::ptr::null(),
            0,
            &mut raw,
        )
        .ok()
        .map_err(RegisterError::from_windows)?;
        IMFVirtualCamera::from_raw(raw)
    };
    // SAFETY: a live camera object.
    unsafe { camera.Start(None::<&IMFAsyncCallback>) }.map_err(RegisterError::from_windows)?;
    Ok(camera)
}

/// Where the registered camera DLL is, if the camera is registered machine-wide.
pub fn installed_path() -> Option<PathBuf> {
    let key_path = HSTRING::from(format!(r"Software\Classes\CLSID\{CLSID}\InprocServer32"));
    // SAFETY: the key is closed before returning and the buffer is sized for the value.
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            &key_path,
            None,
            KEY_READ | KEY_WOW64_64KEY,
            &mut key,
        ) != ERROR_SUCCESS
        {
            return None;
        }
        let mut kind = REG_VALUE_TYPE::default();
        let mut buffer = [0u16; 1024];
        let mut size = (buffer.len() * 2) as u32;
        let status = RegQueryValueExW(
            key,
            PCWSTR::null(),
            None,
            Some(&mut kind),
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        );
        let _ = RegCloseKey(key);
        if status != ERROR_SUCCESS || kind != REG_SZ {
            return None;
        }
        let length = (size as usize / 2).min(buffer.len());
        let text = String::from_utf16_lossy(&buffer[..length]);
        Some(PathBuf::from(text.trim_end_matches('\0')))
    }
}

/// Whether the camera DLL is registered and present, so [`register_with_windows`] can work.
/// When false, the interface offers "Install camera".
pub fn is_installed() -> bool {
    installed_path().is_some_and(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::AtomicU32;

    use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_NONE, OPEN_EXISTING, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
    };

    /// Values of `#define NAME value` lines in the shared C header.
    fn header_defines() -> Vec<(String, String)> {
        include_str!("../../../native/vcam/include/paircam_vcam.h")
            .lines()
            .filter_map(|line| {
                let rest = line.strip_prefix("#define ")?;
                let (name, value) = rest.split_once(char::is_whitespace)?;
                let value = value.split("/*").next()?.trim();
                Some((name.to_string(), value.to_string()))
            })
            .collect()
    }

    fn define(name: &str) -> String {
        header_defines()
            .into_iter()
            .find(|(key, _)| key == name)
            .unwrap_or_else(|| panic!("{name} is missing from paircam_vcam.h"))
            .1
    }

    fn number(name: &str) -> u64 {
        let text = define(name);
        let text = text.trim_end_matches('u');
        match text.strip_prefix("0x") {
            Some(hex) => u64::from_str_radix(hex, 16).unwrap(),
            None => text.parse().unwrap(),
        }
    }

    fn string(name: &str) -> String {
        define(name).trim_matches('"').replace(r"\\", r"\")
    }

    #[test]
    fn constants_match_the_shared_header() {
        assert_eq!(string("PCVC_CLSID_STRING"), CLSID);
        assert_eq!(string("PCVC_FRIENDLY_NAME"), FRIENDLY_NAME);
        assert_eq!(string("PCVC_PIPE_NAME"), PIPE_NAME);
        assert_eq!(number("PCVC_MAGIC"), u64::from(MAGIC));
        assert_eq!(number("PCVC_VERSION"), u64::from(VERSION));
        assert_eq!(number("PCVC_TYPE_FRAME"), u64::from(TYPE_FRAME));
        assert_eq!(number("PCVC_TYPE_IDLE"), u64::from(TYPE_IDLE));
        assert_eq!(number("PCVC_TYPE_STATUS"), u64::from(TYPE_STATUS));
        assert_eq!(number("PCVC_WIDTH"), u64::from(WIDTH));
        assert_eq!(number("PCVC_HEIGHT"), u64::from(HEIGHT));
        assert_eq!(number("PCVC_FPS"), u64::from(FPS));
        assert_eq!(number("PCVC_HEADER_SIZE"), HEADER_SIZE as u64);
        assert_eq!(number("PCVC_FRAME_INFO_SIZE"), FRAME_INFO_SIZE as u64);
        assert_eq!(number("PCVC_STATUS_SIZE"), STATUS_SIZE as u64);
        // "PCVC" read as a little-endian u32.
        assert_eq!(&MAGIC.to_le_bytes(), b"PCVC");

        // The CLSID initialiser spells the same GUID as the string.
        let source = include_str!("../../../native/vcam/include/paircam_vcam.h");
        let init = &source[source.find("#define PCVC_CLSID_INIT").unwrap()..];
        let init = &init[..init.find("}}").unwrap()];
        let numbers: Vec<u128> = init
            .split("0x")
            .skip(1)
            .map(|part| {
                let digits: String = part.chars().take_while(char::is_ascii_hexdigit).collect();
                u128::from_str_radix(&digits, 16).unwrap()
            })
            .collect();
        let hex: String = CLSID.chars().filter(char::is_ascii_hexdigit).collect();
        let guid = GUID::from_u128(u128::from_str_radix(&hex, 16).unwrap());
        let mut expected = vec![
            u128::from(guid.data1),
            u128::from(guid.data2),
            u128::from(guid.data3),
        ];
        expected.extend(guid.data4.iter().map(|&byte| u128::from(byte)));
        assert_eq!(numbers, expected);
    }

    #[test]
    fn frame_prefix_layout() {
        let prefix = wire::frame_prefix(0x0102_0304_0506_0708);
        assert_eq!(&prefix[0..4], b"PCVC");
        assert_eq!(u16::from_le_bytes([prefix[4], prefix[5]]), 1);
        assert_eq!(u16::from_le_bytes([prefix[6], prefix[7]]), TYPE_FRAME);
        assert_eq!(
            u32::from_le_bytes(prefix[8..12].try_into().unwrap()) as usize,
            FRAME_INFO_SIZE + NV12_SIZE
        );
        assert_eq!(&prefix[12..16], &[0; 4]);
        assert_eq!(u32::from_le_bytes(prefix[16..20].try_into().unwrap()), 1920);
        assert_eq!(u32::from_le_bytes(prefix[20..24].try_into().unwrap()), 1080);
        assert_eq!(
            u64::from_le_bytes(prefix[24..32].try_into().unwrap()),
            0x0102_0304_0506_0708
        );
        let idle = wire::idle();
        assert_eq!(u16::from_le_bytes([idle[6], idle[7]]), TYPE_IDLE);
        assert_eq!(&idle[8..12], &[0; 4]);
    }

    #[test]
    fn status_round_trip_and_rejection() {
        let message = wire::status(true, 4242);
        assert_eq!(
            wire::parse_status(&message),
            Some(wire::Status {
                streaming: true,
                process_id: 4242
            })
        );
        let mut bad = message;
        bad[0] = b'X';
        assert_eq!(wire::parse_status(&bad), None);
        let mut newer = message;
        newer[4] = 2;
        assert_eq!(wire::parse_status(&newer), None);
    }

    #[test]
    fn letterbox_keeps_the_aspect_ratio() {
        let full = Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(letterbox(1920, 1080), full);
        assert_eq!(letterbox(1280, 720), full);
        assert_eq!(letterbox(3840, 2160), full);
        // Portrait phone video: bars at the sides.
        assert_eq!(
            letterbox(1080, 1920),
            Rect {
                x: 656,
                y: 0,
                width: 608,
                height: 1080
            }
        );
        // 4:3: bars at the sides.
        assert_eq!(
            letterbox(1440, 1080),
            Rect {
                x: 240,
                y: 0,
                width: 1440,
                height: 1080
            }
        );
        // Wider than 16:9: bars above and below.
        assert_eq!(
            letterbox(2560, 1080),
            Rect {
                x: 0,
                y: 134,
                width: 1920,
                height: 810
            }
        );
        // Degenerate sizes stay valid.
        let tiny = letterbox(1, 10_000);
        assert!(tiny.width >= 2 && tiny.width.is_multiple_of(2) && tiny.x.is_multiple_of(2));
        for (width, height) in [(1170, 2532), (1179, 2556), (1284, 2778), (3, 3), (7, 5)] {
            let rect = letterbox(width, height);
            assert!(rect.x + rect.width <= WIDTH && rect.y + rect.height <= HEIGHT);
            assert!(rect.x.is_multiple_of(2) && rect.y.is_multiple_of(2));
            assert!(rect.width.is_multiple_of(2) && rect.height.is_multiple_of(2));
        }
    }

    fn pattern(width: u32, height: u32) -> Picture {
        let luma = (width * height) as usize;
        let chroma = (width.div_ceil(2) * 2 * height.div_ceil(2)) as usize;
        let mut data = vec![0u8; luma + chroma];
        for (index, value) in data[..luma].iter_mut().enumerate() {
            // Bright content, far from black, so bars and picture are easy to tell apart.
            *value = 120 + (index % 100) as u8;
        }
        data[luma..].fill(90);
        Picture {
            width,
            height,
            timestamp_us: 7,
            data,
        }
    }

    #[test]
    fn compose_letterboxes_portrait_video() {
        let mut composer = Composer::new();
        let mut picture = pattern(1080, 1920);
        let mut output = Vec::new();
        composer.compose(&mut picture, &mut output).unwrap();
        assert_eq!(output.len(), NV12_SIZE);

        let rect = letterbox(1080, 1920);
        let row = 540 * WIDTH as usize;
        // Bars are black, the middle is picture.
        assert_eq!(output[row], BLACK_LUMA);
        assert_eq!(output[row + rect.x as usize - 1], BLACK_LUMA);
        assert!(output[row + 960] >= 110);
        assert_eq!(output[row + (rect.x + rect.width) as usize], BLACK_LUMA);
        assert_eq!(output[LUMA_SIZE + 270 * WIDTH as usize], NEUTRAL_CHROMA);
        assert!((output[LUMA_SIZE + 270 * WIDTH as usize + 960] as i32 - 90).abs() <= 2);
    }

    #[test]
    fn compose_passes_camera_sized_pictures_through() {
        let mut composer = Composer::new();
        let mut picture = pattern(WIDTH, HEIGHT);
        let expected = picture.data.clone();
        let mut output = Vec::new();
        composer.compose(&mut picture, &mut output).unwrap();
        assert_eq!(output, expected);
    }

    #[test]
    fn compose_scales_16_by_9_to_the_full_frame() {
        let mut composer = Composer::new();
        let mut picture = pattern(1280, 720);
        let mut output = Vec::new();
        composer.compose(&mut picture, &mut output).unwrap();
        // No bars: every corner is picture.
        assert!(output[0] >= 110);
        assert!(output[LUMA_SIZE - 1] >= 110);
    }

    fn unique_pipe_name() -> String {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        format!(
            r"\\.\pipe\PairCam.Camera.test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// A stand-in for the DLL: a plain synchronous pipe client.
    struct FakeCamera(Handle);

    impl FakeCamera {
        fn connect(name: &str) -> Self {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                // SAFETY: opening a pipe by name.
                let result = unsafe {
                    CreateFileW(
                        &HSTRING::from(name),
                        GENERIC_READ.0 | GENERIC_WRITE.0,
                        FILE_SHARE_NONE,
                        None,
                        OPEN_EXISTING,
                        SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                        None,
                    )
                };
                match result {
                    Ok(handle) => return Self(Handle(handle)),
                    Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
                    Err(error) => panic!("cannot connect to the camera pipe: {error}"),
                }
            }
        }

        fn write(&self, data: &[u8]) {
            let mut written = 0;
            // SAFETY: synchronous write from a live buffer.
            unsafe { WriteFile(self.0.0, Some(data), Some(&mut written), None) }.unwrap();
            assert_eq!(written as usize, data.len());
        }

        fn read_exact(&self, data: &mut [u8]) {
            let mut filled = 0;
            while filled < data.len() {
                let mut read = 0;
                // SAFETY: synchronous read into a live buffer.
                unsafe { ReadFile(self.0.0, Some(&mut data[filled..]), Some(&mut read), None) }
                    .unwrap();
                filled += read as usize;
            }
        }
    }

    fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn pipe_round_trip() {
        let name = unique_pipe_name();
        let camera = VirtualCamera::start_on(&name).unwrap();
        let changes = Arc::new(Mutex::new(Vec::new()));
        {
            let changes = changes.clone();
            camera.set_streaming_listener(move |streaming| lock(&changes).push(streaming));
        }

        // A second server on the same name is refused.
        assert!(matches!(
            VirtualCamera::start_on(&name),
            Err(PipeError::AlreadyRunning)
        ));

        let client = FakeCamera::connect(&name);
        assert!(wait_until(|| camera
            .inner
            .connected
            .load(Ordering::Acquire)
            == 1));
        assert!(!camera.is_streaming());
        client.write(&wire::status(true, 1234));
        assert!(wait_until(|| camera.is_streaming()));

        // A 16:9 picture arrives as a full camera frame, exactly what the composer makes.
        let picture = pattern(1280, 720);
        let mut expected = Vec::new();
        Composer::new()
            .compose(&mut pattern(1280, 720), &mut expected)
            .unwrap();
        camera.inner.submit_picture(picture);

        let mut prefix = [0u8; HEADER_SIZE + FRAME_INFO_SIZE];
        client.read_exact(&mut prefix);
        assert_eq!(prefix, wire::frame_prefix(7));
        let mut frame = vec![0u8; NV12_SIZE];
        client.read_exact(&mut frame);
        assert!(
            frame == expected,
            "the frame differs from the composed picture"
        );

        // The phone stops: the camera is told to show its placeholder.
        camera.sink().stopped();
        let mut idle = [0u8; HEADER_SIZE];
        client.read_exact(&mut idle);
        assert_eq!(idle, wire::idle());

        client.write(&wire::status(false, 1234));
        assert!(wait_until(|| !camera.is_streaming()));

        // Disconnecting while streaming also counts as "stopped".
        client.write(&wire::status(true, 1234));
        assert!(wait_until(|| camera.is_streaming()));
        drop(client);
        assert!(wait_until(|| !camera.is_streaming()));
        assert!(wait_until(|| camera
            .inner
            .connected
            .load(Ordering::Acquire)
            == 0));
        assert_eq!(*lock(&changes), vec![true, false, true, false]);

        // A second camera can connect after the first left; garbage gets it disconnected.
        let client = FakeCamera::connect(&name);
        assert!(wait_until(|| camera
            .inner
            .connected
            .load(Ordering::Acquire)
            == 1));
        client.write(&[0xAB; STATUS_MESSAGE_SIZE]);
        assert!(wait_until(|| camera
            .inner
            .connected
            .load(Ordering::Acquire)
            == 0));
        drop(client);

        // Dropping the camera stops every thread promptly.
        let started = Instant::now();
        drop(camera);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn pipe_security_names_the_current_user() {
        let sid = current_user_sid().unwrap();
        assert!(sid.starts_with("S-1-5-"), "{sid}");
        assert!(pipe_security().is_ok());
    }

    #[test]
    fn installation_check_does_not_fail() {
        // Not installed on a development machine; the call must simply answer.
        let _ = is_installed();
    }

    /// The real DLL against the real pipe server, both in user space (no registration).
    /// Build the DLL first with `native/vcam/build.ps1`, then run
    /// `cargo test -p paircam-engine vcam -- --ignored`. Uses the real pipe name, so PairCam
    /// itself must not be running.
    #[test]
    #[ignore = "needs target/vcam from native/vcam/build.ps1"]
    fn end_to_end_with_the_dll() {
        let harness = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/vcam/paircam_vcam_test.exe");
        assert!(
            harness.is_file(),
            "{} is missing; run native/vcam/build.ps1",
            harness.display()
        );

        let camera = VirtualCamera::start().unwrap();
        let changes = Arc::new(Mutex::new(Vec::new()));
        {
            let changes = changes.clone();
            camera.set_streaming_listener(move |streaming| lock(&changes).push(streaming));
        }

        // A flat grey portrait picture, sent continuously like a phone stream.
        let running = Arc::new(AtomicBool::new(true));
        let sender = {
            let inner = camera.inner.clone();
            let running = running.clone();
            thread::spawn(move || {
                while running.load(Ordering::Relaxed) {
                    let (width, height) = (1080u32, 1920u32);
                    let luma = (width * height) as usize;
                    let mut data = vec![200u8; luma + luma / 2];
                    data[luma..].fill(NEUTRAL_CHROMA);
                    inner.submit_picture(Picture {
                        width,
                        height,
                        timestamp_us: 0,
                        data,
                    });
                    thread::sleep(Duration::from_millis(30));
                }
            })
        };

        let status = std::process::Command::new(&harness)
            .arg("--external-app")
            .status()
            .unwrap();
        running.store(false, Ordering::Relaxed);
        sender.join().unwrap();

        assert!(status.success(), "the harness reported failures");
        assert!(wait_until(|| !camera.is_streaming()));
        // One "in use" period per format the harness tried.
        assert_eq!(*lock(&changes), vec![true, false, true, false]);
    }

    /// For trying the installed camera by hand before the app wires it up: adds "PairCam" to
    /// Windows and plays a moving test pattern for `PAIRCAM_VCAM_SECONDS` (default 120).
    /// Needs `scripts/register-vcam.ps1` to have run. Open the Camera app (or Teams, Chrome)
    /// meanwhile and pick PairCam:
    /// `cargo test -p paircam-engine manual_camera -- --ignored --nocapture`
    #[test]
    #[ignore = "adds a camera to Windows; run by hand"]
    fn manual_camera() {
        let seconds: u64 = std::env::var("PAIRCAM_VCAM_SECONDS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(120);
        let camera = VirtualCamera::start().unwrap();
        camera.set_streaming_listener(|streaming| {
            println!("camera in use: {streaming}");
        });
        let registered = register_with_windows().unwrap_or_else(|error| panic!("{error}"));
        println!("PairCam is in the camera list for {seconds} s");

        let started = Instant::now();
        let mut index = 0u32;
        while started.elapsed() < Duration::from_secs(seconds) {
            // Grey ramp with a bright bar sweeping across and a slowly cycling tint.
            let mut data = vec![0u8; NV12_SIZE];
            let bar = (index * 16) % WIDTH;
            for (offset, value) in data[..LUMA_SIZE].iter_mut().enumerate() {
                let (row, column) = (offset / WIDTH as usize, offset % WIDTH as usize);
                *value = if (column as u32).abs_diff(bar) < 24 {
                    235
                } else {
                    (16 + (column + row) * 200 / (WIDTH + HEIGHT) as usize) as u8
                };
            }
            let tint = (index % 256) as u8;
            for (offset, value) in data[LUMA_SIZE..].iter_mut().enumerate() {
                // U on even bytes, V on odd ones.
                *value = if offset % 2 == 0 { tint } else { 255 - tint };
            }
            camera.inner.submit_picture(Picture {
                width: WIDTH,
                height: HEIGHT,
                timestamp_us: u64::from(index) * 33_333,
                data,
            });
            index += 1;
            thread::sleep(Duration::from_millis(33));
        }
        drop(registered);
        println!("PairCam removed from the camera list");
    }
}
