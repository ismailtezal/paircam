//! What other apps see: the picture settings, the geometry that turns them into a pixel plan,
//! and the framing that moves the crop smoothly, by hand or after a face.
//!
//! Nothing here touches pixels. The pipeline asks a [`Framer`] for this frame's crop, turns it
//! into a [`Plan`] with [`plan`], and hands that plan to the C layer, which does the work for
//! the virtual camera and the preview alike.
//!
//! Two coordinate spaces meet here:
//!
//! - **Source** space is the decoded picture as the phone encoded it. Face rectangles from the
//!   phone are in source space.
//! - **Oriented** space is that picture after the rotation, mirror and flip the user chose: the
//!   picture as it appears on screen. Crops, zoom and pan live in oriented space, so dragging
//!   the preview moves the picture the way the pointer moves whatever the orientation.
//!
//! Both use normalised coordinates: 0 to 1 on each axis, origin at the top left.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// The camera picture other apps receive, and the shape of the preview.
pub const OUTPUT_WIDTH: u32 = 1920;
pub const OUTPUT_HEIGHT: u32 = 1080;

/// The digital zoom range.
pub const MIN_ZOOM: f32 = 1.0;
pub const MAX_ZOOM: f32 = 8.0;

/// Face tracking never crops so tightly that the output is enlarged more than this.
const MAX_TRACKING_UPSCALE: f32 = 2.0;
/// How much of the crop's height the face takes, from the widest framing to the closest.
const FACE_SHARE_WIDE: f32 = 0.14;
const FACE_SHARE_CLOSE: f32 = 0.40;
/// Where the centre of the face sits, as a fraction of the crop's height from the top. A
/// little above the middle puts the eyes near the upper third, with the shoulders below.
const FACE_LINE: f32 = 0.38;
/// The tracked crop only moves once the face has drifted this far from where the crop wants
/// it, as a fraction of the crop's size, so small head movements leave the picture still.
const DEAD_ZONE_MOVE: f32 = 0.10;
/// ...or once the face has grown or shrunk by this fraction.
const DEAD_ZONE_SCALE: f32 = 0.18;
/// A face report older than this counts as gone, in case the phone falls silent.
const FACE_STALE: Duration = Duration::from_millis(1000);
/// How long the crop holds still after the face is lost, before easing back.
const LOST_GRACE: Duration = Duration::from_millis(1500);

/// Spring responses: the period one oscillation would take without damping. Every spring here
/// is critically damped, so it settles without overshoot in about 1.5 responses.
mod response {
    /// Zoom and pan the user sets, which should feel direct.
    pub const MANUAL: f32 = 0.35;
    /// Following a face: slow enough to feel like a camera operator, not a jitter.
    pub const TRACKING: f32 = 1.1;
    /// Easing back to the user's framing after the face is lost.
    pub const RETURN: f32 = 1.2;
}

// ---------------------------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------------------------

/// A clockwise turn of the picture, in quarter turns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub enum Rotation {
    #[default]
    None,
    Clockwise90,
    HalfTurn,
    Clockwise270,
}

impl Rotation {
    pub const ALL: [Self; 4] = [
        Self::None,
        Self::Clockwise90,
        Self::HalfTurn,
        Self::Clockwise270,
    ];

    pub fn degrees(self) -> u16 {
        match self {
            Self::None => 0,
            Self::Clockwise90 => 90,
            Self::HalfTurn => 180,
            Self::Clockwise270 => 270,
        }
    }

    pub fn from_degrees(degrees: u16) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|rotation| rotation.degrees() == degrees)
    }

    /// One more quarter turn clockwise.
    pub fn next(self) -> Self {
        Self::from_degrees((self.degrees() + 90) % 360).unwrap_or_default()
    }

    /// The turn that undoes this one.
    pub fn inverse(self) -> Self {
        Self::from_degrees((360 - self.degrees()) % 360).unwrap_or_default()
    }

    /// Whether the picture's width and height trade places.
    pub fn swaps_axes(self) -> bool {
        matches!(self, Self::Clockwise90 | Self::Clockwise270)
    }
}

impl TryFrom<u16> for Rotation {
    type Error = String;

    fn try_from(degrees: u16) -> Result<Self, Self::Error> {
        Self::from_degrees(degrees).ok_or_else(|| format!("{degrees} is not a quarter turn"))
    }
}

impl From<Rotation> for u16 {
    fn from(rotation: Rotation) -> Self {
        rotation.degrees()
    }
}

/// How the picture fills the 16:9 camera frame when its shape differs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FitMode {
    /// The whole picture, with black bars.
    #[default]
    Fit,
    /// Cropped to cover the frame.
    Fill,
    /// Stretched to the frame.
    Stretch,
}

