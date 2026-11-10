use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::*;

pub struct Example;
impl Render for Example {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .gap_2()
            .size_full()
            .items_center()
            .justify_center()
            .child("PairCam")
            .child(Button::new("ok").primary().label("Connect"))
    }
}

fn main() {
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| cx.new(|_| Example))
            .expect("Failed to open window");
    });
}
