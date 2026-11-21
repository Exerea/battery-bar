use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW,
    GetDC, GetMonitorInfoW, MonitorFromWindow, ReleaseDC, SelectObject, SetBkMode, SetTextColor,
    AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER,
    BLENDFUNCTION, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DIB_RGB_COLORS, DT_CALCRECT, DT_LEFT,
    DT_NOCLIP, DT_SINGLELINE, DT_VCENTER, FW_NORMAL, FW_SEMIBOLD, HBITMAP, HDC, HFONT,
    MONITORINFO, MONITOR_DEFAULTTOPRIMARY, OUT_TT_PRECIS, TRANSPARENT,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowExW, FindWindowW, GetClassNameW,
    GetForegroundWindow, GetWindow, GetWindowRect, IsWindow, IsWindowVisible, LoadCursorW,
    PostMessageW, RegisterClassW, SetWindowPos, ShowWindow, UpdateLayeredWindow, CS_HREDRAW,
    CS_VREDRAW, GW_HWNDPREV, HWND_TOPMOST, IDC_HAND, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONUP, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::model::DeviceKind;

const WIDGET_CLASS: &str = "BatteryBarTaskbarStrip";
const WM_MOUSELEAVE: u32 = 0x02A3;
const GLYPH_CHARGING: &str = "\u{E945}"; // Segoe Fluent Icons LightningBolt

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StripItem {
    pub kind: DeviceKind,
    pub short_label: String,
    pub percent: u8,
    pub charging: bool,
    pub critical: bool,
    pub low: bool,
}

pub struct TaskbarWidget {
    inst: HINSTANCE,
    hwnd: HWND,
    width: i32,
    height: i32,
    light_theme: bool,
    hovered: bool,
    visible: bool,
    last_pos: Option<(i32, i32)>,
    items: Vec<StripItem>,
}

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

// Controller window target for click & hover notifications from the layered popup strip.
static mut NOTIFY_TARGET: (isize, u32, u32) = (0, 0, 0);

