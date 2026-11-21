use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, GetDC, ReleaseDC, BI_RGB, BITMAPINFO,
    BITMAPINFOHEADER, DIB_RGB_COLORS,
};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_NOSOUND, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos,
    MessageBoxW, PostMessageW, SetForegroundWindow, TrackPopupMenu, HICON, ICONINFO, IDYES,
    MB_DEFBUTTON2, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_YESNO, MF_CHECKED, MF_GRAYED,
    MF_POPUP, MF_SEPARATOR, MF_STRING, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, WM_NULL,
};

use crate::config::{AppConfig, DisplayMode};
use crate::model::DeviceReading;

pub const ID_MODE_LOW_ONLY: usize = 1001;
pub const ID_MODE_ALWAYS: usize = 1002;
pub const ID_SHOW_LABELS: usize = 1003;
pub const ID_HIDE_CHARGING: usize = 1004;
pub const ID_REFRESH_NOW: usize = 1005;
pub const ID_AUTOSTART: usize = 1006;
pub const ID_UNINSTALL_EXIT: usize = 1007;
pub const ID_EXIT: usize = 1008;

pub const THRESHOLDS: &[u8] = &[15, 20, 25, 30, 40, 50];
pub const ID_THRESHOLD_BASE: usize = 2000;

// Per-device submenu ID stride: each device slot `i` owns 10 consecutive command IDs:
//   base + 0       -> monitor ON/OFF
//   base + 1       -> use global threshold
//   base + 2..2+N  -> override per-device threshold to THRESHOLDS[k]
pub const ID_DEVICE_BASE: usize = 4000;
pub const DEVICE_CMD_STRIDE: usize = 10;

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

pub struct TrayIcon {
    hwnd: HWND,
    callback_msg: u32,
    icon: HICON,
    added: bool,
    last_tooltip: String,
    last_alert: Option<bool>,
}

impl TrayIcon {
    pub fn new(hwnd: HWND, callback_msg: u32) -> Self {
        let icon = create_battery_icon(false);
        let mut t = Self {
            hwnd,
            callback_msg,
            icon,
            added: false,
            last_tooltip: String::new(),
            last_alert: None,
        };
        t.add_or_update("BatteryBar", false);
        t
    }

    /// Re-register the tray icon after Explorer restarts (TaskbarCreated).
    pub fn re_add(&mut self) {
        self.added = false;
        let tip = if self.last_tooltip.is_empty() {
            "BatteryBar".to_string()
        } else {
            self.last_tooltip.clone()
        };
        let alert = self.last_alert.unwrap_or(false);
        self.last_alert = None;
        self.add_or_update(&tip, alert);
    }

    pub fn add_or_update(&mut self, tooltip: &str, any_low: bool) {
        if self.added && self.last_tooltip == tooltip && self.last_alert == Some(any_low) {
            return;
        }
        unsafe {
            let new_icon = create_battery_icon(any_low);
            let mut nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: 1,
                uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                uCallbackMessage: self.callback_msg,
                hIcon: new_icon,
                ..Default::default()
            };
            for (i, ch) in tooltip.encode_utf16().take(127).enumerate() {
                nid.szTip[i] = ch;
            }
            let cmd = if self.added { NIM_MODIFY } else { NIM_ADD };
            if Shell_NotifyIconW(cmd, &nid).as_bool() {
                self.added = true;
                self.last_tooltip = tooltip.to_string();
                self.last_alert = Some(any_low);
            }
            if !self.icon.0.is_null() {
                let _ = DestroyIcon(self.icon);
            }
            self.icon = new_icon;
        }
    }

    /// Show a silent notification so first-time users immediately see that the app is active.
    pub fn notify_silent(&self, title: &str, body: &str) {
        if !self.added {
            return;
        }
        unsafe {
            let mut nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: 1,
                uFlags: NIF_INFO,
                dwInfoFlags: NIIF_NOSOUND,
                ..Default::default()
            };
            for (i, ch) in title.encode_utf16().take(63).enumerate() {
                nid.szInfoTitle[i] = ch;
            }
            for (i, ch) in body.encode_utf16().take(255).enumerate() {
                nid.szInfo[i] = ch;
            }
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

}

