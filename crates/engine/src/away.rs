//! The away screen: what other apps see while the user is not in the picture.
//!
//! It is off until the user asks for it. With [`AwaySettings::automatic`] on, the engine
//! watches the incoming picture ([`crate::presence`]) and, once nobody has been in it for
//! [`AwaySettings::delay`] seconds, cross-fades the camera feed to the away screen
//! ([`crate::screen`]): a video or still picture of the user's choice, or a plain card, with a
//! short message over it. The live picture fades back as soon as someone returns. The user can
//! also show the away screen by hand.
//!
//! This module holds the settings, what the interface is told, and the two small state
//! machines: [`Watch`] decides whether the person is away, and [`Fade`] eases between the two
//! pictures. Neither touches pixels.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// The range of [`AwaySettings::delay`], in seconds.
pub const MIN_DELAY: f32 = 2.0;
pub const MAX_DELAY: f32 = 30.0;
pub const DEFAULT_DELAY: f32 = 5.0;

/// The longest message the away screen shows, in characters.
pub const MAX_MESSAGE_CHARS: usize = 80;

/// How long the cross-fade to the away screen takes, and the one back. Coming back is quicker:
/// the person is already there.
const FADE_AWAY: Duration = Duration::from_millis(350);
const FADE_BACK: Duration = Duration::from_millis(220);

/// A reading older than this says nothing about now, so the watch stops counting absence.
const READING_STALE: Duration = Duration::from_millis(1500);

/// Movement that counts as someone being there: the share of the cells around where the face
/// was last seen that changed, or the share of the whole picture.
const NEAR_MOTION: f32 = 0.008;
const WIDE_MOTION: f32 = 0.012;
/// Movement that brings the live picture back when no face is seen, such as someone sitting
/// down while looking away. It has to last [`ARRIVAL_READINGS`] readings in a row.
const ARRIVAL_MOTION: f32 = 0.03;
const ARRIVAL_READINGS: u32 = 2;
/// With no face in view, the person still counts as there while less than this share of where
/// they were looks different from when their face was last seen. Looking down changes the
/// head; leaving changes nearly everything.
const STILL_THERE: f32 = 0.25;

/// The away screen as the user set it up. Kept per computer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AwaySettings {
    /// Show the away screen by itself when nobody is in the picture.
    pub automatic: bool,
    /// Seconds without anyone in the picture before the away screen shows.
    pub delay: f32,
    /// The video or still picture to show. `None` shows a plain card.
    pub media: Option<PathBuf>,
    /// The words over the away screen. `None` uses the app's default; empty shows none.
    pub message: Option<String>,
}

impl Default for AwaySettings {
    fn default() -> Self {
        Self {
            automatic: false,
            delay: DEFAULT_DELAY,
            media: None,
            message: None,
        }
    }
}

impl AwaySettings {
    /// The settings with the delay in range and the message trimmed to what fits.
    pub fn normalized(mut self) -> Self {
        self.delay = if self.delay.is_finite() {
            self.delay.clamp(MIN_DELAY, MAX_DELAY)
        } else {
            DEFAULT_DELAY
        };
        self.message = self.message.map(|message| {
            message
                .trim()
                .chars()
                .filter(|character| !character.is_control())
                .take(MAX_MESSAGE_CHARS)
                .collect()
        });
        self.media = self.media.filter(|path| !path.as_os_str().is_empty());
        self
    }
}

/// Whether the chosen video or picture can be shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AwayMedia {
    /// No file was chosen: the plain card.
    #[default]
    Card,
    /// The file is being opened.
    Loading,
    Ready,
    /// The file is missing or cannot be played; the plain card shows instead.
    Failed,
}

/// What the interface is told about the away screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AwayStatus {
    /// Other apps see the away screen right now.
    pub showing: bool,
    /// Whether someone is in the picture. `None` while nothing is watching: the automatic
    /// switch is off, or no video is coming in.
    pub in_picture: Option<bool>,
    pub media: AwayMedia,
}

/// What one look at the picture found.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Reading {
    /// A face is in the picture.
    pub(crate) face: bool,
    /// The share of the cells around where the face was last seen that changed.
    pub(crate) near_motion: f32,
    /// The share of all cells that changed.
    pub(crate) wide_motion: f32,
    /// With no face in view: the share of the cells where the person was that look different
    /// from when their face was last seen. `None` with a face in view, or before any face.
    pub(crate) changed_since_face: Option<f32>,
}

impl Reading {
    /// The place where the person was still looks as it did with them in it.
    fn still_there(&self) -> bool {
        self.changed_since_face
            .is_some_and(|share| share < STILL_THERE)
    }

    fn shows_someone(&self) -> bool {
        self.face
            || self.still_there()
            || self.near_motion >= NEAR_MOTION
            || self.wide_motion >= WIDE_MOTION
    }

    /// Someone seems to have come back, though no face is in view yet.
    fn shows_arrival(&self) -> bool {
        self.still_there()
            || self.near_motion >= ARRIVAL_MOTION
            || self.wide_motion >= ARRIVAL_MOTION
    }
}

