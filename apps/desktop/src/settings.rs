//! App preferences (appearance and language) and the settings sheet.

use std::fs;
use std::path::{Path, PathBuf};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{
    ActiveTheme as _, IndexPath, Selectable as _, Sizable as _, StyledExt as _, ThemeMode,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, WeakEntity, Window, WindowAppearance, div, px,
};
use paircam_engine::{Engine, PairedCamera};
use paircam_ui::tokens::font;
use paircam_ui::{panel, section_label};
use serde::{Deserialize, Serialize};

use crate::app::PairCamApp;
use crate::text;

/// The languages the interface is translated into, as locale code and own name.
const LANGUAGES: [(&str, &str); 2] = [("en", "English"), ("tr", "Türkçe")];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    Dark,
    Light,
    /// Follow Windows.
    System,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub appearance: Appearance,
    /// A locale code, or `None` to follow the system language.
    pub language: Option<String>,
}

impl Preferences {
    fn path(directory: &Path) -> PathBuf {
        directory.join("preferences.json")
    }

    pub fn load(directory: &Path) -> Self {
        fs::read(Self::path(directory))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, directory: &Path) {
        let result = fs::create_dir_all(directory).and_then(|()| {
            let json = serde_json::to_vec_pretty(self).expect("preferences serialize");
            fs::write(Self::path(directory), json)
        });
        if let Err(error) = result {
            log::error!("cannot save preferences: {error}");
        }
    }
}

/// Picks the interface language: the first system language PairCam has, otherwise English.
pub fn system_locale() -> String {
    sys_locale::get_locales()
        .filter_map(|locale| locale.split(['-', '_']).next().map(str::to_lowercase))
        .find(|language| LANGUAGES.iter().any(|(code, _)| code == language))
        .unwrap_or_else(|| "en".to_owned())
}

pub fn apply_language(language: Option<&str>) {
    let locale = language.map_or_else(system_locale, str::to_owned);
    rust_i18n::set_locale(&locale);
}

pub fn apply_appearance(appearance: Appearance, window: &Window, cx: &mut App) {
    let mode = match appearance {
        Appearance::Dark => ThemeMode::Dark,
        Appearance::Light => ThemeMode::Light,
        Appearance::System => match window.appearance() {
            WindowAppearance::Light | WindowAppearance::VibrantLight => ThemeMode::Light,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
        },
    };
    paircam_ui::set_mode(mode, cx);
}

/// The content of the settings sheet.
pub struct SettingsView {
    app: WeakEntity<PairCamApp>,
    engine: Engine,
    directory: PathBuf,
    preferences: Preferences,
    paired: Vec<PairedCamera>,
    language: Entity<SelectState<Vec<SharedString>>>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    pub fn new(
        app: WeakEntity<PairCamApp>,
        engine: Engine,
        directory: PathBuf,
        preferences: Preferences,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let language = Self::language_select(&preferences, window, cx);
        let subscription = cx.subscribe_in(
            &language,
            window,
            |this, _, _: &SelectEvent<Vec<SharedString>>, window, cx| {
                this.language_chosen(window, cx);
            },
        );
        Self {
            app,
            paired: engine.paired_cameras(),
            engine,
            directory,
            preferences,
            language,
            _subscriptions: vec![subscription],
        }
    }

