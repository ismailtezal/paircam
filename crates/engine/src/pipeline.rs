//! Turns decoded pictures into what other apps and the preview see, on a thread of its own.
//!
//! The decoding thread only copies each picture out of the decoder ([`Pipeline::submit`]). The
//! picture thread then asks the [`Framer`] for this frame's crop, plans it, and lets the C layer
//! crop, scale, turn and mirror it in one pass:
//!
//! - while an app uses the virtual camera, into a 1920x1080 frame for the [`FrameSink`], and the
//!   preview is scaled down from that same frame;
//! - otherwise straight into a preview-sized frame with the same plan.
//!
//! Either way the frame is then graded with the chosen look and colour adjustments (inside the
//! picture, not the bars), so the preview shows exactly what other apps get. A newer picture
//! replaces one the thread has not started on, so delay never builds up.
//!
//! The thread also runs the away screen ([`crate::away`]): it hands a small copy of the
//! picture to the presence detector a few times a second and, while the person is away or the
//! user asked for it, cross-fades the finished frame to the away screen's picture.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use async_channel::Sender;

use crate::away::{AwayMedia, AwaySettings, AwayStatus, Fade, Watch, blend};
use crate::colour::{ColourSettings, ColourTables};
use crate::decoder::{Nv12Frame, Nv12Planes, Nv12PlanesMut, Scaler, apply_colour};
use crate::picture::{
    Framer, OUTPUT_HEIGHT, OUTPUT_WIDTH, PictureSettings, PixelRect, Rect, SubjectState, View, plan,
};
use crate::presence::{self, Detector};
use crate::screen::AwayScreen;
use crate::{CameraFrame, Event, FrameSink, PreviewFrame, lock};

/// The preview follows the stream, up to 60 pictures a second. The virtual camera gets every
/// picture regardless.
const PREVIEW_INTERVAL: Duration = Duration::from_millis(15);

/// A picture copied out of the decoder: tightly packed NV12.
pub(crate) struct Picture {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) timestamp_us: u64,
    pub(crate) data: Vec<u8>,
}

impl Picture {
    pub(crate) fn planes(&self) -> Nv12Planes<'_> {
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

/// A tightly packed NV12 buffer of a fixed size, reused frame after frame.
pub(crate) struct Canvas {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) data: Vec<u8>,
}

impl Canvas {
    pub(crate) fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            data: Vec::new(),
        }
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.data
            .resize(width as usize * height as usize * 3 / 2, 0);
    }

    /// Paints the whole picture one grey.
    pub(crate) fn fill(&mut self, luma: u8) {
        let luma_size = self.width as usize * self.height as usize;
        self.data[..luma_size].fill(luma);
        self.data[luma_size..].fill(128);
    }

    pub(crate) fn planes(&self) -> Nv12Planes<'_> {
        let luma = self.width as usize * self.height as usize;
        Nv12Planes {
            width: self.width,
            height: self.height,
            y: &self.data[..luma],
            y_stride: self.width as usize,
            uv: &self.data[luma..],
            uv_stride: self.width as usize,
        }
    }

    pub(crate) fn planes_mut(&mut self) -> Nv12PlanesMut<'_> {
        let luma = self.width as usize * self.height as usize;
        let (y, uv) = self.data.split_at_mut(luma);
        Nv12PlanesMut {
            width: self.width,
            height: self.height,
            y,
            y_stride: self.width as usize,
            uv,
            uv_stride: self.width as usize,
        }
    }
}

/// The colour stage's tables, rebuilt only when the colour settings change.
#[derive(Default)]
struct ColourStage {
    built_for: Option<ColourSettings>,
    /// `None` when the settings change nothing, so the frame is not touched at all.
    tables: Option<Box<ColourTables>>,
}

impl ColourStage {
    /// Grades the picture inside `content` (the bars around it stay black).
    fn apply(&mut self, settings: &ColourSettings, target: Nv12PlanesMut<'_>, content: PixelRect) {
        if self.built_for != Some(*settings) {
            self.tables = (!settings.is_identity()).then(|| ColourTables::build(settings));
            self.built_for = Some(*settings);
        }
        if let Some(tables) = &self.tables
            && let Err(error) = apply_colour(tables, target, content)
        {
            log::warn!("cannot grade a picture: {error}");
        }
    }
}

#[derive(Default)]
struct Pending {
    /// The newest picture not yet started on. A newer one replaces it.
    picture: Option<(u64, Picture)>,
    spare: Vec<Vec<u8>>,
}