/// Show the tray/widget popup menu outside any RefCell borrow because TrackPopupMenu runs a
/// nested modal Win32 message loop. Returns the selected command ID (or 0 if dismissed).
pub fn show_context_menu(
    hwnd: HWND,
    cfg: &AppConfig,
    devices: &[DeviceReading],
    autostart_on: bool,
) -> usize {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return 0;
        };

            let selected_count = devices
                .iter()
                .filter(|d| cfg.selected_keys.contains(&d.key))
                .count();
            let visible_count = devices
                .iter()
                .filter(|d| cfg.should_show_on_taskbar(d))
                .count();
            let summary = match cfg.display_mode {
                DisplayMode::LowOnly => format!(
                    "見守り中 {}台 / タスクバー表示 {}台（{}%以下）",
                    selected_count, visible_count, cfg.low_threshold
                ),
                DisplayMode::Always => format!(
                    "見守り中 {}台 / 常に表示中 {}台",
                    selected_count, visible_count
                ),
            };
            let summary_w = to_wide(&summary);
            let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(summary_w.as_ptr()));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

            if devices.is_empty() {
                let lbl = to_wide("検出されたワイヤレス機器なし");
                let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(lbl.as_ptr()));
            } else {
                let mut printed_offline_sep = false;
                for (i, d) in devices.iter().enumerate() {
                    if !d.connected && !printed_offline_sep && i > 0 {
                        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                        printed_offline_sep = true;
                    }
                    let selected = cfg.selected_keys.contains(&d.key);
                    let pct = d
                        .percent
                        .map(|p| format!("{p}%"))
                        .unwrap_or_else(|| "--".to_string());
                    let state_suffix = match (d.connected, d.charging) {
                        (false, _) => "（未接続）",
                        (true, true) => "（充電中）",
                        (true, false) => "",
                    };
                    let th_note = match cfg.per_device_threshold.get(&d.key) {
                        Some(th) if selected => format!(" [{th}%以下]"),
                        _ => String::new(),
                    };
                    let text = format!("{}  {}{}{}", d.name, pct, state_suffix, th_note);
                    let w = to_wide(&text);

                    if let Ok(dev_sub) = CreatePopupMenu() {
                        let base = ID_DEVICE_BASE + i * DEVICE_CMD_STRIDE;
                        let toggle_lbl = to_wide("残量を見守る");
                        let _ = AppendMenuW(
                            dev_sub,
                            MF_STRING | if selected { MF_CHECKED } else { Default::default() },
                            base,
                            PCWSTR(toggle_lbl.as_ptr()),
                        );
                        let _ = AppendMenuW(dev_sub, MF_SEPARATOR, 0, PCWSTR::null());

                        let custom_th = cfg.per_device_threshold.get(&d.key).copied();
                        let def_lbl = to_wide(&format!("全体の設定を使う（{}%以下）", cfg.low_threshold));
                        let _ = AppendMenuW(
                            dev_sub,
                            MF_STRING | if custom_th.is_none() { MF_CHECKED } else { Default::default() },
                            base + 1,
                            PCWSTR(def_lbl.as_ptr()),
                        );
                        for (ti, &th) in THRESHOLDS.iter().enumerate() {
                            let tl = to_wide(&format!("{th}% 以下で表示"));
                            let _ = AppendMenuW(
                                dev_sub,
                                MF_STRING | if custom_th == Some(th) { MF_CHECKED } else { Default::default() },
                                base + 2 + ti,
                                PCWSTR(tl.as_ptr()),
                            );
                        }
                        let parent_flags = MF_POPUP
                            | if selected {
                                MF_CHECKED
                            } else {
                                Default::default()
                            };
                        let _ = AppendMenuW(menu, parent_flags, dev_sub.0 as usize, PCWSTR(w.as_ptr()));
                    }
                }
            }

            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

            let m_low = to_wide(&format!("残量が減った機器だけ表示（既定 {}%以下）", cfg.low_threshold));
            let _ = AppendMenuW(
                menu,
                MF_STRING | if cfg.display_mode == DisplayMode::LowOnly { MF_CHECKED } else { Default::default() },
                ID_MODE_LOW_ONLY,
                PCWSTR(m_low.as_ptr()),
            );
            let m_always = to_wide("選んだ機器をすべて表示");
            let _ = AppendMenuW(
                menu,
                MF_STRING | if cfg.display_mode == DisplayMode::Always { MF_CHECKED } else { Default::default() },
                ID_MODE_ALWAYS,
                PCWSTR(m_always.as_ptr()),
            );

            if let Ok(sub) = CreatePopupMenu() {
                for (i, &th) in THRESHOLDS.iter().enumerate() {
                    let t = to_wide(&format!("{th}% 以下で表示"));
                    let flags = MF_STRING
                        | if cfg.low_threshold == th {
                            MF_CHECKED
                        } else {
                            Default::default()
                        };
                    let _ = AppendMenuW(sub, flags, ID_THRESHOLD_BASE + i, PCWSTR(t.as_ptr()));
                }
                let sub_lbl = to_wide("表示する残量の目安（全体）");
                let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, PCWSTR(sub_lbl.as_ptr()));
            }

            let m_labels = to_wide("タスクバーに短い機器名を表示");
            let _ = AppendMenuW(
                menu,
                MF_STRING | if cfg.show_labels { MF_CHECKED } else { Default::default() },
                ID_SHOW_LABELS,
                PCWSTR(m_labels.as_ptr()),
            );
            let m_hide_chg = to_wide("充電中の機器は隠す");
            let mut hide_chg_flags = MF_STRING;
            if !cfg.show_while_charging {
                hide_chg_flags |= MF_CHECKED;
            }
            if cfg.display_mode == DisplayMode::Always {
                hide_chg_flags |= MF_GRAYED;
            }
            let _ = AppendMenuW(menu, hide_chg_flags, ID_HIDE_CHARGING, PCWSTR(m_hide_chg.as_ptr()));

            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

            let m_ref = to_wide("今すぐ更新");
            let _ = AppendMenuW(menu, MF_STRING, ID_REFRESH_NOW, PCWSTR(m_ref.as_ptr()));
            let m_auto = to_wide("Windows 起動時に自動起動");
            let _ = AppendMenuW(
                menu,
                MF_STRING | if autostart_on { MF_CHECKED } else { Default::default() },
                ID_AUTOSTART,
                PCWSTR(m_auto.as_ptr()),
            );

            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let m_uninst = to_wide("自動起動と設定を消して終了...");
            let _ = AppendMenuW(menu, MF_STRING, ID_UNINSTALL_EXIT, PCWSTR(m_uninst.as_ptr()));
            let m_exit = to_wide("終了");
            let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, PCWSTR(m_exit.as_ptr()));

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = SetForegroundWindow(hwnd);
            let chosen = TrackPopupMenu(
                menu,
                TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD,
                pt.x,
                pt.y,
                0,
                hwnd,
                None,
            );
            // Required by Win32 tray menu semantics (KB135788) so clicking outside dismisses cleanly.
            let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            chosen.0.max(0) as usize
        }
}