unsafe extern "system" fn widget_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_LBUTTONUP | WM_RBUTTONUP => {
            let (target, click_msg, _) = NOTIFY_TARGET;
            if target != 0 && click_msg != 0 {
                let _ = PostMessageW(HWND(target as *mut _), click_msg, WPARAM(0), LPARAM(0));
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            let _ = TrackMouseEvent(&mut tme);
            let (target, _, hover_msg) = NOTIFY_TARGET;
            if target != 0 && hover_msg != 0 {
                let _ = PostMessageW(HWND(target as *mut _), hover_msg, WPARAM(1), LPARAM(0));
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            let (target, _, hover_msg) = NOTIFY_TARGET;
            if target != 0 && hover_msg != 0 {
                let _ = PostMessageW(HWND(target as *mut _), hover_msg, WPARAM(0), LPARAM(0));
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

impl TaskbarWidget {
    pub fn new(
        inst: HINSTANCE,
        notify_hwnd: HWND,
        click_msg: u32,
        hover_msg: u32,
        light_theme: bool,
    ) -> Self {
        unsafe {
            NOTIFY_TARGET = (notify_hwnd.0 as isize, click_msg, hover_msg);
            let cls = to_wide(WIDGET_CLASS);
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(widget_wnd_proc),
                hInstance: inst,
                hCursor: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
                lpszClassName: PCWSTR(cls.as_ptr()),
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);
        }

        let mut w = Self {
            inst,
            hwnd: HWND::default(),
            width: 1,
            height: 32,
            light_theme,
            hovered: false,
            visible: false,
            last_pos: None,
            items: Vec::new(),
        };
        w.create_window();
        w
    }

    /// Create a top-level layered popup window anchored over the Windows 11 taskbar.
    /// Using WS_POPUP + WS_EX_TOPMOST avoids Windows 11's XAML taskbar host occluding WS_CHILD strips.
    fn create_window(&mut self) -> bool {
        self.destroy();
        unsafe {
            let cls = to_wide(WIDGET_CLASS);
            let empty = to_wide("");
            let Ok(wnd) = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                PCWSTR(cls.as_ptr()),
                PCWSTR(empty.as_ptr()),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                self.inst,
                None,
            ) else {
                return false;
            };
            self.hwnd = wnd;
            true
        }
    }

    pub fn destroy(&mut self) {
        unsafe {
            if !self.hwnd.0.is_null() {
                let _ = DestroyWindow(self.hwnd);
                self.hwnd = HWND::default();
            }
        }
    }

    pub fn set_light_theme(&mut self, light: bool) {
        if self.light_theme != light {
            self.light_theme = light;
            self.paint_and_place();
        }
    }

    pub fn set_hovered(&mut self, hovered: bool) {
        if self.hovered != hovered {
            self.hovered = hovered;
            self.paint_and_place();
        }
    }

    pub fn set_items(&mut self, items: Vec<StripItem>) {
        // Avoid redundant DIB reallocation and layered repaints when displayed values are unchanged.
        if self.items == items && (!items.is_empty() || !self.visible) {
            return;
        }
        self.items = items;
        self.paint_and_place();
    }

    pub fn ensure_alive_and_reposition(&mut self) {
        unsafe {
            if self.hwnd.0.is_null() || !IsWindow(self.hwnd).as_bool() {
                self.visible = false;
                self.last_pos = None;
                if self.create_window() {
                    self.paint_and_place();
                }
                return;
            }
            if self.items.is_empty() {
                if self.visible {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                    self.visible = false;
                }
                return;
            }
            if let Some((x, y, _)) = self.compute_taskbar_anchor() {
                let pos_changed = self.last_pos != Some((x, y));
                let taskbar_above_us = is_taskbar_above(self.hwnd);
                if pos_changed {
                    let _ = SetWindowPos(
                        self.hwnd,
                        HWND_TOPMOST,
                        x,
                        y,
                        self.width,
                        self.height,
                        SWP_NOACTIVATE,
                    );
                    self.last_pos = Some((x, y));
                } else if taskbar_above_us {
                    // Clicking the taskbar raises Shell_TrayWnd to top of the topmost band;
                    // re-raise our strip promptly without reallocating or moving.
                    let _ = SetWindowPos(
                        self.hwnd,
                        HWND_TOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                    );
                }
                if !self.visible {
                    let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                    self.visible = true;
                }
            } else if self.visible {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
                self.visible = false;
            }
        }
    }

    fn dpi(&self, tb: HWND) -> i32 {
        unsafe {
            let d = if !tb.0.is_null() {
                GetDpiForWindow(tb)
            } else {
                96
            };
            if d == 0 { 96 } else { d as i32 }
        }
    }

    fn scale(&self, px: i32, dpi: i32) -> i32 {
        (px * dpi + 48) / 96
    }

    /// Compute screen-space (x, y, bar_height) immediately to the left of TrayNotifyWnd.
    /// Returns None if the taskbar is absent, auto-hidden, or obscured by a fullscreen window.
    fn compute_taskbar_anchor(&self) -> Option<(i32, i32, i32)> {
        unsafe {
            let tb_cls = to_wide("Shell_TrayWnd");
            let tb = FindWindowW(PCWSTR(tb_cls.as_ptr()), PCWSTR::null()).ok()?;
            if tb.0.is_null() || !IsWindowVisible(tb).as_bool() {
                return None;
            }
            let notify_cls = to_wide("TrayNotifyWnd");
            let tray = FindWindowExW(tb, None, PCWSTR(notify_cls.as_ptr()), PCWSTR::null()).ok()?;
            if tray.0.is_null() {
                return None;
            }

            let mut bar = RECT::default();
            let mut tr = RECT::default();
            GetWindowRect(tb, &mut bar).ok()?;
            GetWindowRect(tray, &mut tr).ok()?;

            let bar_h = bar.bottom - bar.top;
            let bar_w = bar.right - bar.left;
            if bar_h < 24 || bar_w < 200 {
                return None;
            }

            // Hide when a fullscreen window (game/video) covers the same monitor as the taskbar.
            let mon = MonitorFromWindow(tb, MONITOR_DEFAULTTOPRIMARY);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(mon, &mut mi).as_bool() {
                let mr = mi.rcMonitor;
                // Auto-hidden taskbar sits off-edge
                if bar.top >= mr.bottom - 4 || bar.bottom <= mr.top + 4 {
                    return None;
                }
                let fg = GetForegroundWindow();
                if !fg.0.is_null()
                    && fg != tb
                    && fg != self.hwnd
                    && IsWindowVisible(fg).as_bool()
                    && !is_desktop_or_shell_window(fg)
                {
                    let mut fr = RECT::default();
                    if GetWindowRect(fg, &mut fr).is_ok()
                        && fr.left <= mr.left
                        && fr.top <= mr.top
                        && fr.right >= mr.right
                        && fr.bottom >= mr.bottom
                    {
                        return None;
                    }
                }
            }

            let mut anchor_left = tr.left;
            for cls_name in ["ShellFeedsTaskbarViewWindow", "DynamicContent2"] {
                let cw = to_wide(cls_name);
                if let Ok(feeds) = FindWindowExW(tb, None, PCWSTR(cw.as_ptr()), PCWSTR::null()) {
                    if !feeds.0.is_null() && IsWindowVisible(feeds).as_bool() {
                        let mut fr = RECT::default();
                        if GetWindowRect(feeds, &mut fr).is_ok()
                            && fr.right <= tr.left + 8
                            && fr.left > bar.left + bar_w / 2
                        {
                            anchor_left = anchor_left.min(fr.left);
                        }
                    }
                }
            }

            let dpi = self.dpi(tb);
            let margin = self.scale(8, dpi);
            let x = (anchor_left - self.width - margin).max(bar.left + self.scale(4, dpi));
            let y = bar.top + (bar_h - self.height) / 2;
            Some((x, y, bar_h))
        }
    }

    fn paint_and_place(&mut self) {
        unsafe {
            if self.hwnd.0.is_null() || !IsWindow(self.hwnd).as_bool() {
                if !self.create_window() {
                    return;
                }
            }
            if self.items.is_empty() {
                if self.visible {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                    self.visible = false;
                }
                return;
            }

            let tb_cls = to_wide("Shell_TrayWnd");
            let tb = FindWindowW(PCWSTR(tb_cls.as_ptr()), PCWSTR::null()).unwrap_or_default();
            let dpi = self.dpi(tb);

            let bar_h = self
                .compute_taskbar_anchor()
                .map(|(_, _, bh)| bh)
                .unwrap_or_else(|| self.scale(48, dpi));
            let h = (bar_h - self.scale(12, dpi)).clamp(self.scale(28, dpi), self.scale(36, dpi));

            // 4px scale spacing & two clean font weights (400 / 600)
            let icon_font = make_font("Segoe Fluent Icons", self.scale(13, dpi), FW_NORMAL.0 as i32);
            let chg_font = make_font("Segoe Fluent Icons", self.scale(10, dpi), FW_NORMAL.0 as i32);
            let label_font = make_font("Segoe UI", self.scale(12, dpi), FW_NORMAL.0 as i32);
            let pct_font = make_font("Segoe UI", self.scale(13, dpi), FW_SEMIBOLD.0 as i32);

            let pad_x = self.scale(8, dpi);
            let gap_inner = self.scale(4, dpi);
            let sep_pad = self.scale(8, dpi);
            let sep_w = self.scale(1, dpi).max(1);
            let icon_box = self.scale(14, dpi);
            let chg_w = self.scale(10, dpi);

            let screen_dc = GetDC(None);
            let measure_dc = CreateCompatibleDC(screen_dc);
            let mut cur_x = pad_x;
            let mut layouts = Vec::with_capacity(self.items.len());
            let mut sep_positions = Vec::new();

            for (idx, item) in self.items.iter().enumerate() {
                if idx > 0 {
                    cur_x += sep_pad;
                    sep_positions.push(cur_x);
                    cur_x += sep_w + sep_pad;
                }
                let item_start = cur_x;
                let glyph = item.kind.glyph_codepoint().to_string();
                let icon_x = cur_x;
                cur_x += icon_box + gap_inner;

                let lw = if item.short_label.is_empty() {
                    0
                } else {
                    measure_text(measure_dc, label_font, &item.short_label)
                };
                let label_x = cur_x;
                if lw > 0 {
                    cur_x += lw + gap_inner;
                }

                let pct_str = format!("{}%", item.percent);
                let pw = measure_text(measure_dc, pct_font, &pct_str);
                let pct_x = cur_x;
                cur_x += pw;

                let chg_x = if item.charging {
                    let cx = cur_x + self.scale(2, dpi);
                    cur_x = cx + chg_w;
                    Some(cx)
                } else {
                    None
                };

                layouts.push((item_start, cur_x, glyph, icon_x, label_x, lw, pct_str, pct_x, pw, chg_x));
            }
            let _ = DeleteDC(measure_dc);
            let _ = ReleaseDC(None, screen_dc);

            let w = (cur_x + pad_x).max(self.scale(48, dpi));
            self.width = w;
            self.height = h;

            let Some((mem_dc, bmp, bits)) = create_dib(w, h) else {
                let _ = DeleteObject(icon_font);
                let _ = DeleteObject(chg_font);
                let _ = DeleteObject(label_font);
                let _ = DeleteObject(pct_font);
                return;
            };
            let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);

            // Calm native neutral surface + subtle 8% hairline border (no glow, no gradient).
            let (bg_r, bg_g, bg_b, bg_a, border_a) = if self.light_theme {
                if self.hovered {
                    (236u8, 239u8, 244u8, 242u8, 34u8)
                } else {
                    (246u8, 247u8, 249u8, 234u8, 24u8)
                }
            } else if self.hovered {
                (42u8, 46u8, 52u8, 236u8, 36u8)
            } else {
                (30u8, 33u8, 38u8, 224u8, 24u8)
            };
            fill_rounded_rect(
                pixels,
                w,
                h,
                self.scale(6, dpi),
                bg_r,
                bg_g,
                bg_b,
                bg_a,
                border_a,
                self.light_theme,
            );

            // Hairline vertical dividers between devices (10% neutral ink).
            let (dr, dg, db, da) = if self.light_theme {
                (0u32, 0u32, 0u32, 26u32)
            } else {
                (255u32, 255u32, 255u32, 28u32)
            };
            let div_y0 = self.scale(7, dpi);
            let div_y1 = (h - self.scale(7, dpi)).max(div_y0 + 1);
            for &sx in &sep_positions {
                for y in div_y0..div_y1 {
                    for dx in 0..sep_w {
                        let px_x = sx + dx;
                        if px_x >= 0 && px_x < w {
                            let idx = (y * w + px_x) as usize;
                            pixels[idx] = blend_over_premul(pixels[idx], dr, dg, db, da);
                        }
                    }
                }
            }

            // Quiet muted role for icon & label; semantic accent reserved for percentage + 2px gauge bar.
            let (muted_r, muted_g, muted_b) = if self.light_theme {
                (82u8, 91u8, 102u8)
            } else {
                (176u8, 185u8, 198u8)
            };
            let text_y = -self.scale(1, dpi);
            let bar_h_px = self.scale(2, dpi).max(2);
            let bar_y = h - self.scale(4, dpi);

            for (item, (start_x, end_x, glyph, icon_x, label_x, lw, pct_str, pct_x, pw, chg_x)) in
                self.items.iter().zip(layouts.iter())
            {
                let (acc_r, acc_g, acc_b) = accent_color(item, self.light_theme);
                // Icon in calm muted ink (tinted only on critical/charging for instant scanability)
                let (ir, ig, ib) = if item.critical || item.charging {
                    (acc_r, acc_g, acc_b)
                } else {
                    (muted_r, muted_g, muted_b)
                };
                draw_text_alpha(pixels, w, h, icon_font, glyph, *icon_x, text_y, icon_box + 4, h, ir, ig, ib);

                if *lw > 0 {
                    draw_text_alpha(
                        pixels,
                        w,
                        h,
                        label_font,
                        &item.short_label,
                        *label_x,
                        text_y,
                        *lw + 4,
                        h,
                        muted_r,
                        muted_g,
                        muted_b,
                    );
                }

                draw_text_alpha(
                    pixels,
                    w,
                    h,
                    pct_font,
                    pct_str,
                    *pct_x,
                    text_y,
                    *pw + 4,
                    h,
                    acc_r,
                    acc_g,
                    acc_b,
                );

                if let Some(cx) = chg_x {
                    draw_text_alpha(
                        pixels,
                        w,
                        h,
                        chg_font,
                        GLYPH_CHARGING,
                        *cx,
                        text_y,
                        chg_w + 4,
                        h,
                        acc_r,
                        acc_g,
                        acc_b,
                    );
                }

                // Minimal 2px horizontal level bar under each item (track + solid accent fill).
                let track_w = (*end_x - *start_x).max(8);
                let fill_w = ((track_w as i32 * item.percent as i32) / 100).max(2);
                let (tr, tg, tb, ta) = if self.light_theme {
                    (0u32, 0u32, 0u32, 28u32)
                } else {
                    (255u32, 255u32, 255u32, 34u32)
                };
                for y in bar_y..(bar_y + bar_h_px).min(h) {
                    for x in *start_x..(*start_x + track_w).min(w) {
                        let idx = (y * w + x) as usize;
                        pixels[idx] = blend_over_premul(pixels[idx], tr, tg, tb, ta);
                    }
                    for x in *start_x..(*start_x + fill_w).min(w) {
                        let idx = (y * w + x) as usize;
                        pixels[idx] = blend_over_premul(
                            pixels[idx],
                            acc_r as u32,
                            acc_g as u32,
                            acc_b as u32,
                            235,
                        );
                    }
                }
            }

            let Some((dst_x, dst_y, _)) = self.compute_taskbar_anchor() else {
                let _ = DeleteObject(bmp);
                let _ = DeleteDC(mem_dc);
                let _ = DeleteObject(icon_font);
                let _ = DeleteObject(chg_font);
                let _ = DeleteObject(label_font);
                let _ = DeleteObject(pct_font);
                let _ = ShowWindow(self.hwnd, SW_HIDE);
                self.visible = false;
                return;
            };

            let dst_pt = POINT { x: dst_x, y: dst_y };
            let src_pt = POINT { x: 0, y: 0 };
            let sz = SIZE { cx: w, cy: h };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };

            let _ = UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&dst_pt),
                Some(&sz),
                mem_dc,
                Some(&src_pt),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );

            let _ = SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                dst_x,
                dst_y,
                w,
                h,
                SWP_NOACTIVATE,
            );
            self.last_pos = Some((dst_x, dst_y));
            if !self.visible {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                self.visible = true;
            }

            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem_dc);
            let _ = DeleteObject(icon_font);
            let _ = DeleteObject(chg_font);
            let _ = DeleteObject(label_font);
            let _ = DeleteObject(pct_font);
        }
    }
}