/// A point in normalised coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// A rectangle in normalised coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const FULL: Self = Self {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };

    pub fn center(&self) -> Point {
        Point {
            x: self.x + self.width / 2.0,
            y: self.y + self.height / 2.0,
        }
    }

    fn around(center: Point, width: f32, height: f32) -> Self {
        Self {
            x: center.x - width / 2.0,
            y: center.y - height / 2.0,
            width,
            height,
        }
    }

    /// The rectangle moved and shrunk until it lies inside the unit square.
    fn inside_unit(&self) -> Self {
        let width = self.width.clamp(f32::EPSILON, 1.0);
        let height = self.height.clamp(f32::EPSILON, 1.0);
        Self {
            x: self.x.clamp(0.0, 1.0 - width),
            y: self.y.clamp(0.0, 1.0 - height),
            width,
            height,
        }
    }

    fn rotated(&self, rotation: Rotation) -> Self {
        let Self {
            x,
            y,
            width,
            height,
        } = *self;
        match rotation {
            Rotation::None => *self,
            // A point (u, v) goes to (1 - v, u).
            Rotation::Clockwise90 => Self {
                x: 1.0 - (y + height),
                y: x,
                width: height,
                height: width,
            },
            Rotation::HalfTurn => Self {
                x: 1.0 - (x + width),
                y: 1.0 - (y + height),
                width,
                height,
            },
            // A point (u, v) goes to (v, 1 - u).
            Rotation::Clockwise270 => Self {
                x: y,
                y: 1.0 - (x + width),
                width: height,
                height: width,
            },
        }
    }

    fn mirrored(&self) -> Self {
        Self {
            x: 1.0 - (self.x + self.width),
            ..*self
        }
    }

    fn flipped(&self) -> Self {
        Self {
            y: 1.0 - (self.y + self.height),
            ..*self
        }
    }
}

/// How the picture is adjusted before other apps see it. Kept per computer, not per phone.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PictureSettings {
    /// Mirror left and right.
    pub mirror: bool,
    /// Flip upside down.
    pub flip: bool,
    pub rotation: Rotation,
    pub fit: FitMode,
    /// Digital zoom, from [`MIN_ZOOM`] to [`MAX_ZOOM`].
    pub zoom: f32,
    /// The middle of the zoomed view, in oriented coordinates.
    pub center: Point,
    /// How closely face tracking frames the person, from 0 (wide, head and torso) to 1
    /// (close, head and shoulders).
    pub framing: f32,
}

impl Default for PictureSettings {
    fn default() -> Self {
        Self {
            mirror: false,
            flip: false,
            rotation: Rotation::None,
            fit: FitMode::Fit,
            zoom: MIN_ZOOM,
            center: Point { x: 0.5, y: 0.5 },
            framing: 0.5,
        }
    }
}

impl PictureSettings {
    /// The settings with every value in range: zoom and framing clamped, and the centre moved
    /// so the zoomed view stays inside the picture.
    pub fn normalized(mut self) -> Self {
        self.zoom = if self.zoom.is_finite() {
            self.zoom.clamp(MIN_ZOOM, MAX_ZOOM)
        } else {
            MIN_ZOOM
        };
        self.framing = if self.framing.is_finite() {
            self.framing.clamp(0.0, 1.0)
        } else {
            0.5
        };
        let half = 0.5 / self.zoom;
        let finite = |value: f32| if value.is_finite() { value } else { 0.5 };
        self.center = Point {
            x: finite(self.center.x).clamp(half, 1.0 - half),
            y: finite(self.center.y).clamp(half, 1.0 - half),
        };
        self
    }

    /// Whether the view is zoomed in, so dragging it pans.
    pub fn is_zoomed(&self) -> bool {
        self.zoom > MIN_ZOOM + 0.001
    }

    /// The settings back at their defaults, keeping the face tracking framing, which belongs
    /// to the Tracking section rather than to the picture.
    pub fn reset(&self) -> Self {
        Self {
            framing: self.framing,
            ..Self::default()
        }
    }

    pub fn orientation(&self) -> Orientation {
        Orientation {
            rotation: self.rotation,
            mirror: self.mirror,
            flip: self.flip,
        }
    }

    /// The settings after dragging the view by `dx`, `dy` (fractions of the output frame) while
    /// it shows `view`. The picture follows the pointer.
    pub fn panned(&self, view: &View, dx: f32, dy: f32) -> Self {
        let scale_x = view.crop.width / view.content.width.max(f32::EPSILON);
        let scale_y = view.crop.height / view.content.height.max(f32::EPSILON);
        Self {
            center: Point {
                x: self.center.x - dx * scale_x,
                y: self.center.y - dy * scale_y,
            },
            ..*self
        }
        .normalized()
    }
}

/// The rotation, mirror and flip that take source space to oriented space. The rotation comes
/// first, then the mirror, then the flip, all as the picture is seen on screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Orientation {
    pub rotation: Rotation,
    pub mirror: bool,
    pub flip: bool,
}

impl Orientation {
    /// The size of a `width` x `height` picture once oriented.
    pub fn oriented_size(&self, width: u32, height: u32) -> (u32, u32) {
        if self.rotation.swaps_axes() {
            (height, width)
        } else {
            (width, height)
        }
    }

    /// A source rectangle as it appears on screen.
    pub fn to_oriented(&self, rect: Rect) -> Rect {
        let mut rect = rect.rotated(self.rotation);
        if self.mirror {
            rect = rect.mirrored();
        }
        if self.flip {
            rect = rect.flipped();
        }
        rect
    }

    /// An on-screen rectangle in the source picture.
    pub fn to_source(&self, rect: Rect) -> Rect {
        let mut rect = rect;
        if self.flip {
            rect = rect.flipped();
        }
        if self.mirror {
            rect = rect.mirrored();
        }
        rect.rotated(self.rotation.inverse())
    }
}

// ---------------------------------------------------------------------------------------------
// From a crop to pixels
// ---------------------------------------------------------------------------------------------

