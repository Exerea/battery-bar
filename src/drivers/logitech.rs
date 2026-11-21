use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows::Win32::Devices::HumanInterfaceDevice::{
    HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetHidGuid, HidD_GetProductString,
    HidD_GetPreparsedData, HidP_GetCaps, HIDD_ATTRIBUTES, HIDP_CAPS, PHIDP_PREPARSED_DATA,
};
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_IO_PENDING, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED,
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use crate::model::{DeviceKind, DeviceReading};

const LOGITECH_VID: u16 = 0x046D;
// Software ID 0x0D isolates our read-only HID++ 2.0 responses from G HUB / Options+.
const SW_ID: u8 = 0x0D;

#[derive(Clone)]
struct HidCollection {
    path: String,
    pid: u16,
    in_len: usize,
    out_len: usize,
    product_string: String,
}

struct HidDevice {
    handle: HANDLE,
    in_len: usize,
    out_len: usize,
}

impl Drop for HidDevice {
    fn drop(&mut self) {
        unsafe {
            if !self.handle.is_invalid() {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

impl HidDevice {
    fn open(coll: &HidCollection) -> Option<Self> {
        let wide: Vec<u16> = OsString::from(&coll.path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
                None,
            )
            .ok()?
        };
        if handle.is_invalid() {
            return None;
        }
        Some(Self {
            handle,
            in_len: coll.in_len,
            out_len: coll.out_len,
        })
    }

    fn write_report(&self, data: &[u8]) -> bool {
        let mut buf = vec![0u8; self.out_len.max(data.len())];
        let copy_len = data.len().min(buf.len());
        buf[..copy_len].copy_from_slice(&data[..copy_len]);

        unsafe {
            let Ok(event) = CreateEventW(None, true, false, PCWSTR::null()) else {
                return false;
            };
            let mut ov = OVERLAPPED {
                hEvent: event,
                ..Default::default()
            };
            let mut written = 0u32;
            let ok = match WriteFile(self.handle, Some(&buf), Some(&mut written), Some(&mut ov)) {
                Ok(()) => true,
                Err(_) => {
                    if GetLastError() == ERROR_IO_PENDING {
                        let w = WaitForSingleObject(event, 200);
                        if w == WAIT_OBJECT_0 {
                            GetOverlappedResult(self.handle, &ov, &mut written, false).is_ok()
                        } else {
                            // Wait for cancellation to complete before stack-allocated OVERLAPPED/buf unwind.
                            if CancelIoEx(self.handle, Some(&ov)).is_ok() {
                                let _ = GetOverlappedResult(self.handle, &ov, &mut written, true);
                            }
                            false
                        }
                    } else {
                        false
                    }
                }
            };
            let _ = CloseHandle(event);
            ok
        }
    }

    fn read_report(&self, timeout_ms: u32) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; self.in_len.max(20)];
        unsafe {
            let event = CreateEventW(None, true, false, PCWSTR::null()).ok()?;
            let mut ov = OVERLAPPED {
                hEvent: event,
                ..Default::default()
            };
            let mut got = 0u32;
            let ok = match ReadFile(self.handle, Some(&mut buf), Some(&mut got), Some(&mut ov)) {
                Ok(()) => true,
                Err(_) => {
                    if GetLastError() == ERROR_IO_PENDING {
                        let w = WaitForSingleObject(event, timeout_ms);
                        if w == WAIT_OBJECT_0 {
                            GetOverlappedResult(self.handle, &ov, &mut got, false).is_ok()
                        } else {
                            // Wait for cancellation to complete before stack-allocated OVERLAPPED/buf unwind.
                            if CancelIoEx(self.handle, Some(&ov)).is_ok() {
                                let _ = GetOverlappedResult(self.handle, &ov, &mut got, true);
                            }
                            false
                        }
                    } else {
                        false
                    }
                }
            };
            let _ = CloseHandle(event);
            if ok && got >= 7 {
                buf.truncate(got as usize);
                Some(buf)
            } else {
                None
            }
        }
    }
}

struct HidppSession {
    short_dev: Option<HidDevice>,
    long_dev: Option<HidDevice>,
}

impl HidppSession {
    fn open(short_c: Option<&HidCollection>, long_c: Option<&HidCollection>) -> Option<Self> {
        let short_dev = short_c.and_then(HidDevice::open);
        let long_dev = long_c.and_then(HidDevice::open);
        if short_dev.is_none() && long_dev.is_none() {
            return None;
        }
        Some(Self { short_dev, long_dev })
    }

