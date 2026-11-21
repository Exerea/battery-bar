#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod drivers;
mod model;
mod startup;
mod ui;

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
};
#[cfg(not(debug_assertions))]
use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowW, GetMessageW, KillTimer,
    PostMessageW, PostQuitMessage, RegisterClassW, SetTimer, TranslateMessage, MSG, WM_APP,
    WM_COMMAND, WM_DESTROY, WM_LBUTTONUP, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER, WNDCLASSW,
    WINDOW_EX_STYLE, WINDOW_STYLE,
};

use crate::config::{AppConfig, DisplayMode};
use crate::model::{DeviceKind, DeviceReading};
use crate::ui::taskbar::{StripItem, TaskbarWidget};
use crate::ui::tray::{
    TrayIcon, DEVICE_CMD_STRIDE, ID_AUTOSTART, ID_DEVICE_BASE, ID_EXIT, ID_MODE_ALWAYS,
    ID_MODE_LOW_ONLY, ID_OPEN_CONFIG, ID_REFRESH_NOW, ID_SHOW_CHARGING, ID_THRESHOLD_BASE,
    THRESHOLDS,
};

const CONTROLLER_CLASS: &str = "BatteryBarHiddenController";

const WM_TRAY_CALLBACK: u32 = WM_APP + 1;
const WM_WIDGET_CLICK: u32 = WM_APP + 2;
const WM_POLL_DONE: u32 = WM_APP + 3;
const WM_WIDGET_HOVER: u32 = WM_APP + 4;
const WM_ACTIVATE_EXISTING: u32 = WM_APP + 5;

const TIMER_POLL: usize = 1;
const TIMER_REPOSITION: usize = 2;

struct AppState {
    hwnd: HWND,
    cfg: AppConfig,
    devices: Vec<DeviceReading>,
    widget: TaskbarWidget,
    tray: TrayIcon,
    poll_rx: Option<Receiver<Vec<DeviceReading>>>,
    polling: bool,
    startup_notified: bool,
}

thread_local! {
    static APP: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--list" || a == "--json") {
        run_cli(&args);
        return;
    }

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        // Single-instance guard: if already running, ask the existing instance to open its menu
        // instead of exiting silently, so double-clicking the executable always responds visibly.
        let mutex_name = to_wide("Local\\BatteryBarSingleInstance");
        let _mutex = CreateMutexW(None, false, PCWSTR(mutex_name.as_ptr()));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let cls = to_wide(CONTROLLER_CLASS);
            if let Ok(existing) = FindWindowW(PCWSTR(cls.as_ptr()), PCWSTR::null()) {
                if !existing.0.is_null() {
                    let _ = PostMessageW(existing, WM_ACTIVATE_EXISTING, WPARAM(0), LPARAM(0));
                }
            }
            return;
        }

        let hmod = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let inst = HINSTANCE(hmod.0);
        let cls = to_wide(CONTROLLER_CLASS);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(main_wnd_proc),
            hInstance: inst,
            lpszClassName: PCWSTR(cls.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);

        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(cls.as_ptr()),
            PCWSTR(cls.as_ptr()),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            inst,
            None,
        ) else {
            return;
        };

        let mut cfg = AppConfig::load();
        if args.iter().any(|a| a == "--always") {
            cfg.display_mode = DisplayMode::Always;
        }
        let light = startup::is_taskbar_light_theme();
        let widget = TaskbarWidget::new(inst, hwnd, WM_WIDGET_CLICK, WM_WIDGET_HOVER, light);
        let tray = TrayIcon::new(hwnd, WM_TRAY_CALLBACK);
        let poll_ms = (cfg.poll_interval_secs.max(10) * 1000) as u32;

        APP.with(|cell| {
            *cell.borrow_mut() = Some(AppState {
                hwnd,
                cfg,
                devices: Vec::new(),
                widget,
                tray,
                poll_rx: None,
                polling: false,
                startup_notified: false,
            });
        });

        trigger_async_poll();
        let _ = SetTimer(hwnd, TIMER_POLL, poll_ms, None);
        let _ = SetTimer(hwnd, TIMER_REPOSITION, 1000, None);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let _ = KillTimer(hwnd, TIMER_POLL);
        let _ = KillTimer(hwnd, TIMER_REPOSITION);
        APP.with(|cell| {
            *cell.borrow_mut() = None;
        });
    }
}