impl Drop for TaskbarWidget {
    fn drop(&mut self) {
        self.destroy();
    }
}

/// Exclude desktop background and Windows shell chrome from fullscreen occlusion detection
/// so clicking the desktop or tray overflow never hides the battery strip.
unsafe fn is_desktop_or_shell_window(hwnd: HWND) -> bool {
    let mut buf = [0u16; 64];
    let len = GetClassNameW(hwnd, &mut buf) as usize;
    if len == 0 {
        return false;
    }
    let cls = String::from_utf16_lossy(&buf[..len]);
    matches!(
        cls.as_str(),
        "Progman"
            | "WorkerW"
            | "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "NotifyIconOverflowWindow"
            | "TopLevelWindowForOverflowXamlIsland"
    )
}

/// Return true if Shell_TrayWnd currently sits higher in Z-order than `hwnd`.
unsafe fn is_taskbar_above(hwnd: HWND) -> bool {
    let tb_cls = to_wide("Shell_TrayWnd");
    let Ok(tb) = FindWindowW(PCWSTR(tb_cls.as_ptr()), PCWSTR::null()) else {
        return false;
    };
    if tb.0.is_null() {
        return false;
    }
    let mut cur = GetWindow(hwnd, GW_HWNDPREV).unwrap_or_default();
    let mut steps = 0;
    while !cur.0.is_null() && steps < 32 {
        if cur == tb {
            return true;
        }
        cur = GetWindow(cur, GW_HWNDPREV).unwrap_or_default();
        steps += 1;
    }
    false
}