/// A rectangle in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Everything the C layer needs to make one output picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The part of the decoded picture to use, in its pixels. Every edge is even.
    pub crop: PixelRect,
    pub rotation: Rotation,
    pub mirror: bool,
    pub flip: bool,
    /// Where the picture lands in the output; the rest is black. Every edge is even.
    pub content: PixelRect,
    pub output_width: u32,
    pub output_height: u32,
}

/// What the output shows, for turning pointer movement into a pan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// The part of the oriented picture that is visible.
    pub crop: Rect,
    /// Where it lands in the output, as fractions of the output.
    pub content: Rect,
}

/// Fits a `source_width` x `source_height` picture into `width` x `height`, keeping its aspect
/// ratio: bars above and below a wider picture, at the sides of a taller one. Every edge is
/// even, as NV12 requires.
pub fn letterbox(source_width: u32, source_height: u32, width: u32, height: u32) -> PixelRect {
    let source_width = u64::from(source_width.max(1));
    let source_height = u64::from(source_height.max(1));
    let (target_width, target_height) = (u64::from(width.max(2)), u64::from(height.max(2)));

    // Rounds a / b to the nearest even number in 2..=limit.
    let even = |a: u64, b: u64, limit: u64| ((a + b) / (2 * b) * 2).clamp(2, limit & !1);

    let (fitted_width, fitted_height) =
        if source_width * target_height >= source_height * target_width {
            let fitted = even(source_height * target_width, source_width, target_height);
            (target_width & !1, fitted)
        } else {
            let fitted = even(source_width * target_height, source_height, target_width);
            (fitted, target_height & !1)
        };
    PixelRect {
        x: (((target_width - fitted_width) / 2) & !1) as u32,
        y: (((target_height - fitted_height) / 2) & !1) as u32,
        width: fitted_width as u32,
        height: fitted_height as u32,
    }
}

/// Converts a normalised source rectangle to even pixel edges inside a `width` x `height`
/// picture, never smaller than 2 x 2.
fn even_pixels(rect: Rect, width: u32, height: u32) -> PixelRect {
    let axis = |start: f32, length: f32, size: u32| {
        let limit = (size & !1).max(2);
        let begin = ((start.max(0.0) * size as f32).floor() as u32 & !1).min(limit - 2);
        let span =
            ((length.max(0.0) * size as f32 / 2.0).round() as u32 * 2).clamp(2, limit - begin);
        (begin, span)
    };
    let (x, crop_width) = axis(rect.x, rect.width, width);
    let (y, crop_height) = axis(rect.y, rect.height, height);
    PixelRect {
        x,
        y,
        width: crop_width,
        height: crop_height,
    }
}

/// Turns a crop of the oriented picture into a pixel plan for a `source` picture and an
/// `output` frame, applying the fit mode. Also says what the output then shows.
pub fn plan(
    crop: Rect,
    settings: &PictureSettings,
    source: (u32, u32),
    output: (u32, u32),
) -> (Plan, View) {
    let orientation = settings.orientation();
    let (source_width, source_height) = (source.0.max(2), source.1.max(2));
    let (output_width, output_height) = (output.0.max(2) & !1, output.1.max(2) & !1);
    let (oriented_width, oriented_height) = orientation.oriented_size(source_width, source_height);
    let (oriented_width, oriented_height) = (oriented_width as f32, oriented_height as f32);

    let crop = crop.inside_unit();
    // The crop in oriented pixels.
    let (mut x, mut y) = (crop.x * oriented_width, crop.y * oriented_height);
    let (mut width, mut height) = (crop.width * oriented_width, crop.height * oriented_height);
    let output_aspect = output_width as f32 / output_height as f32;

    let full = PixelRect {
        x: 0,
        y: 0,
        width: output_width,
        height: output_height,
    };
    let content = match settings.fit {
        FitMode::Fit => letterbox(
            width.round().max(1.0) as u32,
            height.round().max(1.0) as u32,
            output_width,
            output_height,
        ),
        FitMode::Fill => {
            // Trim the crop to the output's shape, keeping its middle.
            if width / height > output_aspect {
                let trimmed = height * output_aspect;
                x += (width - trimmed) / 2.0;
                width = trimmed;
            } else {
                let trimmed = width / output_aspect;
                y += (height - trimmed) / 2.0;
                height = trimmed;
            }
            full
        }
        FitMode::Stretch => full,
    };

    let visible = Rect {
        x: x / oriented_width,
        y: y / oriented_height,
        width: width / oriented_width,
        height: height / oriented_height,
    };
    let plan = Plan {
        crop: even_pixels(orientation.to_source(visible), source_width, source_height),
        rotation: orientation.rotation,
        mirror: orientation.mirror,
        flip: orientation.flip,
        content,
        output_width,
        output_height,
    };
    let view = View {
        crop: visible,
        content: Rect {
            x: content.x as f32 / output_width as f32,
            y: content.y as f32 / output_height as f32,
            width: content.width as f32 / output_width as f32,
            height: content.height as f32 / output_height as f32,
        },
    };
    (plan, view)
}

/// The crop the user's zoom and pan describe, in oriented coordinates.
pub fn manual_crop(settings: &PictureSettings) -> Rect {
    let settings = settings.normalized();
    let size = 1.0 / settings.zoom;
    Rect::around(settings.center, size, size).inside_unit()
}

