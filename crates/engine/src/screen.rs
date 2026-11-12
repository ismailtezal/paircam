//! The away screen's picture: a looping video or a still picture of the user's choice, or a
//! plain card, with the message drawn over it.
//!
//! An [`AwayScreen`] always has a finished camera-sized picture ready, so the pipeline can
//! fade to it the moment it is needed. A video only plays while the screen is showing: it
//! starts from its first picture every time, on a thread of its own, at its own pace.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::away::AwayMedia;
use crate::decoder::{DecodeError, Nv12Planes, Scaler};
use crate::lock;
use crate::media::MediaFile;
use crate::picture::{FitMode, OUTPUT_HEIGHT, OUTPUT_WIDTH, PictureSettings, Rect, Rotation, plan};
use crate::pipeline::Canvas;
use crate::text::{self, TextMask};

/// The plain card: the app's dark surface, in video-range luma.
const CARD_LUMA: u8 = 24;
/// The message: nearly white, like the app's text.
const TEXT_LUMA: i32 = 222;
const NEUTRAL_CHROMA: i32 = 128;
/// The height of the message's letters on the card, and over a video or picture.
const CARD_TEXT_HEIGHT: u32 = 64;
const MEDIA_TEXT_HEIGHT: u32 = 56;
/// The message wraps before it gets this wide.
const TEXT_WIDTH: u32 = 1600;
/// Over a video or picture the message sits this far above the bottom edge...
const TEXT_BOTTOM_MARGIN: u32 = 84;
/// ...on a shade that darkens the picture by up to this much (of 256), rising over this many
/// rows above the message, so the words read on any picture.
const SHADE_STRENGTH: u32 = 150;
const SHADE_RISE: u32 = 220;

/// A video that falls this far behind skips the wait instead of trying to catch up.
const LATE: Duration = Duration::from_millis(250);
/// The gap after the last picture of a video before it starts again, when the file does not
/// say.
const DEFAULT_GAP: Duration = Duration::from_millis(40);

struct Control {
    playing: bool,
    shutting_down: bool,
}

struct Shared {
    /// The finished picture: camera-sized NV12, always complete.
    picture: Mutex<Vec<u8>>,
    control: Mutex<Control>,
    wake: Condvar,
}

pub(crate) struct AwayScreen {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl AwayScreen {
    /// Builds the screen. The plain card is ready when this returns; a video or picture
    /// replaces it once it has been opened. `on_media` hears how that goes, on another thread.
    pub(crate) fn new(
        media: Option<&Path>,
        message: &str,
        on_media: impl Fn(AwayMedia) + Send + 'static,
    ) -> Self {
        let mut card = Canvas::new();
        card.resize(OUTPUT_WIDTH, OUTPUT_HEIGHT);
        card.fill(CARD_LUMA);
        if let Some(mask) = text::draw(message, CARD_TEXT_HEIGHT, TEXT_WIDTH) {
            let top = OUTPUT_HEIGHT.saturating_sub(mask.height) / 2;
            draw_text(&mut card, &mask, top);
        }
        let shared = Arc::new(Shared {
            picture: Mutex::new(card.data),
            control: Mutex::new(Control {
                playing: false,
                shutting_down: false,
            }),
            wake: Condvar::new(),
        });

        let thread = media.and_then(|path| {
            on_media(AwayMedia::Loading);
            let shared = shared.clone();
            let path = path.to_owned();
            let message = message.to_owned();
            let spawned = thread::Builder::new()
                .name("paircam-away".into())
                .spawn(move || {
                    let mask = text::draw(&message, MEDIA_TEXT_HEIGHT, TEXT_WIDTH);
                    let mut player = Player {
                        shared: &shared,
                        path,
                        mask,
                        scaler: Scaler::new(),
                        canvas: Canvas::new(),
                        on_media: Box::new(on_media),
                    };
                    if let Err(error) = player.run() {
                        log::warn!(
                            "cannot play the away screen file {}: {error}",
                            player.path.display()
                        );
                        (player.on_media)(AwayMedia::Failed);
                    }
                });
            match spawned {
                Ok(thread) => Some(thread),
                Err(error) => {
                    log::error!("cannot start the away screen thread: {error}");
                    None
                }
            }
        });
        Self { shared, thread }
    }