fn accent_color(item: &StripItem, light_theme: bool) -> (u8, u8, u8) {
    if item.charging {
        return if light_theme { (3, 105, 161) } else { (96, 165, 250) };
    }
    if item.critical {
        return if light_theme { (200, 30, 30) } else { (248, 113, 113) };
    }
    if item.low {
        return if light_theme { (180, 83, 9) } else { (245, 176, 65) };
    }
    if light_theme { (24, 28, 34) } else { (243, 245, 248) }
}

unsafe fn make_font(face: &str, height_px: i32, weight: i32) -> HFONT {
    let mut name = [0u16; 32];
    for (i, ch) in face.encode_utf16().take(31).enumerate() {
        name[i] = ch;
    }
    CreateFontW(
        -height_px,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_TT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        ANTIALIASED_QUALITY.0 as u32,
        0,
        PCWSTR(name.as_ptr()),
    )
}

unsafe fn measure_text(hdc: HDC, font: HFONT, text: &str) -> i32 {
    let old = SelectObject(hdc, font);
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    let mut rc = RECT::default();
    DrawTextW(
        hdc,
        &mut wide,
        &mut rc,
        DT_CALCRECT | DT_SINGLELINE | DT_LEFT | DT_VCENTER,
    );
    let _ = SelectObject(hdc, old);
    (rc.right - rc.left).max(0)
}