/// How long the transforms took since the last report.
#[derive(Default)]
struct Timing {
    count: AtomicU32,
    total_us: AtomicU64,
}

/// The away screen as the picture thread needs it.
struct AwayControl {
    automatic: bool,
    delay: Duration,
    /// The user asked for the away screen by hand.
    forced: bool,
    screen: Arc<AwayScreen>,
    /// What the current screen was built from, so other changes do not rebuild it.
    built_for: (Option<PathBuf>, String),
}

/// What the interface was last told, and which screen may still report about its file.
#[derive(Default)]
struct AwayReport {
    status: AwayStatus,
    screen: u64,
}

struct Shared {
    events: Sender<Event>,
    away: Mutex<AwayControl>,
    away_report: Arc<Mutex<AwayReport>>,
    preview: Sender<PreviewFrame>,
    preview_max_width: AtomicU32,
    sink: Mutex<Option<Box<dyn FrameSink>>>,
    settings: Mutex<PictureSettings>,
    subject: Mutex<SubjectState>,
    /// The user is dragging the view: the crop follows the pointer without easing.
    panning: AtomicBool,
    /// What the output showed last, for turning pointer movement into a pan.
    view: Mutex<Option<View>>,
    pending: Mutex<Pending>,
    ready: Condvar,
    /// Increases when a stream ends, so pictures from before are not delivered after.
    generation: AtomicU64,
    last_preview: Mutex<Option<Instant>>,
    timing: Timing,
    shutting_down: AtomicBool,
}

impl Shared {
    fn wants_camera_frames(&self) -> bool {
        lock(&self.sink)
            .as_ref()
            .is_some_and(|sink| sink.wants_frames())
    }

    fn preview_due(&self) -> bool {
        !self.preview.is_full()
            && lock(&self.last_preview).is_none_or(|last| last.elapsed() >= PREVIEW_INTERVAL)
    }

    /// Changes what the interface knows about the away screen, and tells it if that differs.
    fn report_away(&self, change: impl FnOnce(&mut AwayStatus)) {
        report_away(&self.away_report, &self.events, change);
    }
}

fn report_away(
    report: &Mutex<AwayReport>,
    events: &Sender<Event>,
    change: impl FnOnce(&mut AwayStatus),
) {
    let mut report = lock(report);
    let before = report.status;
    change(&mut report.status);
    if report.status != before {
        // The interface owns the receiver for the whole run; a closed channel means shutdown.
        let _ = events.try_send(Event::Away(report.status));
    }
}

/// Builds the away screen for a file and a message. Its file's fate is reported as long as it
/// is still the current screen.
fn build_screen(
    media: Option<&Path>,
    message: &str,
    report: &Arc<Mutex<AwayReport>>,
    events: &Sender<Event>,
) -> Arc<AwayScreen> {
    let id = {
        let mut report = lock(report);
        report.screen += 1;
        report.screen
    };
    report_away(report, events, |status| status.media = AwayMedia::Card);
    let (report, events) = (report.clone(), events.clone());
    Arc::new(AwayScreen::new(media, message, move |state| {
        let current = lock(&report).screen == id;
        if current {
            report_away(&report, &events, |status| status.media = state);
        }
    }))
}

pub(crate) struct Pipeline {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Pipeline {
    pub(crate) fn new(
        preview: Sender<PreviewFrame>,
        events: Sender<Event>,
        settings: PictureSettings,
        away: &AwaySettings,
    ) -> Self {
        let away_report = Arc::new(Mutex::new(AwayReport::default()));
        // The app's default message arrives with the first `set_away`.
        let message = away.message.clone().unwrap_or_default();
        let screen = build_screen(away.media.as_deref(), &message, &away_report, &events);
        let shared = Arc::new(Shared {
            events,
            away: Mutex::new(AwayControl {
                automatic: away.automatic,
                delay: Duration::from_secs_f32(away.delay),
                forced: false,
                screen,
                built_for: (away.media.clone(), message),
            }),
            away_report,
            preview,
            preview_max_width: AtomicU32::new(1280),
            sink: Mutex::new(None),
            settings: Mutex::new(settings.normalized()),
            subject: Mutex::new(SubjectState::default()),
            panning: AtomicBool::new(false),
            view: Mutex::new(None),
            pending: Mutex::new(Pending::default()),
            ready: Condvar::new(),
            generation: AtomicU64::new(0),
            last_preview: Mutex::new(None),
            timing: Timing::default(),
            shutting_down: AtomicBool::new(false),
        });
        let thread = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("paircam-picture".into())
                .spawn(move || run(&shared))
        };
        let thread = match thread {
            Ok(thread) => Some(thread),
            Err(error) => {
                log::error!("cannot start the picture thread: {error}");
                None
            }
        };
        Self { shared, thread }
    }