/// The crop that frames a face naturally, in oriented coordinates: the output's shape, the
/// face centred across and near the upper third, as tight as `framing` asks but never enlarged
/// more than [`MAX_TRACKING_UPSCALE`].
///
/// `face` is in oriented coordinates; `oriented` is the oriented picture's size in pixels.
pub fn tracking_crop(face: Rect, framing: f32, oriented: (u32, u32), output: (u32, u32)) -> Rect {
    let (picture_width, picture_height) = (oriented.0.max(1) as f32, oriented.1.max(1) as f32);
    let (output_width, output_height) = (output.0.max(1) as f32, output.1.max(1) as f32);
    let aspect = output_width / output_height;

    let face_height = face.height.max(0.0) * picture_height;
    let face_center = face.center();
    let share = FACE_SHARE_WIDE + (FACE_SHARE_CLOSE - FACE_SHARE_WIDE) * framing.clamp(0.0, 1.0);

    let mut height = (face_height / share).max(output_height / MAX_TRACKING_UPSCALE);
    let mut width = height * aspect;
    // Never larger than the picture itself.
    let shrink = (picture_width / width)
        .min(picture_height / height)
        .min(1.0);
    width *= shrink;
    height *= shrink;

    let x = (face_center.x * picture_width - width / 2.0).clamp(0.0, picture_width - width);
    let y =
        (face_center.y * picture_height - FACE_LINE * height).clamp(0.0, picture_height - height);
    Rect {
        x: x / picture_width,
        y: y / picture_height,
        width: width / picture_width,
        height: height / picture_height,
    }
}

// ---------------------------------------------------------------------------------------------
// Motion
// ---------------------------------------------------------------------------------------------

/// One value moved by a critically damped spring: it reaches its target as fast as it can
/// without overshooting, and keeps its velocity when the target changes mid-flight.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spring {
    pub position: f32,
    pub velocity: f32,
}

impl Spring {
    pub fn at(position: f32) -> Self {
        Self {
            position,
            velocity: 0.0,
        }
    }

    /// Advances `elapsed` seconds toward `target`. `response` is the period one oscillation
    /// would take without damping, in seconds; zero jumps straight to the target.
    pub fn step(&mut self, target: f32, response: f32, elapsed: f32) {
        if response <= 0.0 || !elapsed.is_finite() {
            *self = Self::at(target);
            return;
        }
        let elapsed = elapsed.max(0.0);
        let omega = std::f32::consts::TAU / response;
        let offset = self.position - target;
        let slope = self.velocity + omega * offset;
        let decay = (-omega * elapsed).exp();
        self.position = target + (offset + slope * elapsed) * decay;
        self.velocity = (self.velocity - omega * slope * elapsed) * decay;
    }

    fn is_settled(&self, target: f32) -> bool {
        (self.position - target).abs() < 1e-4 && self.velocity.abs() < 1e-3
    }
}

/// A crop moved by four springs: its centre and its size.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CropSpring {
    x: Spring,
    y: Spring,
    width: Spring,
    height: Spring,
}

impl CropSpring {
    fn at(rect: Rect) -> Self {
        let center = rect.center();
        Self {
            x: Spring::at(center.x),
            y: Spring::at(center.y),
            width: Spring::at(rect.width),
            height: Spring::at(rect.height),
        }
    }

    fn step(&mut self, target: Rect, response: f32, elapsed: f32) {
        let center = target.center();
        self.x.step(center.x, response, elapsed);
        self.y.step(center.y, response, elapsed);
        self.width.step(target.width, response, elapsed);
        self.height.step(target.height, response, elapsed);
    }

    fn is_settled(&self, target: Rect) -> bool {
        let center = target.center();
        self.x.is_settled(center.x)
            && self.y.is_settled(center.y)
            && self.width.is_settled(target.width)
            && self.height.is_settled(target.height)
    }

    fn rect(&self) -> Rect {
        Rect::around(
            Point {
                x: self.x.position,
                y: self.y.position,
            },
            self.width.position,
            self.height.position,
        )
        .inside_unit()
    }
}

/// What the phone last said about the person in view.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SubjectState {
    /// Whether face tracking is on.
    pub enabled: bool,
    /// The face in source coordinates, if one is in view.
    pub face: Option<Rect>,
    /// When the phone last reported, with or without a face.
    pub reported: Option<Instant>,
}

/// Where the tracked crop is heading. It only moves when the face leaves the dead zone.
#[derive(Debug, Default)]
struct Tracker {
    target: Option<Rect>,
}

impl Tracker {
    fn follow(&mut self, wanted: Rect) -> Rect {
        match self.target {
            Some(target) if !leaves_dead_zone(target, wanted) => target,
            _ => {
                self.target = Some(wanted);
                wanted
            }
        }
    }
}

/// Whether the crop `wanted` is different enough from `target` to move toward it.
fn leaves_dead_zone(target: Rect, wanted: Rect) -> bool {
    let (from, to) = (target.center(), wanted.center());
    let moved_x = (to.x - from.x).abs() / target.width.max(f32::EPSILON);
    let moved_y = (to.y - from.y).abs() / target.height.max(f32::EPSILON);
    let scaled = (wanted.height / target.height.max(f32::EPSILON) - 1.0).abs();
    moved_x > DEAD_ZONE_MOVE || moved_y > DEAD_ZONE_MOVE || scaled > DEAD_ZONE_SCALE
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Manual,
    Tracking,
    /// Easing back to the manual crop after tracking.
    Returning,
}

