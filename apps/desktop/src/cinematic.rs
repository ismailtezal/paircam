//! Cinematic mode: its switch, blur and focus line in the inspector's Camera section, the focus
//! boxes over the preview, and what a click or a long press on the preview asks the phone for.
//!
//! The phone renders the effect into the frames (see "Cinematic mode" in `docs/PROTOCOL.md`);
//! the desktop turns it on, sets how strongly the background blurs, and directs the focus the
//! way Apple's Camera app does: click a subject to focus on it and click it again to hold the
//! focus there, click anywhere else to focus on whatever is there, and press and hold to keep
//! the focus at that distance.

use std::time::{Duration, Instant};

use gpui_kit::base::{Easing, Spring, spring};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Div, ElementId, Entity, IntoElement,
    ParentElement as _, Pixels, SharedString, SpringConfig, SpringState, Styled as _, Subscription,
    Window, div, point, px, size,
};
use paircam_engine::picture::{self, OUTPUT_HEIGHT, OUTPUT_WIDTH};
use paircam_engine::protocol::{
    CameraInfo, CinematicCapabilities, CinematicFocus, CinematicState, CinematicSubject,
    ControlValues, Range, Rect, SubjectFocus, SubjectKind,
};
use paircam_ui::tokens::{self, font, motion, radius};
use paircam_ui::{color, palette};

use crate::app::PairCamApp;
use crate::controls::{toggle, valued_field};
use crate::text;

/// Marked f-numbers a third of a stop apart, as lenses and Apple's own depth control show them.
const THIRD_STOPS: [f32; 31] = [
    1.0, 1.1, 1.2, 1.4, 1.6, 1.8, 2.0, 2.2, 2.5, 2.8, 3.2, 3.5, 4.0, 4.5, 5.0, 5.6, 6.3, 7.1, 8.0,
    9.0, 10.0, 11.0, 13.0, 14.0, 16.0, 18.0, 20.0, 22.0, 25.0, 29.0, 32.0,
];

/// A marked f-number closer than this to an end of the phone's range, in stops, gives way to the
/// end itself: half of a third of a stop.
const STOP_MARGIN: f32 = 1.0 / 6.0;

/// The square that marks a click on empty picture, as a share of the frame's height: about the
/// size of what the camera then follows.
const FOCUS_SQUARE: f32 = 0.14;

/// The click feedback starts this much larger and springs down into place, as the focus square
/// in the Camera app does.
const PULSE_SCALE: f32 = 1.4;

// ---------------------------------------------------------------------------------------------
// The blur
// ---------------------------------------------------------------------------------------------

/// How many stops apart two f-numbers are. A stop doubles or halves the light, which moves the
/// f-number by a factor of the square root of two.
fn stops_between(a: f32, b: f32) -> f32 {
    2.0 * (b / a).log2().abs()
}

/// The f-numbers the blur slider stops at, from the strongest blur (the smallest number) to the
/// sharpest: the phone's two ends and the marked third stops between them. F-numbers are
/// geometric, so evenly spaced stops make the slider logarithmic.
fn aperture_stops(range: Range) -> Vec<f32> {
    if !(range.min.is_finite() && range.max.is_finite()) {
        return Vec::new();
    }
    let (min, max) = (range.min.min(range.max), range.max.max(range.min));
    if min <= 0.0 {
        return Vec::new();
    }
    let apart = |a: f32, b: f32| stops_between(a, b) >= STOP_MARGIN;
    let mut stops = vec![min];
    stops.extend(
        THIRD_STOPS
            .into_iter()
            .filter(|stop| *stop > min && *stop < max && apart(min, *stop) && apart(*stop, max)),
    );
    if apart(min, max) {
        stops.push(max);
    }
    stops
}

/// The f-number at a slider position. The slider runs from the sharpest picture on the left to
/// the strongest blur on the right, so it reads the stops backwards.
fn aperture_at(stops: &[f32], position: f32) -> Option<f32> {
    let last = stops.len().checked_sub(1)?;
    let index = (position.round().max(0.0) as usize).min(last);
    stops.get(last - index).copied()
}

