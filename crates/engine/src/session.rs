//! One connection to a phone, from the first byte to the last frame.

use std::io::BufReader;
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use paircam_protocol::{
    Bye, CameraInfo, Capabilities, Configure, Error as ProtocolError, Hello, HostHandshake, Lens,
    Message, Mode, Opener, Position, Sealer, Status, UNPAIRED, VideoConfig, VideoFrame,
    random_bytes, read_record, write_record,
};

use crate::decoder::{Decoder, Nv12Frame, Scaler};
use crate::store::StreamPreferences;
use crate::{Event, Failure, Inner, LinkStats, Phase, PreviewFrame, VideoInfo, lock};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the user has to confirm the pairing code on the phone.
const PAIRING_TIMEOUT: Duration = Duration::from_secs(120);
/// The phone answers a ping every second, so this much silence means it is gone.
const SILENCE_TIMEOUT: Duration = Duration::from_secs(5);
const PING_INTERVAL: Duration = Duration::from_secs(1);
/// The preview is capped at 30 pictures a second; the virtual camera gets every one.
const PREVIEW_INTERVAL: Duration = Duration::from_millis(33);
const KEYFRAME_REQUEST_INTERVAL: Duration = Duration::from_secs(1);

fn now_us() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_micros() as u64
}

/// The sending half of a session: the socket and the cipher state for outgoing records.
struct Writer {
    stream: TcpStream,
    sealer: Sealer,
}

/// What the engine keeps to reach a running session from other threads.
pub(crate) struct Shared {
    pub(crate) id: u64,
    stopped: AtomicBool,
    stream: Mutex<Option<TcpStream>>,
    writer: Mutex<Option<Writer>>,
}

impl Shared {
    pub(crate) fn new(id: u64) -> Self {
        Self {
            id,
            stopped: AtomicBool::new(false),
            stream: Mutex::new(None),
            writer: Mutex::new(None),
        }
    }

    /// Sends a message if the session is past the handshake. Failures surface on the reading
    /// side as a lost connection, so they are only logged here.
    pub(crate) fn send(&self, message: &Message) {
        let mut writer = lock(&self.writer);
        let Some(writer) = writer.as_mut() else {
            return;
        };
        let mut body = message.encode();
        let result = writer
            .sealer
            .seal(&mut body)
            .and_then(|()| write_record(&mut writer.stream, &body));
        if let Err(error) = result {
            log::debug!("cannot send to the phone: {error}");
        }
    }

    /// Ends the session, saying goodbye first when a reason is given.
    pub(crate) fn stop(&self, reason: Option<&str>) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(reason) = reason {
            self.send(&Message::Bye(Bye {
                reason: reason.to_owned(),
            }));
        }
        if let Some(stream) = lock(&self.stream).as_ref() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

/// How one attempt ended.
enum End {
    /// The peer said goodbye or the user disconnected.
    Closed,
    Failed(Failure),
    /// Resuming did not work; the phone no longer knows this computer. Pair again.
    PairAgain,
}

impl From<ProtocolError> for End {
    fn from(error: ProtocolError) -> Self {
        End::Failed(match error {
            ProtocolError::Io(_) => Failure::Lost,
            ProtocolError::UnsupportedVersion(_)
            | ProtocolError::Refused(Status::UnsupportedVersion) => Failure::VersionMismatch,
            ProtocolError::Refused(Status::Busy) => Failure::Busy,
            other => Failure::Protocol(other.to_string()),
        })
    }
}

impl From<std::io::Error> for End {
    fn from(_: std::io::Error) -> Self {
        End::Failed(Failure::Lost)
    }
}

pub(crate) fn spawn(inner: Arc<Inner>, shared: Arc<Shared>, address: SocketAddr) {
    let result = thread::Builder::new()
        .name("paircam-session".into())
        .spawn(move || run(&inner, &shared, address));
    if let Err(error) = result {
        log::error!("cannot start the session thread: {error}");
    }
}

fn run(inner: &Arc<Inner>, shared: &Arc<Shared>, address: SocketAddr) {
    let mut mode = if lock(&inner.store).has_cameras() {
        Mode::Resume
    } else {
        Mode::Pair
    };

    let end = loop {
        match attempt(inner, shared, address, mode) {
            End::PairAgain if mode == Mode::Resume => mode = Mode::Pair,
            End::PairAgain => break End::Failed(Failure::Protocol("pairing was refused".into())),
            end => break end,
        }
    };

    *lock(&shared.writer) = None;
    if let Some(sink) = lock(&inner.sink).as_mut() {
        sink.stopped();
    }

    // A session the user replaced or closed has already been reported.
    if shared.is_stopped() || !inner.is_current(shared.id) {
        return;
    }
    inner.emit(Event::Phase(match end {
        End::Closed | End::PairAgain => Phase::Idle,
        End::Failed(failure) => Phase::Failed(failure),
    }));
}

fn attempt(inner: &Arc<Inner>, shared: &Arc<Shared>, address: SocketAddr, mode: Mode) -> End {
    match connect(inner, shared, address, mode) {
        Ok(end) | Err(end) => end,
    }
}

fn read_message(
    reader: &mut BufReader<TcpStream>,
    opener: &mut Opener,
) -> Result<Message, ProtocolError> {
    let mut record = read_record(reader)?;
    opener.open(&mut record)?;
    Message::decode(record)
}

fn connect(
    inner: &Arc<Inner>,
    shared: &Arc<Shared>,
    address: SocketAddr,
    mode: Mode,
) -> Result<End, End> {
    inner.emit(Event::Phase(Phase::Connecting { address }));

    let mut stream = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT)
        .map_err(|_| End::Failed(Failure::Unreachable))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    *lock(&shared.stream) = Some(stream.try_clone()?);
    if shared.is_stopped() {
        return Ok(End::Closed);
    }
    let mut reader = BufReader::with_capacity(256 * 1024, stream.try_clone()?);