/// Decides the crop for each frame and moves it there smoothly.
#[derive(Debug)]
pub struct Framer {
    spring: Option<CropSpring>,
    tracker: Tracker,
    mode: Mode,
    last_face: Option<Instant>,
    last_step: Option<Instant>,
    /// What the springs' coordinates mean. When it changes, the crop jumps instead of
    /// travelling through a meaningless path.
    space: Option<(Orientation, (u32, u32))>,
}

impl Default for Framer {
    fn default() -> Self {
        Self::new()
    }
}

impl Framer {
    pub fn new() -> Self {
        Self {
            spring: None,
            tracker: Tracker::default(),
            mode: Mode::Manual,
            last_face: None,
            last_step: None,
            space: None,
        }
    }

    /// Whether the crop is following a face right now.
    pub fn is_tracking(&self) -> bool {
        self.mode == Mode::Tracking
    }

    /// The crop for a frame shown at `now`, in oriented coordinates. `immediate` puts the crop
    /// on its target at once, for a pan that follows the pointer.
    pub fn step(
        &mut self,
        now: Instant,
        settings: &PictureSettings,
        source: (u32, u32),
        subject: &SubjectState,
        immediate: bool,
    ) -> Rect {
        let settings = settings.normalized();
        let orientation = settings.orientation();
        let oriented = orientation.oriented_size(source.0, source.1);
        let output = (OUTPUT_WIDTH, OUTPUT_HEIGHT);

        let fresh = subject
            .reported
            .is_some_and(|reported| now.saturating_duration_since(reported) < FACE_STALE);
        let face = subject.face.filter(|_| subject.enabled && fresh);
        if face.is_some() {
            self.last_face = Some(now);
        }
        let in_grace = subject.enabled
            && self
                .last_face
                .is_some_and(|seen| now.saturating_duration_since(seen) < LOST_GRACE);

        let target = match (face, self.tracker.target) {
            (Some(face), _) => {
                self.mode = Mode::Tracking;
                let wanted = tracking_crop(
                    orientation.to_oriented(face),
                    settings.framing,
                    oriented,
                    output,
                );
                self.tracker.follow(wanted)
            }
            // The face just left: hold still for a moment in case it comes back.
            (None, Some(held)) if in_grace => held,
            _ => {
                if self.mode == Mode::Tracking {
                    self.mode = Mode::Returning;
                }
                self.tracker.target = None;
                self.last_face = None;
                manual_crop(&settings)
            }
        };

        let elapsed = self
            .last_step
            .map_or(0.0, |last| {
                now.saturating_duration_since(last).as_secs_f32()
            })
            .min(0.1);
        self.last_step = Some(now);

        let space = Some((orientation, source));
        let jump = immediate || self.space != space;
        self.space = space;
        let spring = self.spring.get_or_insert_with(|| CropSpring::at(target));
        if jump {
            *spring = CropSpring::at(target);
        } else {
            let response = match self.mode {
                Mode::Manual => response::MANUAL,
                Mode::Tracking => response::TRACKING,
                Mode::Returning => response::RETURN,
            };
            spring.step(target, response, elapsed);
        }
        if self.mode == Mode::Returning && spring.is_settled(target) {
            self.mode = Mode::Manual;
        }
        // Manual changes after tracking should feel direct again.
        if self.mode == Mode::Returning && immediate {
            self.mode = Mode::Manual;
        }
        spring.rect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn close_rect(a: Rect, b: Rect) -> bool {
        close(a.x, b.x) && close(a.y, b.y) && close(a.width, b.width) && close(a.height, b.height)
    }

    fn orientations() -> Vec<Orientation> {
        let mut all = Vec::new();
        for rotation in Rotation::ALL {
            for mirror in [false, true] {
                for flip in [false, true] {
                    all.push(Orientation {
                        rotation,
                        mirror,
                        flip,
                    });
                }
            }
        }
        all
    }

    #[test]
    fn rotations_turn_and_undo() {
        assert_eq!(Rotation::None.next(), Rotation::Clockwise90);
        assert_eq!(Rotation::Clockwise270.next(), Rotation::None);
        for rotation in Rotation::ALL {
            assert_eq!(
                Rotation::from_degrees((rotation.degrees() + rotation.inverse().degrees()) % 360),
                Some(Rotation::None)
            );
        }
        assert_eq!(
            serde_json::to_string(&Rotation::Clockwise270).unwrap(),
            "270"
        );
        assert_eq!(
            serde_json::from_str::<Rotation>("90").unwrap(),
            Rotation::Clockwise90
        );
        assert!(serde_json::from_str::<Rotation>("45").is_err());
    }

    #[test]
    fn rotation_swaps_the_dimensions() {
        let turned = Orientation {
            rotation: Rotation::Clockwise90,
            ..Default::default()
        };
        assert_eq!(turned.oriented_size(1920, 1080), (1080, 1920));
        let upside_down = Orientation {
            rotation: Rotation::HalfTurn,
            mirror: true,
            flip: false,
        };
        assert_eq!(upside_down.oriented_size(1920, 1080), (1920, 1080));
    }

    #[test]
    fn orientation_round_trips_every_rectangle() {
        let rect = Rect {
            x: 0.1,
            y: 0.2,
            width: 0.3,
            height: 0.4,
        };
        for orientation in orientations() {
            let oriented = orientation.to_oriented(rect);
            assert!(
                close_rect(orientation.to_source(oriented), rect),
                "{orientation:?}"
            );
        }
    }

    #[test]
    fn a_quarter_turn_moves_the_top_left_to_the_top_right() {
        let corner = Rect {
            x: 0.0,
            y: 0.0,
            width: 0.25,
            height: 0.5,
        };
        let turned = Orientation {
            rotation: Rotation::Clockwise90,
            ..Default::default()
        };
        assert!(close_rect(
            turned.to_oriented(corner),
            Rect {
                x: 0.5,
                y: 0.0,
                width: 0.5,
                height: 0.25,
            }
        ));
        let mirrored = Orientation {
            mirror: true,
            ..Default::default()
        };
        assert!(close_rect(
            mirrored.to_oriented(corner),
            Rect {
                x: 0.75,
                y: 0.0,
                width: 0.25,
                height: 0.5,
            }
        ));
    }

    #[test]
    fn letterbox_keeps_the_aspect_ratio() {
        assert_eq!(
            letterbox(1920, 1080, 1920, 1080),
            PixelRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080
            }
        );
        assert_eq!(
            letterbox(1080, 1920, 1920, 1080),
            PixelRect {
                x: 656,
                y: 0,
                width: 608,
                height: 1080
            }
        );
        assert_eq!(
            letterbox(1440, 1080, 1280, 720),
            PixelRect {
                x: 160,
                y: 0,
                width: 960,
                height: 720
            }
        );
    }

