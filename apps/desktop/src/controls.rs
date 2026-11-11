//! The inspector's stream and camera controls: what to ask the phone for, and live adjustments.

use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, IndexPath, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Div, Entity, IntoElement, ParentElement as _, SharedString,
    Styled as _, Subscription, Window, div, px,
};
use paircam_engine::StreamPreferences;
use paircam_engine::protocol::{
    Capabilities, Codec, ControlValues, FocusMode, Format, Lens, Position, WhiteBalanceMode,
};
use paircam_ui::section_label;
use paircam_ui::tokens::font;

use crate::app::PairCamApp;
use crate::text;

type Choice = Entity<SelectState<Vec<SharedString>>>;

/// Bitrates offered as "Quality", in bits per second.
const QUALITIES: [(&str, u32); 4] = [
    ("quality.low", 6_000_000),
    ("quality.medium", 12_000_000),
    ("quality.high", 20_000_000),
    ("quality.maximum", 40_000_000),
];

const FRAME_RATES: [u32; 2] = [30, 60];

pub struct StreamControls {
    capabilities: Capabilities,
    camera_index: usize,
    /// The chosen camera's formats, largest first.
    formats: Vec<Format>,
    /// The frame rates the chosen format supports.
    frame_rates: Vec<u32>,
    camera: Choice,
    resolution: Choice,
    frame_rate: Choice,
    quality: Choice,
    codec: Choice,
    zoom: Option<Entity<SliderState>>,
    exposure: Option<Entity<SliderState>>,
    _subscriptions: Vec<Subscription>,
}

fn pixels(format: &Format) -> u64 {
    u64::from(format.width) * u64::from(format.height)
}

fn resolution_label(format: &Format) -> SharedString {
    let name = match (format.width, format.height) {
        (3840, 2160) => " · 4K",
        (1920, 1080) => " · Full HD",
        (1280, 720) => " · HD",
        _ => "",
    };
    format!("{} × {}{name}", format.width, format.height).into()
}

fn codec_label(codec: Codec) -> SharedString {
    match codec {
        Codec::Hevc => text!("codec.hevc"),
        Codec::H264 => text!("codec.h264"),
    }
}

impl StreamControls {
    pub fn new(
        capabilities: Capabilities,
        preferences: &StreamPreferences,
        values: &ControlValues,
        window: &mut Window,
        cx: &mut Context<PairCamApp>,
    ) -> Option<Self> {
        let cameras = &capabilities.cameras;
        let camera_index = preferences
            .camera
            .as_deref()
            .and_then(|id| cameras.iter().position(|camera| camera.id == id))
            .or_else(|| {
                cameras.iter().position(|camera| {
                    camera.position == Position::Back && camera.lens == Lens::Wide
                })
            })
            .unwrap_or(0);
        let chosen_camera = cameras.get(camera_index)?;

        let mut formats = chosen_camera.formats.clone();
        formats.sort_by_key(|format| std::cmp::Reverse(pixels(format)));
        let wanted = u64::from(preferences.width) * u64::from(preferences.height);
        let format_index = formats
            .iter()
            .position(|format| {
                format.width == preferences.width && format.height == preferences.height
            })
            .or_else(|| formats.iter().position(|format| pixels(format) <= wanted))
            .unwrap_or(0);
        let chosen_format = formats.get(format_index)?;

        let mut frame_rates: Vec<u32> = FRAME_RATES
            .into_iter()
            .filter(|rate| *rate <= chosen_format.max_fps)
            .collect();
        if frame_rates.is_empty() {
            frame_rates.push(chosen_format.max_fps.max(1));
        }
        let frame_rate_index = frame_rates
            .iter()
            .rposition(|rate| *rate <= preferences.fps)
            .unwrap_or(0);

        let quality_index = QUALITIES
            .iter()
            .rposition(|(_, bitrate)| *bitrate <= preferences.bitrate)
            .unwrap_or(0);
        let codec_index = capabilities
            .codecs
            .iter()
            .position(|codec| *codec == preferences.codec)
            .unwrap_or(0);

        let mut subscriptions = Vec::new();
        let mut choice =
            |labels: Vec<SharedString>, selected: usize, cx: &mut Context<PairCamApp>| {
                let state = cx.new(|cx| {
                    SelectState::new(labels, Some(IndexPath::default().row(selected)), window, cx)
                });
                subscriptions.push(cx.subscribe_in(
                    &state,
                    window,
                    |this, _, _: &SelectEvent<Vec<SharedString>>, window, cx| {
                        this.apply_stream_selection(window, cx);
                    },
                ));
                state
            };

        let camera = choice(
            cameras
                .iter()
                .map(|camera| camera.name.clone().into())
                .collect(),
            camera_index,
            cx,
        );
        let resolution = choice(
            formats.iter().map(resolution_label).collect(),
            format_index,
            cx,
        );
        let frame_rate = choice(
            frame_rates
                .iter()
                .map(|rate| text!("inspector.frame_rate_value", fps = rate))
                .collect(),
            frame_rate_index,
            cx,
        );
        let quality = choice(
            QUALITIES.iter().map(|(key, _)| text!(*key)).collect(),
            quality_index,
            cx,
        );
        let codec = choice(
            capabilities
                .codecs
                .iter()
                .copied()
                .map(codec_label)
                .collect(),
            codec_index,
            cx,
        );

        let mut slider = |range: paircam_engine::protocol::Range,
                          value: f32,
                          apply: fn(f32) -> ControlValues,
                          cx: &mut Context<PairCamApp>| {
            let state = cx.new(|_| {
                SliderState::new()
                    .min(range.min)
                    .max(range.max)
                    .step(0.1)
                    .default_value(value.clamp(range.min, range.max))
            });
            subscriptions.push(
                cx.subscribe(&state, move |this, _, event: &SliderEvent, cx| {
                    if let SliderEvent::Change(value) = event {
                        this.set_control(apply(value.start()), cx);
                    }
                }),
            );
            state
        };
        let zoom = capabilities.controls.zoom.map(|range| {
            slider(
                range,
                values.zoom.unwrap_or(range.min),
                |zoom| ControlValues {
                    zoom: Some(zoom),
                    ..Default::default()
                },
                cx,
            )
        });
        let exposure = capabilities.controls.exposure_bias.map(|range| {
            slider(
                range,
                values.exposure_bias.unwrap_or(0.0),
                |exposure_bias| ControlValues {
                    exposure_bias: Some(exposure_bias),
                    ..Default::default()
                },
                cx,
            )
        });

        Some(Self {
            capabilities,
            camera_index,
            formats,
            frame_rates,
            camera,
            resolution,
            frame_rate,
            quality,
            codec,
            zoom,
            exposure,
            _subscriptions: subscriptions,
        })
    }

