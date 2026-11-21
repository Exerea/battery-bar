use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::core::PCWSTR;
use windows::Win32::Foundation::FreeLibrary;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use crate::model::{DeviceKind, DeviceReading};

#[repr(C)]
#[derive(Default)]
struct XInputBatteryInformation {
    battery_type: u8,
    battery_level: u8,
}

type XInputGetBatteryInformationFn =
    unsafe extern "system" fn(u32, u8, *mut XInputBatteryInformation) -> u32;

const BATTERY_DEVTYPE_GAMEPAD: u8 = 0x00;
const BATTERY_TYPE_DISCONNECTED: u8 = 0x00;
const BATTERY_TYPE_WIRED: u8 = 0x01;
const BATTERY_TYPE_UNKNOWN: u8 = 0xFF;

/// Poll connected wireless Xbox / XInput controllers by dynamically loading xinput1_4.dll.
pub fn poll() -> Vec<DeviceReading> {
    let mut out = Vec::new();
    unsafe {
        let dll_name: Vec<u16> = OsStr::new("xinput1_4.dll")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let Ok(module) = LoadLibraryW(PCWSTR(dll_name.as_ptr())) else {
            return out;
        };
        let proc = GetProcAddress(module, windows::core::s!("XInputGetBatteryInformation"));
        if let Some(raw_fn) = proc {
            let func: XInputGetBatteryInformationFn = std::mem::transmute(raw_fn);
            for slot in 0u32..4 {
                let mut info = XInputBatteryInformation::default();
                if func(slot, BATTERY_DEVTYPE_GAMEPAD, &mut info) != 0 {
                    continue;
                }
                if matches!(
                    info.battery_type,
                    BATTERY_TYPE_DISCONNECTED | BATTERY_TYPE_WIRED | BATTERY_TYPE_UNKNOWN
                ) {
                    continue;
                }
                // XInput reports 4 coarse levels: 0=Empty, 1=Low, 2=Medium, 3=Full.
                let pct = match info.battery_level {
                    0 => 5,
                    1 => 20,
                    2 => 60,
                    3 => 100,
                    _ => continue,
                };
                out.push(DeviceReading {
                    key: format!("xinput:gamepad:{slot}"),
                    name: format!("Xbox Controller #{}", slot + 1),
                    kind: DeviceKind::Controller,
                    source: "xinput".to_string(),
                    percent: Some(pct),
                    charging: false,
                    connected: true,
                    stale: false,
                });
            }
        }
        let _ = FreeLibrary(module);
    }
    out
}
