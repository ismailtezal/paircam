//! Looks at the incoming picture a few times a second to tell whether someone is in it.
//!
//! The picture thread shrinks a picture to a small grey [`Sample`] and hands it to a thread of
//! its own, which looks for a face (with the detector built into Windows) and for movement
//! (cells of the picture whose brightness changed over the last second), and answers with a
//! [`Reading`]. [`crate::away::Watch`] turns the readings into "away" or "here".
//!
//! Movement matters because a face detector only sees faces that look at the camera: someone
//! writing in a notebook is still there. Movement near where the face was last seen counts for
//! more than movement elsewhere, so a fan in the corner does not stand in for a person.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::away::Reading;
use crate::decoder::Nv12Planes;
use crate::lock;

/// How often the picture is looked at.
pub(crate) const INTERVAL: Duration = Duration::from_millis(200);

/// The longer side of a sample, in pixels: enough for a face across a room, small enough to
/// cost next to nothing.
const SAMPLE_SIDE: u32 = 640;
/// A cell is this many sample pixels on each side.
const CELL: u32 = 8;
/// Movement is measured against the picture this long ago, so slow movement shows too.
const LOOK_BACK: Duration = Duration::from_millis(1000);
/// A cell changed when its brightness moved this far from the picture's overall drift.
const CELL_CHANGE: i32 = 8;
/// Where the person is taken to be, from the last face: this many face widths to each side
/// and face heights above, and everything below.
const BODY_SIDES: f32 = 1.0;
const BODY_ABOVE: f32 = 0.5;

/// A small grey copy of a picture.
#[derive(Default)]
pub(crate) struct Sample {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) luma: Vec<u8>,
}

impl Sample {
    /// Shrinks `picture` by a whole factor, averaging, into a sample whose sides are multiples
    /// of the cell size.
    pub(crate) fn fill(&mut self, picture: Nv12Planes<'_>) {
        let factor = picture
            .width
            .max(picture.height)
            .div_ceil(SAMPLE_SIDE)
            .max(1);
        self.width = picture.width / factor / CELL * CELL;
        self.height = picture.height / factor / CELL * CELL;
        self.luma.clear();
        self.luma
            .resize(self.width as usize * self.height as usize, 0);
        // Large factors read every other pixel of each box: plenty for an average.
        let step = if factor >= 4 { 2 } else { 1 };
        let taps = factor.div_ceil(step);
        let count = taps * taps;
        let factor = factor as usize;
        let step = step as usize;
        for (row, target) in self.luma.chunks_exact_mut(self.width as usize).enumerate() {
            for (column, value) in target.iter_mut().enumerate() {
                let mut sum = 0u32;
                for y in (0..factor).step_by(step) {
                    let start = (row * factor + y) * picture.y_stride + column * factor;
                    for x in (0..factor).step_by(step) {
                        sum += u32::from(picture.y[start + x]);
                    }
                }
                *value = (sum / count) as u8;
            }
        }
    }
}

/// A rectangle in sample pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FaceBox {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

/// The average brightness of every cell of a sample.
#[derive(Clone, Debug, PartialEq)]
struct Grid {
    columns: u32,
    rows: u32,
    means: Vec<u8>,
}

impl Grid {
    fn of(sample: &Sample) -> Self {
        let columns = sample.width / CELL;
        let rows = sample.height / CELL;
        let mut means = Vec::with_capacity((columns * rows) as usize);
        let width = sample.width as usize;
        let cell = CELL as usize;
        for row in 0..rows as usize {
            for column in 0..columns as usize {
                let mut sum = 0u32;
                for y in 0..cell {
                    let start = (row * cell + y) * width + column * cell;
                    sum += sample.luma[start..start + cell]
                        .iter()
                        .map(|&value| u32::from(value))
                        .sum::<u32>();
                }
                means.push((sum / (CELL * CELL)) as u8);
            }
        }
        Self {
            columns,
            rows,
            means,
        }
    }

