//! User settings, persisted as JSON next to the app's config directory.
//! Everything stays on the machine; nothing here is ever sent anywhere.

use crate::model::ConflictPolicy;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_PORT: u16 = 47_654;
/// Chunk used for every streamed read/write. Bounded so a 20 GB file costs
/// this much RAM, not 20 GB.
pub const STREAM_CHUNK: usize = 256 * 1024;
pub const PART_DIR: &str = ".sharelocal-part";

fn default_port() -> u16 {
    DEFAULT_PORT
}
fn default_max_concurrent() -> usize {
    3
}
fn default_true() -> bool {
    true
}
fn default_theme() -> String {
    "system".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Stable identity, generated once. Not derived from anything personal.
    pub device_id: String,
    pub device_name: String,
    pub download_dir: PathBuf,

    // Transfers
    pub auto_accept: bool,
    pub pin: Option<String>,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,
    pub conflict_policy: ConflictPolicy,

    // Network
    #[serde(default = "default_port")]
    pub port: u16,
    pub preferred_interface: Option<String>,
    #[serde(default = "default_true")]
    pub discovery_mdns: bool,
    #[serde(default = "default_true")]
    pub discovery_udp: bool,
    pub tls: bool,

    // General
    #[serde(default = "default_theme")]
    pub theme: String,
    pub autostart: bool,
    pub start_minimized: bool,
    #[serde(default = "default_true")]
    pub notifications: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            device_id: uuid::Uuid::new_v4().to_string(),
            device_name: default_device_name(),
            download_dir: default_download_dir(),
            auto_accept: false,
            pin: None,
            max_concurrent: default_max_concurrent(),
            conflict_policy: ConflictPolicy::KeepBoth,
            port: DEFAULT_PORT,
            preferred_interface: None,
            discovery_mdns: true,
            discovery_udp: true,
            tls: false,
            theme: default_theme(),
            autostart: false,
            start_minimized: false,
            notifications: true,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(mut s) => {
                    s.normalise();
                    s
                }
                Err(err) => {
                    eprintln!("[settings] {path:?} is unreadable ({err}); using defaults");
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        // Write-then-rename so a crash can never leave a truncated config.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }

    /// Clamp anything a hand-edited file could have made nonsensical.
    pub fn normalise(&mut self) {
        if self.device_id.trim().is_empty() {
            self.device_id = uuid::Uuid::new_v4().to_string();
        }
        if self.device_name.trim().is_empty() {
            self.device_name = default_device_name();
        }
        self.device_name = self.device_name.chars().take(64).collect();
        if self.max_concurrent == 0 {
            self.max_concurrent = 1;
        }
        self.max_concurrent = self.max_concurrent.min(16);
        if self.port < 1024 {
            self.port = DEFAULT_PORT;
        }
        if let Some(pin) = &self.pin {
            let cleaned: String = pin.chars().filter(|c| c.is_ascii_digit()).collect();
            self.pin = if cleaned.len() >= 4 {
                Some(cleaned)
            } else {
                None
            };
        }
        if !matches!(self.theme.as_str(), "system" | "light" | "dark") {
            self.theme = default_theme();
        }
        if self.download_dir.as_os_str().is_empty() {
            self.download_dir = default_download_dir();
        }
    }

    pub fn pin_required(&self) -> bool {
        self.pin.is_some()
    }

    /// Constant-time PIN comparison so a peer cannot time-probe the digits.
    pub fn pin_matches(&self, candidate: Option<&str>) -> bool {
        use subtle::ConstantTimeEq;
        match (&self.pin, candidate) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(expected), Some(given)) => {
                if expected.len() != given.len() {
                    // Still burn a comparison to keep the branch cheap and
                    // uniform; length is not secret.
                    return false;
                }
                expected.as_bytes().ct_eq(given.as_bytes()).into()
            }
        }
    }

    pub fn os_name() -> &'static str {
        if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else {
            "unknown"
        }
    }

    pub fn device_kind() -> &'static str {
        // Desktop builds always report a computer; phones reach us through the
        // browser client, which identifies itself separately.
        if cfg!(target_os = "macos") {
            "laptop"
        } else {
            "desktop"
        }
    }
}

pub fn default_device_name() -> String {
    if let Ok(name) = std::env::var("SHARELOCAL_DEVICE_NAME") {
        if !name.trim().is_empty() {
            return name;
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("scutil")
            .arg("--get")
            .arg("ComputerName")
            .output()
        {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !name.is_empty() {
                return name;
            }
        }
    }
    for key in ["COMPUTERNAME", "HOSTNAME", "HOST"] {
        if let Ok(v) = std::env::var(key) {
            if !v.trim().is_empty() {
                return v.trim().to_string();
            }
        }
    }
    if let Ok(out) = std::process::Command::new("hostname").output() {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !name.is_empty() {
            return name.split('.').next().unwrap_or(&name).to_string();
        }
    }
    format!("{} device", Settings::os_name())
}

pub fn default_download_dir() -> PathBuf {
    home_dir()
        .map(|h| h.join("Downloads").join("ShareLocal"))
        .unwrap_or_else(|| PathBuf::from("ShareLocal"))
}

pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Ok(p) = std::env::var("USERPROFILE") {
            if !p.is_empty() {
                return Some(PathBuf::from(p));
            }
        }
    }
    std::env::var("HOME")
        .ok()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_clamps_hand_edited_values() {
        let mut s = Settings {
            device_name: "  ".into(),
            max_concurrent: 0,
            port: 80,
            theme: "neon".into(),
            pin: Some("12".into()),
            ..Default::default()
        };
        s.normalise();
        assert!(!s.device_name.trim().is_empty());
        assert_eq!(s.max_concurrent, 1);
        assert_eq!(s.port, DEFAULT_PORT);
        assert_eq!(s.theme, "system");
        assert_eq!(s.pin, None, "a PIN shorter than 4 digits is discarded");

        s.max_concurrent = 999;
        s.normalise();
        assert_eq!(s.max_concurrent, 16);
    }

    #[test]
    fn pin_matching_is_exact() {
        let mut s = Settings::default();
        assert!(s.pin_matches(None), "no PIN configured accepts anything");

        s.pin = Some("4821".into());
        assert!(s.pin_matches(Some("4821")));
        assert!(!s.pin_matches(Some("4822")));
        assert!(!s.pin_matches(Some("482")));
        assert!(!s.pin_matches(Some("48210")));
        assert!(!s.pin_matches(None));
    }

    #[test]
    fn pin_strips_non_digits_on_normalise() {
        let mut s = Settings {
            pin: Some("48-21".into()),
            ..Default::default()
        };
        s.normalise();
        assert_eq!(s.pin.as_deref(), Some("4821"));
    }

    #[test]
    fn settings_round_trip_through_disk() {
        let dir = std::env::temp_dir().join(format!("sl-cfg-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        let original = Settings {
            device_name: "Test Box".into(),
            pin: Some("9137".into()),
            auto_accept: true,
            ..Default::default()
        };
        original.save(&path).unwrap();

        let loaded = Settings::load(&path);
        assert_eq!(loaded.device_name, "Test Box");
        assert_eq!(loaded.pin.as_deref(), Some("9137"));
        assert!(loaded.auto_accept);
        assert_eq!(loaded.device_id, original.device_id);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_settings_file_falls_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!("sl-cfg-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, b"{ not json").unwrap();

        let loaded = Settings::load(&path);
        assert_eq!(loaded.port, DEFAULT_PORT);
        std::fs::remove_dir_all(&dir).ok();
    }
}
