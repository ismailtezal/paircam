//! The main window: one view that follows the engine's state and shows the matching screen.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Placement, Sizable as _, StyledExt as _, TitleBar, WindowExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ObjectFit, ParentElement as _, Render, RenderImage, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Subscription, Window, black,
    div, img, px,
};
use image::{Frame, RgbaImage};
use paircam_engine::protocol::{Capabilities, ControlValues, DEFAULT_PORT, Stats, Thermal};
use paircam_engine::{
    Channels, DiscoveredCamera, Engine, Event, Failure, LinkStats, PairedCamera, Phase,
    PreviewFrame, VideoInfo,
};
use paircam_ui::tokens::{font, radius};
use paircam_ui::{
    LinkState, Stat, StatusDot, StatusPill, color, material, palette, panel, section_label,
};
use smallvec::SmallVec;

use crate::controls::StreamControls;
use crate::settings::{self, Appearance, Preferences, SettingsView};
use crate::text;

/// What the app is started with.
pub struct Startup {
    /// Connect to this address right away.
    pub connect_to: Option<SocketAddr>,
    /// Open the settings sheet right away.
    pub show_settings: bool,
    /// Where settings are kept.
    pub data_directory: PathBuf,
    pub preferences: Preferences,
}

/// One phone on the connect screen.
struct PhoneRow {
    name: String,
    detail: SharedString,
    address: Option<SocketAddr>,
    /// Whether the phone is advertising on the network right now.
    nearby: bool,
}

pub struct PairCamApp {
    engine: Engine,
    phase: Phase,
    /// The address of the current or last connection, for "Try again".
    last_address: Option<SocketAddr>,
    paired: Vec<PairedCamera>,
    /// Phones advertising on the network right now.
    discovered: Vec<DiscoveredCamera>,
    /// Whether to connect by itself when a paired phone appears. Turned off when the user
    /// disconnects, so the app does not reconnect against their wishes.
    auto_connect: bool,
    stream_controls: Option<StreamControls>,
    /// Whether the phone has reported its control values since the session began.
    controls_synced: bool,
    controls: ControlValues,
    video: Option<VideoInfo>,
    link: LinkStats,
    camera_stats: Option<Stats>,
    preview: Option<Arc<RenderImage>>,
    /// The picture shown before `preview`. It is released one frame late so the renderer never
    /// loses a picture it is still drawing.
    retired_preview: Option<Arc<RenderImage>>,
    address: Entity<InputState>,
    data_directory: PathBuf,
    preferences: Preferences,
    _subscriptions: Vec<Subscription>,
}

