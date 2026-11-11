//! The "PairCam" webcam other apps see: whether it is installed, running and in use.

use std::path::PathBuf;
use std::process::Command;

use paircam_engine::vcam::{self, RegisterError, RegisteredCamera};

/// Where the virtual camera stands.
pub enum CameraStatus {
    /// Registered with Windows; apps can pick "PairCam".
    Active {
        /// Held only so the camera stays registered; dropping it removes the camera.
        _registration: RegisteredCamera,
    },
    /// The camera component has not been installed on this computer yet.
    NotInstalled,
    /// This version of Windows has no virtual cameras (Windows 10 and older).
    Unsupported,
    /// Windows refused to start it.
    Failed(String),
}

impl CameraStatus {
    /// Adds the camera to Windows for as long as the returned value lives.
    pub fn activate() -> Self {
        if !vcam::is_installed() {
            return Self::NotInstalled;
        }
        match vcam::register_with_windows() {
            Ok(camera) => Self::Active {
                _registration: camera,
            },
            Err(RegisterError::NotInstalled) => Self::NotInstalled,
            Err(RegisterError::Unsupported) => Self::Unsupported,
            Err(error) => {
                log::error!("cannot start the virtual camera: {error}");
                Self::Failed(error.to_string())
            }
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active { .. })
    }
}

/// Finds the install script next to a development build. Packaged builds install the camera
/// from their installer instead.
fn install_script() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    executable
        .ancestors()
        .map(|directory| directory.join("scripts").join("register-vcam.ps1"))
        .find(|script| script.is_file())
}

pub fn can_install() -> bool {
    install_script().is_some()
}

/// Runs the install script and waits for it. Windows asks the user for administrator rights.
/// Call this off the main thread.
pub fn install() -> bool {
    let Some(script) = install_script() else {
        return false;
    };
    let mut command = Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(script)
        .arg("-NoPause");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // CREATE_NO_WINDOW: only the elevated step shows a window.
        command.creation_flags(0x0800_0000);
    }
    match command.status() {
        Ok(status) => status.success() && vcam::is_installed(),
        Err(error) => {
            log::error!("cannot run the camera install script: {error}");
            false
        }
    }
}