    pub(crate) fn set_sink(&self, sink: Box<dyn FrameSink>) {
        *lock(&self.shared.sink) = Some(sink);
    }

    pub(crate) fn set_preview_max_width(&self, width: u32) {
        self.shared
            .preview_max_width
            .store(width, Ordering::Relaxed);
    }

    /// What the output showed last, or `None` before the first picture of a stream.
    pub(crate) fn view(&self) -> Option<View> {
        *lock(&self.shared.view)
    }

    pub(crate) fn settings(&self) -> PictureSettings {
        *lock(&self.shared.settings)
    }

    pub(crate) fn set_settings(&self, settings: PictureSettings) {
        *lock(&self.shared.settings) = settings.normalized();
    }

    /// The settings after dragging the view by `dx`, `dy` (fractions of the output frame). The
    /// crop follows without easing until [`Pipeline::end_pan`].
    pub(crate) fn pan(&self, dx: f32, dy: f32) -> PictureSettings {
        self.shared.panning.store(true, Ordering::Relaxed);
        let view = *lock(&self.shared.view);
        let mut settings = lock(&self.shared.settings);
        if let Some(view) = view {
            *settings = settings.panned(&view, dx, dy);
        }
        *settings
    }

    pub(crate) fn end_pan(&self) {
        self.shared.panning.store(false, Ordering::Relaxed);
    }

    /// Applies the away screen settings. `fallback_message` stands in when the user has not
    /// written a message of their own.
    pub(crate) fn set_away(&self, settings: &AwaySettings, fallback_message: &str) {
        let shared = &self.shared;
        let message = settings
            .message
            .clone()
            .unwrap_or_else(|| fallback_message.to_owned());
        let spec = (settings.media.clone(), message);
        let mut away = lock(&shared.away);
        away.automatic = settings.automatic;
        away.delay = Duration::from_secs_f32(settings.delay);
        if away.built_for != spec {
            let screen = build_screen(
                spec.0.as_deref(),
                &spec.1,
                &shared.away_report,
                &shared.events,
            );
            let old = std::mem::replace(&mut away.screen, screen);
            away.built_for = spec;
            drop(away);
            // Closing a file can take a moment, and the caller is the interface.
            let _ = thread::Builder::new()
                .name("paircam-away-close".into())
                .spawn(move || drop(old));
        }
    }

    /// Shows the away screen until told otherwise, whoever is in the picture.
    pub(crate) fn set_away_now(&self, on: bool) {
        lock(&self.shared.away).forced = on;
    }

    pub(crate) fn away_now(&self) -> bool {
        lock(&self.shared.away).forced
    }

    pub(crate) fn away_status(&self) -> AwayStatus {
        lock(&self.shared.away_report).status
    }

    /// Face tracking turned on or off on the phone.
    pub(crate) fn set_tracking(&self, enabled: bool) {
        let mut subject = lock(&self.shared.subject);
        subject.enabled = enabled;
        if !enabled {
            subject.face = None;
        }
    }

    /// The phone reported where the face is, or that nobody is in view. Reports only count
    /// while the phone's control state says face tracking is on, so one still in flight after
    /// the user turned it off cannot pull the crop back.
    pub(crate) fn set_face(&self, face: Option<Rect>) {
        let mut subject = lock(&self.shared.subject);
        subject.face = face;
        subject.reported = Some(Instant::now());
    }

    /// The average transform time since the last call, in milliseconds.
    pub(crate) fn take_transform_ms(&self) -> Option<f32> {
        let count = self.shared.timing.count.swap(0, Ordering::Relaxed);
        let total = self.shared.timing.total_us.swap(0, Ordering::Relaxed);
        (count > 0).then(|| total as f32 / count as f32 / 1000.0)
    }

    /// Takes a decoded picture. Called on the decoding thread: it copies the picture when the
    /// camera or the preview needs it and returns.
    pub(crate) fn submit(&self, frame: &Nv12Frame<'_>) {
        let shared = &self.shared;
        if !shared.wants_camera_frames() && !shared.preview_due() {
            return;
        }
        let generation = shared.generation.load(Ordering::Acquire);
        let mut data = lock(&shared.pending).spare.pop().unwrap_or_default();
        frame.copy_packed(&mut data);
        let picture = Picture {
            width: frame.width(),
            height: frame.height(),
            timestamp_us: frame.timestamp_us(),
            data,
        };
        let mut pending = lock(&shared.pending);
        if let Some((_, older)) = pending.picture.replace((generation, picture)) {
            pending.spare.push(older.data);
        }
        drop(pending);
        shared.ready.notify_one();
    }

