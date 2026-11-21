use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Mouse,
    Keyboard,
    Headset,
    Phone,
    Controller,
    Other,
}

impl DeviceKind {
    pub fn glyph_codepoint(self) -> char {
        // Segoe Fluent Icons / Segoe MDL2 Assets (built into Windows 10/11)
        match self {
            DeviceKind::Mouse => '\u{E962}',      // Mouse
            DeviceKind::Keyboard => '\u{E765}',   // KeyboardClassic
            DeviceKind::Headset => '\u{E95B}',    // Headphone
            DeviceKind::Phone => '\u{E8EA}',      // CellPhone
            DeviceKind::Controller => '\u{E7FC}', // Game
            DeviceKind::Other => '\u{E702}',      // Bluetooth
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceReading {
    /// Stable identifier across restarts (e.g. "logitech:046D:C54D:1", "bt:{guid}", "phonelink:phone").
    pub key: String,
    pub name: String,
    pub kind: DeviceKind,
    pub source: String,
    pub percent: Option<u8>,
    pub charging: bool,
    pub connected: bool,
    pub stale: bool,
}

impl DeviceReading {
    /// Classify device kind heuristically from its display name when the driver cannot report it.
    pub fn infer_kind(name: &str) -> DeviceKind {
        let lower = name.to_ascii_lowercase();
        if lower.contains("iphone")
            || lower.contains("pixel")
            || lower.contains("galaxy s")
            || lower.contains("galaxy z")
            || lower.contains("xperia")
            || lower.contains("phone")
        {
            DeviceKind::Phone
        } else if lower.contains("headset")
            || lower.contains("headphone")
            || lower.contains("g535")
            || lower.contains("g733")
            || lower.contains("g435")
            || lower.contains("airpods")
            || lower.contains("buds")
            || lower.contains("wh-")
            || lower.contains("wf-")
            || lower.contains("ear")
            || lower.contains("audio")
        {
            DeviceKind::Headset
        } else if lower.contains("keyboard")
            || lower.contains("keys")
            || lower.contains("keychron")
            || lower.contains("k380")
            || lower.contains("hhkb")
        {
            DeviceKind::Keyboard
        } else if lower.contains("mouse")
            || lower.contains("superlight")
            || lower.contains("pro x")
            || lower.contains("pro wireless")
            || lower.contains("g pro")
            || lower.contains("mx master")
            || lower.contains("mx anywhere")
            || lower.contains("viper")
            || lower.contains("deathadder")
        {
            DeviceKind::Mouse
        } else if lower.contains("controller")
            || lower.contains("dualsense")
            || lower.contains("xbox")
        {
            DeviceKind::Controller
        } else {
            DeviceKind::Other
        }
    }
}
