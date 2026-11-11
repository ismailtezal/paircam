//! The PairCam desktop engine.
//!
//! It connects to a phone, pairs or resumes, receives and decodes the video, adjusts each
//! picture as the user asked (crop, zoom, rotation, mirror, face tracking; see [`picture`]),
//! and hands the result to the preview and to a [`FrameSink`] (the virtual camera). It runs on
//! its own threads and talks to the interface through channels: [`Event`]s, the latest
//! [`PreviewFrame`], and the [`PictureSettings`] whenever they change.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_channel::{Receiver, Sender};
pub use paircam_protocol as protocol;
use paircam_protocol::{
    Capabilities, Codec, ControlValues, DeviceId, Gesture, LogEntry, Message, Stats,
};

pub mod decoder;
pub mod discovery;
pub mod picture;
mod pipeline;
mod session;
mod store;
#[cfg(windows)]
pub mod vcam;

pub use decoder::Nv12Frame;
pub use discovery::DiscoveredCamera;
pub use picture::{FitMode, PictureSettings, Rotation};
pub use store::{PairedCamera, StreamPreferences};
#[cfg(windows)]
pub use vcam::{VirtualCamera, VirtualCameraSink};

/// Where the connection to a phone stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// Not connected.
    Idle,
    /// Reaching the phone and exchanging keys.
    Connecting { address: SocketAddr },
    /// Waiting for the user to confirm on the phone that `code` matches.
    Pairing { camera_name: String, code: String },
    /// Paired and talking. Video follows once [`Event::Video`] arrives.
    Connected {
        camera_id: DeviceId,
        camera_name: String,
    },
    /// The last attempt or session ended with a problem.
    Failed(Failure),
}

/// Why a connection attempt or session ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    /// Nothing answered at the address.
    Unreachable,
    /// The phone is streaming to another computer.
    Busy,
    /// The phone app and this app speak different protocol versions.
    VersionMismatch,
    /// The user tapped Decline on the phone, or pairing timed out.
    PairingDeclined,
    /// The connection dropped.
    Lost,
    /// The peer sent something this app could not make sense of.
    Protocol(String),
}

/// The stream being received.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoInfo {
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    /// Whether the GPU is decoding. False means software decoding.
    pub hardware_decoding: bool,
}

/// What the desktop measures about the stream, once a second.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkStats {
    /// Pictures decoded per second.
    pub fps: f32,
    /// Video bits received per second.
    pub bitrate: u32,
    /// Average time to decode one picture, in milliseconds.
    pub decode_ms: f32,
    /// Round trip of the last ping, in milliseconds.
    pub round_trip_ms: Option<f32>,
    /// Average time to crop, scale, turn and mirror one picture, in milliseconds.
    pub transform_ms: Option<f32>,
}

#[derive(Clone, Debug)]
pub enum Event {
    Phase(Phase),
    /// The phones currently advertising themselves on the network.
    Discovered(Vec<DiscoveredCamera>),
    Capabilities(Capabilities),
    ControlState(ControlValues),
    Video(VideoInfo),
    Link(LinkStats),
    /// Stats the phone reports about itself.
    CameraStats(Stats),
    /// A log line forwarded from the phone.
    CameraLog(LogEntry),
    /// The phone recognised a hand gesture and acted on it.
    Gesture(Gesture),
}

/// A picture for the preview: tightly packed BGRA, already scaled down. It has the camera
/// frame's 16:9 shape and shows exactly what other apps get, bars included.
pub struct PreviewFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// A finished camera picture: what other apps see.
pub struct CameraFrame<'a> {
    pub width: u32,
    pub height: u32,
    pub timestamp_us: u64,
    /// Tightly packed NV12: `width * height` bytes of luma, then the interleaved chroma.
    pub nv12: &'a [u8],
}

/// Receives every finished camera picture, [`picture::OUTPUT_WIDTH`] by
/// [`picture::OUTPUT_HEIGHT`]. The virtual camera implements this.
pub trait FrameSink: Send {
    /// Whether anyone takes pictures right now. While false, the engine does not make camera
    /// pictures at all, only the smaller preview.
    fn wants_frames(&self) -> bool;

