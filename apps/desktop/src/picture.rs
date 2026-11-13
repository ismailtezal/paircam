//! The inspector's Picture and Tracking sections, and the notice that names a hand gesture on
//! the stage.
//!
//! The adjustments themselves live in the engine (`paircam_engine::picture`), which applies
//! them to the virtual camera and the preview alike; these controls only read and change them.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::base::{Spring, spring};
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderScale, SliderState};
use gpui_kit::component::{ActiveTheme as _, Icon, Selectable as _, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Div, Entity, FontWeight, IntoElement,
    ParentElement as _, SharedString, Styled as _, Subscription, Window, div, px,
};
use paircam_engine::picture::{MAX_ZOOM, MIN_ZOOM};
use paircam_engine::protocol::{ControlCapabilities, ControlValues, GestureKind};
use paircam_engine::{FitMode, PictureSettings, Rotation};
use paircam_ui::tokens::{font, motion, space};
use paircam_ui::{material, section_label};

use crate::app::PairCamApp;
use crate::controls::{field, switch_row, toggle, valued_field, worded_field};
use crate::text;

const FIT_MODES: [FitMode; 3] = [FitMode::Fit, FitMode::Fill, FitMode::Stretch];

/// The sliders, which keep state of their own between renders.
pub struct PictureControls {
    zoom: Entity<SliderState>,
    framing: Entity<SliderState>,
    _subscriptions: Vec<Subscription>,
}

impl PictureControls {
    pub fn new(settings: &PictureSettings, cx: &mut Context<PairCamApp>) -> Self {
        // Logarithmic, so each stretch of the track zooms by the same factor.
        let zoom = cx.new(|_| {
            SliderState::new()
                .min(MIN_ZOOM)
                .max(MAX_ZOOM)
                .scale(SliderScale::Logarithmic)
                .step(0.01)
                .default_value(settings.zoom)
        });
        let framing = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(1.0)
                .step(0.05)
                .default_value(settings.framing)
        });
        let subscriptions = vec![
            cx.subscribe(&zoom, |this, _, event: &SliderEvent, cx| {
                if let SliderEvent::Change(value) = event {
                    let zoom = value.start();
                    this.change_picture(|settings| settings.zoom = zoom, cx);
                }
            }),
            cx.subscribe(&framing, |this, _, event: &SliderEvent, cx| {
                if let SliderEvent::Change(value) = event {
                    let framing = value.start();
                    this.change_picture(|settings| settings.framing = framing, cx);
                }
            }),
        ];
        Self {
            zoom,
            framing,
            _subscriptions: subscriptions,
        }
    }

    /// Moves the sliders to settings changed elsewhere: the wheel, Reset, or another window.
    pub fn sync(&self, settings: &PictureSettings, window: &mut Window, cx: &mut App) {
        for (slider, value) in [
            (&self.zoom, settings.zoom),
            (&self.framing, settings.framing),
        ] {
            if (slider.read(cx).value().start() - value).abs() > 0.005 {
                slider.update(cx, |slider, cx| slider.set_value(value, window, cx));
            }
        }
    }

    /// The Picture section: what other apps see, adjusted.
    pub fn render_picture(
        &self,
        settings: &PictureSettings,
        tracking: bool,
        cx: &mut Context<PairCamApp>,
    ) -> Div {
        let changed = *settings != settings.reset();
        let rotation = ButtonGroup::new("picture-rotation")
            .small()
            .w_full()
            .children(Rotation::ALL.iter().enumerate().map(|(index, rotation)| {
                Button::new(("picture-rotation", index))
                    .flex_1()
                    .label(text!("picture.degrees", degrees = rotation.degrees()))
                    .selected(settings.rotation == *rotation)
            }))
            .on_click(cx.listener(|this, selected: &Vec<usize>, _, cx| {
                if let Some(rotation) = selected.first().and_then(|index| Rotation::ALL.get(*index))
                {
                    this.change_picture(|settings| settings.rotation = *rotation, cx);
                }
            }));
        let fit = ButtonGroup::new("picture-fit")
            .small()
            .w_full()
            .children(FIT_MODES.iter().enumerate().map(|(index, mode)| {
                Button::new(("picture-fit", index))
                    .flex_1()
                    .label(fit_label(*mode))
                    .selected(settings.fit == *mode)
            }))
            .on_click(cx.listener(|this, selected: &Vec<usize>, _, cx| {
                if let Some(mode) = selected.first().and_then(|index| FIT_MODES.get(*index)) {
                    this.change_picture(|settings| settings.fit = *mode, cx);
                }
            }));

        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .justify_between()
                    .child(section_label(text!("picture.title"), cx))
                    .when(changed, |header| {
                        header.child(
                            Button::new("picture-reset")
                                .ghost()
                                .xsmall()
                                .label(text!("picture.reset"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.change_picture(
                                        |settings| *settings = settings.reset(),
                                        cx,
                                    );
                                })),
                        )
                    }),
            )
            .child(
                valued_field(text!("picture.zoom"), format!("{:.1}×", settings.zoom), cx)
                    // Face tracking frames the picture by itself while it is on.
                    .child(Slider::new(&self.zoom).disabled(tracking)),
            )
            .child(field(text!("picture.rotation"), cx).child(rotation))
            .child(field(text!("picture.fit"), cx).child(fit))
            .child(switch_row(
                "picture-mirror",
                text!("picture.mirror"),
                settings.mirror,
                cx,
                |this, on, cx| this.change_picture(|settings| settings.mirror = on, cx),
            ))
            .child(switch_row(
                "picture-flip",
                text!("picture.flip"),
                settings.flip,
                cx,
                |this, on, cx| this.change_picture(|settings| settings.flip = on, cx),
            ))
    }

    /// The Tracking section, with only what the phone announced. `None` when it announced
    /// neither face tracking nor gestures.
    pub fn render_tracking(
        &self,
        capabilities: &ControlCapabilities,
        values: &ControlValues,
        settings: &PictureSettings,
        cx: &mut Context<PairCamApp>,
    ) -> Option<Div> {
        let gestures: Vec<GestureKind> = capabilities
            .gestures
            .iter()
            .copied()
            .filter(|kind| *kind != GestureKind::Unknown)
            .collect();
        if !capabilities.face_tracking && gestures.is_empty() {
            return None;
        }
        let muted = cx.theme().muted_foreground;
        let gesture_rows = gestures.iter().filter_map(|kind| {
            let (icon, description) = gesture_description(*kind)?;
            Some(
                div()
                    .h_flex()
                    .items_start()
                    .gap_2()
                    .text_color(muted)
                    .child(Icon::new(icon).size(px(font::size::MD)).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(font::size::SM))
                            .child(description),
                    ),
            )
        });

        Some(
            div()
                .v_flex()
                .gap_3()
                .child(section_label(text!("tracking.title"), cx))
                .when(capabilities.face_tracking, |section| {
                    section
                        .child(toggle(
                            "face-tracking",
                            text!("tracking.face"),
                            values.face_tracking.unwrap_or(false),
                            |on| ControlValues {
                                face_tracking: Some(on),
                                ..Default::default()
                            },
                            cx,
                        ))
                        .child(
                            worded_field(
                                text!("tracking.framing"),
                                framing_label(settings.framing),
                                cx,
                            )
                            .child(Slider::new(&self.framing)),
                        )
                })
                .when(!gestures.is_empty(), |section| {
                    section
                        .child(toggle(
                            "hand-gestures",
                            text!("tracking.gestures"),
                            values.gestures.unwrap_or(false),
                            |on| ControlValues {
                                gestures: Some(on),
                                ..Default::default()
                            },
                            cx,
                        ))
                        .child(div().v_flex().gap_1p5().children(gesture_rows))
                }),
        )
    }
}

