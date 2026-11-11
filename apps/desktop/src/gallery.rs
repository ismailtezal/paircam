//! The component gallery: every colour, type size and control in the current theme, on one
//! screen. Open it with `--gallery` to review the design system. Its labels are developer
//! facing and therefore not translated.

use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath, Selectable as _, Sizable as _, StyledExt as _,
    Theme, ThemeMode, TitleBar,
};
use gpui_kit::{
    AppContext as _, Context, Div, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, px,
};
use paircam_engine::LookId;
use paircam_ui::tokens::{self, Palette, font};
use paircam_ui::{
    LinkState, Stat, StatusDot, StatusPill, color, material, palette, panel, section_label,
};

use crate::colour::{look_swatches, look_tile};

pub struct Gallery {
    input: Entity<InputState>,
    select: Entity<SelectState<Vec<SharedString>>>,
    slider: Entity<SliderState>,
    checked: bool,
    switched: bool,
}

impl Gallery {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            input: cx.new(|cx| InputState::new(window, cx).placeholder("Placeholder")),
            select: cx.new(|cx| {
                SelectState::new(
                    vec!["1920 × 1080 · Full HD".into(), "1280 × 720 · HD".into()],
                    Some(IndexPath::default()),
                    window,
                    cx,
                )
            }),
            slider: cx.new(|_| {
                SliderState::new()
                    .min(1.0)
                    .max(10.0)
                    .step(0.1)
                    .default_value(3.0)
            }),
            checked: true,
            switched: true,
        }
    }

    fn section(title: &'static str, cx: &Context<Self>) -> Div {
        div().v_flex().gap_3().child(section_label(title, cx))
    }

    fn row() -> Div {
        div().h_flex().flex_wrap().items_center().gap_3()
    }

    fn swatch(name: &'static str, value: u32, cx: &Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .gap_1p5()
            .w(px(104.))
            .child(
                div()
                    .h(px(40.))
                    .rounded(px(tokens::radius::MD))
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(color(value)),
            )
            .child(
                div()
                    .text_size(px(font::size::XS))
                    .text_color(cx.theme().muted_foreground)
                    .child(name),
            )
    }

    fn colours(palette: &Palette, cx: &Context<Self>) -> impl IntoElement {
        Self::section("COLOUR", cx).child(
            Self::row()
                .child(Self::swatch("background", palette.background, cx))
                .child(Self::swatch("surface", palette.surface, cx))
                .child(Self::swatch("surface-raised", palette.surface_raised, cx))
                .child(Self::swatch("surface-hover", palette.surface_hover, cx))
                .child(Self::swatch("border", palette.border, cx))
                .child(Self::swatch("border-strong", palette.border_strong, cx))
                .child(Self::swatch("text", palette.text, cx))
                .child(Self::swatch("text-muted", palette.text_muted, cx))
                .child(Self::swatch("text-subtle", palette.text_subtle, cx))
                .child(Self::swatch("primary", palette.primary, cx))
                .child(Self::swatch("accent", palette.accent, cx))
                .child(Self::swatch("live", palette.live, cx))
                .child(Self::swatch("warning", palette.warning, cx))
                .child(Self::swatch("danger", palette.danger, cx)),
        )
    }

    fn type_scale(cx: &Context<Self>) -> impl IntoElement {
        let sample = |name: &'static str, size: f32, weight: FontWeight| {
            div()
                .h_flex()
                .items_baseline()
                .gap_4()
                .child(
                    div()
                        .w(px(72.))
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(font::size::XS))
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{name} · {size}")),
                )
                .child(
                    div()
                        .text_size(px(size))
                        .font_weight(weight)
                        .child("Use your iPhone as a webcam"),
                )
        };
        Self::section("TYPE", cx)
            .child(sample("xxl", font::size::XXL, FontWeight::SEMIBOLD))
            .child(sample("xl", font::size::XL, FontWeight::SEMIBOLD))
            .child(sample("lg", font::size::LG, FontWeight::MEDIUM))
            .child(sample("md", font::size::MD, FontWeight::MEDIUM))
            .child(sample("base", font::size::BASE, FontWeight::NORMAL))
            .child(sample("sm", font::size::SM, FontWeight::NORMAL))
            .child(sample("xs", font::size::XS, FontWeight::MEDIUM))
    }

    fn buttons(cx: &Context<Self>) -> impl IntoElement {
        Self::section("BUTTONS", cx)
            .child(
                Self::row()
                    .child(Button::new("primary").primary().label("Connect"))
                    .child(Button::new("default").label("Cancel"))
                    .child(Button::new("ghost").ghost().label("Settings"))
                    .child(Button::new("danger").danger().label("Forget"))
                    .child(Button::new("link").link().label("Learn more"))
                    .child(Button::new("selected").label("Selected").selected(true))
                    .child(
                        Button::new("disabled")
                            .primary()
                            .label("Disabled")
                            .disabled(true),
                    )
                    .child(
                        Button::new("loading")
                            .primary()
                            .label("Connecting")
                            .loading(true),
                    ),
            )
            .child(
                Self::row()
                    .child(Button::new("xsmall").xsmall().label("Extra small"))
                    .child(Button::new("small").small().label("Small"))
                    .child(Button::new("medium").label("Medium"))
                    .child(Button::new("large").large().label("Large")),
            )
    }

    fn inputs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        Self::section("INPUTS", cx)
            .child(
                Self::row()
                    .child(div().w(px(220.)).child(Input::new(&self.input)))
                    .child(div().w(px(220.)).child(Select::new(&self.select)))
                    .child(div().w(px(220.)).child(Slider::new(&self.slider))),
            )
            .child(
                Self::row()
                    .child(
                        Switch::new("switch")
                            .label("Flashlight")
                            .checked(self.switched)
                            .on_change(cx.listener(|this, on: &bool, _, cx| {
                                this.switched = *on;
                                cx.notify();
                            })),
                    )
                    .child(
                        Checkbox::new("checkbox")
                            .label("Start with Windows")
                            .checked(self.checked)
                            .on_change(cx.listener(|this, on: &bool, _, cx| {
                                this.checked = *on;
                                cx.notify();
                            })),
                    )
                    .child(Switch::new("switch-off").label("Disabled").disabled(true)),
            )
    }

    fn status(cx: &Context<Self>) -> impl IntoElement {
        Self::section("STATUS", cx)
            .child(
                Self::row()
                    .child(StatusPill::new(LinkState::Live, "Live"))
                    .child(StatusPill::new(LinkState::Connecting, "Connecting"))
                    .child(StatusPill::new(LinkState::Idle, "Not connected"))
                    .child(StatusPill::new(LinkState::Error, "Disconnected"))
                    .child(StatusDot::new(LinkState::Live))
                    .child(Tag::secondary().child("HEVC"))
                    .child(Tag::success().child("GPU"))
                    .child(Tag::warning().child("Warm"))
                    .child(Tag::danger().child("Too hot")),
            )
            .child(
                Self::row()
                    .child(
                        panel(cx)
                            .h_flex()
                            .gap_5()
                            .px_4()
                            .py_3()
                            .child(Stat::new("RESOLUTION", "1920 × 1080"))
                            .child(Stat::new("FPS", "60"))
                            .child(Stat::new("BITRATE", "12.4").unit("Mbps")),
                    )
                    .child(
                        div()
                            .p_4()
                            .rounded(px(tokens::radius::LG))
                            .bg(color(palette(cx).accent))
                            .child(
                                material(cx)
                                    .h_flex()
                                    .gap_5()
                                    .px_4()
                                    .py_3()
                                    .child(Stat::new("PING", "4").unit("ms"))
                                    .child(Stat::new("DECODER", "GPU")),
                            ),
                    ),
            )
    }

    /// The look tiles as the inspector shows them, the second one selected.
    fn looks(cx: &Context<Self>) -> impl IntoElement {
        Self::section("LOOKS", cx).child(
            div()
                .w(px(248.))
                .grid()
                .grid_cols(3)
                .gap(px(tokens::space::S_1_5))
                .children(LookId::ALL.iter().enumerate().map(|(index, look)| {
                    look_tile(
                        ("gallery-look", index),
                        *look,
                        &look_swatches(*look),
                        index == 1,
                        cx,
                    )
                })),
        )
    }

    fn feedback(cx: &Context<Self>) -> impl IntoElement {
        Self::section("FEEDBACK", cx)
            .child(
                Self::row()
                    .child(Spinner::new().small())
                    .child(Spinner::new())
                    .child(Spinner::new().large())
                    .child(
                        div()
                            .w(px(200.))
                            .child(Progress::new("progress").value(64.0)),
                    )
                    .child(Skeleton::new().w(px(160.)).h_4().rounded_md()),
            )
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .w(px(520.))
                    .child(
                        Alert::info("alert-info", "Open PairCam on your iPhone to connect.")
                            .title("Waiting for a phone"),
                    )
                    .child(
                        Alert::warning("alert-warning", "Quality was reduced to let it cool.")
                            .title("The phone is getting warm"),
                    )
                    .child(
                        Alert::error(
                            "alert-error",
                            "Check that both devices are on the same Wi-Fi.",
                        )
                        .title("The connection was lost"),
                    ),
            )
    }
}

impl Render for Gallery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = Theme::global(cx).is_dark();
        let palette = palette(cx);

        div()
            .v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                TitleBar::new()
                    .child(
                        div()
                            .text_size(px(font::size::BASE))
                            .font_weight(FontWeight::MEDIUM)
                            .child("PairCam · Component gallery"),
                    )
                    .child(
                        div().pr_1().child(
                            Button::new("mode")
                                .ghost()
                                .small()
                                .label(if dark { "Light" } else { "Dark" })
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    let mode = if dark {
                                        ThemeMode::Light
                                    } else {
                                        ThemeMode::Dark
                                    };
                                    paircam_ui::set_mode(mode, cx);
                                    cx.notify();
                                })),
                        ),
                    ),
            )
            .child(
                div()
                    .id("gallery")
                    .v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_8()
                    .p_8()
                    .overflow_y_scroll()
                    .child(Self::colours(palette, cx))
                    .child(Self::type_scale(cx))
                    .child(Self::buttons(cx))
                    .child(self.inputs(cx))
                    .child(Self::status(cx))
                    .child(Self::looks(cx))
                    .child(Self::feedback(cx)),
            )
    }
}