    /// Starts the video from its first picture, or stops it and goes back there.
    pub(crate) fn set_playing(&self, playing: bool) {
        let mut control = lock(&self.shared.control);
        if control.playing != playing {
            control.playing = playing;
            drop(control);
            self.shared.wake.notify_all();
        }
    }

    /// Lends the current picture: camera-sized NV12.
    pub(crate) fn with_picture<R>(&self, use_it: impl FnOnce(&[u8]) -> R) -> R {
        use_it(&lock(&self.shared.picture))
    }
}

impl Drop for AwayScreen {
    fn drop(&mut self) {
        lock(&self.shared.control).shutting_down = true;
        self.shared.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Why the player stopped waiting.
enum Wake {
    Play,
    Stop,
    ShutDown,
}

struct Player<'a> {
    shared: &'a Shared,
    path: PathBuf,
    mask: Option<TextMask>,
    scaler: Scaler,
    canvas: Canvas,
    on_media: Box<dyn Fn(AwayMedia)>,
}

impl Player<'_> {
    /// Opens the file and plays it whenever the screen is showing. Returns when the screen is
    /// dropped, or with the reason the file cannot be played.
    fn run(&mut self) -> Result<(), DecodeError> {
        self.canvas.resize(OUTPUT_WIDTH, OUTPUT_HEIGHT);
        let mut file = MediaFile::open(&self.path)?;
        let rotation = file.rotation();
        if !self.show_next(&mut file, rotation)? {
            return Err(DecodeError::new("the file holds no picture"));
        }
        (self.on_media)(AwayMedia::Ready);
        // A still picture has nothing after its first picture.
        let mut still = false;

        loop {
            match self.wait(|control| control.playing) {
                Wake::ShutDown => return Ok(()),
                Wake::Play | Wake::Stop => {}
            }
            if still {
                if matches!(self.wait(|control| !control.playing), Wake::ShutDown) {
                    return Ok(());
                }
                continue;
            }

            let mut started = Instant::now();
            // How far into the current run of the file the clock is, and the time of its
            // last picture: at the end, the next run starts one gap later.
            let mut run_start = Duration::ZERO;
            let mut last = Duration::ZERO;
            let mut pictures_in_run = 1u32;
            loop {
                let run_started = started + run_start;
                let due = move |time: Duration| run_started + time;
                let mut next_time = None;
                let shown = self.show_next_at(&mut file, rotation, |time| {
                    next_time = Some(time);
                    due(time)
                })?;
                match shown {
                    Shown::Picture => {
                        let time = next_time.unwrap_or(last);
                        // Fell behind (a large file, a busy computer): carry on from here.
                        let behind = Instant::now().saturating_duration_since(due(time));
                        if behind > LATE {
                            started += behind;
                        }
                        last = time;
                        pictures_in_run += 1;
                    }
                    Shown::End => {
                        if pictures_in_run <= 1 {
                            still = true;
                            break;
                        }
                        let gap = (last / (pictures_in_run - 1)).max(Duration::from_millis(8));
                        let gap = if last.is_zero() { DEFAULT_GAP } else { gap };
                        run_start += last + gap;
                        last = Duration::ZERO;
                        pictures_in_run = 0;
                        reopen(&mut file, &self.path)?;
                    }
                    Shown::Stopped => break,
                    Shown::ShutDown => return Ok(()),
                }
            }

            if !still {
                // Back to the first picture, ready for the next time.
                reopen(&mut file, &self.path)?;
                if !self.show_next(&mut file, rotation)? {
                    return Err(DecodeError::new("the file holds no picture"));
                }
            }
        }
    }