/// The slider position of the stop nearest to an f-number.
fn position_of(stops: &[f32], aperture: f32) -> f32 {
    let nearest = stops
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            stops_between(**a, aperture).total_cmp(&stops_between(**b, aperture))
        })
        .map_or(0, |(index, _)| index);
    stops.len().saturating_sub(1).saturating_sub(nearest) as f32
}

/// An f-number as photographers write it: "ƒ2.8", "ƒ11".
fn aperture_label(aperture: f32) -> String {
    let tenths = (aperture * 10.0).round() / 10.0;
    if tenths < 10.0 {
        format!("ƒ{tenths:.1}")
    } else {
        format!("ƒ{tenths:.0}")
    }
}

// ---------------------------------------------------------------------------------------------
// The focus
// ---------------------------------------------------------------------------------------------

/// Whether the phone can film in Cinematic mode with `camera`.
pub fn supported(capabilities: &CinematicCapabilities, camera: Option<&CameraInfo>) -> bool {
    camera.is_some_and(|camera| capabilities.positions.contains(&camera.position))
}

/// The subject the camera focuses on, if any. There is at most one.
fn focused(state: &CinematicState) -> Option<&CinematicSubject> {
    state
        .subjects
        .iter()
        .find(|subject| matches!(subject.focus, SubjectFocus::Weak | SubjectFocus::Strong))
}

/// Which subject is in focus and how firmly, to notice when the focus moves.
fn focus_key(state: &CinematicState) -> Option<(i64, SubjectFocus)> {
    focused(state).map(|subject| (subject.id, subject.focus))
}

/// What a click on a subject asks for, as a tap does in the Camera app: the first click focuses
/// on it, the next holds the focus there, and one more lets the camera rack away again.
pub fn focus_after_click(subject: &CinematicSubject) -> CinematicFocus {
    match (subject.kind, subject.focus) {
        // A focus held at a distance follows nothing, so it cannot loosen; letting go of it
        // hands the choice back to the camera.
        (SubjectKind::Fixed, _) => CinematicFocus::Auto,
        (_, SubjectFocus::Weak) => CinematicFocus::Subject {
            id: subject.id,
            strong: true,
        },
        _ => CinematicFocus::Subject {
            id: subject.id,
            strong: false,
        },
    }
}

/// Where the focus is held, in words, or `None` while the camera chooses by itself.
fn held_focus(state: &CinematicState) -> Option<SharedString> {
    let subject = focused(state).filter(|subject| subject.focus == SubjectFocus::Strong)?;
    Some(match subject.kind {
        SubjectKind::Face => text!("cinematic.held_face"),
        SubjectKind::Head => text!("cinematic.held_head"),
        SubjectKind::Body => text!("cinematic.held_body"),
        SubjectKind::Hand => text!("cinematic.held_hand"),
        SubjectKind::Animal => text!("cinematic.held_animal"),
        SubjectKind::Object => text!("cinematic.held_object"),
        SubjectKind::Fixed => text!("cinematic.held_distance"),
        SubjectKind::Unknown => text!("cinematic.held_subject"),
    })
}

// ---------------------------------------------------------------------------------------------
// The controls
// ---------------------------------------------------------------------------------------------

/// The Cinematic mode controls in the Camera section. The blur slider keeps state of its own
/// between renders.
pub struct CinematicControls {
    /// The f-numbers the blur slider stops at, smallest first.
    stops: Vec<f32>,
    /// `None` when the phone does not say which apertures it can simulate.
    blur: Option<Entity<SliderState>>,
    _subscriptions: Vec<Subscription>,
}

