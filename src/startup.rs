use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::core::PCWSTR;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_SZ,
};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "BatteryBar";

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// Check whether BatteryBar is registered in HKCU\...\Run (no admin rights required).
pub fn is_enabled() -> bool {
    unsafe {
        let subkey = to_wide(RUN_KEY);
        let name = to_wide(VALUE_NAME);
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(subkey.as_ptr()), 0, KEY_READ, &mut hkey).is_err() {
            return false;
        }
        let mut size = 0u32;
        let exists = RegQueryValueExW(
            hkey,
            PCWSTR(name.as_ptr()),
            None,
            None,
            None,
            Some(&mut size),
        )
        .is_ok()
            && size > 0;
        let _ = RegCloseKey(hkey);
        exists
    }
}

pub fn set_enabled(enable: bool) -> Result<(), String> {
    unsafe {
        let subkey = to_wide(RUN_KEY);
        let name = to_wide(VALUE_NAME);
        let mut hkey = HKEY::default();
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        )
        .ok()
        .map_err(|e| format!("open Run key failed: {e}"))?;

        let res = if enable {
            let exe = std::env::current_exe()
                .map_err(|e| format!("resolve current_exe failed: {e}"))?;
            let cmd = format!("\"{}\"", exe.display());
            let wide = to_wide(&cmd);
            let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
            RegSetValueExW(hkey, PCWSTR(name.as_ptr()), 0, REG_SZ, Some(bytes))
                .ok()
                .map_err(|e| format!("set Run value failed: {e}"))
        } else {
            let _ = RegDeleteValueW(hkey, PCWSTR(name.as_ptr()));
            Ok(())
        };
        let _ = RegCloseKey(hkey);
        res
    }
}

/// If autostart is already enabled, keep its registered path synchronized with the current exe
/// so moving battery-bar.exe to a new folder does not silently break Windows startup.
pub fn sync_path_if_enabled() {
    if is_enabled() {
        let _ = set_enabled(true);
    }
}

/// Remove the HKCU Run startup value cleanly without failing if it was never set.
pub fn remove_startup_entry() {
    let _ = set_enabled(false);
}

/// Detect whether the Windows taskbar uses light theme (SystemUsesLightTheme).
pub fn is_taskbar_light_theme() -> bool {
    unsafe {
        let subkey = to_wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
        let name = to_wide("SystemUsesLightTheme");
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(subkey.as_ptr()), 0, KEY_READ, &mut hkey).is_err() {
            return false;
        }
        let mut val: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let ok = RegQueryValueExW(
            hkey,
            PCWSTR(name.as_ptr()),
            None,
            None,
            Some(&mut val as *mut u32 as *mut u8),
            Some(&mut size),
        )
        .is_ok();
        let _ = RegCloseKey(hkey);
        ok && val != 0
    }
}
