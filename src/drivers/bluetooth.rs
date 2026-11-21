use std::collections::HashMap;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

use windows::core::{GUID, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
    SetupDiGetDevicePropertyW, DIGCF_ALLCLASSES, DIGCF_PRESENT, SP_DEVINFO_DATA,
};
use windows::Win32::Devices::Properties::{
    DEVPKEY_Device_ContainerId, DEVPKEY_Device_DeviceDesc, DEVPKEY_Device_FriendlyName,
    DEVPROPKEY, DEVPROPTYPE, DEVPROP_TYPE_BOOLEAN, DEVPROP_TYPE_BYTE, DEVPROP_TYPE_GUID,
    DEVPROP_TYPE_STRING,
};

use crate::model::DeviceReading;

// Windows undocumented-but-standard property keys surfaced in Settings > Bluetooth & devices.
const DEVPKEY_BLUETOOTH_BATTERY: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x104EA319_6EE2_4701_BD47_8DDBF425BBE5),
    pid: 2,
};

const DEVPKEY_DEVICE_IS_CONNECTED: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x83DA6326_97A6_4088_9453_A1923F573B29),
    pid: 15,
};

#[derive(Default)]
struct BtGroup {
    name: Option<String>,
    fallback_name: Option<String>,
    battery: Option<u8>,
    connected: bool,
}

