use std::collections::HashSet;
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};

use windows::core::PCWSTR;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows::Win32::Devices::HumanInterfaceDevice::{
    HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetFeature, HidD_GetHidGuid,
    HidD_GetPreparsedData, HidD_GetProductString, HidP_GetCaps, HidP_GetUsageValue,
    HidP_GetValueCaps, HidP_Feature, HIDD_ATTRIBUTES, HIDP_CAPS, HIDP_VALUE_CAPS,
    PHIDP_PREPARSED_DATA,
};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

use crate::model::DeviceReading;

const LOGITECH_VID: u16 = 0x046D;
const APPLE_VID: u16 = 0x05AC;
const DIGITIZER_USAGE_PAGE: u16 = 0x0D;

fn is_battery_usage(page: u16, usage: u16) -> bool {
    // 0x06:0x20 = Generic Device Controls / Battery Strength
    // 0x85:0x65 = Battery System / Absolute State Of Charge
    (page == 0x06 && usage == 0x20) || (page == 0x85 && usage == 0x65)
}

/// Poll any USB/Bluetooth HID device that exposes a standard HID Battery feature report.
/// Explicitly skips Digitizer (pen tablet) devices to avoid interfering with tablet axis state,
/// and skips Logitech VID (handled with richer status by the HID++ 2.0 driver).
pub fn poll() -> Vec<DeviceReading> {
    let mut out = Vec::new();
    let mut seen_keys: HashSet<String> = HashSet::new();

    unsafe {
        let hid_guid = HidD_GetHidGuid();
        let Ok(hdev) = SetupDiGetClassDevsW(
            Some(&hid_guid),
            PCWSTR::null(),
            None,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        ) else {
            return out;
        };

        // Pass 1: collect VID:PID pairs belonging to Digitizer collections so we never touch pen tablets.
        let mut paths = Vec::new();
        let mut idx = 0u32;
        loop {
            let mut if_data = SP_DEVICE_INTERFACE_DATA {
                cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInterfaces(hdev, None, &hid_guid, idx, &mut if_data).is_err() {
                break;
            }
            idx += 1;

            let mut req_size = 0u32;
            let _ = SetupDiGetDeviceInterfaceDetailW(
                hdev,
                &if_data,
                None,
                0,
                Some(&mut req_size),
                None,
            );
            if req_size == 0 {
                continue;
            }
            let mut detail_buf = vec![0u8; req_size as usize];
            let detail_ptr = detail_buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail_ptr).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(
                hdev,
                &if_data,
                Some(detail_ptr),
                req_size,
                None,
                None,
            )
            .is_err()
            {
                continue;
            }
            let path_ptr = std::ptr::addr_of!((*detail_ptr).DevicePath) as *const u16;
            let mut len = 0usize;
            while *path_ptr.add(len) != 0 {
                len += 1;
            }
            let path = OsString::from_wide(std::slice::from_raw_parts(path_ptr, len))
                .to_string_lossy()
                .to_string();
            paths.push(path);
        }
        let _ = SetupDiDestroyDeviceInfoList(hdev);

        let mut digitizer_vid_pids: HashSet<(u16, u16)> = HashSet::new();
        struct Candidate {
            path: String,
            vid: u16,
            pid: u16,
            name: String,
            feature_len: usize,
        }
        let mut candidates = Vec::new();

        for path in paths {
            let wide: Vec<u16> = OsString::from(&path)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let Ok(handle) = CreateFileW(
                PCWSTR(wide.as_ptr()),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            ) else {
                continue;
            };
            if handle.is_invalid() {
                continue;
            }

            let mut attrs = HIDD_ATTRIBUTES {
                Size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32,
                ..Default::default()
            };
            let attr_ok = HidD_GetAttributes(handle, &mut attrs).as_bool();
            let mut pp = PHIDP_PREPARSED_DATA::default();
            let mut caps = HIDP_CAPS::default();
            let caps_ok = HidD_GetPreparsedData(handle, &mut pp).as_bool() && {
                let ok = HidP_GetCaps(pp, &mut caps).is_ok();
                let _ = HidD_FreePreparsedData(pp);
                ok
            };

            if attr_ok && caps_ok {
                if caps.UsagePage == DIGITIZER_USAGE_PAGE {
                    digitizer_vid_pids.insert((attrs.VendorID, attrs.ProductID));
                } else if attrs.VendorID != LOGITECH_VID
                    && caps.NumberFeatureValueCaps > 0
                    && caps.FeatureReportByteLength > 1
                {
                    let mut prod_buf = [0u16; 128];
                    let name = if HidD_GetProductString(
                        handle,
                        prod_buf.as_mut_ptr() as *mut _,
                        (prod_buf.len() * 2) as u32,
                    )
                    .as_bool()
                    {
                        let nul = prod_buf.iter().position(|&c| c == 0).unwrap_or(prod_buf.len());
                        OsString::from_wide(&prod_buf[..nul])
                            .to_string_lossy()
                            .trim()
                            .to_string()
                    } else {
                        String::new()
                    };
                    candidates.push(Candidate {
                        path,
                        vid: attrs.VendorID,
                        pid: attrs.ProductID,
                        name,
                        feature_len: caps.FeatureReportByteLength as usize,
                    });
                }
            }
            let _ = CloseHandle(handle);
        }

        // Pass 2: query standard battery feature reports on non-digitizer candidates.
        for c in candidates {
            if digitizer_vid_pids.contains(&(c.vid, c.pid)) {
                continue;
            }
            let key = format!("hid:{:04X}:{:04X}", c.vid, c.pid);
            if seen_keys.contains(&key) {
                continue;
            }
            let wide: Vec<u16> = OsString::from(&c.path)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let Ok(handle) = CreateFileW(
                PCWSTR(wide.as_ptr()),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            ) else {
                continue;
            };
            if handle.is_invalid() {
                continue;
            }

            let mut pp = PHIDP_PREPARSED_DATA::default();
            if HidD_GetPreparsedData(handle, &mut pp).as_bool() {
                let mut caps = HIDP_CAPS::default();
                if HidP_GetCaps(pp, &mut caps).is_ok() && caps.NumberFeatureValueCaps > 0 {
                    let mut len = caps.NumberFeatureValueCaps;
                    let mut vcaps = vec![HIDP_VALUE_CAPS::default(); len as usize];
                    if HidP_GetValueCaps(HidP_Feature, vcaps.as_mut_ptr(), &mut len, pp).is_ok() {
                        for v in &vcaps[..len as usize] {
                            let usage = if v.IsRange.as_bool() {
                                v.Anonymous.Range.UsageMin
                            } else {
                                v.Anonymous.NotRange.Usage
                            };
                            if !is_battery_usage(v.UsagePage, usage) {
                                continue;
                            }
                            let mut buf = vec![0u8; c.feature_len];
                            buf[0] = v.ReportID;
                            if HidD_GetFeature(handle, buf.as_mut_ptr() as *mut _, buf.len() as u32)
                                .as_bool()
                            {
                                let mut raw_val = 0u32;
                                if HidP_GetUsageValue(
                                    HidP_Feature,
                                    v.UsagePage,
                                    0,
                                    usage,
                                    &mut raw_val,
                                    pp,
                                    &buf,
                                )
                                .is_ok()
                                {
                                    let min_v = v.LogicalMin;
                                    let max_v = v.LogicalMax;
                                    let pct = if c.vid == APPLE_VID && raw_val <= 100 {
                                        raw_val as u8
                                    } else if max_v > min_v {
                                        let clamped = (raw_val as i32).clamp(min_v, max_v);
                                        (((clamped - min_v) * 100) / (max_v - min_v)).clamp(0, 100)
                                            as u8
                                    } else if raw_val <= 100 {
                                        raw_val as u8
                                    } else {
                                        continue;
                                    };
                                    let name = if c.name.is_empty() {
                                        format!("HID Device {:04X}:{:04X}", c.vid, c.pid)
                                    } else {
                                        c.name.clone()
                                    };
                                    let kind = DeviceReading::infer_kind(&name);
                                    seen_keys.insert(key.clone());
                                    out.push(DeviceReading {
                                        key,
                                        name,
                                        kind,
                                        source: "hid-standard".to_string(),
                                        percent: Some(pct),
                                        charging: false,
                                        connected: true,
                                        stale: false,
                                    });
                                    break;
                                }
                            }
                        }
                    }
                }
                let _ = HidD_FreePreparsedData(pp);
            }
            let _ = CloseHandle(handle);
        }
    }
    out
}