    /// Moves the sliders to values the phone changed by itself, such as a zoom gesture.
    pub fn sync(&self, values: &ControlValues, window: &mut Window, cx: &mut App) {
        for (slider, value) in [
            (self.zoom.as_ref(), values.zoom),
            (self.exposure.as_ref(), values.exposure_bias),
        ] {
            let (Some(slider), Some(value)) = (slider, value) else {
                continue;
            };
            if (slider.read(cx).value().start() - value).abs() > 0.05 {
                slider.update(cx, |slider, cx| slider.set_value(value, window, cx));
            }
        }
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// The preferences the selects currently describe.
    pub fn preferences(&self, cx: &App) -> Option<StreamPreferences> {
        let selected = |choice: &Choice| choice.read(cx).selected_index(cx).map(|path| path.row);

        let camera = self.capabilities.cameras.get(selected(&self.camera)?)?;
        // After the camera changes, the other lists still describe the previous one until the
        // controls are rebuilt, so their selections are only trusted for the same camera.
        let same_camera = selected(&self.camera)? == self.camera_index;
        let format = same_camera
            .then(|| selected(&self.resolution).and_then(|index| self.formats.get(index)))
            .flatten();
        let fps = selected(&self.frame_rate).and_then(|index| self.frame_rates.get(index));
        let (_, bitrate) = QUALITIES.get(selected(&self.quality)?)?;
        let codec = self.capabilities.codecs.get(selected(&self.codec)?)?;
        let fallback = self.formats.first();

        Some(StreamPreferences {
            camera: Some(camera.id.clone()),
            width: format.or(fallback).map_or(1920, |format| format.width),
            height: format.or(fallback).map_or(1080, |format| format.height),
            fps: fps.copied().unwrap_or(30),
            codec: *codec,
            bitrate: *bitrate,
        })
    }

    pub fn render(&self, values: &ControlValues, cx: &mut Context<PairCamApp>) -> impl IntoElement {
        let controls = &self.capabilities.controls;
        let focus_locked = matches!(values.focus, Some(FocusMode::Locked | FocusMode::Manual));
        let white_balance_locked = values.white_balance == Some(WhiteBalanceMode::Locked);

        div()
            .v_flex()
            .gap_5()
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(section_label(text!("inspector.stream"), cx))
                    .child(
                        field(text!("inspector.camera"), cx)
                            .child(Select::new(&self.camera).small()),
                    )
                    .child(
                        field(text!("inspector.resolution"), cx)
                            .child(Select::new(&self.resolution).small()),
                    )
                    .child(
                        field(text!("inspector.frame_rate"), cx)
                            .child(Select::new(&self.frame_rate).small()),
                    )
                    .child(
                        field(text!("inspector.quality"), cx)
                            .child(Select::new(&self.quality).small()),
                    )
                    .child(
                        field(text!("inspector.codec"), cx).child(Select::new(&self.codec).small()),
                    ),
            )
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(section_label(text!("inspector.controls"), cx))
                    .when_some(self.zoom.as_ref(), |section, zoom| {
                        section.child(
                            valued_field(
                                text!("inspector.zoom"),
                                format!("{:.1}×", values.zoom.unwrap_or(1.0)),
                                cx,
                            )
                            .child(Slider::new(zoom)),
                        )
                    })
                    .when_some(self.exposure.as_ref(), |section, exposure| {
                        section.child(
                            valued_field(
                                text!("inspector.exposure"),
                                format!("{:+.1}", values.exposure_bias.unwrap_or(0.0)),
                                cx,
                            )
                            .child(Slider::new(exposure)),
                        )
                    })
                    .when(controls.torch, |section| {
                        section.child(toggle(
                            "torch",
                            text!("inspector.torch"),
                            values.torch.unwrap_or(false),
                            |on| ControlValues {
                                torch: Some(on),
                                ..Default::default()
                            },
                            cx,
                        ))
                    })
                    .when(controls.focus.contains(&FocusMode::Locked), |section| {
                        section.child(toggle(
                            "focus-lock",
                            text!("inspector.focus_lock"),
                            focus_locked,
                            |on| ControlValues {
                                focus: Some(if on {
                                    FocusMode::Locked
                                } else {
                                    FocusMode::Auto
                                }),
                                ..Default::default()
                            },
                            cx,
                        ))
                    })
                    .when(
                        controls.white_balance.contains(&WhiteBalanceMode::Locked),
                        |section| {
                            section.child(toggle(
                                "white-balance-lock",
                                text!("inspector.white_balance_lock"),
                                white_balance_locked,
                                |on| ControlValues {
                                    white_balance: Some(if on {
                                        WhiteBalanceMode::Locked
                                    } else {
                                        WhiteBalanceMode::Auto
                                    }),
                                    ..Default::default()
                                },
                                cx,
                            ))
                        },
                    ),
            )
    }
}