/// Decides from the readings whether the person is away.
///
/// Leaving is slow and coming back is fast. A face keeps the live picture, and so does a
/// person who looks down or away: where they sit still looks like them, or moves. Sitting
/// perfectly still over a notebook does not count as leaving. A face brings the live picture
/// back at once; clear movement, or the seat looking occupied again, does after a moment. When
/// the readings stop, nothing changes: without eyes, the watch never hides anyone.
#[derive(Debug, Default)]
pub(crate) struct Watch {
    away: bool,
    /// When the picture was first seen empty, while it has stayed empty.
    absent_since: Option<Instant>,
    last_reading: Option<Instant>,
    /// Readings in a row with clear movement while away.
    arriving: u32,
}

impl Watch {
    pub(crate) fn observe(&mut self, at: Instant, reading: &Reading) {
        self.last_reading = Some(at);
        if self.away {
            if reading.face {
                self.come_back();
            } else if reading.shows_arrival() {
                self.arriving += 1;
                if self.arriving >= ARRIVAL_READINGS {
                    self.come_back();
                }
            } else {
                self.arriving = 0;
            }
        } else if reading.shows_someone() {
            self.absent_since = None;
        } else if self.absent_since.is_none() {
            self.absent_since = Some(at);
        }
    }

    /// Whether the person is away, `delay` being how long the picture must stay empty first.
    pub(crate) fn step(&mut self, now: Instant, delay: Duration) -> bool {
        let fresh = self
            .last_reading
            .is_some_and(|at| now.saturating_duration_since(at) <= READING_STALE);
        if !fresh {
            // Not watching any more: absence starts counting again with the next reading.
            if !self.away {
                self.absent_since = None;
            }
            return self.away;
        }
        if !self.away
            && self
                .absent_since
                .is_some_and(|since| now.saturating_duration_since(since) >= delay)
        {
            self.away = true;
            self.arriving = 0;
        }
        self.away
    }

    /// Whether someone is in the picture, once there has been a reading.
    pub(crate) fn in_picture(&self) -> Option<bool> {
        self.last_reading
            .map(|_| !self.away && self.absent_since.is_none())
    }

    fn come_back(&mut self) {
        self.away = false;
        self.absent_since = None;
        self.arriving = 0;
    }
}

/// The cross-fade between the live picture and the away screen.
#[derive(Debug, Default)]
pub(crate) struct Fade {
    /// From 0 (live) to 1 (away), moving at a steady pace; [`Fade::mix`] eases it.
    level: f32,
    stepped: Option<Instant>,
}

impl Fade {
    /// Moves towards the away screen or back, and returns how much of the away screen to mix
    /// in, from 0 to 256.
    pub(crate) fn step(&mut self, now: Instant, away: bool) -> u32 {
        let elapsed = self
            .stepped
            .map_or(Duration::ZERO, |last| now.saturating_duration_since(last));
        self.stepped = Some(now);
        let (target, duration) = if away {
            (1.0, FADE_AWAY)
        } else {
            (0.0, FADE_BACK)
        };
        let change = elapsed.as_secs_f32() / duration.as_secs_f32();
        self.level = if self.level < target {
            (self.level + change).min(target)
        } else {
            (self.level - change).max(target)
        };
        self.mix()
    }

    /// The eased mix, from 0 to 256.
    pub(crate) fn mix(&self) -> u32 {
        // Smoothstep: the fade starts and ends gently in both directions.
        let eased = self.level * self.level * (3.0 - 2.0 * self.level);
        (eased * 256.0).round() as u32
    }

    /// Goes straight to the away screen or the live picture, without a fade.
    pub(crate) fn jump(&mut self, away: bool) {
        self.level = if away { 1.0 } else { 0.0 };
        self.stepped = None;
    }
}