unsafe fn create_dib(w: i32, h: i32) -> Option<(HDC, HBITMAP, *mut std::ffi::c_void)> {
    let screen_dc = GetDC(None);
    let mem_dc = CreateCompatibleDC(screen_dc);
    let _ = ReleaseDC(None, screen_dc);
    if mem_dc.is_invalid() {
        return None;
    }
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
    let Ok(bmp) = CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
        let _ = DeleteDC(mem_dc);
        return None;
    };
    if bits.is_null() {
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(mem_dc);
        return None;
    }
    SelectObject(mem_dc, bmp);
    std::ptr::write_bytes(bits, 0, (w * h * 4) as usize);
    Some((mem_dc, bmp, bits))
}

unsafe fn draw_text_alpha(
    dst: &mut [u32],
    dst_w: i32,
    dst_h: i32,
    font: HFONT,
    text: &str,
    x: i32,
    y: i32,
    box_w: i32,
    box_h: i32,
    r: u8,
    g: u8,
    b: u8,
) {
    let bw = box_w.max(1);
    let bh = box_h.max(1);
    let Some((dc, bmp, bits)) = create_dib(bw, bh) else {
        return;
    };
    let old_font = SelectObject(dc, font);
    SetBkMode(dc, TRANSPARENT);
    SetTextColor(dc, COLORREF(0x00FF_FFFF));
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: bw,
        bottom: bh,
    };
    DrawTextW(dc, &mut wide, &mut rc, DT_SINGLELINE | DT_LEFT | DT_VCENTER | DT_NOCLIP);
    let _ = SelectObject(dc, old_font);

    let mask = std::slice::from_raw_parts(bits as *const u32, (bw * bh) as usize);
    for my in 0..bh {
        let dy = y + my;
        if dy < 0 || dy >= dst_h {
            continue;
        }
        for mx in 0..bw {
            let dx = x + mx;
            if dx < 0 || dx >= dst_w {
                continue;
            }
            let cov = (mask[(my * bw + mx) as usize] & 0xFF) as u32;
            if cov == 0 {
                continue;
            }
            let idx = (dy * dst_w + dx) as usize;
            dst[idx] = blend_over_premul(dst[idx], r as u32, g as u32, b as u32, cov);
        }
    }

    let _ = DeleteObject(bmp);
    let _ = DeleteDC(dc);
}

