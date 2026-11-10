//! What the desktop remembers between runs: its own identity, paired cameras and stream
//! preferences. Stored as JSON in the app data folder; pairing keys are encrypted for the
//! current Windows user before they touch the disk.

use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use paircam_protocol::{Codec, DeviceId, PairingKey};
use serde::{Deserialize, Serialize};

/// A camera this computer has paired with.
#[derive(Clone, Debug, PartialEq)]
pub struct PairedCamera {
    pub id: DeviceId,
    pub name: String,
    /// Where it was last reached, for reconnecting before discovery finds it.
    pub last_address: Option<SocketAddr>,
}

/// What to ask the phone for when a stream starts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StreamPreferences {
    /// The camera id last used, if it still exists on the phone.
    pub camera: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: Codec,
    /// Bits per second.
    pub bitrate: u32,
}

impl Default for StreamPreferences {
    fn default() -> Self {
        Self {
            camera: None,
            width: 1920,
            height: 1080,
            fps: 30,
            codec: Codec::Hevc,
            bitrate: 12_000_000,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct StoredFile {
    host_id: String,
    cameras: Vec<StoredCamera>,
    preferences: StreamPreferences,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredCamera {
    id: String,
    name: String,
    /// The pairing key, encrypted for the current user and hex encoded.
    key: String,
    #[serde(default)]
    last_address: Option<SocketAddr>,
}

pub struct Store {
    path: PathBuf,
    host_id: DeviceId,
    file: StoredFile,
}

impl Store {
    /// Loads the store from `directory`, creating it (and this computer's id) on first run.
    pub fn open(directory: &Path) -> Self {
        let path = directory.join("config.json");
        let mut file: StoredFile = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();

        let host_id = DeviceId::from_hex(&file.host_id).unwrap_or_else(|| {
            let id = DeviceId::random();
            file.host_id = id.to_hex();
            id
        });

        let store = Self {
            path,
            host_id,
            file,
        };
        store.save();
        store
    }

    pub fn host_id(&self) -> DeviceId {
        self.host_id
    }

    pub fn preferences(&self) -> &StreamPreferences {
        &self.file.preferences
    }

    pub fn set_preferences(&mut self, preferences: StreamPreferences) {
        if self.file.preferences != preferences {
            self.file.preferences = preferences;
            self.save();
        }
    }

    pub fn cameras(&self) -> Vec<PairedCamera> {
        self.file
            .cameras
            .iter()
            .filter_map(|camera| {
                Some(PairedCamera {
                    id: DeviceId::from_hex(&camera.id)?,
                    name: camera.name.clone(),
                    last_address: camera.last_address,
                })
            })
            .collect()
    }

    pub fn has_cameras(&self) -> bool {
        !self.file.cameras.is_empty()
    }

    /// The pairing key for a camera, if this computer is paired with it.
    pub fn pairing_key(&self, id: DeviceId) -> Option<PairingKey> {
        let id = id.to_hex();
        let camera = self.file.cameras.iter().find(|camera| camera.id == id)?;
        let key = protection::unprotect(&decode_hex(&camera.key)?)?;
        key.try_into().ok()
    }

    /// Remembers a camera after pairing, or updates its name and address after reconnecting.
    pub fn remember(
        &mut self,
        id: DeviceId,
        name: &str,
        key: Option<&PairingKey>,
        address: SocketAddr,
    ) {
        let hex_id = id.to_hex();
        let existing = self
            .file
            .cameras
            .iter()
            .position(|camera| camera.id == hex_id);
        let key = match (key, existing) {
            (Some(key), _) => match protection::protect(key) {
                Some(protected) => encode_hex(&protected),
                None => {
                    log::error!(
                        "cannot encrypt the pairing key; the camera will not be remembered"
                    );
                    return;
                }
            },
            (None, Some(index)) => self.file.cameras[index].key.clone(),
            (None, None) => return,
        };

        let camera = StoredCamera {
            id: hex_id,
            name: name.to_owned(),
            key,
            last_address: Some(address),
        };
        match existing {
            Some(index) => self.file.cameras[index] = camera,
            None => self.file.cameras.push(camera),
        }
        self.save();
    }

    pub fn forget(&mut self, id: DeviceId) {
        let id = id.to_hex();
        self.file.cameras.retain(|camera| camera.id != id);
        self.save();
    }

    fn save(&self) {
        let result = self
            .path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| {
                let json = serde_json::to_vec_pretty(&self.file).expect("the store serializes");
                fs::write(&self.path, json)
            });
        if let Err(error) = result {
            log::error!("cannot save {}: {error}", self.path.display());
        }
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).ok())
        .collect()
}

/// Encrypts secrets for the signed-in Windows user with DPAPI.
#[cfg(windows)]
mod protection {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    fn blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr().cast_mut(),
        }
    }

    /// Copies a blob DPAPI allocated and releases it.
    fn take(output: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        // SAFETY: DPAPI returned `cbData` bytes at `pbData`, allocated with LocalAlloc.
        unsafe {
            let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
            bytes
        }
    }

    pub fn protect(secret: &[u8]) -> Option<Vec<u8>> {
        let mut output = CRYPT_INTEGER_BLOB::default();
        // SAFETY: the input blob borrows `secret` for the duration of the call.
        unsafe {
            CryptProtectData(
                &blob(secret),
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
            .ok()?;
        }
        Some(take(output))
    }

    pub fn unprotect(protected: &[u8]) -> Option<Vec<u8>> {
        let mut output = CRYPT_INTEGER_BLOB::default();
        // SAFETY: the input blob borrows `protected` for the duration of the call.
        unsafe {
            CryptUnprotectData(
                &blob(protected),
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
            .ok()?;
        }
        Some(take(output))
    }
}

#[cfg(not(windows))]
mod protection {
    pub fn protect(secret: &[u8]) -> Option<Vec<u8>> {
        Some(secret.to_vec())
    }

    pub fn unprotect(protected: &[u8]) -> Option<Vec<u8>> {
        Some(protected.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("paircam-store-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        directory
    }

    #[test]
    fn keeps_its_identity_and_cameras_across_runs() {
        let directory = temp_dir("identity");
        let camera_id = DeviceId([7; 16]);
        let address: SocketAddr = "192.168.1.20:47810".parse().unwrap();

        let host_id = {
            let mut store = Store::open(&directory);
            assert!(!store.has_cameras());
            store.remember(camera_id, "Test iPhone", Some(&[9; 32]), address);
            store.host_id()
        };

        let mut store = Store::open(&directory);
        assert_eq!(store.host_id(), host_id);
        assert_eq!(store.pairing_key(camera_id), Some([9; 32]));
        assert_eq!(
            store.cameras(),
            vec![PairedCamera {
                id: camera_id,
                name: "Test iPhone".into(),
                last_address: Some(address),
            }]
        );

        store.forget(camera_id);
        assert_eq!(store.pairing_key(camera_id), None);
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn does_not_write_pairing_keys_in_the_clear() {
        let directory = temp_dir("secrecy");
        let mut store = Store::open(&directory);
        let key = [0xAB; 32];
        store.remember(
            DeviceId([1; 16]),
            "Phone",
            Some(&key),
            "10.0.0.2:47810".parse().unwrap(),
        );

        let saved = fs::read_to_string(directory.join("config.json")).unwrap();
        if cfg!(windows) {
            assert!(!saved.contains(&encode_hex(&key)));
        }
        let _ = fs::remove_dir_all(&directory);
    }
}