impl CinematicControls {
    pub fn new(
        capabilities: &CinematicCapabilities,
        values: &ControlValues,
        cx: &mut Context<PairCamApp>,
    ) -> Self {
        let stops = capabilities
            .aperture
            .map(aperture_stops)
            .unwrap_or_default();
        let mut subscriptions = Vec::new();
        let blur = (stops.len() > 1).then(|| {
            let start = values.aperture.map_or_else(
                || ((stops.len() - 1) / 2) as f32,
                |aperture| position_of(&stops, aperture),
            );
            let slider = cx.new(|_| {
                SliderState::new()
                    .min(0.0)
                    .max((stops.len() - 1) as f32)
                    .step(1.0)
                    .default_value(start)
            });
            let slider_stops = stops.clone();
            subscriptions.push(
                cx.subscribe(&slider, move |this, _, event: &SliderEvent, cx| {
                    // The slider reports every pointer move; the phone only hears new stops.
                    if let SliderEvent::Change(value) = event
                        && let Some(aperture) = aperture_at(&slider_stops, value.start())
                        && this.controls().aperture != Some(aperture)
                    {
                        this.set_control(
                            ControlValues {
                                aperture: Some(aperture),
                                ..Default::default()
                            },
                            cx,
                        );
                    }
                }),
            );
            slider
        });
        Self {
            stops,
            blur,
            _subscriptions: subscriptions,
        }
    }

    /// Moves the blur slider to an aperture the phone set by itself.
    pub fn sync(&self, values: &ControlValues, window: &mut Window, cx: &mut App) {
        let (Some(blur), Some(aperture)) = (self.blur.as_ref(), values.aperture) else {
            return;
        };
        let position = position_of(&self.stops, aperture);
        if (blur.read(cx).value().start() - position).abs() > 0.5 {
            blur.update(cx, |slider, cx| slider.set_value(position, window, cx));
        }
    }

    /// The switch, and while it is on, the blur, where the focus is and how to move it.
    pub fn render(
        &self,
        values: &ControlValues,
        state: Option<&CinematicState>,
        cx: &mut Context<PairCamApp>,
    ) -> Collapsible {
        let on = values.cinematic == Some(true);
        let muted = cx.theme().muted_foreground;
        let held = state.and_then(held_focus);
        let low_light = state.is_some_and(|state| state.low_light);
        let aperture = values.aperture.or_else(|| {
            let blur = self.blur.as_ref()?;
            aperture_at(&self.stops, blur.read(cx).value().start())
        });

        let focus = div()
            .h_flex()
            .items_center()
            .justify_between()
            .gap_2()
            // As tall as the button, so the line keeps its place when the button comes.
            .min_h_5()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(font::size::SM))
                    .text_color(muted)
                    .child(
                        held.clone()
                            .unwrap_or_else(|| text!("cinematic.focus_automatic")),
                    ),
            )
            .when(held.is_some(), |line| {
                line.child(
                    Button::new("cinematic-automatic")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .label(text!("cinematic.automatic"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_cinematic_focus(CinematicFocus::Auto, cx);
                        })),
                )
            });
        let content = div()
            .v_flex()
            .w_full()
            .gap_3()
            .pt_3()
            .when_some(self.blur.as_ref(), |content, blur| {
                content.child(
                    valued_field(
                        text!("cinematic.blur"),
                        aperture.map(aperture_label).unwrap_or_default(),
                        cx,
                    )
                    .child(Slider::new(blur)),
                )
            })
            .child(
                div().v_flex().gap_1().child(focus).child(
                    div()
                        .text_size(px(font::size::SM))
                        .text_color(muted)
                        .child(text!("cinematic.how_to_focus")),
                ),
            )
            .when(low_light, |content| {
                content.child(
                    div()
                        .text_size(px(font::size::SM))
                        .text_color(color(palette(cx).warning))
                        .child(text!("cinematic.low_light")),
                )
            });

        Collapsible::new()
            .motion_id("cinematic")
            .w_full()
            .open(on)
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(toggle(
                        "cinematic",
                        text!("cinematic.mode"),
                        on,
                        |on| ControlValues {
                            cinematic: Some(on),
                            ..Default::default()
                        },
                        cx,
                    ))
                    .child(
                        div()
                            .text_size(px(font::size::SM))
                            .text_color(muted)
                            .child(text!("cinematic.hint")),
                    ),
            )
            .content(content)
    }
}