/// Enumerate all present Windows devices exposing DEVPKEY_Bluetooth_Battery (BLE BAS / HFP)
/// and group child nodes by physical DEVPKEY_Device_ContainerId.
pub fn poll() -> Vec<DeviceReading> {
    let mut groups: HashMap<String, BtGroup> = HashMap::new();

    unsafe {
        let Ok(hdev) = SetupDiGetClassDevsW(
            None,
            PCWSTR::null(),
            None,
            DIGCF_PRESENT | DIGCF_ALLCLASSES,
        ) else {
            return Vec::new();
        };

        let mut idx: u32 = 0;
        loop {
            let mut dev_info = SP_DEVINFO_DATA {
                cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInfo(hdev, idx, &mut dev_info).is_err() {
                break;
            }
            idx += 1;

            let Some(container_id) = read_guid_prop(hdev, &dev_info, &DEVPKEY_Device_ContainerId)
            else {
                continue;
            };
            // Skip root machine container {00000000-...}
            if container_id == "{00000000-0000-0000-0000-000000000000}" {
                continue;
            }

            let battery = read_byte_prop(hdev, &dev_info, &DEVPKEY_BLUETOOTH_BATTERY)
                .filter(|&b| b <= 100);
            let connected = read_bool_prop(hdev, &dev_info, &DEVPKEY_DEVICE_IS_CONNECTED)
                .unwrap_or(false);
            let friendly = read_string_prop(hdev, &dev_info, &DEVPKEY_Device_FriendlyName);
            let desc = read_string_prop(hdev, &dev_info, &DEVPKEY_Device_DeviceDesc);

            // Track container whenever it has battery, or already exists, or has a clean BT friendly name.
            let entry = groups.entry(container_id).or_default();
            if let Some(b) = battery {
                entry.battery = Some(b);
            }
            if connected {
                entry.connected = true;
            }
            if let Some(f) = friendly {
                if is_better_name(&f, entry.name.as_deref()) {
                    entry.name = Some(f);
                }
            }
            if let Some(d) = desc {
                if entry.fallback_name.is_none() && !is_generic_bt_service(&d) {
                    entry.fallback_name = Some(d);
                }
            }
        }

        let _ = SetupDiDestroyDeviceInfoList(hdev);
    }

    let mut out = Vec::new();
    for (cid, g) in groups {
        let Some(pct) = g.battery else {
            continue;
        };
        let name = g
            .name
            .or(g.fallback_name)
            .unwrap_or_else(|| "Bluetooth Device".to_string());
        let kind = DeviceReading::infer_kind(&name);
        out.push(DeviceReading {
            key: format!("bt:{cid}"),
            name,
            kind,
            source: "bluetooth".to_string(),
            percent: Some(pct),
            charging: false,
            connected: g.connected,
            stale: false,
        });
    }
    out
}

fn is_generic_bt_service(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.contains("generic access")
        || l.contains("generic attribute")
        || l.contains("battery service")
        || l.contains("device information")
        || l.contains("human interface")
        || l.contains("hogp")
        || l.contains("avrcp")
        || l.contains("hands-free")
        || l.contains("rfcomm")
        || l.contains("phonebook")
        || l.contains("bluetooth le")
        || l.contains("hid-compliant")
        || l.contains("microsoft bluetooth")
}

fn is_better_name(candidate: &str, current: Option<&str>) -> bool {
    if candidate.trim().is_empty() || is_generic_bt_service(candidate) {
        return false;
    }
    match current {
        None => true,
        Some(cur) => {
            // Prefer shorter top-level device name over verbose sub-service names like "MX KEYS S - Stereo".
            is_generic_bt_service(cur) || candidate.len() < cur.len()
        }
    }
}

unsafe fn read_byte_prop(
    hdev: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
    dev: &SP_DEVINFO_DATA,
    key: &DEVPROPKEY,
) -> Option<u8> {
    let mut prop_type = DEVPROPTYPE::default();
    let mut buf = [0u8; 4];
    let mut req = 0u32;
    let res = SetupDiGetDevicePropertyW(
        hdev,
        dev,
        key,
        &mut prop_type,
        Some(&mut buf),
        Some(&mut req),
        0,
    );
    if res.is_ok() && prop_type == DEVPROP_TYPE_BYTE && req >= 1 {
        Some(buf[0])
    } else {
        None
    }
}

unsafe fn read_bool_prop(
    hdev: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
    dev: &SP_DEVINFO_DATA,
    key: &DEVPROPKEY,
) -> Option<bool> {
    let mut prop_type = DEVPROPTYPE::default();
    let mut buf = [0u8; 4];
    let mut req = 0u32;
    let res = SetupDiGetDevicePropertyW(
        hdev,
        dev,
        key,
        &mut prop_type,
        Some(&mut buf),
        Some(&mut req),
        0,
    );
    if res.is_ok() && prop_type == DEVPROP_TYPE_BOOLEAN && req >= 1 {
        Some(buf[0] != 0)
    } else {
        None
    }
}

unsafe fn read_guid_prop(
    hdev: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
    dev: &SP_DEVINFO_DATA,
    key: &DEVPROPKEY,
) -> Option<String> {
    let mut prop_type = DEVPROPTYPE::default();
    let mut buf = [0u8; 16];
    let mut req = 0u32;
    let res = SetupDiGetDevicePropertyW(
        hdev,
        dev,
        key,
        &mut prop_type,
        Some(&mut buf),
        Some(&mut req),
        0,
    );
    if res.is_ok() && prop_type == DEVPROP_TYPE_GUID && req == 16 {
        let d1 = u32::from_le_bytes(buf[0..4].try_into().ok()?);
        let d2 = u16::from_le_bytes(buf[4..6].try_into().ok()?);
        let d3 = u16::from_le_bytes(buf[6..8].try_into().ok()?);
        let d4 = &buf[8..16];
        Some(format!(
            "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
            d1, d2, d3, d4[0], d4[1], d4[2], d4[3], d4[4], d4[5], d4[6], d4[7]
        ))
    } else {
        None
    }
}

unsafe fn read_string_prop(
    hdev: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
    dev: &SP_DEVINFO_DATA,
    key: &DEVPROPKEY,
) -> Option<String> {
    let mut prop_type = DEVPROPTYPE::default();
    let mut buf = [0u8; 512];
    let mut req = 0u32;
    let res = SetupDiGetDevicePropertyW(
        hdev,
        dev,
        key,
        &mut prop_type,
        Some(&mut buf),
        Some(&mut req),
        0,
    );
    if res.is_ok() && prop_type == DEVPROP_TYPE_STRING && req >= 2 {
        let u16_len = (req as usize) / 2;
        let mut wide = Vec::with_capacity(u16_len);
        for chunk in buf[..u16_len * 2].chunks_exact(2) {
            let w = u16::from_le_bytes([chunk[0], chunk[1]]);
            if w == 0 {
                break;
            }
            wide.push(w);
        }
        let s = OsString::from_wide(&wide).to_string_lossy().trim().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    None
}