#[inline]
fn blend_over_premul(dst: u32, sr: u32, sg: u32, sb: u32, sa: u32) -> u32 {
    let da = (dst >> 24) & 0xFF;
    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >> 8) & 0xFF;
    let db = dst & 0xFF;

    let pr = (sr * sa + 127) / 255;
    let pg = (sg * sa + 127) / 255;
    let pb = (sb * sa + 127) / 255;
    let inv = 255 - sa;

    let oa = sa + (da * inv + 127) / 255;
    let or = pr + (dr * inv + 127) / 255;
    let og = pg + (dg * inv + 127) / 255;
    let ob = pb + (db * inv + 127) / 255;

    (oa.min(255) << 24) | (or.min(255) << 16) | (og.min(255) << 8) | ob.min(255)
}

fn fill_rounded_rect(
    dst: &mut [u32],
    w: i32,
    h: i32,
    radius: i32,
    r: u8,
    g: u8,
    b: u8,
    alpha: u8,
    border_alpha: u8,
    light_theme: bool,
) {
    let rad = radius.min(w / 2).min(h / 2) as f32;
    let (br, bg, bb) = if light_theme { (0u32, 0u32, 0u32) } else { (255u32, 255u32, 255u32) };
    for y in 0..h {
        for x in 0..w {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let cx = if fx < rad {
                rad
            } else if fx > w as f32 - rad {
                w as f32 - rad
            } else {
                fx
            };
            let cy = if fy < rad {
                rad
            } else if fy > h as f32 - rad {
                h as f32 - rad
            } else {
                fy
            };
            let dist = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            if dist > rad {
                continue;
            }
            let edge_cov = (rad - dist).clamp(0.0, 1.0);
            let a = (alpha as f32 * edge_cov) as u32;
            let mut px = blend_over_premul(0, r as u32, g as u32, b as u32, a);
            if dist > rad - 1.1 || x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                let ba = (border_alpha as f32 * edge_cov) as u32;
                px = blend_over_premul(px, br, bg, bb, ba);
            }
            dst[(y * w + x) as usize] = px;
        }
    }
}