// ---------------------------------------------------------------------------------------------
// Over the preview
// ---------------------------------------------------------------------------------------------

/// How a focus box is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusMark {
    /// Something the camera could focus on.
    Candidate,
    /// In focus, until the camera finds something that matters more.
    Weak,
    /// In focus until it leaves the picture, or held at a distance.
    Held,
}

impl FocusMark {
    fn of(focus: SubjectFocus) -> Self {
        match focus {
            SubjectFocus::Weak => Self::Weak,
            SubjectFocus::Strong => Self::Held,
            SubjectFocus::None | SubjectFocus::Unknown => Self::Candidate,
        }
    }
}

/// A focus box as Apple's Cinematic camera draws one: a thin white frame around something the
/// camera could focus on, and a yellow one around the subject in focus, dashed while the camera
/// may still rack away and solid once the focus is held. It always sits over the camera
/// picture, so it takes the dark palette whatever the theme.
pub fn focus_box(mark: FocusMark) -> Div {
    let picture = &tokens::DARK;
    let frame = div().rounded(px(radius::XS));
    match mark {
        FocusMark::Candidate => frame.border_1().border_color(color(picture.text)),
        FocusMark::Weak => frame
            .border_2()
            .border_dashed()
            .border_color(color(picture.focus)),
        FocusMark::Held => frame.border_2().border_color(color(picture.focus)),
    }
}

/// Where the picture sits on the stage.
#[derive(Clone, Copy, Debug)]
pub struct PictureLayout {
    /// The 16:9 frame other apps get, in the stage's own pixels.
    pub frame: Bounds<Pixels>,
    /// The part of the frame the camera picture covers, as fractions of it. The rest is bars,
    /// where nothing is drawn.
    pub area: picture::Rect,
}

/// A rectangle given as fractions of `frame`, in the pixels `frame` is measured in.
fn within(frame: Bounds<Pixels>, rect: picture::Rect) -> Bounds<Pixels> {
    Bounds::new(
        point(
            frame.origin.x + frame.size.width * rect.x,
            frame.origin.y + frame.size.height * rect.y,
        ),
        size(
            frame.size.width * rect.width,
            frame.size.height * rect.height,
        ),
    )
}

/// `element` pinned to `bounds`, which are measured from its parent's top left corner.
fn pinned(element: Div, bounds: Bounds<Pixels>) -> Div {
    element
        .absolute()
        .left(bounds.origin.x)
        .top(bounds.origin.y)
        .w(bounds.size.width)
        .h(bounds.size.height)
}

fn contains(rect: &picture::Rect, point: picture::Point) -> bool {
    (rect.x..=rect.x + rect.width).contains(&point.x)
        && (rect.y..=rect.y + rect.height).contains(&point.y)
}

/// The square that marks a click on empty picture, centred on it, as fractions of the frame.
pub fn focus_square(center: picture::Point) -> picture::Rect {
    let height = FOCUS_SQUARE;
    let width = FOCUS_SQUARE * OUTPUT_HEIGHT as f32 / OUTPUT_WIDTH as f32;
    picture::Rect {
        x: center.x - width / 2.0,
        y: center.y - height / 2.0,
        width,
        height,
    }
}

/// A subject's rectangle in the encoded picture, eased toward each new report: the phone sends
/// them ten times a second, which would make the boxes step.
fn smoothed(subject: &CinematicSubject, window: &mut Window, cx: &mut App) -> Rect {
    let id = ElementId::from(("cinematic-subject", subject.id as u64));
    let snappy = motion::spring::SNAPPY;
    let policy =
        Spring::new(Duration::from_secs_f32(snappy.response)).with_damping(snappy.damping_fraction);
    let mut follow = |channel: &'static str, target: f32| {
        spring((id.clone(), channel), target, policy, window, cx)
    };
    Rect {
        x: follow("x", subject.rect.x),
        y: follow("y", subject.rect.y),
        width: follow("width", subject.rect.width),
        height: follow("height", subject.rect.height),
    }
}