    /// The stream ended: drop what is waiting, forget the face, and show the placeholder.
    pub(crate) fn stop(&self) {
        let shared = &self.shared;
        {
            let mut pending = lock(&shared.pending);
            shared.generation.fetch_add(1, Ordering::AcqRel);
            if let Some((_, picture)) = pending.picture.take() {
                pending.spare.push(picture.data);
            }
        }
        *lock(&shared.subject) = SubjectState::default();
        *lock(&shared.view) = None;
        // Nothing is watching or showing without a stream.
        lock(&shared.away).screen.set_playing(false);
        shared.report_away(|status| {
            status.showing = false;
            status.in_picture = None;
        });
        // Under the sink's lock, so a frame finished before this cannot land after it.
        if let Some(sink) = lock(&shared.sink).as_mut() {
            sink.stopped();
        }
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.shared.shutting_down.store(true, Ordering::Release);
        // Take the lock so the thread cannot miss the signal between its check and its wait.
        drop(lock(&self.shared.pending));
        self.shared.ready.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The size of the preview: the output's shape, at most `max_width` wide.
fn preview_size(max_width: u32) -> (u32, u32) {
    let width = max_width.clamp(160, OUTPUT_WIDTH) & !1;
    let height = (u64::from(width) * u64::from(OUTPUT_HEIGHT) / u64::from(OUTPUT_WIDTH)) as u32;
    (width, height.max(2) & !1)
}

/// The picture thread.
fn run(shared: &Shared) {
    let mut framer = Framer::new();
    // No stream yet: the first picture starts one.
    let mut framer_generation = u64::MAX;
    let mut detector: Option<Detector> = None;
    let mut watch = Watch::default();
    let mut fade = Fade::default();
    let mut last_sample: Option<Instant> = None;
    // One scaler per target size, so FFmpeg's contexts are not rebuilt every frame.
    let mut camera_scaler = Scaler::new();
    let mut preview_scaler = Scaler::new();
    let mut bgra_scaler = Scaler::new();
    let mut camera = Canvas::new();
    let mut preview = Canvas::new();
    let mut colour = ColourStage::default();

    loop {
        let (generation, picture) = {
            let mut pending = lock(&shared.pending);
            loop {
                if shared.shutting_down.load(Ordering::Acquire) {
                    return;
                }
                if let Some(picture) = pending.picture.take() {
                    break picture;
                }
                pending = shared
                    .ready
                    .wait(pending)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let new_stream = generation != framer_generation;
        if new_stream {
            framer = Framer::new();
            framer_generation = generation;
        }

        // The away screen: watch for the person, and ease towards it or back.
        let now = Instant::now();
        let (automatic, delay, forced, screen) = {
            let away = lock(&shared.away);
            (away.automatic, away.delay, away.forced, away.screen.clone())
        };
        if new_stream {
            watch = Watch::default();
            last_sample = None;
            if let Some(detector) = &detector {
                detector.reset();
            }
            // A stream that starts while the user is away by hand never shows them first.
            fade.jump(forced);
        }
        if automatic {
            if detector.is_none() {
                detector = Detector::start();
            }
            if let Some(detector) = &detector {
                if let Some((at, reading)) = detector.take_reading() {
                    watch.observe(at, &reading);
                }
                if last_sample.is_none_or(|last| now.duration_since(last) >= presence::INTERVAL)
                    && let Some(mut sample) = detector.begin()
                {
                    sample.fill(picture.planes());
                    detector.submit(now, sample);
                    last_sample = Some(now);
                }
            }
        } else if detector.take().is_some() {
            watch = Watch::default();
            last_sample = None;
        }
        let away_now = forced || (automatic && watch.step(now, delay));
        let mix = fade.step(now, away_now);
        screen.set_playing(away_now || mix > 0);
        shared.report_away(|status| {
            status.showing = away_now;
            status.in_picture = if automatic { watch.in_picture() } else { None };
        });

        let settings = *lock(&shared.settings);
        let subject = *lock(&shared.subject);
        let panning = shared.panning.load(Ordering::Relaxed);
        let source = (picture.width, picture.height);
        let crop = framer.step(Instant::now(), &settings, source, &subject, panning);

        let wants_camera = shared.wants_camera_frames();
        let preview_due = shared.preview_due();
        let started = Instant::now();
        let mut bgra = None;

        // The away screen is a camera-sized picture, so the preview is made from the camera
        // frame while any of it shows.
        if wants_camera || (mix > 0 && preview_due) {
            camera.resize(OUTPUT_WIDTH, OUTPUT_HEIGHT);
            let mut finished = false;
            if mix < 256 {
                let (plan, view) = plan(crop, &settings, source, (OUTPUT_WIDTH, OUTPUT_HEIGHT));
                *lock(&shared.view) = Some(view);
                match camera_scaler.transform_nv12(picture.planes(), &plan, camera.planes_mut()) {
                    Ok(()) => {
                        colour.apply(&settings.colour, camera.planes_mut(), plan.content);
                        finished = true;
                    }
                    Err(error) => log::warn!("cannot transform a picture: {error}"),
                }
            }
            if mix > 0 {
                screen.with_picture(|away| {
                    if finished {
                        blend(&mut camera.data, away, mix);
                    } else {
                        camera.data.copy_from_slice(away);
                    }
                });
                finished = true;
            }
            if finished {
                if wants_camera {
                    deliver(shared, generation, &picture, &camera);
                }
                if preview_due {
                    let (width, height) =
                        preview_size(shared.preview_max_width.load(Ordering::Relaxed));
                    let mut pixels = Vec::new();
                    match bgra_scaler.planes_to_bgra(camera.planes(), width, height, &mut pixels) {
                        Ok(()) => bgra = Some((width, height, pixels)),
                        Err(error) => log::warn!("cannot scale a preview picture: {error}"),
                    }
                }
            }
        } else if preview_due {
            let (width, height) = preview_size(shared.preview_max_width.load(Ordering::Relaxed));
            let (plan, view) = plan(crop, &settings, source, (width, height));
            *lock(&shared.view) = Some(view);
            preview.resize(width, height);
            let result = preview_scaler
                .transform_nv12(picture.planes(), &plan, preview.planes_mut())
                .and_then(|()| {
                    colour.apply(&settings.colour, preview.planes_mut(), plan.content);
                    let mut pixels = Vec::new();
                    bgra_scaler
                        .planes_to_bgra(preview.planes(), width, height, &mut pixels)
                        .map(|()| (width, height, pixels))
                });
            match result {
                Ok(frame) => bgra = Some(frame),
                Err(error) => log::warn!("cannot transform a preview picture: {error}"),
            }
        }

        // The transform time is about the live picture; the away screen would skew it.
        if (wants_camera || bgra.is_some()) && mix == 0 {
            shared
                .timing
                .total_us
                .fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
            shared.timing.count.fetch_add(1, Ordering::Relaxed);
        }
        if let Some((width, height, pixels)) = bgra
            && shared.generation.load(Ordering::Acquire) == generation
        {
            *lock(&shared.last_preview) = Some(Instant::now());
            let _ = shared.preview.force_send(PreviewFrame {
                width,
                height,
                bgra: pixels,
            });
        }

        let mut pending = lock(&shared.pending);
        if pending.spare.len() < 2 {
            pending.spare.push(picture.data);
        }
    }
}

/// Hands a finished camera frame to the sink, unless the stream it came from has ended.
fn deliver(shared: &Shared, generation: u64, picture: &Picture, camera: &Canvas) {
    let mut sink = lock(&shared.sink);
    if shared.generation.load(Ordering::Acquire) != generation {
        return;
    }
    if let Some(sink) = sink.as_mut() {
        sink.frame(&CameraFrame {
            width: camera.width,
            height: camera.height,
            timestamp_us: picture.timestamp_us,
            nv12: &camera.data,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::looks::LookId;
    use crate::picture::{FitMode, Rotation};

    /// A picture whose luma says where each pixel came from: the left half dark, the right
    /// half bright, and a mid-grey top band, so turns, mirrors and flips are easy to check.
    fn marked(width: u32, height: u32) -> Picture {
        let luma = (width * height) as usize;
        let mut data = vec![128u8; luma + luma / 2];
        for row in 0..height as usize {
            for column in 0..width as usize {
                data[row * width as usize + column] = if row < height as usize / 4 {
                    120
                } else if column < width as usize / 2 {
                    40
                } else {
                    220
                };
            }
        }
        Picture {
            width,
            height,
            timestamp_us: 0,
            data,
        }
    }

    fn render(picture: &Picture, settings: &PictureSettings, crop: Rect) -> Canvas {
        let mut canvas = Canvas::new();
        canvas.resize(OUTPUT_WIDTH, OUTPUT_HEIGHT);
        let (plan, _) = plan(
            crop,
            settings,
            (picture.width, picture.height),
            (OUTPUT_WIDTH, OUTPUT_HEIGHT),
        );
        Scaler::new()
            .transform_nv12(picture.planes(), &plan, canvas.planes_mut())
            .unwrap();
        canvas
    }

    fn luma(canvas: &Canvas, x: u32, y: u32) -> u8 {
        canvas.data[(y * canvas.width + x) as usize]
    }

    #[test]
    fn the_plain_transform_copies_the_picture() {
        let picture = marked(1920, 1080);
        let out = render(&picture, &PictureSettings::default(), Rect::FULL);
        assert_eq!(luma(&out, 100, 900), 40);
        assert_eq!(luma(&out, 1800, 900), 220);
        assert_eq!(luma(&out, 960, 50), 120);
    }

    #[test]
    fn mirror_and_flip_move_the_halves() {
        let picture = marked(1920, 1080);
        let mirrored = PictureSettings {
            mirror: true,
            ..Default::default()
        };
        let out = render(&picture, &mirrored, Rect::FULL);
        assert_eq!(luma(&out, 100, 900), 220);
        assert_eq!(luma(&out, 1800, 900), 40);

        let flipped = PictureSettings {
            flip: true,
            ..Default::default()
        };
        let out = render(&picture, &flipped, Rect::FULL);
        // The grey band is now at the bottom.
        assert_eq!(luma(&out, 960, 1060), 120);
        assert_eq!(luma(&out, 100, 100), 40);
    }

    #[test]
    fn mirroring_reverses_every_pixel_and_keeps_chroma_pairs_together() {
        let (width, height) = (1920u32, 1080u32);
        let luma_size = (width * height) as usize;
        let mut data = vec![0u8; luma_size + luma_size / 2];
        for row in 0..height as usize {
            for column in 0..width as usize {
                data[row * width as usize + column] = (column % 251) as u8;
            }
        }
        for (index, value) in data[luma_size..].iter_mut().enumerate() {
            let pair = (index % width as usize) / 2;
            // U then V, each telling which pair it belongs to.
            *value = if index % 2 == 0 {
                (pair % 200) as u8
            } else {
                255 - (pair % 200) as u8
            };
        }
        let picture = Picture {
            width,
            height,
            timestamp_us: 0,
            data,
        };
        let mirrored = PictureSettings {
            mirror: true,
            ..Default::default()
        };
        let out = render(&picture, &mirrored, Rect::FULL);
        for column in [0u32, 1, 7, 8, 15, 16, 959, 960, 1903, 1919] {
            assert_eq!(
                luma(&out, column, 500),
                ((width - 1 - column) % 251) as u8,
                "column {column}"
            );
        }
        let chroma = &out.data[luma_size + 100 * width as usize..];
        for pair in [0usize, 1, 3, 4, 479, 480, 959] {
            let source = 959 - pair;
            assert_eq!(chroma[2 * pair], (source % 200) as u8, "U of pair {pair}");
            assert_eq!(
                chroma[2 * pair + 1],
                255 - (source % 200) as u8,
                "V of pair {pair}"
            );
        }
    }

    #[test]
    fn a_quarter_turn_puts_the_top_on_the_right() {
        let picture = marked(1920, 1080);
        let turned = PictureSettings {
            rotation: Rotation::Clockwise90,
            fit: FitMode::Stretch,
            ..Default::default()
        };
        let out = render(&picture, &turned, Rect::FULL);
        // The source's top band is now a column on the right; its left half is the top.
        assert_eq!(luma(&out, 1900, 540), 120);
        assert_eq!(luma(&out, 500, 100), 40);
        assert_eq!(luma(&out, 500, 1000), 220);

        let back = PictureSettings {
            rotation: Rotation::Clockwise270,
            mirror: true,
            fit: FitMode::Stretch,
            ..Default::default()
        };
        // A counter-clockwise turn puts the band on the left; the mirror moves it right again.
        let out = render(&picture, &back, Rect::FULL);
        assert_eq!(luma(&out, 1900, 540), 120);
        assert_eq!(luma(&out, 500, 100), 220);
    }

    #[test]
    fn fit_mode_draws_black_bars() {
        let picture = marked(1080, 1920);
        let out = render(&picture, &PictureSettings::default(), Rect::FULL);
        assert_eq!(luma(&out, 10, 540), 16);
        assert_eq!(luma(&out, 1910, 540), 16);
        assert_eq!(luma(&out, 1100, 900), 220);
        // Neutral chroma in the bars.
        assert_eq!(out.data[(OUTPUT_WIDTH * OUTPUT_HEIGHT) as usize], 128);
    }

    #[test]
    fn zooming_crops_into_the_picture() {
        let picture = marked(1920, 1080);
        // The right half only: all bright below the band.
        let right = Rect {
            x: 0.5,
            y: 0.5,
            width: 0.5,
            height: 0.5,
        };
        let out = render(&picture, &PictureSettings::default(), right);
        assert_eq!(luma(&out, 10, 540), 220);
        assert_eq!(luma(&out, 1900, 1070), 220);
    }

    #[test]
    fn a_look_grades_the_picture_but_not_the_bars() {
        let picture = marked(1080, 1920);
        let settings = PictureSettings {
            colour: ColourSettings {
                look: LookId::Dusk,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut out = render(&picture, &settings, Rect::FULL);
        let (fixed, _) = plan(
            Rect::FULL,
            &settings,
            (picture.width, picture.height),
            (OUTPUT_WIDTH, OUTPUT_HEIGHT),
        );
        ColourStage::default().apply(&settings.colour, out.planes_mut(), fixed.content);
        // The bars stay limited-range black with neutral chroma...
        assert_eq!(luma(&out, 10, 540), 16);
        assert_eq!(out.data[(OUTPUT_WIDTH * OUTPUT_HEIGHT) as usize], 128);
        // ...while the picture is darker and its blacks lifted.
        assert!(luma(&out, 1100, 900) < 220);
        assert!(luma(&out, 900, 900) > 40);
    }

    #[test]
    fn no_look_leaves_the_frame_untouched() {
        let picture = marked(1920, 1080);
        let settings = PictureSettings::default();
        let mut out = render(&picture, &settings, Rect::FULL);
        let before = out.data.clone();
        let content = PixelRect {
            x: 0,
            y: 0,
            width: OUTPUT_WIDTH,
            height: OUTPUT_HEIGHT,
        };
        ColourStage::default().apply(&settings.colour, out.planes_mut(), content);
        assert!(out.data == before);
    }

    /// What the colour stage costs per 1920x1080 frame, and what rebuilding its tables costs.
    /// Run with
    /// `cargo test --release -p paircam-engine colour_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, not a check"]
    fn colour_cost() {
        let (width, height) = (OUTPUT_WIDTH, OUTPUT_HEIGHT);
        let luma_size = (width * height) as usize;
        let mut state = 0x2545_f491_u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        // Like a camera picture: smooth light and colour with a little sensor noise.
        let mut natural = vec![0u8; luma_size + luma_size / 2];
        for (index, value) in natural.iter_mut().enumerate() {
            let (x, y) = if index < luma_size {
                (index % width as usize, index / width as usize)
            } else {
                (
                    (index - luma_size) % width as usize,
                    (index - luma_size) / width as usize * 2,
                )
            };
            let (fx, fy) = (x as f32 / width as f32, y as f32 / height as f32);
            let smooth = if index < luma_size {
                30.0 + 180.0 * (0.5 + 0.5 * (fx * 7.0).sin() * (fy * 5.0).cos())
            } else if index % 2 == 0 {
                128.0 + 30.0 * (fx * 3.0 + fy * 4.0).sin()
            } else {
                128.0 + 30.0 * (fx * 5.0 - fy * 2.0).cos()
            };
            *value = (smooth + (next() % 7) as f32 - 3.0) as u8;
        }
        // The worst case: noise over a wide range of colour, so the chroma map is read all
        // over and many blocks need pulling back into RGB.
        let mut noisy = vec![0u8; luma_size + luma_size / 2];
        for (index, value) in noisy.iter_mut().enumerate() {
            let smooth = if index < luma_size {
                (index % width as usize) * 200 / width as usize + 20
            } else {
                (index % width as usize) * 120 / width as usize + 68
            };
            *value = (smooth as u32 + next() % 24) as u8;
        }
        let content = PixelRect {
            x: 0,
            y: 0,
            width,
            height,
        };
        for look in [LookId::Natural, LookId::Cinema, LookId::Silver] {
            let settings = ColourSettings {
                look,
                ..Default::default()
            };
            let rounds = 120;
            let started = Instant::now();
            for _ in 0..rounds {
                std::hint::black_box(ColourTables::build(&settings));
            }
            let build = started.elapsed().as_secs_f64() * 1000.0 / f64::from(rounds);

            let tables = ColourTables::build(&settings);
            let mut times = Vec::new();
            for data in [&natural, &noisy] {
                let mut canvas = Canvas {
                    width,
                    height,
                    data: data.clone(),
                };
                // Each frame timed alone, so the median and the best are not thrown off by
                // whatever else the computer is doing.
                let mut frames: Vec<f64> = (0..rounds)
                    .map(|_| {
                        canvas.data.copy_from_slice(data);
                        let started = Instant::now();
                        apply_colour(&tables, canvas.planes_mut(), content).unwrap();
                        started.elapsed().as_secs_f64() * 1000.0
                    })
                    .collect();
                frames.sort_by(f64::total_cmp);
                times.push((frames[frames.len() / 2], frames[0]));
            }
            println!(
                "{look:?}: {:.2} ms per 1080p frame like a camera's (best {:.2}), {:.2} ms for \
                 noise (best {:.2}); {build:.2} ms to build the tables",
                times[0].0, times[0].1, times[1].0, times[1].1
            );
        }
    }

    #[test]
    fn preview_sizes_keep_the_output_shape() {
        assert_eq!(preview_size(1280), (1280, 720));
        assert_eq!(preview_size(4000), (1920, 1080));
        assert_eq!(preview_size(641), (640, 360));
    }

    /// How long one transform takes, for the numbers in docs and reports. Run with
    /// `cargo test --release -p paircam-engine transform_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, not a check"]
    fn transform_cost() {
        let cases = [
            (
                "1080p, plain",
                (1920, 1080),
                PictureSettings::default(),
                Rect::FULL,
            ),
            (
                "1080p, mirrored and zoomed",
                (1920, 1080),
                PictureSettings {
                    mirror: true,
                    ..Default::default()
                },
                Rect {
                    x: 0.2,
                    y: 0.2,
                    width: 0.5,
                    height: 0.5,
                },
            ),
            (
                "1080p, turned 90",
                (1920, 1080),
                PictureSettings {
                    rotation: Rotation::Clockwise90,
                    ..Default::default()
                },
                Rect::FULL,
            ),
            (
                "4K, plain",
                (3840, 2160),
                PictureSettings::default(),
                Rect::FULL,
            ),
            (
                "4K, mirrored and zoomed",
                (3840, 2160),
                PictureSettings {
                    mirror: true,
                    ..Default::default()
                },
                Rect {
                    x: 0.2,
                    y: 0.2,
                    width: 0.5,
                    height: 0.5,
                },
            ),
            (
                "4K, turned 90",
                (3840, 2160),
                PictureSettings {
                    rotation: Rotation::Clockwise90,
                    ..Default::default()
                },
                Rect::FULL,
            ),
        ];
        for (name, (width, height), settings, crop) in cases {
            let picture = marked(width, height);
            let mut canvas = Canvas::new();
            canvas.resize(OUTPUT_WIDTH, OUTPUT_HEIGHT);
            let mut scaler = Scaler::new();
            let (fixed, _) = plan(
                crop,
                &settings,
                (width, height),
                (OUTPUT_WIDTH, OUTPUT_HEIGHT),
            );
            scaler
                .transform_nv12(picture.planes(), &fixed, canvas.planes_mut())
                .unwrap();
            let rounds = 120;
            let started = Instant::now();
            for _ in 0..rounds {
                scaler
                    .transform_nv12(picture.planes(), &fixed, canvas.planes_mut())
                    .unwrap();
            }
            let steady = started.elapsed().as_secs_f64() * 1000.0 / f64::from(rounds);

            // A crop that changes every frame, as while zooming or tracking.
            let started = Instant::now();
            for round in 0..rounds {
                let moving = Rect {
                    width: crop.width * (1.0 - round as f32 * 0.001),
                    height: crop.height * (1.0 - round as f32 * 0.001),
                    ..crop
                };
                let (plan, _) = plan(
                    moving,
                    &settings,
                    (width, height),
                    (OUTPUT_WIDTH, OUTPUT_HEIGHT),
                );
                scaler
                    .transform_nv12(picture.planes(), &plan, canvas.planes_mut())
                    .unwrap();
            }
            let moving = started.elapsed().as_secs_f64() * 1000.0 / f64::from(rounds);
            println!("{name}: {steady:.2} ms steady, {moving:.2} ms while the crop moves");
        }
    }
}