    /// Execute a read-only HID++ 2.0 function call and return the parameter bytes on success.
    fn call(&self, dev_idx: u8, feature_idx: u8, func: u8, params: &[u8]) -> Option<Vec<u8>> {
        let use_long = self.long_dev.is_some();
        let report_id = if use_long { 0x11u8 } else { 0x10u8 };
        let mut msg = vec![0u8; if use_long { 20 } else { 7 }];
        msg[0] = report_id;
        msg[1] = dev_idx;
        msg[2] = feature_idx;
        msg[3] = (func << 4) | SW_ID;
        for (i, &b) in params.iter().enumerate() {
            if 4 + i < msg.len() {
                msg[4 + i] = b;
            }
        }

        let writer = if use_long {
            self.long_dev.as_ref()?
        } else {
            self.short_dev.as_ref()?
        };
        if !writer.write_report(&msg) {
            return None;
        }

        let deadline = Instant::now() + Duration::from_millis(280);
        while Instant::now() < deadline {
            for dev in [self.long_dev.as_ref(), self.short_dev.as_ref()].into_iter().flatten() {
                let Some(r) = dev.read_report(20) else {
                    continue;
                };
                if r.len() < 7 || (r[0] != 0x10 && r[0] != 0x11) || r[1] != dev_idx {
                    continue;
                }
                if r[2] == feature_idx && r[3] == msg[3] {
                    return Some(r[4..].to_vec());
                }
                // HID++ 2.0 or 1.0 error reply for our exact request -> stop waiting immediately.
                if (r[2] == 0xFF || r[2] == 0x8F) && r[3] == feature_idx && r[4] == msg[3] {
                    return None;
                }
            }
        }
        None
    }

    fn feature_index(&self, dev_idx: u8, feature_id: u16) -> Option<u8> {
        let p = self.call(
            dev_idx,
            0x00,
            0,
            &[(feature_id >> 8) as u8, (feature_id & 0xFF) as u8],
        )?;
        let idx = *p.first()?;
        if idx == 0 {
            None
        } else {
            Some(idx)
        }
    }

    fn device_name(&self, dev_idx: u8) -> Option<String> {
        let feat = self.feature_index(dev_idx, 0x0005)?;
        let len_p = self.call(dev_idx, feat, 0, &[])?;
        let total = (*len_p.first()? as usize).min(64);
        if total == 0 {
            return None;
        }
        let mut bytes = Vec::with_capacity(total);
        while bytes.len() < total {
            let chunk = self.call(dev_idx, feat, 1, &[bytes.len() as u8])?;
            if chunk.is_empty() {
                break;
            }
            let take = (total - bytes.len()).min(chunk.len());
            bytes.extend_from_slice(&chunk[..take]);
        }
        let s = String::from_utf8_lossy(&bytes)
            .trim_matches('\0')
            .trim()
            .to_string();
        if s.is_empty() { None } else { Some(s) }
    }

    fn device_kind(&self, dev_idx: u8, fallback_name: &str) -> DeviceKind {
        if let Some(feat) = self.feature_index(dev_idx, 0x0005) {
            if let Some(p) = self.call(dev_idx, feat, 2, &[]) {
                if let Some(&t) = p.first() {
                    return match t {
                        0 => DeviceKind::Keyboard,
                        3 => DeviceKind::Mouse,
                        8 => DeviceKind::Headset,
                        6 => DeviceKind::Controller,
                        _ => DeviceReading::infer_kind(fallback_name),
                    };
                }
            }
        }
        DeviceReading::infer_kind(fallback_name)
    }