    fn language_select(
        preferences: &Preferences,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<SelectState<Vec<SharedString>>> {
        let mut labels = vec![text!("settings.language_system")];
        labels.extend(LANGUAGES.iter().map(|(_, name)| SharedString::from(*name)));
        // Row 0 is "follow the system"; the languages follow in order.
        let selected = preferences
            .language
            .as_deref()
            .and_then(|language| LANGUAGES.iter().position(|(code, _)| *code == language))
            .map_or(0, |index| index + 1);
        cx.new(|cx| SelectState::new(labels, Some(IndexPath::default().row(selected)), window, cx))
    }

    fn language_chosen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self
            .language
            .read(cx)
            .selected_index(cx)
            .map(|path| path.row)
        else {
            return;
        };
        self.preferences.language = row
            .checked_sub(1)
            .and_then(|index| LANGUAGES.get(index))
            .map(|(code, _)| (*code).to_owned());
        apply_language(self.preferences.language.as_deref());
        self.preferences.save(&self.directory);

        // Text that was built once has to be built again in the new language.
        let language = Self::language_select(&self.preferences, window, cx);
        self._subscriptions = vec![cx.subscribe_in(
            &language,
            window,
            |this, _, _: &SelectEvent<Vec<SharedString>>, window, cx| {
                this.language_chosen(window, cx);
            },
        )];
        self.language = language;
        let _ = self
            .app
            .update(cx, |app, cx| app.language_changed(window, cx));
        window.refresh();
        cx.notify();
    }

    fn set_appearance(
        &mut self,
        appearance: Appearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preferences.appearance = appearance;
        self.preferences.save(&self.directory);
        apply_appearance(appearance, window, cx);
        let _ = self
            .app
            .update(cx, |app, cx| app.set_appearance(appearance, cx));
        cx.notify();
    }

    fn forget(&mut self, camera: &PairedCamera, cx: &mut Context<Self>) {
        self.engine.forget(camera.id);
        self.paired = self.engine.paired_cameras();
        let _ = self.app.update(cx, |app, cx| app.paired_changed(cx));
        cx.notify();
    }

    fn appearance_button(
        &self,
        id: &'static str,
        label: SharedString,
        appearance: Appearance,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        Button::new(id)
            .small()
            .label(label)
            .selected(self.preferences.appearance == appearance)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.set_appearance(appearance, window, cx);
            }))
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let phones = self
            .paired
            .clone()
            .into_iter()
            .enumerate()
            .map(|(index, camera)| {
                div()
                    .h_flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .when(index > 0, |row| {
                        row.border_t_1().border_color(cx.theme().border)
                    })
                    .child(
                        div()
                            .text_size(px(font::size::BASE))
                            .truncate()
                            .child(SharedString::from(camera.name.clone())),
                    )
                    .child(
                        Button::new(("forget", index))
                            .ghost()
                            .small()
                            .label(text!("settings.forget"))
                            .on_click(cx.listener(move |this, _, _, cx| this.forget(&camera, cx))),
                    )
            });
        let muted = |label: SharedString| {
            div()
                .text_size(px(font::size::BASE))
                .text_color(cx.theme().muted_foreground)
                .child(label)
        };

        div()
            .v_flex()
            .gap_6()
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .child(section_label(text!("settings.appearance"), cx))
                    .child(
                        div()
                            .h_flex()
                            .gap_1p5()
                            .child(self.appearance_button(
                                "appearance-dark",
                                text!("settings.dark"),
                                Appearance::Dark,
                                cx,
                            ))
                            .child(self.appearance_button(
                                "appearance-light",
                                text!("settings.light"),
                                Appearance::Light,
                                cx,
                            ))
                            .child(self.appearance_button(
                                "appearance-system",
                                text!("settings.system"),
                                Appearance::System,
                                cx,
                            )),
                    ),
            )
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .child(section_label(text!("settings.language"), cx))
                    .child(Select::new(&self.language).small()),
            )
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .child(section_label(text!("settings.phones"), cx))
                    .child(if self.paired.is_empty() {
                        muted(text!("settings.no_phones")).into_any_element()
                    } else {
                        panel(cx).v_flex().children(phones).into_any_element()
                    }),
            )
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(section_label(text!("settings.about"), cx))
                    .child(
                        div()
                            .text_size(px(font::size::BASE))
                            .font_weight(FontWeight::MEDIUM)
                            .child(text!(
                                "settings.version",
                                version = env!("CARGO_PKG_VERSION")
                            )),
                    )
                    .child(muted(text!("settings.licence"))),
            )
    }
}
