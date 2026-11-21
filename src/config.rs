use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING};

use crate::model::DeviceReading;

fn to_wide_path(path: &std::path::Path) -> Vec<u16> {
    OsStr::new(path).encode_wide().chain(std::iter::once(0)).collect()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayMode {
    /// Show only selected devices whose battery is at or below the threshold.
    LowOnly,
    /// Show all selected devices regardless of battery percentage.
    Always,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub display_mode: DisplayMode,
    /// Global low-battery threshold (%) for LowOnly display and amber/red highlighting.
    pub low_threshold: u8,
    /// Poll interval in seconds.
    pub poll_interval_secs: u64,
    /// Whether to show short device labels (e.g. "MX", "PRO", "スマホ") beside each icon.
    #[serde(default = "default_true")]
    pub show_labels: bool,
    /// Whether to still show a low device while it is actively charging.
    #[serde(default)]
    pub show_while_charging: bool,
    /// Explicit set of device keys selected for taskbar monitoring.
    #[serde(default)]
    pub selected_keys: HashSet<String>,
    #[serde(default)]
    pub selection_initialized: bool,
    /// Optional per-device threshold overrides (device key -> threshold %).
    #[serde(default)]
    pub per_device_threshold: HashMap<String, u8>,
    /// Optional custom short labels shown next to icons (device key -> label).
    #[serde(default)]
    pub custom_labels: HashMap<String, String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            display_mode: DisplayMode::LowOnly,
            low_threshold: 30,
            poll_interval_secs: 60,
            show_labels: true,
            show_while_charging: false,
            selected_keys: HashSet::new(),
            selection_initialized: false,
            per_device_threshold: HashMap::new(),
            custom_labels: HashMap::new(),
        }
    }
}

impl AppConfig {
    pub fn config_dir() -> PathBuf {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("BatteryBar")
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Persist config atomically via a temporary file so power loss never leaves a truncated JSON.
    pub fn save(&self) -> Result<(), String> {
        let dir = Self::config_dir();
        fs::create_dir_all(&dir).map_err(|e| format!("create config dir failed: {e}"))?;
        let path = Self::config_path();
        let tmp_path = dir.join("config.json.tmp");
        let body = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&tmp_path, body).map_err(|e| format!("write temp config failed: {e}"))?;
        let src = to_wide_path(&tmp_path);
        let dst = to_wide_path(&path);
        let replaced = unsafe {
            MoveFileExW(
                PCWSTR(src.as_ptr()),
                PCWSTR(dst.as_ptr()),
                MOVEFILE_REPLACE_EXISTING,
            )
        };
        if let Err(e) = replaced {
            let _ = fs::remove_file(&tmp_path);
            return Err(format!("replace config failed: {e}"));
        }
        Ok(())
    }

    /// Delete all saved configuration files and remove %APPDATA%\BatteryBar cleanly on uninstall.
    pub fn remove_all_data() -> Result<(), String> {
        let dir = Self::config_dir();
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(|e| format!("remove config dir failed: {e}"))?;
        }
        Ok(())
    }

    /// On first run, select only currently connected devices so disconnected legacy BT pairings
    /// do not clutter monitoring; the user can toggle any device anytime from the tray menu.
    pub fn seed_selection_if_needed(&mut self, readings: &[DeviceReading]) -> bool {
        if self.selection_initialized || readings.is_empty() {
            return false;
        }
        for r in readings.iter().filter(|d| d.connected) {
            self.selected_keys.insert(r.key.clone());
        }
        self.selection_initialized = true;
        true
    }

    pub fn effective_threshold(&self, key: &str) -> u8 {
        self.per_device_threshold
            .get(key)
            .copied()
            .unwrap_or(self.low_threshold)
    }

    /// Decide whether a device should currently appear in the horizontal taskbar strip.
    pub fn should_show_on_taskbar(&self, reading: &DeviceReading) -> bool {
        if !self.selected_keys.contains(&reading.key) {
            return false;
        }
        if !reading.connected {
            return false;
        }
        let Some(pct) = reading.percent else {
            return false;
        };
        match self.display_mode {
            DisplayMode::Always => true,
            DisplayMode::LowOnly => {
                if reading.charging && !self.show_while_charging {
                    return false;
                }
                pct <= self.effective_threshold(&reading.key)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DeviceKind;

    fn sample(key: &str, pct: u8, charging: bool, connected: bool) -> DeviceReading {
        DeviceReading {
            key: key.to_string(),
            name: "Sample".to_string(),
            kind: DeviceKind::Mouse,
            source: "test".to_string(),
            percent: Some(pct),
            charging,
            connected,
            stale: false,
        }
    }

    #[test]
    fn low_only_shows_only_selected_unplugged_low_devices() {
        let mut cfg = AppConfig::default();
        cfg.selected_keys.insert("a".to_string());
        cfg.selection_initialized = true;

        assert!(cfg.should_show_on_taskbar(&sample("a", 30, false, true)));
        assert!(cfg.should_show_on_taskbar(&sample("a", 10, false, true)));
        assert!(!cfg.should_show_on_taskbar(&sample("a", 31, false, true)));
        assert!(!cfg.should_show_on_taskbar(&sample("a", 10, true, true)));
        assert!(!cfg.should_show_on_taskbar(&sample("a", 10, false, false)));
        assert!(!cfg.should_show_on_taskbar(&sample("b", 10, false, true)));
    }

    #[test]
    fn per_device_threshold_and_always_mode_work() {
        let mut cfg = AppConfig::default();
        cfg.selected_keys.insert("phone".to_string());
        cfg.per_device_threshold.insert("phone".to_string(), 40);
        assert!(cfg.should_show_on_taskbar(&sample("phone", 38, false, true)));

        cfg.display_mode = DisplayMode::Always;
        assert!(cfg.should_show_on_taskbar(&sample("phone", 95, true, true)));
    }

    #[test]
    fn default_shows_labels_for_non_technical_clarity() {
        let cfg = AppConfig::default();
        assert!(cfg.show_labels);
        assert_eq!(cfg.low_threshold, 30);
    }
}
