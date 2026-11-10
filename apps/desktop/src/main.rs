// Release builds are a windowed app; debug builds keep the console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::*;
use paircam_ui::{LinkState, Stat, StatusPill};

pub struct Example;

impl Render for Example {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(TitleBar::new().child("PairCam"))
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .gap_4()
                    .items_center()
                    .justify_center()
                    .child(StatusPill::new(LinkState::Live, "Live"))
                    .child(
                        div()
                            .h_flex()
                            .gap_6()
                            .child(Stat::new("RESOLUTION", "1920 × 1080"))
                            .child(Stat::new("FPS", "60"))
                            .child(Stat::new("BITRATE", "12.4").unit("Mbps")),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(Button::new("connect").primary().label("Connect"))
                            .child(Button::new("cancel").label("Cancel"))
                            .child(Button::new("ghost").ghost().label("Settings")),
                    ),
            )
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            paircam_ui::init(cx).expect("failed to set up the PairCam theme");

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(960.), px(640.)),
                    cx,
                ))),
                window_min_size: Some(size(px(720.), px(480.))),
                ..TitleBar::window_options()
            };
            gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| Example))
                .expect("failed to open the main window");
        });
}