fn fit_label(mode: FitMode) -> SharedString {
    match mode {
        FitMode::Fit => text!("picture.fit_fit"),
        FitMode::Fill => text!("picture.fit_fill"),
        FitMode::Stretch => text!("picture.fit_stretch"),
    }
}

fn framing_label(framing: f32) -> SharedString {
    if framing < 1.0 / 3.0 {
        text!("tracking.framing_wide")
    } else if framing < 2.0 / 3.0 {
        text!("tracking.framing_medium")
    } else {
        text!("tracking.framing_close")
    }
}

fn gesture_icon(kind: GestureKind) -> Option<IconName> {
    match kind {
        GestureKind::Palm => Some(IconName::Hand),
        GestureKind::Frame => Some(IconName::Frame),
        GestureKind::Cross => Some(IconName::X),
        GestureKind::Spread => Some(IconName::MoveHorizontal),
        GestureKind::Unknown => None,
    }
}

/// What a gesture does, for the Tracking section.
fn gesture_description(kind: GestureKind) -> Option<(IconName, SharedString)> {
    let description = match kind {
        GestureKind::Palm => text!("tracking.gesture_palm"),
        GestureKind::Frame => text!("tracking.gesture_frame"),
        GestureKind::Cross => text!("tracking.gesture_cross"),
        GestureKind::Spread => text!("tracking.gesture_spread"),
        GestureKind::Unknown => return None,
    };
    Some((gesture_icon(kind)?, description))
}

/// The gesture's name, for the notice on the stage.
fn gesture_name(kind: GestureKind) -> Option<SharedString> {
    match kind {
        GestureKind::Palm => Some(text!("gesture.palm")),
        GestureKind::Frame => Some(text!("gesture.frame")),
        GestureKind::Cross => Some(text!("gesture.cross")),
        GestureKind::Spread => Some(text!("gesture.spread")),
        GestureKind::Unknown => None,
    }
}

/// A brief notice on the stage naming the gesture the phone just acted on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GestureNotice {
    pub kind: GestureKind,
    /// Whether it should be showing; it fades out once this turns false.
    pub visible: bool,
    /// Tells notices apart, so a timer only hides the one it was started for.
    pub id: u64,
}

impl GestureNotice {
    /// How long a one-off gesture's notice stays.
    pub const HOLD: Duration = Duration::from_millis(motion::duration::NOTICE);
    /// How long a continuous gesture's notice stays after the hands stop.
    pub const LINGER: Duration = Duration::from_millis(motion::duration::SLOW);

    /// The notice, faded and lifted in and out with a spring. Nothing once fully faded out.
    pub fn render(notice: Option<&Self>, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        let visible = notice.is_some_and(|notice| notice.visible);
        let response = Duration::from_secs_f32(motion::spring::SMOOTH.response);
        let progress = spring(
            "gesture-notice",
            if visible { 1.0 } else { 0.0 },
            Spring::new(response).with_damping(motion::spring::SMOOTH.damping_fraction),
            window,
            cx,
        );
        let notice = notice?;
        if progress <= 0.01 {
            return None;
        }
        let name = gesture_name(notice.kind)?;
        let icon = gesture_icon(notice.kind)?;
        // It settles down into place from just above, and lifts away as it fades.
        let top = space::S_3 - (1.0 - progress) * space::S_2;

        Some(
            div()
                .absolute()
                .top(px(top))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .opacity(progress)
                .child(
                    material(cx)
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_1p5()
                        .child(Icon::new(icon).size(px(font::size::MD)))
                        .child(
                            div()
                                .text_size(px(font::size::SM))
                                .font_weight(FontWeight::MEDIUM)
                                .child(name),
                        ),
                )
                .into_any_element(),
        )
    }
}