    /// The share of cells that changed since `earlier`, in the whole picture and inside
    /// `body` (cell coordinates: left, top, right, bottom), leaving out a change every cell
    /// shares, such as the exposure settling.
    fn motion(&self, earlier: &Grid, body: Option<(u32, u32, u32, u32)>) -> (f32, f32) {
        if self.means.is_empty() || (self.columns, self.rows) != (earlier.columns, earlier.rows) {
            return (0.0, 0.0);
        }
        // The median change: what most of the picture did, even when a good part of it changed
        // for real.
        let mut changes: Vec<i16> = self
            .means
            .iter()
            .zip(&earlier.means)
            .map(|(&now, &then)| i16::from(now) - i16::from(then))
            .collect();
        let middle = changes.len() / 2;
        let drift = i32::from(*changes.select_nth_unstable(middle).1);

        let mut wide = 0u32;
        let mut near = 0u32;
        for (index, (&now, &then)) in self.means.iter().zip(&earlier.means).enumerate() {
            if (i32::from(now) - i32::from(then) - drift).abs() < CELL_CHANGE {
                continue;
            }
            wide += 1;
            if let Some((left, top, right, bottom)) = body {
                let column = index as u32 % self.columns;
                let row = index as u32 / self.columns;
                if (left..right).contains(&column) && (top..bottom).contains(&row) {
                    near += 1;
                }
            }
        }
        let near_share = body.map_or(0.0, |(left, top, right, bottom)| {
            let cells = (right - left) * (bottom - top);
            if cells == 0 {
                0.0
            } else {
                near as f32 / cells as f32
            }
        });
        (near_share, wide as f32 / self.means.len() as f32)
    }

    /// The cells where a person with this face is taken to be.
    fn body(&self, face: &FaceBox) -> (u32, u32, u32, u32) {
        let cell = CELL as f32;
        let clamp = |value: f32, most: u32| (value.max(0.0) as u32).min(most);
        let left = clamp((face.x - face.width * BODY_SIDES) / cell, self.columns);
        let right = clamp(
            ((face.x + face.width * (1.0 + BODY_SIDES)) / cell).ceil(),
            self.columns,
        );
        let top = clamp((face.y - face.height * BODY_ABOVE) / cell, self.rows);
        (left, top, right, self.rows)
    }
}

/// Turns samples into readings: remembers the last second of grids, and how the picture
/// looked when a face was last in it.
#[derive(Default)]
struct Observer {
    history: VecDeque<(Instant, Grid)>,
    /// The picture when a face was last seen, and the cells where that person was.
    with_face: Option<(Grid, (u32, u32, u32, u32))>,
}

impl Observer {
    fn observe(&mut self, at: Instant, sample: &Sample, face: Option<FaceBox>) -> Reading {
        let grid = Grid::of(sample);
        if self
            .history
            .back()
            .is_some_and(|(_, last)| (last.columns, last.rows) != (grid.columns, grid.rows))
        {
            // The stream changed size: nothing from before compares.
            self.history.clear();
            self.with_face = None;
        }
        if let Some(face) = face {
            self.with_face = Some((grid.clone(), grid.body(&face)));
        }
        let body = self.with_face.as_ref().map(|(_, body)| *body);
        // With no face in the picture: how much of where the person was looks different from
        // when their face was last seen. Someone who looked down is still mostly there;
        // someone who left has been replaced by the room behind them.
        let changed_since_face = match (&self.with_face, face) {
            (Some((then, body)), None) => Some(grid.motion(then, Some(*body)).0),
            _ => None,
        };

        while self
            .history
            .front()
            .is_some_and(|(then, _)| at.saturating_duration_since(*then) > LOOK_BACK)
            && self.history.len() > 1
        {
            self.history.pop_front();
        }
        let (near_motion, wide_motion) = self
            .history
            .front()
            .map_or((0.0, 0.0), |(_, earlier)| grid.motion(earlier, body));
        self.history.push_back((at, grid));

        Reading {
            face: face.is_some(),
            near_motion,
            wide_motion,
            changed_since_face,
        }
    }
}

#[derive(Default)]
struct Slot {
    sample: Option<(Instant, Sample)>,
    /// A buffer handed back for the next sample.
    spare: Option<Sample>,
    reading: Option<(Instant, Reading)>,
    busy: bool,
    /// Forget everything seen so far before the next sample.
    reset: bool,
}

struct Shared {
    slot: Mutex<Slot>,
    ready: Condvar,
    shutting_down: AtomicBool,
}

/// The thread that looks at samples.
pub(crate) struct Detector {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Detector {
    pub(crate) fn start() -> Option<Self> {
        let shared = Arc::new(Shared {
            slot: Mutex::new(Slot::default()),
            ready: Condvar::new(),
            shutting_down: AtomicBool::new(false),
        });
        let thread = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("paircam-presence".into())
                .spawn(move || run(&shared))
        };
        match thread {
            Ok(thread) => Some(Self {
                shared,
                thread: Some(thread),
            }),
            Err(error) => {
                log::error!("cannot start the presence thread: {error}");
                None
            }
        }
    }