/// The square that marks the last click or long press.
#[derive(Clone, Copy, Debug)]
struct Pulse {
    /// Where it lands, as fractions of the frame.
    rect: picture::Rect,
    mark: FocusMark,
    started: Instant,
}

impl Pulse {
    /// How the square looks now: its scale, springing down into place, and its opacity, which
    /// holds and then fades. `None` once it is gone.
    fn sample(&self, reduce_motion: bool) -> Option<(f32, f32)> {
        let age = self.started.elapsed().as_secs_f32();
        let hold = Duration::from_millis(motion::duration::SLOW).as_secs_f32();
        let fade = Duration::from_millis(motion::duration::BASE).as_secs_f32();
        if age >= hold + fade {
            return None;
        }
        let scale = if reduce_motion {
            1.0
        } else {
            let snappy = motion::spring::SNAPPY;
            let start = SpringState {
                position: PULSE_SCALE,
                velocity: 0.0,
            };
            SpringConfig::new(snappy.stiffness, snappy.damping, 1.0)
                .step(start, 1.0, age)
                .position
        };
        let opacity = if age <= hold {
            1.0
        } else {
            let [x1, y1, x2, y2] = motion::easing::OUT;
            let fading = (age - hold) / fade;
            let eased =
                Easing::cubic_bezier(x1, y1, x2, y2).map_or(fading, |out| out.sample(fading));
            1.0 - eased
        };
        Some((scale, opacity))
    }
}

/// What Cinematic mode shows over the preview: the subjects the phone reports, framed while the
/// pointer is over the picture and for a moment after the focus moves, and a brief square where
/// the user clicked.
#[derive(Default)]
pub struct FocusOverlay {
    /// What the phone last reported, while Cinematic mode is on.
    state: Option<CinematicState>,
    /// Whether the pointer is over the preview.
    hovered: bool,
    /// The boxes stay until then after the focus moved, then fade.
    shown_until: Option<Instant>,
    pulse: Option<Pulse>,
}

impl FocusOverlay {
    pub fn state(&self) -> Option<&CinematicState> {
        self.state.as_ref()
    }

    /// Takes what the phone reports, and shows the boxes for a moment when the focus moved:
    /// racked by itself, followed a gesture, or went where the user clicked.
    pub fn update(&mut self, state: CinematicState, cx: &mut Context<PairCamApp>) {
        let moved = self.state.as_ref().and_then(focus_key) != focus_key(&state);
        self.state = Some(state);
        if moved {
            self.show_for_a_moment(cx);
        }
    }

    /// Forgets what the phone reported, when Cinematic mode goes off.
    pub fn clear(&mut self) {
        self.state = None;
        self.shown_until = None;
        self.pulse = None;
    }

    pub fn set_hovered(&mut self, hovered: bool) {
        self.hovered = hovered;
    }