/// Mixes `away` into `live` in place: `mix` is from 0 (all live) to 256 (all away). Both are
/// pictures of the same size and layout.
pub(crate) fn blend(live: &mut [u8], away: &[u8], mix: u32) {
    let mix = mix.min(256) as i32;
    for (live, away) in live.iter_mut().zip(away) {
        let from = i32::from(*live);
        *live = (from + (((i32::from(*away) - from) * mix) >> 8)) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY: Reading = Reading {
        face: false,
        near_motion: 0.0,
        wide_motion: 0.0,
        changed_since_face: None,
    };
    const FACE: Reading = Reading {
        face: true,
        ..EMPTY
    };

    fn after(start: Instant, milliseconds: u64) -> Instant {
        start + Duration::from_millis(milliseconds)
    }

    /// Feeds one reading every 200 ms from `from` to `to` and returns the last answer.
    fn watch_for(watch: &mut Watch, start: Instant, from: u64, to: u64, reading: Reading) -> bool {
        let mut away = false;
        let mut at = from;
        while at <= to {
            watch.observe(after(start, at), &reading);
            away = watch.step(after(start, at), Duration::from_secs(5));
            at += 200;
        }
        away
    }

    #[test]
    fn the_person_is_away_only_after_the_delay() {
        let start = Instant::now();
        let mut watch = Watch::default();
        assert!(!watch_for(&mut watch, start, 0, 2000, FACE));
        assert!(!watch_for(&mut watch, start, 2200, 7000, EMPTY));
        assert_eq!(watch.in_picture(), Some(false));
        assert!(watch_for(&mut watch, start, 7200, 7400, EMPTY));
    }

    #[test]
    fn a_face_brings_the_picture_back_at_once() {
        let start = Instant::now();
        let mut watch = Watch::default();
        assert!(watch_for(&mut watch, start, 0, 6000, EMPTY));
        watch.observe(after(start, 6200), &FACE);
        assert!(!watch.step(after(start, 6200), Duration::from_secs(5)));
        assert_eq!(watch.in_picture(), Some(true));
    }

    #[test]
    fn looking_down_while_moving_is_not_leaving() {
        let start = Instant::now();
        let mut watch = Watch::default();
        let writing = Reading {
            face: false,
            near_motion: 0.02,
            wide_motion: 0.004,
            ..EMPTY
        };
        assert!(!watch_for(&mut watch, start, 0, 20_000, writing));
        assert_eq!(watch.in_picture(), Some(true));
    }

    #[test]
    fn sitting_still_and_looking_down_is_not_leaving() {
        let start = Instant::now();
        let mut watch = Watch::default();
        // No face and no movement, but the seat still looks as it did with the person in it.
        let reading = Reading {
            changed_since_face: Some(0.1),
            ..EMPTY
        };
        assert!(!watch_for(&mut watch, start, 0, 60_000, reading));
        assert_eq!(watch.in_picture(), Some(true));
    }

    #[test]
    fn an_emptied_seat_is_leaving_and_a_filled_one_is_coming_back() {
        let start = Instant::now();
        let mut watch = Watch::default();
        let emptied = Reading {
            changed_since_face: Some(0.6),
            ..EMPTY
        };
        assert!(!watch_for(&mut watch, start, 0, 4800, emptied));
        assert!(watch_for(&mut watch, start, 5000, 6000, emptied));
        // Back in the seat, facing away from the camera.
        let seated = Reading {
            changed_since_face: Some(0.1),
            ..EMPTY
        };
        assert!(!watch_for(&mut watch, start, 6200, 6600, seated));
    }

    #[test]
    fn movement_must_last_to_bring_the_picture_back() {
        let start = Instant::now();
        let mut watch = Watch::default();
        assert!(watch_for(&mut watch, start, 0, 6000, EMPTY));
        let walking = Reading {
            face: false,
            wide_motion: 0.08,
            ..EMPTY
        };
        // One flicker is not enough...
        watch.observe(after(start, 6200), &walking);
        assert!(watch.step(after(start, 6200), Duration::from_secs(5)));
        watch.observe(after(start, 6400), &EMPTY);
        assert!(watch.step(after(start, 6400), Duration::from_secs(5)));
        // ...two readings in a row are.
        watch.observe(after(start, 6600), &walking);
        watch.observe(after(start, 6800), &walking);
        assert!(!watch.step(after(start, 6800), Duration::from_secs(5)));
    }

    #[test]
    fn without_readings_nobody_is_hidden() {
        let start = Instant::now();
        let mut watch = Watch::default();
        // Never a reading: stays live for ever.
        assert!(!watch.step(after(start, 60_000), Duration::from_secs(5)));
        assert_eq!(watch.in_picture(), None);
        // Empty for three seconds, then the readings stop: the count does not carry on.
        assert!(!watch_for(&mut watch, start, 0, 3000, EMPTY));
        assert!(!watch.step(after(start, 30_000), Duration::from_secs(5)));
        assert!(!watch_for(&mut watch, start, 30_200, 33_000, EMPTY));
    }

    #[test]
    fn the_fade_takes_its_time_and_ends_exactly() {
        let start = Instant::now();
        let mut fade = Fade::default();
        assert_eq!(fade.step(start, true), 0);
        let halfway = fade.step(start + FADE_AWAY / 2, true);
        assert!((120..=136).contains(&halfway), "{halfway}");
        assert_eq!(fade.step(start + FADE_AWAY, true), 256);
        assert_eq!(fade.step(start + FADE_AWAY * 3, true), 256);
        assert_eq!(fade.step(start + FADE_AWAY * 3 + FADE_BACK, false), 0);
    }

    #[test]
    fn blending_mixes_and_reaches_both_ends() {
        let away = [200u8, 0, 128];
        let mut live = [100u8, 255, 128];
        blend(&mut live, &away, 0);
        assert_eq!(live, [100, 255, 128]);
        blend(&mut live, &away, 128);
        assert_eq!(live, [150, 127, 128]);
        blend(&mut live, &away, 256);
        assert_eq!(live, away);
    }

    #[test]
    fn settings_are_kept_in_range() {
        let settings = AwaySettings {
            automatic: true,
            delay: 500.0,
            media: Some(PathBuf::new()),
            message: Some(format!("  {}  ", "x".repeat(200))),
        }
        .normalized();
        assert_eq!(settings.delay, MAX_DELAY);
        assert_eq!(settings.media, None);
        assert_eq!(
            settings.message.map(|message| message.chars().count()),
            Some(MAX_MESSAGE_CHARS)
        );
    }
}