    // The three plaintext records.
    let host_id = lock(&inner.store).host_id();
    let handshake = HostHandshake::new(host_id, mode, random_bytes());
    write_record(&mut stream, handshake.hello())?;

    let camera = match HostHandshake::read_camera_hello(&read_record(&mut reader)?) {
        Ok(camera) => camera,
        Err(ProtocolError::Refused(Status::NotPaired)) => return Ok(End::PairAgain),
        Err(error) => return Err(error.into()),
    };
    let pairing_key = match mode {
        Mode::Pair => UNPAIRED,
        Mode::Resume => match lock(&inner.store).pairing_key(camera.camera_id) {
            Some(key) => key,
            None => return Ok(End::PairAgain),
        },
    };
    let (host_key, keys) = handshake.finish(&camera, &pairing_key)?;
    write_record(&mut stream, &host_key)?;

    // Everything from here on is encrypted.
    let mut opener = Opener::new(&keys.camera_to_host);
    *lock(&shared.writer) = Some(Writer {
        stream,
        sealer: Sealer::new(&keys.host_to_camera),
    });
    shared.send(&Message::Hello(Hello {
        name: inner.host_name.clone(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        platform: std::env::consts::OS.into(),
        model: String::new(),
    }));

    let camera_hello = match read_message(&mut reader, &mut opener) {
        Ok(Message::Hello(hello)) => hello,
        Ok(_) => return Err(End::Failed(Failure::Protocol("expected hello".into()))),
        // A stale pairing key gives the two sides different keys.
        Err(ProtocolError::Decrypt) if mode == Mode::Resume => return Ok(End::PairAgain),
        Err(error) => return Err(error.into()),
    };

    if mode == Mode::Pair {
        inner.emit(Event::Phase(Phase::Pairing {
            camera_name: camera_hello.name.clone(),
            code: keys.pairing_code_digits(),
        }));
        reader.get_ref().set_read_timeout(Some(PAIRING_TIMEOUT))?;
        let accepted = loop {
            match read_message(&mut reader, &mut opener) {
                Ok(Message::PairResult(result)) => break result.accepted,
                Ok(Message::Bye(_)) | Err(ProtocolError::Io(_)) => break false,
                Ok(_) => {}
                Err(error) => return Err(error.into()),
            }
        };
        if !accepted {
            return Err(End::Failed(Failure::PairingDeclined));
        }
    }
    lock(&inner.store).remember(
        camera.camera_id,
        &camera_hello.name,
        (mode == Mode::Pair).then_some(&keys.pairing_key),
        address,
    );

    inner.emit(Event::Phase(Phase::Connected {
        camera_id: camera.camera_id,
        camera_name: camera_hello.name,
    }));
    reader.get_ref().set_read_timeout(Some(SILENCE_TIMEOUT))?;
    serve(inner, shared, &mut reader, &mut opener)
}

/// Counters the reading thread updates and the ticker thread reports once a second.
#[derive(Default)]
struct Counters {
    frames: AtomicU32,
    bytes: AtomicU64,
    decode_us: AtomicU64,
    /// Round trip of the last ping in microseconds, or 0 before the first pong.
    round_trip_us: AtomicU64,
}

fn serve(
    inner: &Arc<Inner>,
    shared: &Arc<Shared>,
    reader: &mut BufReader<TcpStream>,
    opener: &mut Opener,
) -> Result<End, End> {
    let counters = Arc::new(Counters::default());
    let ticking = Arc::new(AtomicBool::new(true));
    let ticker = spawn_ticker(
        inner.clone(),
        shared.clone(),
        counters.clone(),
        ticking.clone(),
    );

    let mut video = Video::new(counters.clone());
    let result = loop {
        let message = match read_message(reader, opener) {
            Ok(message) => message,
            Err(_) if shared.is_stopped() => break Ok(End::Closed),
            Err(error) => break Err(error.into()),
        };

        match message {
            Message::Capabilities(capabilities) => {
                let preferences = lock(&inner.store).preferences().clone();
                if let Some(configure) = choose_configuration(&capabilities, &preferences) {
                    shared.send(&Message::Configure(configure));
                    shared.send(&Message::StartStream);
                }
                inner.emit(Event::Capabilities(capabilities));
            }
            Message::VideoConfig(config) => video.configure(&config, inner),
            Message::VideoFrame(frame) => video.frame(&frame, inner, shared),
            Message::ControlState(state) => inner.emit(Event::ControlState(state)),
            Message::Stats(stats) => inner.emit(Event::CameraStats(stats)),
            Message::Log(entry) => inner.emit(Event::CameraLog(entry)),
            Message::Ping(timestamp) => shared.send(&Message::Pong(timestamp)),
            Message::Pong(timestamp) => {
                let round_trip = now_us().saturating_sub(timestamp).max(1);
                counters.round_trip_us.store(round_trip, Ordering::Relaxed);
            }
            Message::Bye(bye) => {
                log::info!("the phone closed the connection: {}", bye.reason);
                break Ok(End::Closed);
            }
            _ => {}
        }
    };

    ticking.store(false, Ordering::SeqCst);
    if let Some(ticker) = ticker {
        ticker.thread().unpark();
        let _ = ticker.join();
    }
    result
}

/// Pings the phone and reports link statistics once a second.
fn spawn_ticker(
    inner: Arc<Inner>,
    shared: Arc<Shared>,
    counters: Arc<Counters>,
    ticking: Arc<AtomicBool>,
) -> Option<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("paircam-ticker".into())
        .spawn(move || {
            let mut last = Instant::now();
            while ticking.load(Ordering::SeqCst) {
                thread::park_timeout(PING_INTERVAL);
                if !ticking.load(Ordering::SeqCst) {
                    break;
                }
                shared.send(&Message::Ping(now_us()));

                let elapsed = last.elapsed().as_secs_f32().max(0.001);
                last = Instant::now();
                let frames = counters.frames.swap(0, Ordering::Relaxed);
                let bytes = counters.bytes.swap(0, Ordering::Relaxed);
                let decode_us = counters.decode_us.swap(0, Ordering::Relaxed);
                let round_trip_us = counters.round_trip_us.load(Ordering::Relaxed);
                inner.emit(Event::Link(LinkStats {
                    fps: frames as f32 / elapsed,
                    bitrate: (bytes as f32 * 8.0 / elapsed) as u32,
                    decode_ms: if frames == 0 {
                        0.0
                    } else {
                        decode_us as f32 / frames as f32 / 1000.0
                    },
                    round_trip_ms: (round_trip_us > 0).then(|| round_trip_us as f32 / 1000.0),
                }));
            }
        })
        .ok()
}