fn run_cli(args: &[String]) {
    #[cfg(not(debug_assertions))]
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
    let devices = drivers::poll_all();
    let cfg = AppConfig::load();
    if args.iter().any(|a| a == "--json") {
        if let Ok(s) = serde_json::to_string_pretty(&devices) {
            println!("{s}");
        }
        return;
    }

    println!(
        "BatteryBar detected {} device(s) (mode={:?}, threshold={}%):",
        devices.len(),
        cfg.display_mode,
        cfg.low_threshold
    );
    for d in &devices {
        let pct = d
            .percent
            .map(|p| format!("{p:>3}%"))
            .unwrap_or_else(|| " -- ".to_string());
        let chg = if d.charging { "CHG" } else { "   " };
        let conn = if d.connected { "ON " } else { "OFF" };
        let sel = if !cfg.selection_initialized || cfg.selected_keys.contains(&d.key) {
            "*"
        } else {
            " "
        };
        println!(
            " [{sel}] {pct} {chg} {conn} [{:<14}] {:<28} ({:?}) key={}",
            d.source, d.name, d.kind, d.key
        );
    }
}

fn trigger_async_poll() {
    APP.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(state) = guard.as_mut() else {
            return;
        };
        if state.polling {
            return;
        }
        state.polling = true;
        let (tx, rx) = mpsc::channel();
        state.poll_rx = Some(rx);
        let hwnd_raw = state.hwnd.0 as isize;
        thread::spawn(move || {
            let readings = drivers::poll_all();
            let _ = tx.send(readings);
            unsafe {
                let _ = PostMessageW(HWND(hwnd_raw as *mut _), WM_POLL_DONE, WPARAM(0), LPARAM(0));
            }
        });
    });
}

fn apply_ui_refresh(state: &mut AppState) {
    let visible: Vec<&DeviceReading> = state
        .devices
        .iter()
        .filter(|d| state.cfg.should_show_on_taskbar(d))
        .collect();

    let mut kind_counts: HashMap<DeviceKind, usize> = HashMap::new();
    for d in &visible {
        *kind_counts.entry(d.kind).or_insert(0) += 1;
    }

    let strip_items: Vec<StripItem> = visible
        .iter()
        .filter_map(|d| {
            let pct = d.percent?;
            let th = state.cfg.effective_threshold(&d.key);
            let short_label = if let Some(custom) = state.cfg.custom_labels.get(&d.key) {
                custom.clone()
            } else if kind_counts.get(&d.kind).copied().unwrap_or(0) > 1 {
                short_name_hint(&d.name)
            } else {
                String::new()
            };
            Some(StripItem {
                kind: d.kind,
                short_label,
                percent: pct,
                charging: d.charging,
                critical: pct <= 15,
                low: pct <= th,
            })
        })
        .collect();

    let any_low = state.devices.iter().any(|d| {
        state.cfg.selected_keys.contains(&d.key)
            && d.connected
            && !d.charging
            && d.percent
                .map(|p| p <= state.cfg.effective_threshold(&d.key))
                .unwrap_or(false)
    });

    let mut tip_parts = Vec::new();
    for d in &state.devices {
        if !state.cfg.selected_keys.contains(&d.key) {
            continue;
        }
        if let Some(p) = d.percent {
            let chg = if d.charging { " 充電中" } else { "" };
            tip_parts.push(format!("{}: {p}%{chg}", short_name_hint(&d.name)));
        }
    }
    let tip = if tip_parts.is_empty() {
        format!("BatteryBar（{}%以下で警告）", state.cfg.low_threshold)
    } else {
        tip_parts.join(" / ")
    };

    state.tray.add_or_update(&tip, any_low);
    state.widget.set_items(strip_items);

    if !state.startup_notified {
        state.startup_notified = true;
        let selected_count = state
            .devices
            .iter()
            .filter(|d| state.cfg.selected_keys.contains(&d.key))
            .count();
        let body = format!(
            "監視中 {} 件 / 警告中 {} 件（トレイアイコンから機器と閾値を変更）",
            selected_count,
            visible.len()
        );
        state.tray.notify_silent("BatteryBar を開始しました", &body);
    }
}

fn short_name_hint(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if lower.contains("mx keys") {
        return "MX".to_string();
    }
    if lower.contains("keychron") {
        return "KC".to_string();
    }
    if lower.contains("g535") {
        return "G535".to_string();
    }
    if lower.contains("pro") {
        return "PRO".to_string();
    }
    if lower.contains("iphone") {
        return "iPhone".to_string();
    }
    name.split_whitespace()
        .next()
        .unwrap_or(name)
        .chars()
        .take(6)
        .collect()
}