fn field_label(label: SharedString, cx: &App) -> Div {
    div()
        .text_size(px(font::size::SM))
        .text_color(cx.theme().muted_foreground)
        .child(label)
}

/// A label above a control.
pub(crate) fn field(label: SharedString, cx: &App) -> Div {
    div().v_flex().gap_1p5().child(field_label(label, cx))
}

/// A label and its current number above a control. Numbers use the mono font so they do not
/// jitter as they change.
pub(crate) fn valued_field(label: SharedString, value: String, cx: &App) -> Div {
    labelled_value(
        label,
        div()
            .font_family(cx.theme().mono_font_family.clone())
            .child(SharedString::from(value)),
        cx,
    )
}

/// A label and its current setting, in words, above a control.
pub(crate) fn worded_field(label: SharedString, value: SharedString, cx: &App) -> Div {
    labelled_value(label, div().child(value), cx)
}

fn labelled_value(label: SharedString, value: Div, cx: &App) -> Div {
    div().v_flex().gap_2().child(
        div()
            .h_flex()
            .justify_between()
            .child(field_label(label, cx))
            .child(value.text_size(px(font::size::SM))),
    )
}

/// A label with a switch on the right that changes a camera control.
pub(crate) fn toggle(
    id: &'static str,
    label: SharedString,
    on: bool,
    apply: fn(bool) -> ControlValues,
    cx: &mut Context<PairCamApp>,
) -> Div {
    switch_row(id, label, on, cx, move |this, checked, cx| {
        this.set_control(apply(checked), cx);
    })
}

/// A label with a switch on the right.
pub(crate) fn switch_row(
    id: &'static str,
    label: SharedString,
    on: bool,
    cx: &mut Context<PairCamApp>,
    changed: impl Fn(&mut PairCamApp, bool, &mut Context<PairCamApp>) + 'static,
) -> Div {
    div()
        .h_flex()
        .items_center()
        .justify_between()
        .child(div().text_size(px(font::size::BASE)).child(label))
        .child(
            Switch::new(id).small().checked(on).on_change(
                cx.listener(move |this, checked: &bool, _, cx| changed(this, *checked, cx)),
            ),
        )
}
