// Release builds are a windowed app; debug builds keep the console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::SocketAddr;
use std::path::PathBuf;

use gpui_kit::component::TitleBar;
use gpui_kit::{AppContext as _, Bounds, WindowBounds, WindowOptions, px, size};
use paircam_engine::Engine;

mod app;

rust_i18n::i18n!("locales", fallback = "en");

/// Looks up interface text in the active language as a GPUI string.
#[macro_export]
macro_rules! text {
    ($($argument:tt)*) => {
        gpui_kit::SharedString::from(rust_i18n::t!($($argument)*).into_owned())
    };
}

/// Options for development and automated checks.
#[derive(Default)]
struct Options {
    /// Connect to this address as soon as the window opens.
    connect: Option<SocketAddr>,
    /// Use this language instead of the system one.
    locale: Option<String>,
    /// Keep settings in this folder instead of the user's app data.
    data_directory: Option<PathBuf>,
    light: bool,
}

fn options() -> Options {
    let mut options = Options::default();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--connect" => options.connect = arguments.next().and_then(|value| value.parse().ok()),
            "--locale" => options.locale = arguments.next(),
            "--data" => options.data_directory = arguments.next().map(PathBuf::from),
            "--light" => options.light = true,
            other => log::warn!("ignoring unknown option {other}"),
        }
    }
    options
}

fn data_directory() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("PairCam")
}

fn host_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "PC".to_owned())
}

/// Picks the interface language: the first system language PairCam has, otherwise English.
fn system_locale() -> String {
    let available = rust_i18n::available_locales!();
    sys_locale::get_locales()
        .filter_map(|locale| locale.split(['-', '_']).next().map(str::to_lowercase))
        .find(|language| {
            available
                .iter()
                .any(|locale| locale.as_ref() == language.as_str())
        })
        .unwrap_or_else(|| "en".to_owned())
}

fn main() {
    let _ = simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .init();

    let options = options();
    rust_i18n::set_locale(&options.locale.clone().unwrap_or_else(system_locale));

    let (engine, channels) = Engine::new(
        &options
            .data_directory
            .clone()
            .unwrap_or_else(data_directory),
        host_name(),
    );

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            paircam_ui::init(cx).expect("failed to set up the PairCam theme");
            if options.light {
                paircam_ui::set_mode(gpui_kit::component::ThemeMode::Light, cx);
            }

            let window_options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1120.), px(720.)),
                    cx,
                ))),
                window_min_size: Some(size(px(880.), px(560.))),
                ..TitleBar::window_options()
            };
            gpui_kit::open_window(window_options, cx, |window, cx| {
                window.set_window_title("PairCam");
                cx.new(|cx| app::PairCamApp::new(engine, channels, options.connect, window, cx))
            })
            .expect("failed to open the main window");
        });
}