impl PairCamApp {
    pub fn new(
        engine: Engine,
        channels: Channels,
        startup: Startup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let Startup {
            connect_to,
            show_settings,
            data_directory,
            preferences,
        } = startup;

        // Follow Windows when it switches between light and dark, if the user asked for that.
        cx.observe_window_appearance(window, |this, window, cx| {
            if this.preferences.appearance == Appearance::System {
                settings::apply_appearance(Appearance::System, window, cx);
            }
        })
        .detach();

        let address = cx.new(|cx| {
            InputState::new(window, cx).placeholder(text!("connect.address_placeholder"))
        });
        let subscriptions =
            vec![
                cx.subscribe_in(&address, window, |this, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.connect_to_typed_address(cx);
                    }
                }),
            ];

        let Channels { events, preview } = channels;
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = events.recv().await {
                let updated = this.update_in(cx, |this, window, cx| {
                    this.handle_event(event, window, cx);
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(frame) = preview.recv().await {
                let updated = this.update_in(cx, |this, window, cx| {
                    this.show_preview(frame, window, cx);
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();

        if let Some(address) = connect_to {
            engine.connect(address);
        }
        if show_settings {
            cx.defer_in(window, |this, window, cx| this.open_settings(window, cx));
        }

        Self {
            paired: engine.paired_cameras(),
            discovered: Vec::new(),
            auto_connect: true,
            engine,
            phase: Phase::Idle,
            last_address: connect_to,
            stream_controls: None,
            controls_synced: false,
            controls: ControlValues::default(),
            video: None,
            link: LinkStats::default(),
            camera_stats: None,
            preview: None,
            retired_preview: None,
            address,
            data_directory,
            preferences,
            _subscriptions: subscriptions,
        }
    }

    fn handle_event(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Phase(phase) => {
                match &phase {
                    Phase::Connecting { address } => self.last_address = Some(*address),
                    Phase::Idle | Phase::Failed(_) => self.end_session(window),
                    Phase::Pairing { .. } | Phase::Connected { .. } => {}
                }
                self.paired = self.engine.paired_cameras();
                self.phase = phase;
            }
            Event::Capabilities(capabilities) => {
                self.rebuild_stream_controls(capabilities, window, cx)
            }
            Event::ControlState(controls) => {
                self.controls = controls;
                // The sliders start from the first values the phone reports. Later reports echo
                // the user’s own changes and must not rebuild a slider that is being dragged.
                if !self.controls_synced {
                    self.controls_synced = true;
                    if let Some(capabilities) = self
                        .stream_controls
                        .as_ref()
                        .map(|controls| controls.capabilities().clone())
                    {
                        self.rebuild_stream_controls(capabilities, window, cx);
                    }
                }
            }
            Event::Video(video) => self.video = Some(video),
            Event::Link(link) => self.link = link,
            Event::CameraStats(stats) => self.camera_stats = Some(stats),
            Event::Discovered(cameras) => {
                self.discovered = cameras;
                self.connect_to_a_paired_phone();
            }
            Event::CameraLog(entry) => {
                log::info!("phone [{}] {}", entry.tag, entry.message);
            }
        }
        cx.notify();
    }

    fn end_session(&mut self, window: &mut Window) {
        self.stream_controls = None;
        self.controls = ControlValues::default();
        self.controls_synced = false;
        self.video = None;
        self.camera_stats = None;
        self.link = LinkStats::default();
        for image in [self.preview.take(), self.retired_preview.take()]
            .into_iter()
            .flatten()
        {
            let _ = window.drop_image(image);
        }
    }

    fn show_preview(&mut self, frame: PreviewFrame, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.phase, Phase::Connected { .. }) {
            return;
        }
        let Some(buffer) = RgbaImage::from_raw(frame.width, frame.height, frame.bgra) else {
            return;
        };
        let image = Arc::new(RenderImage::new(SmallVec::from_elem(Frame::new(buffer), 1)));
        if let Some(retired) = self.retired_preview.take() {
            let _ = window.drop_image(retired);
        }
        self.retired_preview = self.preview.replace(image);
        cx.notify();
    }

    /// Ends the connection because the user asked to, and stays disconnected.
    fn disconnect(&mut self) {
        self.auto_connect = false;
        self.engine.disconnect();
    }

    /// Connects to the first paired phone on the network, unless the user chose to disconnect
    /// or something is already under way.
    fn connect_to_a_paired_phone(&mut self) {
        let free = matches!(
            self.phase,
            Phase::Idle | Phase::Failed(Failure::Lost | Failure::Unreachable)
        );
        if !self.auto_connect || !free {
            return;
        }
        let paired_phone = self.discovered.iter().find(|camera| {
            camera
                .id
                .is_some_and(|id| self.paired.iter().any(|paired| paired.id == id))
        });
        if let Some(camera) = paired_phone {
            self.engine.connect(camera.address);
        }
    }

    fn connect_to_typed_address(&mut self, cx: &mut Context<Self>) {
        let typed = self.address.read(cx).value().trim().to_owned();
        let address = typed.parse::<SocketAddr>().ok().or_else(|| {
            typed
                .parse::<IpAddr>()
                .ok()
                .map(|ip| SocketAddr::new(ip, DEFAULT_PORT))
        });
        if let Some(address) = address {
            self.engine.connect(address);
        }
    }

    fn rebuild_stream_controls(
        &mut self,
        capabilities: Capabilities,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let preferences = self.engine.preferences();
        self.stream_controls =
            StreamControls::new(capabilities, &preferences, &self.controls, window, cx);
    }

    /// Applies what the stream selects now say, then rebuilds them because the choices for
    /// one depend on another (a camera has its own sizes; a size has its own frame rates).
    pub(crate) fn apply_stream_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(controls) = self.stream_controls.as_ref() else {
            return;
        };
        let Some(preferences) = controls.preferences(cx) else {
            return;
        };
        let capabilities = controls.capabilities().clone();
        self.engine.set_preferences(preferences, &capabilities);
        self.rebuild_stream_controls(capabilities, window, cx);
        cx.notify();
    }

    /// Sends a camera control change and shows it right away.
    pub(crate) fn set_control(&mut self, change: ControlValues, cx: &mut Context<Self>) {
        self.engine.control(change.clone());
        let current = &mut self.controls;
        current.zoom = change.zoom.or(current.zoom);
        current.focus = change.focus.or(current.focus);
        current.exposure_bias = change.exposure_bias.or(current.exposure_bias);
        current.white_balance = change.white_balance.or(current.white_balance);
        current.torch = change.torch.or(current.torch);
        current.stabilization = change.stabilization.or(current.stabilization);
        cx.notify();
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let app = cx.weak_entity();
        let engine = self.engine.clone();
        let directory = self.data_directory.clone();
        let preferences = self.preferences.clone();
        let view = cx.new(|cx| SettingsView::new(app, engine, directory, preferences, window, cx));
        window.open_sheet_at(Placement::Right, cx, move |sheet, _, _| {
            sheet
                .title(text!("settings.title"))
                .size(px(360.))
                .child(view.clone())
        });
    }

    /// The settings sheet changed how the app should look.
    pub(crate) fn set_appearance(&mut self, appearance: Appearance, cx: &mut Context<Self>) {
        self.preferences.appearance = appearance;
        cx.notify();
    }

    /// The settings sheet changed the language: rebuild text that was built once.
    pub(crate) fn language_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preferences = Preferences::load(&self.data_directory);
        self.address.update(cx, |address, cx| {
            address.set_placeholder(text!("connect.address_placeholder"), window, cx);
        });
        if let Some(capabilities) = self
            .stream_controls
            .as_ref()
            .map(|controls| controls.capabilities().clone())
        {
            self.rebuild_stream_controls(capabilities, window, cx);
        }
        cx.notify();
    }

    /// The settings sheet forgot a phone.
    pub(crate) fn paired_changed(&mut self, cx: &mut Context<Self>) {
        self.paired = self.engine.paired_cameras();
        cx.notify();
    }

    // -----------------------------------------------------------------------------------------
    // Title bar
    // -----------------------------------------------------------------------------------------

    fn status(&self) -> (LinkState, SharedString) {
        match &self.phase {
            Phase::Idle => (LinkState::Idle, text!("status.idle")),
            Phase::Connecting { .. } => (LinkState::Connecting, text!("status.connecting")),
            Phase::Pairing { .. } => (LinkState::Connecting, text!("status.pairing")),
            Phase::Connected { .. } if self.preview.is_some() => {
                (LinkState::Live, text!("status.live"))
            }
            Phase::Connected { .. } => (LinkState::Connecting, text!("status.connected")),
            Phase::Failed(_) => (LinkState::Error, text!("status.failed")),
        }
    }

    fn title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (state, label) = self.status();
        TitleBar::new()
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(font::size::BASE))
                            .font_weight(FontWeight::MEDIUM)
                            .child(text!("app.name")),
                    )
                    .child(StatusPill::new(state, label)),
            )
            .child(
                div().h_flex().items_center().pr_1().child(
                    Button::new("open-settings")
                        .ghost()
                        .small()
                        .icon(IconName::Settings)
                        .tooltip(text!("settings.title"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_settings(window, cx);
                        })),
                ),
            )
    }

    // -----------------------------------------------------------------------------------------
    // Screens
    // -----------------------------------------------------------------------------------------

    /// The frame every centred screen shares: icon or spinner, title, subtitle, then actions.
    fn centered(cx: &Context<Self>) -> gpui_kit::Div {
        div()
            .v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_6()
            .p_8()
            .text_color(cx.theme().foreground)
    }

    fn heading(
        title: SharedString,
        subtitle: SharedString,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        div()
            .v_flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_size(px(font::size::XL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(
                div()
                    .max_w(px(420.))
                    .text_center()
                    .text_size(px(font::size::BASE))
                    .line_height(px(20.))
                    .text_color(cx.theme().muted_foreground)
                    .child(subtitle),
            )
    }

    fn badge_icon(icon: IconName, cx: &Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(56.))
            .rounded_full()
            .border_1()
            .border_color(cx.theme().border)
            .bg(color(palette(cx).surface))
            .child(
                Icon::new(icon)
                    .size_6()
                    .text_color(cx.theme().muted_foreground),
            )
    }

    /// The phones to offer on the connect screen: the ones on the network first, then paired
    /// phones that are not advertising right now, reachable at their last address.
    fn phone_rows(&self) -> Vec<PhoneRow> {
        let is_paired = |camera: &DiscoveredCamera| {
            camera
                .id
                .is_some_and(|id| self.paired.iter().any(|paired| paired.id == id))
        };
        let nearby = self.discovered.iter().map(|camera| PhoneRow {
            name: camera.name.clone(),
            detail: if is_paired(camera) {
                text!("connect.nearby_paired")
            } else {
                text!("connect.nearby")
            },
            address: Some(camera.address),
            nearby: true,
        });
        let remembered = self
            .paired
            .iter()
            .filter(|paired| {
                !self
                    .discovered
                    .iter()
                    .any(|camera| camera.id == Some(paired.id))
            })
            .map(|paired| PhoneRow {
                name: paired.name.clone(),
                detail: match paired.last_address {
                    Some(address) => {
                        text!("connect.last_seen_at", address = address.ip().to_string())
                    }
                    None => text!("connect.not_nearby"),
                },
                address: paired.last_address,
                nearby: false,
            });
        nearby.chain(remembered).collect()
    }

    fn connect_screen(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.phone_rows();
        let searching = rows.is_empty();
        let phones = rows.into_iter().enumerate().map(|(index, phone)| {
            let address = phone.address;
            div()
                .h_flex()
                .items_center()
                .gap_3()
                .px_3()
                .py_2p5()
                .when(index > 0, |row| {
                    row.border_t_1().border_color(cx.theme().border)
                })
                .child(
                    Icon::new(IconName::Smartphone)
                        .size_4()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    div()
                        .v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(font::size::BASE))
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .child(SharedString::from(phone.name)),
                        )
                        .child(
                            div()
                                .h_flex()
                                .items_center()
                                .gap_1p5()
                                .when(phone.nearby, |detail| {
                                    detail.child(StatusDot::new(LinkState::Live))
                                })
                                .child(
                                    div()
                                        .text_size(px(font::size::XS))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(phone.detail),
                                ),
                        ),
                )
                .when_some(address, |row, address| {
                    row.child(
                        Button::new(("connect-phone", index))
                            .small()
                            .when(phone.nearby, |button| button.primary())
                            .label(text!("connect.connect"))
                            .on_click(cx.listener(move |this, _, _, _| {
                                this.auto_connect = true;
                                this.engine.connect(address);
                            })),
                    )
                })
        });

        let list = if searching {
            panel(cx)
                .h_flex()
                .items_center()
                .gap_3()
                .px_3()
                .py_3()
                .child(Spinner::new().small())
                .child(
                    div()
                        .text_size(px(font::size::BASE))
                        .text_color(cx.theme().muted_foreground)
                        .child(text!("connect.searching")),
                )
        } else {
            panel(cx).v_flex().children(phones)
        };

        Self::centered(cx)
            .child(Self::badge_icon(IconName::Smartphone, cx))
            .child(Self::heading(
                text!("connect.title"),
                text!("connect.subtitle"),
                cx,
            ))
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .w(px(420.))
                    .child(section_label(text!("connect.phones"), cx))
                    .child(list),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .w(px(420.))
                    .child(div().flex_1().child(Input::new(&self.address)))
                    .child(
                        Button::new("connect-address")
                            .label(text!("connect.by_address"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.auto_connect = true;
                                this.connect_to_typed_address(cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn cancel_button(label: SharedString, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("cancel")
            .label(label)
            .on_click(cx.listener(|this, _, _, _| this.disconnect()))
    }

    fn connecting_screen(&self, address: SocketAddr, cx: &mut Context<Self>) -> AnyElement {
        Self::centered(cx)
            .child(Spinner::new().large())
            .child(Self::heading(
                text!("connecting.title"),
                text!("connecting.subtitle", address = address.ip().to_string()),
                cx,
            ))
            .child(Self::cancel_button(text!("connecting.cancel"), cx))
            .into_any_element()
    }

    fn pairing_screen(&self, camera_name: &str, code: &str, cx: &mut Context<Self>) -> AnyElement {
        // Two groups of three digits read more easily than six in a row.
        let (first, second) = code.split_at(code.len().min(3));
        let group = |digits: &str| {
            div()
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(px(48.))
                .line_height(px(56.))
                .font_weight(FontWeight::MEDIUM)
                .child(SharedString::from(digits.to_owned()))
        };

        Self::centered(cx)
            .child(Self::heading(
                text!("pairing.title"),
                text!("pairing.subtitle", name = camera_name),
                cx,
            ))
            .child(
                panel(cx)
                    .h_flex()
                    .gap_5()
                    .px_8()
                    .py_5()
                    .child(group(first))
                    .child(group(second)),
            )
            .child(Self::cancel_button(text!("pairing.cancel"), cx))
            .into_any_element()
    }

    fn failed_screen(&self, failure: &Failure, cx: &mut Context<Self>) -> AnyElement {
        let message = match failure {
            Failure::Unreachable => text!("failed.unreachable"),
            Failure::Busy => text!("failed.busy"),
            Failure::VersionMismatch => text!("failed.version_mismatch"),
            Failure::PairingDeclined => text!("failed.pairing_declined"),
            Failure::Lost => text!("failed.lost"),
            Failure::Protocol(detail) => text!("failed.protocol", detail = detail),
        };
        let retry_address = self.last_address;

        Self::centered(cx)
            .child(Self::badge_icon(IconName::TriangleAlert, cx))
            .child(Self::heading(text!("failed.title"), message, cx))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("back")
                            .label(text!("failed.back"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.phase = Phase::Idle;
                                cx.notify();
                            })),
                    )
                    .when_some(retry_address, |row, address| {
                        row.child(
                            Button::new("retry")
                                .primary()
                                .label(text!("failed.retry"))
                                .on_click(cx.listener(move |this, _, _, _| {
                                    this.engine.connect(address);
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    fn live_screen(&self, cx: &mut Context<Self>) -> AnyElement {
        let picture = match &self.preview {
            Some(image) => img(image.clone())
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            None => div()
                .v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .child(Spinner::new())
                .child(
                    div()
                        .text_size(px(font::size::BASE))
                        .text_color(color(paircam_ui::tokens::DARK.text_muted))
                        .child(text!("live.waiting")),
                )
                .into_any_element(),
        };

        div()
            .flex_1()
            .min_w_0()
            .p_4()
            .child(
                div()
                    .relative()
                    .size_full()
                    .rounded(px(radius::LG))
                    .overflow_hidden()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(black())
                    .child(picture)
                    .when_some(self.video.as_ref(), |stage, video| {
                        stage.child(
                            div()
                                .absolute()
                                .left_3()
                                .bottom_3()
                                .child(self.stats_bar(video, cx)),
                        )
                    }),
            )
            .into_any_element()
    }

    fn stats_bar(&self, video: &VideoInfo, cx: &Context<Self>) -> impl IntoElement {
        let megabits = self.link.bitrate as f32 / 1_000_000.0;
        material(cx)
            .h_flex()
            .items_center()
            .gap_5()
            .px_4()
            .py_2p5()
            .child(Stat::new(
                text!("live.resolution"),
                format!("{} × {}", video.width, video.height),
            ))
            .child(Stat::new(
                text!("live.fps"),
                format!("{:.0}", self.link.fps),
            ))
            .child(
                Stat::new(text!("live.bitrate"), format!("{megabits:.1}"))
                    .unit(text!("live.bitrate_unit")),
            )
            .when_some(self.link.round_trip_ms, |bar, round_trip| {
                bar.child(
                    Stat::new(text!("live.delay"), format!("{round_trip:.0}"))
                        .unit(text!("live.delay_unit")),
                )
            })
            .child(Stat::new(
                text!("live.decoder"),
                if video.hardware_decoding {
                    text!("live.decoder_gpu")
                } else {
                    text!("live.decoder_cpu")
                },
            ))
    }

    // -----------------------------------------------------------------------------------------
    // Inspector
    // -----------------------------------------------------------------------------------------

    fn detail_row(
        label: SharedString,
        value: SharedString,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        div()
            .h_flex()
            .justify_between()
            .text_size(px(font::size::BASE))
            .child(div().text_color(cx.theme().muted_foreground).child(label))
            .child(div().child(value))
    }

    fn inspector(&self, camera_name: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let stats = self.camera_stats.as_ref();
        let battery = stats.map(|stats| {
            let percent = format!("{:.0}%", stats.battery * 100.0);
            if stats.charging {
                SharedString::from(format!("{percent} · {}", text!("inspector.charging")))
            } else {
                SharedString::from(percent)
            }
        });
        let temperature = stats.map(|stats| match stats.thermal {
            Thermal::Nominal => text!("thermal.nominal"),
            Thermal::Fair => text!("thermal.fair"),
            Thermal::Serious => text!("thermal.serious"),
            Thermal::Critical => text!("thermal.critical"),
            Thermal::Unknown => text!("thermal.unknown"),
        });

        let sections = div()
            .id("inspector-sections")
            .v_flex()
            .flex_1()
            .min_h_0()
            .gap_5()
            .p_4()
            .overflow_y_scroll()
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(section_label(text!("inspector.device"), cx))
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::Smartphone)
                                    .size_4()
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(
                                div()
                                    .text_size(px(font::size::MD))
                                    .font_weight(FontWeight::MEDIUM)
                                    .truncate()
                                    .child(SharedString::from(camera_name.to_owned())),
                            ),
                    )
                    .when_some(battery, |section, battery| {
                        section.child(Self::detail_row(text!("inspector.battery"), battery, cx))
                    })
                    .when_some(temperature, |section, temperature| {
                        section.child(Self::detail_row(
                            text!("inspector.temperature"),
                            temperature,
                            cx,
                        ))
                    }),
            )
            .children(
                self.stream_controls
                    .as_ref()
                    .map(|controls| controls.render(&self.controls, cx)),
            );

        // The sections scroll; the way out stays in reach at the bottom.
        div()
            .v_flex()
            .flex_none()
            .w(px(280.))
            .h_full()
            .border_l_1()
            .border_color(cx.theme().border)
            .child(sections)
            .child(
                div()
                    .p_4()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("disconnect")
                            .label(text!("inspector.disconnect"))
                            .w_full()
                            .on_click(cx.listener(|this, _, _, _| this.disconnect())),
                    ),
            )
    }
}

impl Render for PairCamApp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let phase = self.phase.clone();
        let body = match &phase {
            Phase::Idle => self.connect_screen(cx),
            Phase::Connecting { address } => self.connecting_screen(*address, cx),
            Phase::Pairing { camera_name, code } => self.pairing_screen(camera_name, code, cx),
            Phase::Failed(failure) => self.failed_screen(failure, cx),
            Phase::Connected { camera_name, .. } => div()
                .flex()
                .size_full()
                .child(self.live_screen(cx))
                .child(self.inspector(camera_name, cx))
                .into_any_element(),
        };

        div()
            .v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.title_bar(cx))
            .child(div().flex().flex_1().min_h_0().child(body))
    }
}