/// Decoding state for the stream in a session.
struct Video {
    decoder: Option<Decoder>,
    scaler: Scaler,
    counters: Arc<Counters>,
    info: Option<VideoInfo>,
    /// After a decode error, frames are skipped until the next keyframe.
    waiting_for_keyframe: bool,
    last_keyframe_request: Option<Instant>,
    last_preview: Option<Instant>,
}

impl Video {
    fn new(counters: Arc<Counters>) -> Self {
        Self {
            decoder: None,
            scaler: Scaler::new(),
            counters,
            info: None,
            waiting_for_keyframe: false,
            last_keyframe_request: None,
            last_preview: None,
        }
    }

    fn configure(&mut self, config: &VideoConfig, inner: &Inner) {
        if config.rotation != 0 || config.mirrored {
            log::warn!("the phone asked the desktop to rotate or mirror, which is not supported");
        }
        self.waiting_for_keyframe = false;
        self.decoder = match Decoder::new(config.codec, &config.record, true) {
            Ok(decoder) => Some(decoder),
            Err(error) => {
                log::error!("cannot open a {:?} decoder: {error}", config.codec);
                None
            }
        };
        let info = VideoInfo {
            codec: config.codec,
            width: u32::from(config.width),
            height: u32::from(config.height),
            hardware_decoding: self.decoder.as_ref().is_some_and(Decoder::is_hardware),
        };
        self.info = Some(info.clone());
        inner.emit(Event::Video(info));
    }