    /// A buffer to fill when the detector is ready for another sample, or `None` while it is
    /// still looking at the last one.
    pub(crate) fn begin(&self) -> Option<Sample> {
        let mut slot = lock(&self.shared.slot);
        if slot.busy || slot.sample.is_some() {
            return None;
        }
        Some(slot.spare.take().unwrap_or_default())
    }

    /// Hands over a sample of the picture taken at `at`.
    pub(crate) fn submit(&self, at: Instant, sample: Sample) {
        lock(&self.shared.slot).sample = Some((at, sample));
        self.shared.ready.notify_one();
    }

    /// The newest reading not yet taken, with when its picture was taken.
    pub(crate) fn take_reading(&self) -> Option<(Instant, Reading)> {
        lock(&self.shared.slot).reading.take()
    }

    /// A new stream: nothing seen before applies.
    pub(crate) fn reset(&self) {
        let mut slot = lock(&self.shared.slot);
        slot.reset = true;
        slot.reading = None;
        if let Some((_, sample)) = slot.sample.take() {
            slot.spare = Some(sample);
        }
    }
}

impl Drop for Detector {
    fn drop(&mut self) {
        self.shared.shutting_down.store(true, Ordering::Release);
        drop(lock(&self.shared.slot));
        self.shared.ready.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(shared: &Shared) {
    let finder = faces::Finder::new();
    if finder.is_none() {
        log::warn!("face detection is not available; watching for movement only");
    }
    let mut observer = Observer::default();

    loop {
        let (at, sample) = {
            let mut slot = lock(&shared.slot);
            loop {
                if shared.shutting_down.load(Ordering::Acquire) {
                    return;
                }
                if slot.reset {
                    slot.reset = false;
                    observer = Observer::default();
                }
                if let Some(sample) = slot.sample.take() {
                    slot.busy = true;
                    break sample;
                }
                slot = shared
                    .ready
                    .wait(slot)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };

        let face = finder.as_ref().and_then(|finder| finder.find(&sample));
        // The first look has nothing to compare with: without a face it cannot tell whether
        // anyone is there, so it says nothing.
        let telling = face.is_some() || !observer.history.is_empty();
        let reading = observer.observe(at, &sample, face);
        log::trace!(
            "presence: face {}, movement {:.4} near and {:.4} overall, {:?} changed since the face",
            reading.face,
            reading.near_motion,
            reading.wide_motion,
            reading.changed_since_face
        );

        let mut slot = lock(&shared.slot);
        slot.busy = false;
        slot.spare = Some(sample);
        if !slot.reset && telling {
            slot.reading = Some((at, reading));
        }
    }
}

/// Face detection with the detector built into Windows.
#[cfg(windows)]
pub(crate) mod faces {
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::FaceAnalysis::FaceDetector;
    use windows::Security::Cryptography::CryptographicBuffer;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

    use super::{FaceBox, Sample};

    pub(crate) struct Finder {
        detector: FaceDetector,
    }

    impl Finder {
        /// Call on the thread that will use it.
        pub(crate) fn new() -> Option<Self> {
            // SAFETY: plain COM initialisation for this thread; a thread that already joined
            // an apartment keeps it.
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            let create = || -> windows::core::Result<Option<FaceDetector>> {
                if !FaceDetector::IsSupported()?
                    || !FaceDetector::IsBitmapPixelFormatSupported(BitmapPixelFormat::Gray8)?
                {
                    return Ok(None);
                }
                Ok(Some(FaceDetector::CreateAsync()?.get()?))
            };
            match create() {
                Ok(detector) => detector.map(|detector| Self { detector }),
                Err(error) => {
                    log::warn!("cannot start face detection: {error}");
                    None
                }
            }
        }

        /// The largest face in the sample, if any.
        pub(crate) fn find(&self, sample: &Sample) -> Option<FaceBox> {
            let find = || -> windows::core::Result<Option<FaceBox>> {
                let buffer = CryptographicBuffer::CreateFromByteArray(&sample.luma)?;
                let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
                    &buffer,
                    BitmapPixelFormat::Gray8,
                    sample.width as i32,
                    sample.height as i32,
                )?;
                let faces = self.detector.DetectFacesAsync(&bitmap)?.get()?;
                let mut largest: Option<FaceBox> = None;
                for face in faces {
                    let bounds = face.FaceBox()?;
                    let found = FaceBox {
                        x: bounds.X as f32,
                        y: bounds.Y as f32,
                        width: bounds.Width as f32,
                        height: bounds.Height as f32,
                    };
                    if largest
                        .is_none_or(|best| found.width * found.height > best.width * best.height)
                    {
                        largest = Some(found);
                    }
                }
                Ok(largest)
            };
            if sample.width == 0 || sample.height == 0 {
                return None;
            }
            match find() {
                Ok(face) => face,
                Err(error) => {
                    log::debug!("face detection failed on a picture: {error}");
                    None
                }
            }
        }
    }
}

#[cfg(not(windows))]
pub(crate) mod faces {
    use super::{FaceBox, Sample};

    pub(crate) struct Finder;

    impl Finder {
        pub(crate) fn new() -> Option<Self> {
            None
        }

        pub(crate) fn find(&self, _sample: &Sample) -> Option<FaceBox> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey(width: u32, height: u32, value: u8) -> Sample {
        Sample {
            width,
            height,
            luma: vec![value; (width * height) as usize],
        }
    }

    /// Paints a bright block of `size` sample pixels at (`x`, `y`).
    fn paint(sample: &mut Sample, x: u32, y: u32, size: u32) {
        for row in y..y + size {
            for column in x..x + size {
                sample.luma[(row * sample.width + column) as usize] = 220;
            }
        }
    }

    #[test]
    fn a_sample_averages_whole_boxes() {
        let (width, height) = (1920u32, 1080u32);
        let mut y = vec![0u8; (width * height) as usize];
        // The left half bright, the right half dark.
        for row in 0..height as usize {
            y[row * width as usize..row * width as usize + 960].fill(200);
        }
        let uv = vec![128u8; (width * height / 2) as usize];
        let mut sample = Sample::default();
        sample.fill(Nv12Planes {
            width,
            height,
            y: &y,
            y_stride: width as usize,
            uv: &uv,
            uv_stride: width as usize,
        });
        assert_eq!((sample.width, sample.height), (640, 360));
        assert_eq!(sample.luma[100], 200);
        assert_eq!(sample.luma[600], 0);
        assert_eq!(sample.luma.len(), 640 * 360);
    }

    #[test]
    fn a_4k_sample_is_the_same_size() {
        let (width, height) = (3840u32, 2160u32);
        let y = vec![90u8; (width * height) as usize];
        let uv = vec![128u8; (width * height / 2) as usize];
        let mut sample = Sample::default();
        sample.fill(Nv12Planes {
            width,
            height,
            y: &y,
            y_stride: width as usize,
            uv: &uv,
            uv_stride: width as usize,
        });
        assert_eq!((sample.width, sample.height), (640, 360));
        assert!(sample.luma.iter().all(|&value| value == 90));
    }

    #[test]
    fn a_still_picture_shows_no_movement() {
        let start = Instant::now();
        let mut observer = Observer::default();
        let sample = grey(640, 360, 100);
        for step in 0..10 {
            let reading = observer.observe(start + INTERVAL * step, &sample, None);
            assert_eq!(reading.wide_motion, 0.0);
            assert_eq!(reading.near_motion, 0.0);
            assert!(!reading.face);
        }
    }

    #[test]
    fn the_exposure_settling_is_not_movement() {
        let start = Instant::now();
        let mut observer = Observer::default();
        observer.observe(start, &grey(640, 360, 100), None);
        let reading = observer.observe(start + INTERVAL, &grey(640, 360, 130), None);
        assert_eq!(reading.wide_motion, 0.0);
    }

    #[test]
    fn something_moving_shows_as_movement() {
        let start = Instant::now();
        let mut observer = Observer::default();
        let mut first = grey(640, 360, 60);
        paint(&mut first, 80, 80, 64);
        observer.observe(start, &first, None);
        let mut second = grey(640, 360, 60);
        paint(&mut second, 160, 80, 64);
        let reading = observer.observe(start + INTERVAL, &second, None);
        // 64 cells left bright, 64 cells turned bright, of 3600.
        assert!((reading.wide_motion - 128.0 / 3600.0).abs() < 0.002);
        // No face was ever seen, so nothing counts as near.
        assert_eq!(reading.near_motion, 0.0);
    }

    #[test]
    fn movement_under_the_last_face_counts_as_near() {
        let start = Instant::now();
        let mut observer = Observer::default();
        let face = FaceBox {
            x: 280.0,
            y: 80.0,
            width: 80.0,
            height: 80.0,
        };
        let still = grey(640, 360, 60);
        observer.observe(start, &still, Some(face));
        // The face is gone (looking down), and an arm moves below where it was.
        let mut writing = grey(640, 360, 60);
        paint(&mut writing, 300, 240, 32);
        let reading = observer.observe(start + INTERVAL, &writing, None);
        assert!(!reading.face);
        assert!(reading.near_motion > 0.008, "{}", reading.near_motion);

        // The same movement far to the side is not near the person.
        let mut observer = Observer::default();
        observer.observe(start, &still, Some(face));
        let mut elsewhere = grey(640, 360, 60);
        paint(&mut elsewhere, 0, 240, 32);
        let reading = observer.observe(start + INTERVAL, &elsewhere, None);
        assert_eq!(reading.near_motion, 0.0);
        assert!(reading.wide_motion > 0.0);
    }

    #[test]
    fn a_seat_is_compared_with_how_it_looked_with_a_face() {
        let start = Instant::now();
        let mut observer = Observer::default();
        let face = FaceBox {
            x: 280.0,
            y: 80.0,
            width: 80.0,
            height: 80.0,
        };
        // A person: a bright body under the face, against a dark room.
        let mut seated = grey(640, 360, 60);
        paint(&mut seated, 240, 120, 160);
        let reading = observer.observe(start, &seated, Some(face));
        assert_eq!(reading.changed_since_face, None);

        // They look down: no face, and nothing else changes.
        let reading = observer.observe(start + INTERVAL, &seated, None);
        assert_eq!(reading.changed_since_face, Some(0.0));
        assert_eq!(reading.near_motion, 0.0);

        // They leave: the room shows where the body was.
        let reading = observer.observe(start + INTERVAL * 2, &grey(640, 360, 60), None);
        let changed = reading.changed_since_face.unwrap();
        assert!(changed > 0.3, "{changed}");

        // Long after, with nothing moving, the seat is still known to be empty.
        let reading = observer.observe(start + INTERVAL * 50, &grey(640, 360, 60), None);
        assert_eq!(reading.wide_motion, 0.0);
        assert!(reading.changed_since_face.unwrap() > 0.3);
    }

    #[test]
    fn slow_movement_shows_against_a_second_ago() {
        let start = Instant::now();
        let mut observer = Observer::default();
        let mut reading = Reading::default();
        // A block creeping along, two pixels per look.
        for step in 0..6u32 {
            let mut sample = grey(640, 360, 60);
            paint(&mut sample, 100 + step * 2, 100, 64);
            reading = observer.observe(start + INTERVAL * step, &sample, None);
        }
        assert!(reading.wide_motion > 0.0);
    }

    /// Runs the Windows face detector over the pictures in `PAIRCAM_FACE_PHOTOS` and prints
    /// what it finds. Run with
    /// `cargo test -p paircam-engine faces_in_photos -- --ignored --nocapture`.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs pictures; a check by eye, not by assertion"]
    fn faces_in_photos() {
        let Some(folder) = std::env::var_os("PAIRCAM_FACE_PHOTOS") else {
            println!("set PAIRCAM_FACE_PHOTOS to a folder of pictures");
            return;
        };
        let finder = faces::Finder::new().expect("face detection is available");
        for entry in std::fs::read_dir(folder).unwrap().flatten() {
            let Ok(picture) = image::open(entry.path()) else {
                continue;
            };
            // As the pipeline would see it: a 16:9 frame, shrunk to a sample.
            let picture = picture
                .resize_to_fill(1920, 1080, image::imageops::FilterType::Triangle)
                .to_luma8();
            let (width, height) = picture.dimensions();
            let uv = vec![128u8; (width * height / 2) as usize];
            let mut sample = Sample::default();
            sample.fill(Nv12Planes {
                width,
                height,
                y: picture.as_raw(),
                y_stride: width as usize,
                uv: &uv,
                uv_stride: width as usize,
            });
            let started = Instant::now();
            let face = finder.find(&sample);
            println!(
                "{}: {face:?} in {:.1} ms",
                entry.file_name().to_string_lossy(),
                started.elapsed().as_secs_f64() * 1000.0
            );
            // And an empty picture of the same size finds nobody.
            let empty = grey(sample.width, sample.height, 90);
            assert_eq!(finder.find(&empty), None);
        }
    }
}