    /// Waits until `ready` holds or the screen is dropped.
    fn wait(&self, ready: impl Fn(&Control) -> bool) -> Wake {
        let mut control = lock(&self.shared.control);
        loop {
            if control.shutting_down {
                return Wake::ShutDown;
            }
            if ready(&control) {
                return if control.playing {
                    Wake::Play
                } else {
                    Wake::Stop
                };
            }
            control = self
                .shared
                .wake
                .wait(control)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Waits until `due` while the video keeps playing.
    fn wait_until(&self, due: Instant) -> Wake {
        let mut control = lock(&self.shared.control);
        loop {
            if control.shutting_down {
                return Wake::ShutDown;
            }
            if !control.playing {
                return Wake::Stop;
            }
            let left = due.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Wake::Play;
            }
            control = self
                .shared
                .wake
                .wait_timeout(control, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Decodes the next picture and shows it right away. False at the end of the file.
    fn show_next(&mut self, file: &mut MediaFile, rotation: Rotation) -> Result<bool, DecodeError> {
        let Some(picture) = file.next()? else {
            return Ok(false);
        };
        compose(
            &mut self.scaler,
            &mut self.canvas,
            picture.planes(),
            rotation,
            self.mask.as_ref(),
        )?;
        self.publish();
        Ok(true)
    }

    /// Decodes the next picture, waits until it is due (`due_for` turns its time in the file
    /// into a moment) and shows it.
    fn show_next_at(
        &mut self,
        file: &mut MediaFile,
        rotation: Rotation,
        due_for: impl FnOnce(Duration) -> Instant,
    ) -> Result<Shown, DecodeError> {
        let Some(picture) = file.next()? else {
            return Ok(Shown::End);
        };
        let due = due_for(picture.time());
        compose(
            &mut self.scaler,
            &mut self.canvas,
            picture.planes(),
            rotation,
            self.mask.as_ref(),
        )?;
        match self.wait_until(due) {
            Wake::Play => {
                self.publish();
                Ok(Shown::Picture)
            }
            Wake::Stop => Ok(Shown::Stopped),
            Wake::ShutDown => Ok(Shown::ShutDown),
        }
    }

    fn publish(&self) {
        lock(&self.shared.picture).copy_from_slice(&self.canvas.data);
    }
}

enum Shown {
    Picture,
    /// The file ended.
    End,
    /// The screen stopped showing while waiting.
    Stopped,
    ShutDown,
}

/// Goes back to the first picture: by seeking, or by opening the file again where that does
/// not work.
fn reopen(file: &mut MediaFile, path: &Path) -> Result<(), DecodeError> {
    if file.rewind().is_err() {
        *file = MediaFile::open(path)?;
    }
    Ok(())
}

/// Fits one picture of the file into the camera frame, upright, and draws the message.
fn compose(
    scaler: &mut Scaler,
    canvas: &mut Canvas,
    picture: Nv12Planes<'_>,
    rotation: Rotation,
    mask: Option<&TextMask>,
) -> Result<(), DecodeError> {
    let settings = PictureSettings {
        rotation,
        fit: FitMode::Fit,
        ..Default::default()
    };
    let (plan, _) = plan(
        Rect::FULL,
        &settings,
        (picture.width, picture.height),
        (OUTPUT_WIDTH, OUTPUT_HEIGHT),
    );
    scaler.transform_nv12(picture, &plan, canvas.planes_mut())?;
    if let Some(mask) = mask {
        let top = OUTPUT_HEIGHT
            .saturating_sub(TEXT_BOTTOM_MARGIN + mask.height)
            .min(OUTPUT_HEIGHT.saturating_sub(mask.height));
        shade(canvas, top);
        draw_text(canvas, mask, top);
    }
    Ok(())
}

/// Darkens the picture towards the bottom: nothing above `text_top - SHADE_RISE`, rising to
/// full strength at `text_top`, and full strength below.
fn shade(canvas: &mut Canvas, text_top: u32) {
    let width = canvas.width as usize;
    let height = canvas.height;
    let first = text_top.saturating_sub(SHADE_RISE);
    let strength_at = |row: u32| -> i32 {
        let progress = ((row - first) as f32 / SHADE_RISE as f32).min(1.0);
        // Smoothstep, so the shade has no visible edge.
        let eased = progress * progress * (3.0 - 2.0 * progress);
        (eased * SHADE_STRENGTH as f32) as i32
    };
    let luma_size = width * height as usize;
    let (luma, chroma) = canvas.data.split_at_mut(luma_size);
    for row in first..height {
        let keep = 256 - strength_at(row);
        for value in &mut luma[row as usize * width..(row as usize + 1) * width] {
            *value = (16 + (((i32::from(*value) - 16) * keep) >> 8)) as u8;
        }
    }
    for row in first / 2..height / 2 {
        let keep = 256 - strength_at((row * 2).max(first));
        for value in &mut chroma[row as usize * width..(row as usize + 1) * width] {
            *value = (NEUTRAL_CHROMA + (((i32::from(*value) - NEUTRAL_CHROMA) * keep) >> 8)) as u8;
        }
    }
}

/// Draws the mask centred, with its first row at `top`.
fn draw_text(canvas: &mut Canvas, mask: &TextMask, top: u32) {
    let width = canvas.width as usize;
    let height = canvas.height as usize;
    let mask_width = (mask.width as usize).min(width);
    // Even offsets, so the mask lines up with the chroma samples.
    let left = ((width - mask_width) / 2) & !1;
    let top = (top as usize) & !1;
    let rows = (mask.height as usize).min(height.saturating_sub(top));
    let luma_size = width * height;
    let (luma, chroma) = canvas.data.split_at_mut(luma_size);

    for row in 0..rows {
        let coverage = &mask.coverage[row * mask.width as usize..][..mask_width];
        let target = &mut luma[(top + row) * width + left..][..mask_width];
        for (value, &cover) in target.iter_mut().zip(coverage) {
            if cover != 0 {
                let from = i32::from(*value);
                *value = (from + (TEXT_LUMA - from) * i32::from(cover) / 255) as u8;
            }
        }
    }
    // One chroma pair per two by two pixels: pull it to grey by the block's average cover.
    for row in 0..rows / 2 {
        let upper = &mask.coverage[row * 2 * mask.width as usize..][..mask_width];
        let lower = &mask.coverage[(row * 2 + 1) * mask.width as usize..][..mask_width];
        let target = &mut chroma[(top / 2 + row) * width + left..][..mask_width & !1];
        for (pair, block) in target.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let cover = (i32::from(upper[pair * 2])
                + i32::from(upper[pair * 2 + 1])
                + i32::from(lower[pair * 2])
                + i32::from(lower[pair * 2 + 1]))
                / 4;
            if cover != 0 {
                for value in block {
                    let from = i32::from(*value);
                    *value = (from + (NEUTRAL_CHROMA - from) * cover / 255) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(luma: u8, chroma: u8) -> Canvas {
        let mut canvas = Canvas::new();
        canvas.resize(OUTPUT_WIDTH, OUTPUT_HEIGHT);
        let luma_size = (OUTPUT_WIDTH * OUTPUT_HEIGHT) as usize;
        canvas.data[..luma_size].fill(luma);
        canvas.data[luma_size..].fill(chroma);
        canvas
    }

    #[test]
    fn the_shade_darkens_only_the_bottom_and_greys_the_colour() {
        let mut canvas = solid(200, 60);
        shade(&mut canvas, 900);
        let luma = |row: u32| canvas.data[(row * OUTPUT_WIDTH + 500) as usize];
        let chroma = |row: u32| {
            canvas.data[(OUTPUT_WIDTH * OUTPUT_HEIGHT + (row / 2) * OUTPUT_WIDTH + 500) as usize]
        };
        assert_eq!(luma(100), 200);
        assert_eq!(luma(679), 200);
        assert!(luma(800) < 200 && luma(800) > luma(900));
        assert_eq!(luma(900), luma(1079));
        assert!(luma(1079) < 100);
        assert_eq!(chroma(100), 60);
        assert!(chroma(1000) > 60 && chroma(1000) < 128);
    }

    #[test]
    fn text_brightens_where_the_mask_covers_and_nowhere_else() {
        let mut canvas = solid(40, 90);
        let mask = TextMask {
            width: 4,
            height: 2,
            coverage: vec![255, 255, 0, 0, 255, 255, 0, 0],
        };
        draw_text(&mut canvas, &mask, 500);
        let left = (OUTPUT_WIDTH as usize - 4) / 2;
        let row = 500 * OUTPUT_WIDTH as usize;
        assert_eq!(canvas.data[row + left], TEXT_LUMA as u8);
        assert_eq!(canvas.data[row + left + 1], TEXT_LUMA as u8);
        assert_eq!(canvas.data[row + left + 2], 40);
        assert_eq!(canvas.data[row + left - 1], 40);
        let chroma_row = (OUTPUT_WIDTH * OUTPUT_HEIGHT) as usize + 250 * OUTPUT_WIDTH as usize;
        assert_eq!(canvas.data[chroma_row + left], 128);
        assert_eq!(canvas.data[chroma_row + left + 1], 128);
        assert_eq!(canvas.data[chroma_row + left + 2], 90);
    }

    #[cfg(windows)]
    #[test]
    fn the_card_is_dark_with_the_message_in_the_middle() {
        let screen = AwayScreen::new(None, "We'll be right back", |_| {});
        screen.with_picture(|picture| {
            let luma_size = (OUTPUT_WIDTH * OUTPUT_HEIGHT) as usize;
            assert_eq!(picture.len(), luma_size * 3 / 2);
            assert_eq!(picture[0], CARD_LUMA);
            let middle = &picture[530 * OUTPUT_WIDTH as usize..550 * OUTPUT_WIDTH as usize];
            assert!(middle.iter().any(|&value| value > 200));
            let top = &picture[..300 * OUTPUT_WIDTH as usize];
            assert!(top.iter().all(|&value| value == CARD_LUMA));
            assert!(picture[luma_size..].iter().all(|&value| value == 128));
        });
    }

    /// Writes pictures of the away screen for `PAIRCAM_AWAY_MEDIA` (a video or picture; the
    /// plain card without it) into the folder `PAIRCAM_AWAY_OUT`: the first picture, and what
    /// shows after one and three seconds of playing. Run with
    /// `cargo test -p paircam-engine away_screen_pictures -- --ignored --nocapture`.
    #[test]
    #[ignore = "writes pictures to look at"]
    fn away_screen_pictures() {
        let Some(out) = std::env::var_os("PAIRCAM_AWAY_OUT").map(PathBuf::from) else {
            println!("set PAIRCAM_AWAY_OUT to a folder");
            return;
        };
        let media = std::env::var_os("PAIRCAM_AWAY_MEDIA").map(PathBuf::from);
        let message =
            std::env::var("PAIRCAM_AWAY_MESSAGE").unwrap_or_else(|_| "We'll be right back".into());
        let (states, heard) = std::sync::mpsc::channel();
        let screen = AwayScreen::new(media.as_deref(), &message, move |state| {
            let _ = states.send(state);
        });
        let save = |name: &str| {
            let mut bgra = Vec::new();
            screen.with_picture(|picture| {
                let luma_size = (OUTPUT_WIDTH * OUTPUT_HEIGHT) as usize;
                Scaler::new()
                    .planes_to_bgra(
                        Nv12Planes {
                            width: OUTPUT_WIDTH,
                            height: OUTPUT_HEIGHT,
                            y: &picture[..luma_size],
                            y_stride: OUTPUT_WIDTH as usize,
                            uv: &picture[luma_size..],
                            uv_stride: OUTPUT_WIDTH as usize,
                        },
                        OUTPUT_WIDTH,
                        OUTPUT_HEIGHT,
                        &mut bgra,
                    )
                    .unwrap();
            });
            for pixel in bgra.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            image::save_buffer(
                out.join(name),
                &bgra,
                OUTPUT_WIDTH,
                OUTPUT_HEIGHT,
                image::ColorType::Rgba8,
            )
            .unwrap();
        };
        if media.is_some() {
            // Loading, then ready or failed.
            for _ in 0..2 {
                let state = heard.recv_timeout(Duration::from_secs(10)).unwrap();
                println!("media: {state:?}");
            }
        }
        std::fs::create_dir_all(&out).unwrap();
        save("away-first.png");
        screen.set_playing(true);
        thread::sleep(Duration::from_secs(1));
        save("away-1s.png");
        thread::sleep(Duration::from_secs(2));
        save("away-3s.png");
        screen.set_playing(false);
        thread::sleep(Duration::from_millis(500));
        save("away-stopped.png");
    }
}