    /// Shows the boxes for as long as a notice stays, then lets them fade.
    pub fn show_for_a_moment(&mut self, cx: &mut Context<PairCamApp>) {
        let linger = Duration::from_millis(motion::duration::NOTICE);
        self.shown_until = Some(Instant::now() + linger);
        // Draw again when the time is up, so they fade even if no new picture arrives.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(linger).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    /// Marks where the focus was sent, with the square springing into place on `rect` (as
    /// fractions of the frame).
    pub fn pulse(&mut self, rect: picture::Rect, mark: FocusMark) {
        self.pulse = Some(Pulse {
            rect,
            mark,
            started: Instant::now(),
        });
    }

    /// The subject under a point of the frame, with where it shows in the frame. When several
    /// overlap (a face inside a body), the smallest is the one meant.
    pub fn subject_at(
        &self,
        point: picture::Point,
        place: impl Fn(Rect) -> Option<picture::Rect>,
    ) -> Option<(CinematicSubject, picture::Rect)> {
        self.state
            .as_ref()?
            .subjects
            .iter()
            .filter_map(|subject| Some((*subject, place(subject.rect)?)))
            .filter(|(_, shown)| contains(shown, point))
            .min_by(|(_, a), (_, b)| (a.width * a.height).total_cmp(&(b.width * b.height)))
    }

    /// The boxes and the click feedback, for the stage. `place` puts a rectangle of the encoded
    /// picture into the frame. `active` is false while there is nothing to direct, and
    /// `pressing` while a press on the preview is under way.
    ///
    /// Call it on every render of the stage, so the boxes fade in and out from where they are.
    pub fn render(
        &self,
        layout: PictureLayout,
        active: bool,
        pressing: bool,
        place: impl Fn(Rect) -> Option<picture::Rect>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let PictureLayout { frame, area } = layout;
        let subjects = self
            .state
            .as_ref()
            .map_or(&[][..], |state| state.subjects.as_slice());
        let shown = active
            && !subjects.is_empty()
            && (self.hovered || self.shown_until.is_some_and(|until| Instant::now() < until));
        let smooth = motion::spring::SMOOTH;
        let opacity = spring(
            "cinematic-boxes",
            if shown { 1.0 } else { 0.0 },
            Spring::new(Duration::from_secs_f32(smooth.response))
                .with_damping(smooth.damping_fraction),
            window,
            cx,
        );
        let pulse = self
            .pulse
            .filter(|_| active)
            .and_then(|pulse| Some((pulse, pulse.sample(cx.reduce_motion())?)));
        let boxes_drawn = opacity > 0.01;
        if !boxes_drawn && pulse.is_none() {
            return None;
        }

        let area = within(frame, area);
        // Measured from the picture area, which clips them.
        let in_area = |rect: picture::Rect| {
            let bounds = within(frame, rect);
            Bounds::new(bounds.origin - area.origin, bounds.size)
        };

        let mut boxes = Vec::new();
        if boxes_drawn {
            for subject in subjects {
                let rect = smoothed(subject, window, cx);
                let Some(rect) = place(rect) else {
                    continue;
                };
                boxes.push(
                    pinned(focus_box(FocusMark::of(subject.focus)), in_area(rect))
                        // A hand over what a click would focus on, unless a press is moving
                        // the picture.
                        .when(shown && !pressing, |frame| frame.cursor_pointer()),
                );
            }
        }
        let pulse = pulse.map(|(pulse, (scale, pulse_opacity))| {
            window.request_animation_frame();
            let center = pulse.rect.center();
            let (width, height) = (pulse.rect.width * scale, pulse.rect.height * scale);
            let rect = picture::Rect {
                x: center.x - width / 2.0,
                y: center.y - height / 2.0,
                width,
                height,
            };
            pinned(focus_box(pulse.mark), in_area(rect)).opacity(pulse_opacity)
        });

        Some(
            pinned(div(), area)
                .overflow_hidden()
                .when(boxes_drawn, |area| {
                    area.child(div().absolute().inset_0().opacity(opacity).children(boxes))
                })
                .children(pulse)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(min: f32, max: f32) -> Range {
        Range { min, max }
    }

    #[test]
    fn the_blur_stops_at_marked_third_stops_between_the_ends() {
        assert_eq!(
            aperture_stops(range(2.0, 16.0)),
            [
                2.0, 2.2, 2.5, 2.8, 3.2, 3.5, 4.0, 4.5, 5.0, 5.6, 6.3, 7.1, 8.0, 9.0, 10.0, 11.0,
                13.0, 14.0, 16.0
            ]
        );
        // Ends off the marks are kept, and marks crowding them give way.
        assert_eq!(aperture_stops(range(2.4, 4.1)), [2.4, 2.8, 3.2, 3.5, 4.1]);
        assert_eq!(aperture_stops(range(4.0, 4.0)), [4.0]);
        assert_eq!(aperture_stops(range(16.0, 2.0)).first(), Some(&2.0));
        assert!(aperture_stops(range(0.0, 16.0)).is_empty());
        assert!(aperture_stops(range(2.0, f32::NAN)).is_empty());
    }

    #[test]
    fn the_slider_runs_from_sharp_on_the_left_to_blurred_on_the_right() {
        let stops = aperture_stops(range(2.0, 16.0));
        let last = (stops.len() - 1) as f32;
        assert_eq!(aperture_at(&stops, 0.0), Some(16.0));
        assert_eq!(aperture_at(&stops, last), Some(2.0));
        assert_eq!(aperture_at(&stops, last + 3.0), Some(2.0));
        assert_eq!(aperture_at(&stops, -1.0), Some(16.0));
        assert_eq!(aperture_at(&[], 0.0), None);
        for (index, stop) in stops.iter().enumerate() {
            let position = position_of(&stops, *stop);
            assert_eq!(position, last - index as f32);
            assert_eq!(aperture_at(&stops, position), Some(*stop));
        }
        // A value between marks lands on the nearest one, measured in stops.
        assert_eq!(aperture_at(&stops, position_of(&stops, 2.83)), Some(2.8));
        assert_eq!(aperture_at(&stops, position_of(&stops, 11.6)), Some(11.0));
        assert_eq!(aperture_at(&stops, position_of(&stops, 12.0)), Some(13.0));
    }

    #[test]
    fn apertures_read_like_lens_markings() {
        assert_eq!(aperture_label(2.0), "ƒ2.0");
        assert_eq!(aperture_label(2.8), "ƒ2.8");
        assert_eq!(aperture_label(5.6), "ƒ5.6");
        assert_eq!(aperture_label(9.98), "ƒ10");
        assert_eq!(aperture_label(11.0), "ƒ11");
        assert_eq!(aperture_label(16.0), "ƒ16");
    }

    fn subject(id: i64, kind: SubjectKind, focus: SubjectFocus) -> CinematicSubject {
        CinematicSubject {
            id,
            kind,
            rect: Rect::default(),
            focus,
        }
    }

    #[test]
    fn clicking_a_subject_focuses_holds_and_lets_go() {
        let face = |focus| subject(12, SubjectKind::Face, focus);
        assert_eq!(
            focus_after_click(&face(SubjectFocus::None)),
            CinematicFocus::Subject {
                id: 12,
                strong: false
            }
        );
        assert_eq!(
            focus_after_click(&face(SubjectFocus::Weak)),
            CinematicFocus::Subject {
                id: 12,
                strong: true
            }
        );
        assert_eq!(
            focus_after_click(&face(SubjectFocus::Strong)),
            CinematicFocus::Subject {
                id: 12,
                strong: false
            }
        );
        assert_eq!(
            focus_after_click(&subject(4, SubjectKind::Fixed, SubjectFocus::Strong)),
            CinematicFocus::Auto
        );
    }

    #[test]
    fn the_focus_moves_when_another_subject_or_hold_takes_it() {
        let state = |subjects: Vec<CinematicSubject>| CinematicState {
            subjects,
            low_light: false,
        };
        let face = subject(1, SubjectKind::Face, SubjectFocus::Weak);
        let hand = subject(2, SubjectKind::Hand, SubjectFocus::None);
        let auto = state(vec![face, hand]);
        assert_eq!(focus_key(&auto), Some((1, SubjectFocus::Weak)));
        assert_eq!(focus_key(&state(Vec::new())), None);

        let held = state(vec![
            subject(1, SubjectKind::Face, SubjectFocus::Strong),
            hand,
        ]);
        assert_ne!(focus_key(&auto), focus_key(&held));
        assert!(held_focus(&auto).is_none());
        assert!(held_focus(&held).is_some());
    }

    #[test]
    fn the_smallest_subject_under_a_click_is_the_one_meant() {
        let mut body = subject(1, SubjectKind::Body, SubjectFocus::None);
        body.rect = Rect {
            x: 0.2,
            y: 0.2,
            width: 0.6,
            height: 0.8,
        };
        let mut face = subject(2, SubjectKind::Face, SubjectFocus::Weak);
        face.rect = Rect {
            x: 0.4,
            y: 0.25,
            width: 0.2,
            height: 0.2,
        };
        let overlay = FocusOverlay {
            state: Some(CinematicState {
                subjects: vec![body, face],
                low_light: false,
            }),
            ..FocusOverlay::default()
        };
        let place = |rect: Rect| {
            Some(picture::Rect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            })
        };
        let at = |x, y| overlay.subject_at(picture::Point { x, y }, place);
        assert_eq!(at(0.5, 0.3).map(|(subject, _)| subject.id), Some(2));
        assert_eq!(at(0.5, 0.7).map(|(subject, _)| subject.id), Some(1));
        assert!(at(0.1, 0.1).is_none());
        // A subject that is not in view cannot be clicked.
        assert!(
            overlay
                .subject_at(picture::Point { x: 0.5, y: 0.3 }, |_| None)
                .is_none()
        );
    }

    #[test]
    fn the_focus_square_is_square_on_the_frame() {
        let square = focus_square(picture::Point { x: 0.5, y: 0.5 });
        let width = square.width * OUTPUT_WIDTH as f32;
        let height = square.height * OUTPUT_HEIGHT as f32;
        assert!((width - height).abs() < 0.5);
        assert!((square.center().x - 0.5).abs() < 1e-6);
        assert!((square.center().y - 0.5).abs() < 1e-6);
    }

    #[test]
    fn the_click_square_springs_into_place_and_fades() {
        let pulse = |age_ms: u64| Pulse {
            rect: focus_square(picture::Point { x: 0.5, y: 0.5 }),
            mark: FocusMark::Weak,
            started: Instant::now() - Duration::from_millis(age_ms),
        };
        let (scale, opacity) = pulse(0).sample(false).unwrap();
        assert!(scale > 1.3 && opacity == 1.0);
        let (scale, opacity) = pulse(motion::duration::SLOW).sample(false).unwrap();
        assert!((scale - 1.0).abs() < 0.05 && opacity > 0.9);
        let (_, opacity) = pulse(motion::duration::SLOW + motion::duration::BASE / 2)
            .sample(false)
            .unwrap();
        assert!(opacity < 0.5);
        assert!(
            pulse(motion::duration::SLOW + motion::duration::BASE)
                .sample(false)
                .is_none()
        );
        // Without motion it only fades.
        assert_eq!(pulse(0).sample(true), Some((1.0, 1.0)));
    }

    #[test]
    fn every_cinematic_text_is_in_both_languages() {
        for key in [
            "cinematic.mode",
            "cinematic.hint",
            "cinematic.blur",
            "cinematic.focus_automatic",
            "cinematic.held_face",
            "cinematic.held_head",
            "cinematic.held_body",
            "cinematic.held_hand",
            "cinematic.held_animal",
            "cinematic.held_object",
            "cinematic.held_distance",
            "cinematic.held_subject",
            "cinematic.automatic",
            "cinematic.how_to_focus",
            "cinematic.low_light",
            "tracking.gesture_palm_cinematic",
        ] {
            let english = rust_i18n::t!(key, locale = "en");
            let turkish = rust_i18n::t!(key, locale = "tr");
            // A missing key reads back as itself, after the locale.
            assert!(!english.ends_with(key), "{key} is missing in English");
            // Turkish falls back to English when a key is missing.
            assert_ne!(turkish, english, "{key} is missing in Turkish");
        }
    }
}