    fn frame(&mut self, frame: &VideoFrame, inner: &Inner, shared: &Shared) {
        self.counters
            .bytes
            .fetch_add(frame.data.len() as u64, Ordering::Relaxed);
        if self.waiting_for_keyframe && !frame.keyframe {
            return;
        }
        let Some(decoder) = self.decoder.as_mut() else {
            return;
        };

        let started = Instant::now();
        let picture = match decoder.decode(&frame.data, frame.timestamp_us) {
            Ok(Some(picture)) => picture,
            Ok(None) => return,
            Err(error) => {
                log::warn!("cannot decode a frame: {error}");
                self.waiting_for_keyframe = true;
                let due = self
                    .last_keyframe_request
                    .is_none_or(|last| last.elapsed() >= KEYFRAME_REQUEST_INTERVAL);
                if due {
                    self.last_keyframe_request = Some(Instant::now());
                    shared.send(&Message::RequestKeyframe);
                }
                return;
            }
        };
        self.waiting_for_keyframe = false;
        self.counters.frames.fetch_add(1, Ordering::Relaxed);
        self.counters
            .decode_us
            .fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);

        if let Some(sink) = lock(&inner.sink).as_mut() {
            sink.frame(&picture);
        }

        let preview_due = self
            .last_preview
            .is_none_or(|last| last.elapsed() >= PREVIEW_INTERVAL);
        if preview_due {
            self.last_preview = Some(Instant::now());
            send_preview(&mut self.scaler, &picture, inner);
        }

        // The GPU can refuse a stream on the first picture, which flips this to software.
        let hardware = decoder.is_hardware();
        if let Some(info) = self
            .info
            .as_mut()
            .filter(|info| info.hardware_decoding != hardware)
        {
            info.hardware_decoding = hardware;
            inner.emit(Event::Video(info.clone()));
        }
    }
}

fn send_preview(scaler: &mut Scaler, picture: &Nv12Frame<'_>, inner: &Inner) {
    let max_width = inner.preview_max_width.load(Ordering::Relaxed);
    let (width, height) = fit_width(picture.width(), picture.height(), max_width);
    let mut bgra = Vec::new();
    match scaler.nv12_to_bgra(picture, width, height, &mut bgra) {
        Ok(()) => {
            let _ = inner.preview.force_send(PreviewFrame {
                width,
                height,
                bgra,
            });
        }
        Err(error) => log::warn!("cannot scale a preview picture: {error}"),
    }
}

/// Scales a size down to at most `max_width`, keeping the shape and even dimensions.
fn fit_width(width: u32, height: u32, max_width: u32) -> (u32, u32) {
    if width <= max_width {
        return (width, height);
    }
    let scaled_height = (u64::from(height) * u64::from(max_width) / u64::from(width)) as u32;
    (max_width & !1, scaled_height.max(2) & !1)
}