    fn read_battery(&self, dev_idx: u8, pid: u16) -> Option<(u8, bool)> {
        // 1. Unified Battery (0x1004)
        if let Some(feat) = self.feature_index(dev_idx, 0x1004) {
            if let Some(p) = self.call(dev_idx, feat, 1, &[]) {
                if p.len() >= 3 {
                    let soc = p[0];
                    let status = p[2];
                    let charging = matches!(status, 1 | 2 | 3);
                    if soc > 0 && soc <= 100 {
                        return Some((soc, charging));
                    }
                    // Coarse level fallback when exact SoC byte is 0
                    let coarse = match p[1] {
                        8 => Some(90),
                        4 => Some(50),
                        2 => Some(20),
                        1 => Some(5),
                        _ => None,
                    };
                    if let Some(c) = coarse {
                        return Some((c, charging));
                    }
                }
            }
        }
        // 2. Battery Status (0x1000)
        if let Some(feat) = self.feature_index(dev_idx, 0x1000) {
            if let Some(p) = self.call(dev_idx, feat, 0, &[]) {
                if p.len() >= 3 && p[0] <= 100 {
                    let charging = matches!(p[2], 1..=4);
                    return Some((p[0], charging));
                }
            }
        }
        // 3. Battery Voltage (0x1001)
        if let Some(feat) = self.feature_index(dev_idx, 0x1001) {
            if let Some(p) = self.call(dev_idx, feat, 0, &[]) {
                if p.len() >= 3 {
                    let mv = ((p[0] as i32) << 8) | (p[1] as i32);
                    let charging = (p[2] & 0x80) != 0;
                    if mv > 2500 && mv < 5000 {
                        return Some((voltage_to_percent(pid, mv), charging));
                    }
                }
            }
        }
        // 4. ADC Measurement (0x1F20, used by Logitech G headsets including G535)
        if let Some(feat) = self.feature_index(dev_idx, 0x1F20) {
            if let Some(p) = self.call(dev_idx, feat, 0, &[]) {
                if p.len() >= 3 {
                    let mv = ((p[0] as i32) << 8) | (p[1] as i32);
                    let charging = p[2] == 0x03;
                    if mv > 2500 && mv < 5000 {
                        return Some((voltage_to_percent(pid, mv), charging));
                    }
                }
            }
        }
        None
    }
}

fn interpolate(mv: i32, volts: &[i32], pcts: &[i32]) -> u8 {
    if mv >= volts[0] {
        return pcts[0] as u8;
    }
    let last = volts.len() - 1;
    if mv <= volts[last] {
        return pcts[last] as u8;
    }
    for i in 0..last {
        if mv <= volts[i] && mv >= volts[i + 1] {
            let num = (mv - volts[i + 1]) * (pcts[i] - pcts[i + 1]);
            let den = (volts[i] - volts[i + 1]).max(1);
            return (pcts[i + 1] + num / den).clamp(0, 100) as u8;
        }
    }
    0
}

fn voltage_to_percent(pid: u16, mv: i32) -> u8 {
    match pid {
        // G535 Wireless Gaming Headset calibration curve
        0x0AC4 => interpolate(
            mv,
            &[4175, 3817, 3766, 3730, 3664, 3310],
            &[100, 50, 30, 20, 5, 0],
        ),
        _ => interpolate(
            mv,
            &[4186, 4067, 3989, 3922, 3859, 3811, 3778, 3751, 3717, 3671, 3646, 3579, 3500],
            &[100, 90, 80, 70, 60, 50, 40, 30, 20, 15, 10, 5, 0],
        ),
    }
}

fn enumerate_logitech_hidpp() -> Vec<HidCollection> {
    let mut out = Vec::new();
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

            // Quick VID filter before opening handles.
            if !path.to_ascii_lowercase().contains("vid_046d") {
                continue;
            }

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

            let mut prod_buf = [0u16; 128];
            let prod = if HidD_GetProductString(
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
            let _ = CloseHandle(handle);

            if !attr_ok || !caps_ok || attrs.VendorID != LOGITECH_VID {
                continue;
            }

            let is_hidpp_short = caps.OutputReportByteLength == 7
                && ((caps.UsagePage == 0xFF00 && caps.Usage == 0x0001)
                    || (caps.UsagePage == 0xFF43 && caps.Usage == 0x0301));
            let is_hidpp_long = caps.OutputReportByteLength == 20
                && ((caps.UsagePage == 0xFF00 && caps.Usage == 0x0002)
                    || (caps.UsagePage == 0xFF43
                        && (caps.Usage == 0x0202 || caps.Usage == 0x0302)));

            if is_hidpp_short || is_hidpp_long {
                out.push(HidCollection {
                    path,
                    pid: attrs.ProductID,
                    in_len: caps.InputReportByteLength as usize,
                    out_len: caps.OutputReportByteLength as usize,
                    product_string: prod,
                });
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(hdev);
    }
    out
}

/// Poll all connected Logitech HID++ 2.0 wireless devices (LIGHTSPEED, Bolt, Unifying, G headsets).
pub fn poll() -> Vec<DeviceReading> {
    let colls = enumerate_logitech_hidpp();
    let mut by_pid: BTreeMap<u16, (Option<HidCollection>, Option<HidCollection>)> = BTreeMap::new();
    for c in colls {
        let entry = by_pid.entry(c.pid).or_insert((None, None));
        if c.out_len == 7 && entry.0.is_none() {
            entry.0 = Some(c);
        } else if c.out_len == 20 && entry.1.is_none() {
            entry.1 = Some(c);
        }
    }

    let mut out = Vec::new();
    for (pid, (short_c, long_c)) in by_pid {
        let fallback_product = long_c
            .as_ref()
            .or(short_c.as_ref())
            .map(|c| c.product_string.clone())
            .unwrap_or_else(|| format!("Logitech {pid:04X}"));

        let Some(sess) = HidppSession::open(short_c.as_ref(), long_c.as_ref()) else {
            continue;
        };

        // Try direct device index 0xFF first (headsets, wired/BT direct HID++), then receiver slots 1..=6.
        let mut found_direct = false;
        if let Some((pct, charging)) = sess.read_battery(0xFF, pid) {
            let name = sess
                .device_name(0xFF)
                .unwrap_or_else(|| fallback_product.clone());
            let kind = sess.device_kind(0xFF, &name);
            out.push(DeviceReading {
                key: format!("logitech:046D:{pid:04X}:FF"),
                name,
                kind,
                source: "logitech-hidpp".to_string(),
                percent: Some(pct),
                charging,
                connected: true,
                stale: false,
            });
            found_direct = true;
        }

        if !found_direct {
            for slot in 1u8..=6 {
                if let Some((pct, charging)) = sess.read_battery(slot, pid) {
                    let name = sess
                        .device_name(slot)
                        .unwrap_or_else(|| format!("{fallback_product} #{slot}"));
                    let kind = sess.device_kind(slot, &name);
                    out.push(DeviceReading {
                        key: format!("logitech:046D:{pid:04X}:{slot}"),
                        name,
                        kind,
                        source: "logitech-hidpp".to_string(),
                        percent: Some(pct),
                        charging,
                        connected: true,
                        stale: false,
                    });
                }
            }
        }
    }
    out
}