    fn frame(&mut self, frame: &CameraFrame<'_>);

    /// The stream ended; show the placeholder again.
    fn stopped(&mut self);
}

/// The receiving ends the interface listens on.
pub struct Channels {
    pub events: Receiver<Event>,
    /// Holds at most one picture: a newer one replaces an unread older one.
    pub preview: Receiver<PreviewFrame>,
}

pub(crate) struct Inner {
    pub(crate) events: Sender<Event>,
    pub(crate) store: Mutex<store::Store>,
    pub(crate) pipeline: pipeline::Pipeline,
    pub(crate) host_name: String,
    /// Everyone who asked to hear about picture setting changes.
    picture_watchers: Mutex<Vec<Sender<PictureSettings>>>,
    session: Mutex<Option<Arc<session::Shared>>>,
    next_session_id: AtomicU64,
    /// Kept alive so the search for phones runs as long as the engine does.
    _discovery: Option<discovery::Discovery>,
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Inner {
    pub(crate) fn emit(&self, event: Event) {
        // The interface owns the receiver for the whole run; a closed channel means shutdown.
        let _ = self.events.try_send(event);
    }

    /// Whether `id` is still the session the engine is running.
    pub(crate) fn is_current(&self, id: u64) -> bool {
        lock(&self.session)
            .as_ref()
            .is_some_and(|session| session.id == id)
    }

    fn announce_picture(&self, settings: PictureSettings) {
        lock(&self.picture_watchers).retain(|watcher| watcher.try_send(settings).is_ok());
    }
}

#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

impl Engine {
    /// Creates the engine. `data_directory` holds its settings; `host_name` is the computer's
    /// name as the phone will show it.
    pub fn new(data_directory: &Path, host_name: impl Into<String>) -> (Self, Channels) {
        let (events, events_receiver) = async_channel::unbounded();
        let (preview, preview_receiver) = async_channel::bounded(1);
        let discovered = events.clone();
        let discovery =
            discovery::Discovery::start(paircam_protocol::SERVICE_TYPE, move |cameras| {
                let _ = discovered.try_send(Event::Discovered(cameras));
            });
        let store = store::Store::open(data_directory);
        let pipeline = pipeline::Pipeline::new(preview, *store.picture());
        let inner = Arc::new(Inner {
            events,
            store: Mutex::new(store),
            pipeline,
            host_name: host_name.into(),
            picture_watchers: Mutex::new(Vec::new()),
            session: Mutex::new(None),
            next_session_id: AtomicU64::new(1),
            _discovery: discovery,
        });
        (
            Self { inner },
            Channels {
                events: events_receiver,
                preview: preview_receiver,
            },
        )
    }

    /// This computer's permanent id.
    pub fn host_id(&self) -> DeviceId {
        lock(&self.inner.store).host_id()
    }

    /// Sends every finished camera picture to `sink`.
    pub fn set_frame_sink(&self, sink: Box<dyn FrameSink>) {
        self.inner.pipeline.set_sink(sink);
    }

    /// The widest preview picture to produce, in pixels. Larger streams are scaled down.
    pub fn set_preview_max_width(&self, width: u32) {
        self.inner.pipeline.set_preview_max_width(width.max(160));
    }

    /// How the picture is adjusted before other apps see it.
    pub fn picture(&self) -> PictureSettings {
        self.inner.pipeline.settings()
    }

    /// Changes the picture adjustments, saves them and tells every watcher. Zoom and pan
    /// changes ease into place.
    pub fn set_picture(&self, settings: PictureSettings) {
        let settings = settings.normalized();
        if settings == self.picture() {
            return;
        }
        self.inner.pipeline.set_settings(settings);
        lock(&self.inner.store).set_picture(settings);
        self.inner.announce_picture(settings);
    }

    /// Moves the zoomed view while the user drags it, by `dx` and `dy` as fractions of the
    /// frame. The picture follows the pointer exactly. Call [`Engine::end_pan`] on release.
    pub fn pan_picture(&self, dx: f32, dy: f32) {
        let settings = self.inner.pipeline.pan(dx, dy);
        self.inner.announce_picture(settings);
    }

    /// The drag ended: saves where the view was left.
    pub fn end_pan(&self) {
        self.inner.pipeline.end_pan();
        lock(&self.inner.store).set_picture(self.picture());
    }

    /// Hears about every change to the picture settings, whoever made it.
    pub fn watch_picture(&self) -> Receiver<PictureSettings> {
        let (sender, receiver) = async_channel::unbounded();
        lock(&self.inner.picture_watchers).push(sender);
        receiver
    }

    /// Connects to the phone at `address`, replacing any current connection. Progress arrives
    /// as [`Event::Phase`].
    pub fn connect(&self, address: SocketAddr) {
        self.disconnect();
        let id = self.inner.next_session_id.fetch_add(1, Ordering::Relaxed);
        let shared = Arc::new(session::Shared::new(id));
        *lock(&self.inner.session) = Some(shared.clone());
        session::spawn(self.inner.clone(), shared, address);
    }

    /// Ends the current connection, if any.
    pub fn disconnect(&self) {
        if let Some(session) = lock(&self.inner.session).take() {
            session.stop(Some("closed by the user"));
            self.inner.emit(Event::Phase(Phase::Idle));
        }
    }

    pub fn preferences(&self) -> StreamPreferences {
        lock(&self.inner.store).preferences().clone()
    }

    /// Saves the stream preferences and, when connected, applies them right away.
    pub fn set_preferences(&self, preferences: StreamPreferences, capabilities: &Capabilities) {
        lock(&self.inner.store).set_preferences(preferences.clone());
        if let Some(configure) = session::choose_configuration(capabilities, &preferences) {
            self.send(&Message::Configure(configure));
        }
    }

    /// Changes one or more camera controls.
    pub fn control(&self, values: ControlValues) {
        if values.face_tracking == Some(false) {
            // Ease back right away instead of waiting for the phone to confirm.
            self.inner.pipeline.set_tracking(false);
        }
        self.send(&Message::Control(values));
    }

    pub fn paired_cameras(&self) -> Vec<PairedCamera> {
        lock(&self.inner.store).cameras()
    }

    /// Forgets a paired camera. It will have to pair again.
    pub fn forget(&self, id: DeviceId) {
        lock(&self.inner.store).forget(id);
    }

    fn send(&self, message: &Message) {
        if let Some(session) = lock(&self.inner.session).as_ref() {
            session.send(message);
        }
    }
}