/// Picks what to ask the phone for: the preferred camera, size, rate and codec where the phone
/// supports them, and the nearest thing where it does not.
pub(crate) fn choose_configuration(
    capabilities: &Capabilities,
    preferences: &StreamPreferences,
) -> Option<Configure> {
    let camera = preferred_camera(&capabilities.cameras, preferences.camera.as_deref())?;

    let wanted_pixels = u64::from(preferences.width) * u64::from(preferences.height);
    let format = camera
        .formats
        .iter()
        .find(|format| format.width == preferences.width && format.height == preferences.height)
        .or_else(|| {
            // The largest format that is not larger than what was asked for.
            camera
                .formats
                .iter()
                .filter(|format| {
                    u64::from(format.width) * u64::from(format.height) <= wanted_pixels
                })
                .max_by_key(|format| u64::from(format.width) * u64::from(format.height))
        })
        .or_else(|| camera.formats.first())?;

    let codec = if capabilities.codecs.contains(&preferences.codec) {
        preferences.codec
    } else {
        *capabilities.codecs.first()?
    };

    Some(Configure {
        camera: camera.id.clone(),
        width: format.width,
        height: format.height,
        fps: preferences.fps.min(format.max_fps).max(1),
        codec,
        bitrate: preferences.bitrate,
        keyframe_interval: 2.0,
    })
}

fn preferred_camera<'a>(
    cameras: &'a [CameraInfo],
    preferred: Option<&str>,
) -> Option<&'a CameraInfo> {
    preferred
        .and_then(|id| cameras.iter().find(|camera| camera.id == id))
        .or_else(|| {
            cameras
                .iter()
                .find(|camera| camera.position == Position::Back && camera.lens == Lens::Wide)
        })
        .or_else(|| cameras.first())
}

#[cfg(test)]
mod tests {
    use paircam_protocol::{Codec, Format};

    use super::*;

    fn camera(id: &str, position: Position, lens: Lens, formats: &[(u32, u32, u32)]) -> CameraInfo {
        CameraInfo {
            id: id.into(),
            name: id.into(),
            position,
            lens,
            formats: formats
                .iter()
                .map(|&(width, height, max_fps)| Format {
                    width,
                    height,
                    max_fps,
                })
                .collect(),
        }
    }

    fn capabilities() -> Capabilities {
        Capabilities {
            cameras: vec![
                camera("front", Position::Front, Lens::Wide, &[(1280, 720, 30)]),
                camera(
                    "back-wide",
                    Position::Back,
                    Lens::Wide,
                    &[(1280, 720, 60), (1920, 1080, 60), (3840, 2160, 30)],
                ),
            ],
            codecs: vec![Codec::Hevc, Codec::H264],
            ..Default::default()
        }
    }

    #[test]
    fn defaults_to_the_back_wide_camera_at_the_preferred_size() {
        let configure =
            choose_configuration(&capabilities(), &StreamPreferences::default()).unwrap();
        assert_eq!(configure.camera, "back-wide");
        assert_eq!(
            (configure.width, configure.height, configure.fps),
            (1920, 1080, 30)
        );
        assert_eq!(configure.codec, Codec::Hevc);
    }

    #[test]
    fn falls_back_to_what_the_camera_supports() {
        let preferences = StreamPreferences {
            camera: Some("front".into()),
            width: 1920,
            height: 1080,
            fps: 60,
            ..Default::default()
        };
        let configure = choose_configuration(&capabilities(), &preferences).unwrap();
        assert_eq!(configure.camera, "front");
        assert_eq!(
            (configure.width, configure.height, configure.fps),
            (1280, 720, 30)
        );
    }

    #[test]
    fn uses_another_codec_when_the_preferred_one_is_missing() {
        let mut capabilities = capabilities();
        capabilities.codecs = vec![Codec::H264];
        let configure = choose_configuration(&capabilities, &StreamPreferences::default()).unwrap();
        assert_eq!(configure.codec, Codec::H264);
    }

    #[test]
    fn has_nothing_to_ask_for_without_cameras() {
        assert!(
            choose_configuration(&Capabilities::default(), &StreamPreferences::default()).is_none()
        );
    }

    #[test]
    fn preview_sizes_keep_their_shape() {
        assert_eq!(fit_width(1920, 1080, 1280), (1280, 720));
        assert_eq!(fit_width(1280, 720, 1280), (1280, 720));
        assert_eq!(fit_width(3840, 2160, 1280), (1280, 720));
        assert_eq!(fit_width(1080, 1920, 640), (640, 1136));
    }
}
