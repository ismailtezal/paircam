//! The inspector's Away screen section: what other apps see while the user is not in the
//! picture, when it shows by itself, and a way to show it by hand.
//!
//! The away screen itself lives in the engine (`paircam_engine::away`), which watches the
//! picture, fades to the away screen and back, and saves the settings; these controls only read
//! and change them.

use std::path::Path;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Div, Entity, ParentElement as _, PathPromptOptions,
    SharedString, Styled as _, Subscription, Window, div, px,
};
use paircam_engine::away::{MAX_DELAY, MAX_MESSAGE_CHARS, MIN_DELAY};
use paircam_engine::{AwayMedia, AwaySettings, AwayStatus};
use paircam_ui::tokens::font;
use paircam_ui::{LinkState, StatusDot, color, palette, section_label};

use crate::app::PairCamApp;
use crate::controls::{field, switch_row, valued_field};
use crate::text;

/// The delay slider and the message box, which keep state of their own between renders.
pub struct AwayControls {
    delay: Entity<SliderState>,
    message: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl AwayControls {
    pub fn new(settings: &AwaySettings, window: &mut Window, cx: &mut Context<PairCamApp>) -> Self {
        let delay = cx.new(|_| {
            SliderState::new()
                .min(MIN_DELAY)
                .max(MAX_DELAY)
                .step(1.0)
                .default_value(settings.delay)
        });
        // The box holds only the user's own words; empty stands for the default message.
        let typed = settings.message.clone().unwrap_or_default();
        let message = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(text!("away.default_message"))
                .validate(|text, _| text.chars().count() <= MAX_MESSAGE_CHARS)
                .default_value(typed)
        });
        let subscriptions = vec![
            cx.subscribe(&delay, |this, _, event: &SliderEvent, cx| {
                if let SliderEvent::Change(value) = event {
                    let delay = value.start();
                    this.change_away(|away| away.delay = delay, cx);
                }
            }),
            // Applied when the user is done, not per keystroke: a new message rebuilds the
            // away screen.
            cx.subscribe_in(&message, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    apply_typed_message(this, cx);
                }
            }),
        ];
        Self {
            delay,
            message,
            _subscriptions: subscriptions,
        }
    }

    /// The default message is the placeholder, so it follows the language.
    pub fn language_changed(&self, window: &mut Window, cx: &mut App) {
        self.message.update(cx, |message, cx| {
            message.set_placeholder(text!("away.default_message"), window, cx);
        });
    }

    /// The message box as a setting: `None` (the default message) while it is empty.
    fn typed_message(&self, cx: &App) -> Option<String> {
        let typed = self.message.read(cx).value().trim().to_owned();
        (!typed.is_empty()).then_some(typed)
    }

    /// The Away screen section.
    pub fn render(
        &self,
        settings: &AwaySettings,
        status: AwayStatus,
        away_now: bool,
        cx: &mut Context<PairCamApp>,
    ) -> Div {
        let muted = cx.theme().muted_foreground;
        let presence = status.in_picture.map(|in_picture| {
            let (state, label) = if in_picture {
                (LinkState::Live, text!("away.in_picture"))
            } else {
                (LinkState::Idle, text!("away.nobody"))
            };
            div()
                .h_flex()
                .items_center()
                .gap_1p5()
                .child(StatusDot::new(state))
                .child(
                    div()
                        .text_size(px(font::size::SM))
                        .text_color(muted)
                        .child(label),
                )
        });
        let automatic = div()
            .v_flex()
            .w_full()
            .gap_3()
            .pt_3()
            .child(
                valued_field(
                    text!("away.delay"),
                    text!(
                        "away.delay_value",
                        seconds = format!("{:.0}", settings.delay)
                    )
                    .to_string(),
                    cx,
                )
                .child(Slider::new(&self.delay)),
            )
            .children(presence);

        let message_on = shows_message(settings);

        div()
            .v_flex()
            .gap_3()
            .child(section_label(text!("away.title"), cx))
            .child(
                Collapsible::new()
                    .motion_id("away-automatic")
                    .w_full()
                    .open(settings.automatic)
                    .child(
                        div()
                            .v_flex()
                            .gap_1()
                            .child(switch_row(
                                "away-automatic",
                                text!("away.automatic"),
                                settings.automatic,
                                cx,
                                |this, on, cx| this.change_away(|away| away.automatic = on, cx),
                            ))
                            .child(
                                div()
                                    .text_size(px(font::size::SM))
                                    .text_color(muted)
                                    .child(text!("away.automatic_hint")),
                            ),
                    )
                    .content(automatic),
            )
            .child(self.render_media(settings, status.media, cx))
            .child(
                Collapsible::new()
                    .motion_id("away-message")
                    .w_full()
                    .open(message_on)
                    .child(switch_row(
                        "away-message",
                        text!("away.message"),
                        message_on,
                        cx,
                        |this, on, cx| {
                            let typed = this.away_controls().typed_message(cx);
                            this.change_away(
                                |away| {
                                    away.message = if on { typed } else { Some(String::new()) };
                                },
                                cx,
                            );
                        },
                    ))
                    .content(
                        div()
                            .w_full()
                            .pt_2()
                            .child(Input::new(&self.message).small()),
                    ),
            )
            .child(
                Button::new("away-now")
                    .small()
                    .w_full()
                    // While it is on, the way back is the one thing to do here.
                    .when(away_now, |button| button.primary())
                    .label(if away_now {
                        text!("away.back")
                    } else {
                        text!("away.show_now")
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // Clicking a button keeps the message box focused, so words typed
                        // without Enter would not be on the screen this shows.
                        apply_typed_message(this, cx);
                        this.set_away_now(!away_now, cx);
                    })),
            )
    }

    /// The chosen video or picture, or the plain card, with the ways to change it.
    fn render_media(
        &self,
        settings: &AwaySettings,
        media: AwayMedia,
        cx: &mut Context<PairCamApp>,
    ) -> Div {
        let name = settings
            .media
            .as_deref()
            .map_or_else(|| text!("away.plain_card"), file_name);

        field(text!("away.media"), cx)
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::Image)
                            .size(px(font::size::MD))
                            .flex_none()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(font::size::BASE))
                            .child(name),
                    )
                    .when(settings.media.is_some(), |row| {
                        row.child(
                            Button::new("away-remove")
                                .ghost()
                                .small()
                                .flex_none()
                                .label(text!("away.remove"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.change_away(|away| away.media = None, cx);
                                })),
                        )
                    })
                    .child(
                        Button::new("away-choose")
                            .outline()
                            .small()
                            .flex_none()
                            .label(text!("away.choose"))
                            .on_click(cx.listener(|_, _, _, cx| choose_media(cx))),
                    ),
            )
            .when(media == AwayMedia::Failed, |section| {
                section.child(
                    div()
                        .text_size(px(font::size::SM))
                        .text_color(color(palette(cx).danger))
                        .child(text!("away.media_failed")),
                )
            })
    }
}

