//! Running in the background: the notification-area icon, hiding and showing the window, and
//! making sure only one PairCam runs at a time.
//!
//! Closing the window does not quit PairCam, because other apps can only see the camera while
//! PairCam is running. The window hides and the icon next to the clock brings it back.

use async_channel::Sender;
use gpui_kit::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IsIconic, SW_HIDE, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow,
};
use windows::core::w;

/// The window title, which a second copy of PairCam uses to find the first one's window.
pub const WINDOW_TITLE: &str = "PairCam";

const ICON_PNG: &[u8] = include_bytes!("../../../installer/assets/paircam.png");
/// The size the notification area draws icons at on a high-density display.
const ICON_SIZE: u32 = 32;

/// What the user asked for from the notification-area icon.
pub enum TrayAction {
    Open,
    Quit,
}

/// The notification-area icon. It disappears when this is dropped.
pub struct Tray {
    _icon: TrayIcon,
}

impl Tray {
    /// Adds the icon. Call on the main thread. Returns `None` if Windows refuses, in which case
    /// the window must not be hidden, because nothing could bring it back.
    pub fn new(open_label: &str, quit_label: &str, actions: Sender<TrayAction>) -> Option<Self> {
        let picture = image::load_from_memory(ICON_PNG).ok()?;
        let picture = picture
            .resize_exact(ICON_SIZE, ICON_SIZE, image::imageops::FilterType::Lanczos3)
            .into_rgba8();
        let icon = Icon::from_rgba(picture.into_raw(), ICON_SIZE, ICON_SIZE).ok()?;

        let menu = Menu::new();
        let open = MenuItem::new(open_label, true, None);
        let quit = MenuItem::new(quit_label, true, None);
        menu.append(&open).ok()?;
        menu.append(&quit).ok()?;

        let open_id = open.id().clone();
        let quit_id = quit.id().clone();
        let menu_actions = actions.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == open_id {
                let _ = menu_actions.try_send(TrayAction::Open);
            } else if event.id == quit_id {
                let _ = menu_actions.try_send(TrayAction::Quit);
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            let clicked = matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            );
            if clicked {
                let _ = actions.try_send(TrayAction::Open);
            }
        }));

        let icon = TrayIconBuilder::new()
            .with_tooltip(WINDOW_TITLE)
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .ok()?;
        Some(Self { _icon: icon })
    }
}

/// The window's native handle, for hiding and showing it.
pub fn native_handle(window: &Window) -> Option<isize> {
    // Called through the trait: the window has its own method of the same name.
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

fn hwnd(handle: isize) -> HWND {
    HWND(handle as *mut std::ffi::c_void)
}

/// Hides the window without closing it. The app keeps running.
pub fn hide(handle: isize) {
    // SAFETY: the handle belongs to this app's own window.
    unsafe {
        let _ = ShowWindow(hwnd(handle), SW_HIDE);
    }
}

/// Shows the window again and brings it to the front.
pub fn show(handle: isize) {
    // SAFETY: the handle is a window handle; these calls tolerate a window that has gone.
    unsafe {
        let window = hwnd(handle);
        let _ = ShowWindow(
            window,
            if IsIconic(window).as_bool() {
                SW_RESTORE
            } else {
                SW_SHOW
            },
        );
        let _ = SetForegroundWindow(window);
    }
}

/// Returns false when another PairCam is already running, after bringing its window forward.
/// The first copy keeps a named mutex for as long as it runs.
pub fn claim_single_instance() -> bool {
    // SAFETY: plain Win32 calls with a constant name; the mutex handle is kept until exit on
    // purpose, which is what marks this copy as the running one.
    unsafe {
        let mutex = CreateMutexW(None, false, w!("Local\\PairCam.SingleInstance"));
        if mutex.is_ok() && GetLastError() != ERROR_ALREADY_EXISTS {
            return true;
        }
        if let Ok(existing) = FindWindowW(None, w!("PairCam")) {
            show(existing.0 as isize);
        }
        false
    }
}
