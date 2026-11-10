//! Components specific to PairCam, composed from GPUI Kit primitives and the design tokens.

use gpui_kit::component::{ActiveTheme as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Div, FontWeight, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _,
    Window, div, px,
};

use crate::tokens::{font, radius, size};
use crate::{color, palette};

/// The state of the link to a phone, as shown in status pills and device rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    /// Nothing is connected.
    Idle,
    /// Connecting, pairing or waiting for the first frame.
    Connecting,
    /// Video is flowing.
    Live,
    /// The connection failed or dropped.
    Error,
}

/// A small dot showing a [`LinkState`].
#[derive(IntoElement)]
pub struct StatusDot {
    state: LinkState,
}

impl StatusDot {
    pub fn new(state: LinkState) -> Self {
        Self { state }
    }
}

impl RenderOnce for StatusDot {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = palette(cx);
        let fill = match self.state {
            LinkState::Idle => palette.text_subtle,
            LinkState::Connecting => palette.warning,
            LinkState::Live => palette.live,
            LinkState::Error => palette.danger,
        };
        div()
            .flex_none()
            .size(px(6.))
            .rounded_full()
            .bg(color(fill))
    }
}

/// A pill with a status dot and a short label, such as "Live" or "Waiting for phone".
#[derive(IntoElement)]
pub struct StatusPill {
    state: LinkState,
    label: SharedString,
}

impl StatusPill {
    pub fn new(state: LinkState, label: impl Into<SharedString>) -> Self {
        Self {
            state,
            label: label.into(),
        }
    }
}

impl RenderOnce for StatusPill {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = palette(cx);
        div()
            .h_flex()
            .flex_none()
            .items_center()
            .gap_1p5()
            .h(px(size::CONTROL_SM))
            .pl_2()
            .pr_2p5()
            .rounded_full()
            .border_1()
            .border_color(cx.theme().border)
            .bg(color(palette.surface))
            .child(StatusDot::new(self.state))
            .child(
                div()
                    .text_size(px(font::size::SM))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(cx.theme().foreground)
                    .child(self.label),
            )
    }
}

/// One live number with its label, such as `FPS 60` or `BITRATE 12.4 Mbps`.
///
/// Values are set in the mono font with tabular figures so they do not jitter as they change.
#[derive(IntoElement)]
pub struct Stat {
    label: SharedString,
    value: SharedString,
    unit: Option<SharedString>,
}

impl Stat {
    pub fn new(label: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            unit: None,
        }
    }

    pub fn unit(mut self, unit: impl Into<SharedString>) -> Self {
        self.unit = Some(unit.into());
        self
    }
}

impl RenderOnce for Stat {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = palette(cx);
        div()
            .v_flex()
            .gap_0p5()
            .child(
                div()
                    .text_size(px(font::size::XS))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color(palette.text_subtle))
                    .child(self.label),
            )
            .child(
                div()
                    .h_flex()
                    .items_baseline()
                    .gap_1()
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(font::size::MD))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(cx.theme().foreground)
                            .child(self.value),
                    )
                    .when_some(self.unit, |row, unit| {
                        row.child(
                            div()
                                .text_size(px(font::size::XS))
                                .text_color(cx.theme().muted_foreground)
                                .child(unit),
                        )
                    }),
            )
    }
}

/// A quiet surface with a hairline border, for grouping controls beside the preview.
pub fn panel(cx: &App) -> Div {
    div()
        .rounded(px(radius::LG))
        .border_1()
        .border_color(cx.theme().border)
        .bg(color(palette(cx).surface))
}

/// A translucent surface for controls that float over the camera preview.
pub fn material(cx: &App) -> Div {
    let palette = palette(cx);
    div()
        .rounded(px(radius::LG))
        .border_1()
        .border_color(color(palette.material_border))
        .bg(color(palette.material))
}

/// A small heading above a group of controls.
pub fn section_label(label: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_size(px(font::size::XS))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color(palette(cx).text_subtle))
        .child(label.into())
}