    #[test]
    fn the_default_plan_uses_the_whole_picture() {
        let (plan, view) = plan(
            Rect::FULL,
            &PictureSettings::default(),
            (1920, 1080),
            (1920, 1080),
        );
        let full = PixelRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(plan.crop, full);
        assert_eq!(plan.content, full);
        assert_eq!(plan.rotation, Rotation::None);
        assert!(close_rect(view.crop, Rect::FULL));
        assert!(close_rect(view.content, Rect::FULL));
    }

    #[test]
    fn fit_fill_and_stretch_place_a_portrait_picture() {
        let source = (1080, 1920);
        let output = (1920, 1080);
        let fit = PictureSettings::default();
        let (plan_fit, _) = plan(Rect::FULL, &fit, source, output);
        assert_eq!(plan_fit.content, letterbox(1080, 1920, 1920, 1080));
        assert_eq!(plan_fit.crop.height, 1920);

        let fill = PictureSettings {
            fit: FitMode::Fill,
            ..fit
        };
        let (plan_fill, view) = plan(Rect::FULL, &fill, source, output);
        assert_eq!(plan_fill.content.width, 1920);
        // The full width, a 16:9 band of the height around the middle.
        assert_eq!(plan_fill.crop.width, 1080);
        assert_eq!(plan_fill.crop.height, 608);
        assert_eq!(plan_fill.crop.y, 656);
        assert!(close(view.content.width, 1.0));

        let stretch = PictureSettings {
            fit: FitMode::Stretch,
            ..fit
        };
        let (plan_stretch, _) = plan(Rect::FULL, &stretch, source, output);
        assert_eq!(plan_stretch.content.width, 1920);
        assert_eq!(plan_stretch.crop.height, 1920);
    }

