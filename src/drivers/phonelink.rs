use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::model::{DeviceKind, DeviceReading};

const MAX_CARD_BYTES: u64 = 4 * 1024 * 1024;
const STALE_THRESHOLD: Duration = Duration::from_secs(3 * 60 * 60);

/// Read phone battery from Windows Phone Link's local Start Menu Adaptive Card (read-only, offline).
pub fn poll() -> Vec<DeviceReading> {
    let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") else {
        return Vec::new();
    };
    let path = PathBuf::from(local_app_data)
        .join("Packages")
        .join("Microsoft.YourPhone_8wekyb3d8bbwe")
        .join("LocalState")
        .join("StartMenu")
        .join("StartMenuCompanion.json");

    let Ok(meta) = fs::metadata(&path) else {
        return Vec::new();
    };
    if meta.len() == 0 || meta.len() > MAX_CARD_BYTES {
        return Vec::new();
    }
    let stale = meta
        .modified()
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .map(|age| age > STALE_THRESHOLD)
        .unwrap_or(false);

    let Ok(raw) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(root) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };

    let mut parsed = ParsedCard::default();
    walk_json(&root, &mut parsed);

    if parsed.percent.is_none() {
        return Vec::new();
    }

    let name = parsed
        .device_name
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Phone (Phone Link)".to_string());

    vec![DeviceReading {
        key: "phonelink:phone".to_string(),
        name,
        kind: DeviceKind::Phone,
        source: "phonelink".to_string(),
        percent: parsed.percent,
        charging: parsed.charging,
        connected: !parsed.explicitly_disconnected,
        stale,
    }]
}

#[derive(Default)]
struct ParsedCard {
    device_name: Option<String>,
    percent: Option<u8>,
    charging: bool,
    explicitly_disconnected: bool,
}

fn walk_json(v: &Value, out: &mut ParsedCard) {
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                if let Value::String(s) = val {
                    inspect_field(k, s, out);
                } else {
                    walk_json(val, out);
                }
            }
        }
        Value::Array(arr) => {
            for item in arr {
                walk_json(item, out);
            }
        }
        _ => {}
    }
}

fn inspect_field(key: &str, text: &str, out: &mut ParsedCard) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }

    // Battery tooltip/altText in Japanese ("バッテリー残量 63%%" / "充電中") or English ("Battery 63%").
    if matches!(key, "tooltip" | "altText" | "ariaLabel" | "text") {
        let lower = trimmed.to_ascii_lowercase();
        let mentions_battery = lower.contains("battery")
            || trimmed.contains("バッテリ")
            || trimmed.contains("バッテリー");
        if mentions_battery {
            if let Some(pct) = extract_percent(trimmed) {
                out.percent = Some(pct);
            }
            if lower.contains("charging") || trimmed.contains("充電") {
                out.charging = true;
            }
        }
        if lower.contains("disconnected")
            || lower.contains("not connected")
            || trimmed.contains("未接続")
            || trimmed.contains("切断")
        {
            out.explicitly_disconnected = true;
        }
        if out.device_name.is_none()
            && (lower.contains("iphone")
                || lower.contains("pixel")
                || lower.contains("galaxy")
                || lower.contains("xperia"))
            && trimmed.len() <= 40
            && !trimmed.contains('%')
        {
            out.device_name = Some(trimmed.to_string());
        }
    }
}

fn extract_percent(s: &str) -> Option<u8> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            // Skip optional spaces before '%'
            let mut j = i;
            while j < bytes.len() && bytes[j] == b' ' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'%' {
                if let Ok(val) = s[start..i].parse::<u16>() {
                    if val <= 100 {
                        return Some(val as u8);
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    None
}