fn handle_command(cmd_id: usize) {
    let mut need_poll = false;
    APP.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(state) = guard.as_mut() else {
            return;
        };

        match cmd_id {
            ID_MODE_LOW_ONLY => {
                state.cfg.display_mode = DisplayMode::LowOnly;
                let _ = state.cfg.save();
                apply_ui_refresh(state);
            }
            ID_MODE_ALWAYS => {
                state.cfg.display_mode = DisplayMode::Always;
                let _ = state.cfg.save();
                apply_ui_refresh(state);
            }
            ID_SHOW_CHARGING => {
                state.cfg.show_while_charging = !state.cfg.show_while_charging;
                let _ = state.cfg.save();
                apply_ui_refresh(state);
            }
            ID_REFRESH_NOW => {
                need_poll = true;
            }
            ID_AUTOSTART => {
                let next = !startup::is_enabled();
                let _ = startup::set_enabled(next);
            }
            ID_OPEN_CONFIG => {
                let _ = state.cfg.save();
                let path = AppConfig::config_path();
                let _ = Command::new("notepad.exe").arg(path).spawn();
            }
            ID_EXIT => unsafe {
                PostQuitMessage(0);
            },
            id if id >= ID_THRESHOLD_BASE && id < ID_THRESHOLD_BASE + THRESHOLDS.len() => {
                state.cfg.low_threshold = THRESHOLDS[id - ID_THRESHOLD_BASE];
                let _ = state.cfg.save();
                apply_ui_refresh(state);
            }
            id if id >= ID_DEVICE_BASE => {
                let rel = id - ID_DEVICE_BASE;
                let idx = rel / DEVICE_CMD_STRIDE;
                let action = rel % DEVICE_CMD_STRIDE;
                if let Some(dev) = state.devices.get(idx) {
                    let key = dev.key.clone();
                    state.cfg.selection_initialized = true;
                    match action {
                        0 => {
                            if !state.cfg.selected_keys.remove(&key) {
                                state.cfg.selected_keys.insert(key);
                            }
                        }
                        1 => {
                            state.cfg.per_device_threshold.remove(&key);
                        }
                        a if a >= 2 && a - 2 < THRESHOLDS.len() => {
                            state.cfg.selected_keys.insert(key.clone());
                            state.cfg.per_device_threshold.insert(key, THRESHOLDS[a - 2]);
                        }
                        _ => {}
                    }
                    let _ = state.cfg.save();
                    apply_ui_refresh(state);
                }
            }
            _ => {}
        }
    });

    if need_poll {
        trigger_async_poll();
    }
}

unsafe extern "system" fn main_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TIMER => {
            match wparam.0 {
                TIMER_POLL => trigger_async_poll(),
                TIMER_REPOSITION => {
                    APP.with(|cell| {
                        if let Some(state) = cell.borrow_mut().as_mut() {
                            state.widget.ensure_alive_and_reposition();
                        }
                    });
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_POLL_DONE => {
            APP.with(|cell| {
                if let Some(state) = cell.borrow_mut().as_mut() {
                    state.polling = false;
                    if let Some(rx) = state.poll_rx.take() {
                        if let Ok(readings) = rx.try_recv() {
                            if state.cfg.seed_selection_if_needed(&readings) {
                                let _ = state.cfg.save();
                            }
                            state.devices = readings;
                            apply_ui_refresh(state);
                        }
                    }
                }
            });
            LRESULT(0)
        }
        WM_TRAY_CALLBACK => {
            let ev = lparam.0 as u32;
            if ev == WM_RBUTTONUP || ev == WM_LBUTTONUP {
                APP.with(|cell| {
                    if let Some(state) = cell.borrow().as_ref() {
                        state
                            .tray
                            .show_context_menu(&state.cfg, &state.devices, startup::is_enabled());
                    }
                });
            }
            LRESULT(0)
        }
        WM_WIDGET_CLICK | WM_ACTIVATE_EXISTING => {
            APP.with(|cell| {
                if let Some(state) = cell.borrow().as_ref() {
                    state
                        .tray
                        .show_context_menu(&state.cfg, &state.devices, startup::is_enabled());
                }
            });
            LRESULT(0)
        }
        WM_WIDGET_HOVER => {
            let hovered = wparam.0 != 0;
            APP.with(|cell| {
                if let Some(state) = cell.borrow_mut().as_mut() {
                    state.widget.set_hovered(hovered);
                }
            });
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            let light = startup::is_taskbar_light_theme();
            APP.with(|cell| {
                if let Some(state) = cell.borrow_mut().as_mut() {
                    state.widget.set_light_theme(light);
                }
            });
            LRESULT(0)
        }
        WM_COMMAND => {
            handle_command((wparam.0 & 0xFFFF) as usize);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