/// Puts what the message box says on the away screen, if it shows words at all.
fn apply_typed_message(this: &mut PairCamApp, cx: &mut Context<PairCamApp>) {
    let typed = this.away_controls().typed_message(cx);
    this.change_away(
        |away| {
            if shows_message(away) {
                away.message = typed;
            }
        },
        cx,
    );
}

/// Whether the away screen shows words at all: an empty message stands for none.
fn shows_message(settings: &AwaySettings) -> bool {
    settings.message.as_deref() != Some("")
}

/// A file's name without its folder, for showing in a narrow column.
fn file_name(path: &Path) -> SharedString {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
        .into()
}

/// Asks the system for one video or picture and shows it on the away screen.
fn choose_media(cx: &mut Context<PairCamApp>) {
    let chosen = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: None,
    });
    cx.spawn(async move |this, cx| {
        let path = match chosen.await {
            Ok(Ok(Some(paths))) => paths.into_iter().next(),
            Ok(Err(error)) => {
                log::warn!("cannot open the file dialog: {error}");
                None
            }
            Ok(Ok(None)) | Err(_) => None,
        };
        if let Some(path) = path {
            let _ = this.update(cx, |this, cx| {
                this.change_away(|away| away.media = Some(path), cx);
            });
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn only_an_empty_message_hides_the_words() {
        let mut settings = AwaySettings::default();
        assert!(shows_message(&settings));
        settings.message = Some("Back in five".to_owned());
        assert!(shows_message(&settings));
        settings.message = Some(String::new());
        assert!(!shows_message(&settings));
    }

    #[test]
    fn a_file_is_named_without_its_folder() {
        let path = PathBuf::from("Videos").join("coffee break.mp4");
        assert_eq!(file_name(&path).as_ref(), "coffee break.mp4");
    }
}