    #[test]
    fn a_turned_picture_crops_the_source_across() {
        // A landscape source turned a quarter is portrait on screen; the top of the screen is
        // the left of the source.
        let settings = PictureSettings {
            rotation: Rotation::Clockwise90,
            ..Default::default()
        };
        let top_half = Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 0.5,
        };
        let (plan, _) = plan(top_half, &settings, (1920, 1080), (1920, 1080));
        assert_eq!(
            plan.crop,
            PixelRect {
                x: 0,
                y: 0,
                width: 960,
                height: 1080
            }
        );
        // Oriented the crop is 1080 x 960, fitted into the frame with bars at the sides.
        assert_eq!(plan.content, letterbox(1080, 960, 1920, 1080));
        assert_eq!(plan.rotation, Rotation::Clockwise90);
    }

    #[test]
    fn plans_have_even_edges_inside_the_picture() {
        let crops = [
            Rect {
                x: 0.123,
                y: 0.377,
                width: 0.333,
                height: 0.271,
            },
            Rect {
                x: 0.999,
                y: 0.999,
                width: 0.001,
                height: 0.001,
            },
            Rect::FULL,
        ];
        for orientation in orientations() {
            for fit in [FitMode::Fit, FitMode::Fill, FitMode::Stretch] {
                for crop in crops {
                    for source in [(1920, 1080), (1179, 2556), (3840, 2160), (7, 5)] {
                        let settings = PictureSettings {
                            rotation: orientation.rotation,
                            mirror: orientation.mirror,
                            flip: orientation.flip,
                            fit,
                            ..Default::default()
                        };
                        let (plan, _) = plan(crop, &settings, source, (1920, 1080));
                        let c = plan.crop;
                        assert!(c.x % 2 == 0 && c.y % 2 == 0, "{plan:?}");
                        assert!(c.width % 2 == 0 && c.height % 2 == 0, "{plan:?}");
                        assert!(c.width >= 2 && c.height >= 2, "{plan:?}");
                        assert!(c.x + c.width <= source.0.max(2), "{plan:?} {source:?}");
                        assert!(c.y + c.height <= source.1.max(2), "{plan:?} {source:?}");
                        let o = plan.content;
                        assert!(o.x % 2 == 0 && o.y % 2 == 0 && o.width % 2 == 0);
                        assert!(o.x + o.width <= 1920 && o.y + o.height <= 1080);
                    }
                }
            }
        }
    }

    #[test]
    fn zoom_crops_around_the_centre_and_stays_inside() {
        let settings = PictureSettings {
            zoom: 2.0,
            ..Default::default()
        };
        assert!(close_rect(
            manual_crop(&settings),
            Rect {
                x: 0.25,
                y: 0.25,
                width: 0.5,
                height: 0.5,
            }
        ));
        let corner = PictureSettings {
            zoom: 4.0,
            center: Point { x: 0.0, y: 1.0 },
            ..Default::default()
        };
        let crop = manual_crop(&corner);
        assert!(close(crop.x, 0.0) && close(crop.y, 0.75) && close(crop.width, 0.25));

        let wild = PictureSettings {
            zoom: 100.0,
            framing: -3.0,
            center: Point {
                x: f32::NAN,
                y: 2.0,
            },
            ..Default::default()
        }
        .normalized();
        assert_eq!(wild.zoom, MAX_ZOOM);
        assert_eq!(wild.framing, 0.0);
        assert!(close(wild.center.x, 0.5) && close(wild.center.y, 1.0 - 0.5 / MAX_ZOOM));
    }

    #[test]
    fn panning_follows_the_pointer_and_stops_at_the_edge() {
        let settings = PictureSettings {
            zoom: 2.0,
            ..Default::default()
        };
        let (_, view) = plan(
            manual_crop(&settings),
            &settings,
            (1920, 1080),
            (1920, 1080),
        );
        // Dragging right by a tenth of the frame shows what was to the left.
        let panned = settings.panned(&view, 0.1, 0.0);
        assert!(close(panned.center.x, 0.45));
        assert!(close(panned.center.y, 0.5));
        let far = settings.panned(&view, -10.0, 10.0);
        assert!(close(far.center.x, 0.75) && close(far.center.y, 0.25));
    }

    fn face(x: f32, y: f32, size: f32) -> Rect {
        // Faces are taller than wide in a 16:9 picture's normalised coordinates.
        Rect {
            x,
            y,
            width: size * 9.0 / 16.0,
            height: size,
        }
    }

    #[test]
    fn tracking_frames_the_face_in_the_upper_third() {
        let picture = (3840, 2160);
        let output = (1920, 1080);
        let face = face(0.45, 0.30, 0.10);
        let crop = tracking_crop(face, 0.5, picture, output);

        // The output's shape.
        assert!(close(
            crop.width * 3840.0 / (crop.height * 2160.0),
            16.0 / 9.0
        ));
        // The face is centred across and its middle sits a little above the middle.
        let center = face.center();
        assert!(close(crop.center().x, center.x));
        let line = (center.y - crop.y) / crop.height;
        assert!((line - FACE_LINE).abs() < 1e-3, "{line}");
        // Closer framing gives a tighter crop.
        let wide = tracking_crop(face, 0.0, picture, output);
        let close_up = tracking_crop(face, 1.0, picture, output);
        assert!(wide.height > crop.height && crop.height > close_up.height);
    }

    #[test]
    fn tracking_never_enlarges_more_than_twice() {
        // A tiny face in a 1080p picture would want an extreme crop.
        let crop = tracking_crop(face(0.5, 0.5, 0.02), 1.0, (1920, 1080), (1920, 1080));
        assert!(crop.height * 1080.0 >= 1080.0 / MAX_TRACKING_UPSCALE - 0.5);
        assert!(crop.width * 1920.0 >= 1920.0 / MAX_TRACKING_UPSCALE - 0.5);
    }

    #[test]
    fn tracking_stays_inside_the_picture() {
        for (x, y) in [(0.0, 0.0), (0.95, 0.9), (0.0, 0.9), (0.95, 0.0)] {
            for picture in [(1920, 1080), (1080, 1920), (1280, 720)] {
                let crop = tracking_crop(face(x, y, 0.08), 0.5, picture, (1920, 1080));
                assert!(crop.x >= -1e-5 && crop.y >= -1e-5, "{crop:?}");
                assert!(crop.x + crop.width <= 1.0 + 1e-5, "{crop:?}");
                assert!(crop.y + crop.height <= 1.0 + 1e-5, "{crop:?}");
            }
        }
        // A huge face: the whole picture, no more.
        let crop = tracking_crop(face(0.0, 0.0, 1.0), 1.0, (1920, 1080), (1920, 1080));
        assert!(close_rect(crop, Rect::FULL));
    }

    #[test]
    fn small_head_movements_stay_in_the_dead_zone() {
        let mut tracker = Tracker::default();
        let picture = (1920, 1080);
        let output = (1920, 1080);
        let first = tracker.follow(tracking_crop(face(0.45, 0.3, 0.12), 0.5, picture, output));
        let nudged = tracker.follow(tracking_crop(face(0.46, 0.31, 0.12), 0.5, picture, output));
        assert_eq!(first, nudged);
        let moved = tracker.follow(tracking_crop(face(0.20, 0.3, 0.12), 0.5, picture, output));
        assert_ne!(first, moved);
        let leaned_in = tracker.follow(tracking_crop(face(0.20, 0.3, 0.2), 0.5, picture, output));
        assert_ne!(moved, leaned_in);
    }

    #[test]
    fn springs_settle_without_overshoot() {
        let mut spring = Spring::at(0.0);
        let mut previous = 0.0;
        let mut time = 0.0;
        while time < 1.0 {
            spring.step(1.0, response::MANUAL, 1.0 / 60.0);
            assert!(spring.position <= 1.0 + 1e-6, "overshot at {time}");
            assert!(spring.position >= previous, "went backwards at {time}");
            previous = spring.position;
            time += 1.0 / 60.0;
        }
        // About one and a half responses settles it.
        assert!(spring.is_settled(1.0), "{spring:?}");

        // The step size does not change where it ends up.
        let mut coarse = Spring::at(0.0);
        let mut fine = Spring::at(0.0);
        for _ in 0..10 {
            coarse.step(1.0, 0.5, 0.03);
        }
        for _ in 0..30 {
            fine.step(1.0, 0.5, 0.01);
        }
        assert!((coarse.position - fine.position).abs() < 1e-4);

        // Retargeting keeps the velocity: no jump in position.
        let mut moving = Spring::at(0.0);
        moving.step(1.0, 0.5, 0.1);
        let before = moving.position;
        moving.step(-1.0, 0.5, 0.0);
        assert!(close(moving.position, before));
        assert!(moving.velocity > 0.0);
    }

    fn subject(face: Option<Rect>, at: Instant) -> SubjectState {
        SubjectState {
            enabled: true,
            face,
            reported: Some(at),
        }
    }

    #[test]
    fn the_framer_follows_a_face_and_eases_back_after_losing_it() {
        let settings = PictureSettings::default();
        let source = (1920, 1080);
        let start = Instant::now();
        let frame = Duration::from_millis(16);
        let mut framer = Framer::new();

        // No face: the whole picture.
        let crop = framer.step(start, &settings, source, &SubjectState::default(), false);
        assert!(close_rect(crop, Rect::FULL));

        // A face on the left: the crop moves there smoothly and settles.
        let person = face(0.15, 0.3, 0.12);
        let wanted = tracking_crop(person, settings.framing, source, (1920, 1080));
        let mut now = start;
        let mut first_step = None;
        for _ in 0..240 {
            now += frame;
            let crop = framer.step(now, &settings, source, &subject(Some(person), now), false);
            first_step.get_or_insert(crop);
        }
        let first_step = first_step.unwrap();
        assert!(
            first_step.width > 0.9,
            "jumped instead of moving: {first_step:?}"
        );
        let tracked = framer.step(now, &settings, source, &subject(Some(person), now), false);
        assert!(close_rect(tracked, wanted), "{tracked:?} {wanted:?}");
        assert!(framer.is_tracking());

        // The face is lost: the crop holds for a moment...
        let lost_at = now;
        now += Duration::from_millis(500);
        let held = framer.step(now, &settings, source, &subject(None, lost_at), false);
        assert!(close_rect(held, wanted));

        // ...then eases back to the whole picture.
        for _ in 0..300 {
            now += frame;
            framer.step(now, &settings, source, &subject(None, now), false);
        }
        let back = framer.step(now, &settings, source, &subject(None, now), false);
        assert!(close_rect(back, Rect::FULL), "{back:?}");
        assert!(!framer.is_tracking());
    }

    #[test]
    fn turning_tracking_off_returns_without_waiting() {
        let settings = PictureSettings::default();
        let source = (1920, 1080);
        let mut now = Instant::now();
        let mut framer = Framer::new();
        let person = face(0.15, 0.3, 0.12);
        for _ in 0..120 {
            now += Duration::from_millis(16);
            framer.step(now, &settings, source, &subject(Some(person), now), false);
        }
        let off = SubjectState::default();
        now += Duration::from_millis(16);
        let first = framer.step(now, &settings, source, &off, false);
        for _ in 0..240 {
            now += Duration::from_millis(16);
            framer.step(now, &settings, source, &off, false);
        }
        assert!(first.width < 0.99, "jumped back");
        let back = framer.step(now, &settings, source, &off, false);
        assert!(close_rect(back, Rect::FULL));
    }

    #[test]
    fn zoom_changes_animate_and_pans_follow_at_once() {
        let source = (1920, 1080);
        let mut now = Instant::now();
        let mut framer = Framer::new();
        let mut settings = PictureSettings::default();
        framer.step(now, &settings, source, &SubjectState::default(), false);

        settings.zoom = 2.0;
        now += Duration::from_millis(16);
        let moving = framer.step(now, &settings, source, &SubjectState::default(), false);
        assert!(moving.width > 0.5 && moving.width < 1.0);
        for _ in 0..60 {
            now += Duration::from_millis(16);
            framer.step(now, &settings, source, &SubjectState::default(), false);
        }
        let settled = framer.step(now, &settings, source, &SubjectState::default(), false);
        assert!(close_rect(settled, manual_crop(&settings)));

        settings.center = Point { x: 0.3, y: 0.6 };
        now += Duration::from_millis(16);
        let panned = framer.step(now, &settings, source, &SubjectState::default(), true);
        assert!(close_rect(panned, manual_crop(&settings)));
    }

    #[test]
    fn settings_survive_a_round_trip_and_tolerate_gaps() {
        let settings = PictureSettings {
            mirror: true,
            rotation: Rotation::Clockwise270,
            fit: FitMode::Fill,
            zoom: 2.5,
            ..Default::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(
            serde_json::from_str::<PictureSettings>(&json).unwrap(),
            settings
        );
        let partial: PictureSettings = serde_json::from_str(r#"{"flip":true}"#).unwrap();
        assert!(partial.flip);
        assert_eq!(partial.zoom, 1.0);
    }
}