/// Ask the user before clearing startup registration and saved settings.
pub fn confirm_uninstall(hwnd: HWND) -> bool {
    unsafe {
        let title = to_wide("BatteryBar の設定削除");
        let body = to_wide(
            "Windows の自動起動設定と、保存した設定データをすべて削除して終了します。\n\n終了後に battery-bar.exe をゴミ箱へ移せば、PC に何も残らず完全に削除できます。\n実行しますか？",
        );
        MessageBoxW(
            hwnd,
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
        ) == IDYES
    }
}

/// Confirm completion after removing startup registration and saved settings.
pub fn notify_uninstall_done(hwnd: HWND) {
    unsafe {
        let title = to_wide("BatteryBar の削除準備が完了しました");
        let body = to_wide(
            "自動起動設定と保存データを削除しました。\nOK を押すと終了します。不要であれば battery-bar.exe をゴミ箱へ移してください。",
        );
        let _ = MessageBoxW(
            hwnd,
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        unsafe {
            if self.added {
                let nid = NOTIFYICONDATAW {
                    cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                    hWnd: self.hwnd,
                    uID: 1,
                    ..Default::default()
                };
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            }
            if !self.icon.0.is_null() {
                let _ = DestroyIcon(self.icon);
            }
        }
    }
}

/// Crisp 32x32 flat battery tray icon matching assets/app.ico (emerald healthy, crimson low alert).
fn create_battery_icon(alert: bool) -> HICON {
    unsafe {
        let w = 32i32;
        let h = 32i32;
        let screen_dc = GetDC(None);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let Ok(color_bmp) = CreateDIBSection(screen_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = ReleaseDC(None, screen_dc);
            return HICON::default();
        };
        let _ = ReleaseDC(None, screen_dc);

        let px = std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);
        px.fill(0);

        let bg: u32 = 0xFF1C_2026;
        let outline: u32 = if alert { 0xFFF8_7171 } else { 0xFFEE_F2F8 };
        let fill: u32 = if alert { 0xFFEF_4444 } else { 0xFF34_D399 };

        let mut draw_rrect = |x0: f32, y0: f32, x1: f32, y1: f32, rad: f32, col: u32| {
            for y in (y0 as i32).max(0)..=(y1 as i32).min(h - 1) {
                for x in (x0 as i32).max(0)..=(x1 as i32).min(w - 1) {
                    let fx = x as f32 + 0.5;
                    let fy = y as f32 + 0.5;
                    let cx = fx.clamp(x0 + rad, x1 - rad);
                    let cy = fy.clamp(y0 + rad, y1 - rad);
                    if (fx - cx).powi(2) + (fy - cy).powi(2) <= rad * rad {
                        px[(y * w + x) as usize] = col;
                    }
                }
            }
        };

        draw_rrect(2.0, 2.0, 30.0, 30.0, 6.0, bg);
        draw_rrect(5.0, 9.5, 24.5, 22.5, 2.2, outline);
        draw_rrect(7.2, 11.7, 22.3, 20.3, 1.0, bg);
        draw_rrect(25.3, 13.0, 27.6, 19.0, 0.8, outline);
        let bar_end = if alert { 12.2 } else { 19.8 };
        draw_rrect(8.5, 13.0, bar_end, 19.0, 0.7, fill);

        let mask_bmp = CreateBitmap(w, h, 1, 1, None);
        let ii = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask_bmp,
            hbmColor: color_bmp,
        };
        let icon = CreateIconIndirect(&ii).unwrap_or_default();
        let _ = DeleteObject(color_bmp);
        let _ = DeleteObject(mask_bmp);
        icon
    }
}
