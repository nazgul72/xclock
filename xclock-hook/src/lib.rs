#![allow(unsafe_op_in_unsafe_fn)]

use chrono::Datelike;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::time::{Duration, Instant};
use winapi::shared::minwindef::{BOOL, DWORD, HINSTANCE, LPARAM, LRESULT, WPARAM};
use winapi::shared::windef::{HWND, RECT, HHOOK__};
use winapi::um::sysinfoapi::GetTickCount;
use winapi::um::winuser::*;

// DLL attach/detach constants
const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_PROCESS_DETACH: u32 = 0;

// Global state for the hook
static HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);
static HOOK_HANDLE: AtomicPtr<HHOOK__> = AtomicPtr::new(ptr::null_mut());
static mut DLL_INSTANCE: HINSTANCE = ptr::null_mut();
static mut LAST_TOOLTIP_UPDATE: Option<Instant> = None;
const TOOLTIP_UPDATE_COOLDOWN: Duration = Duration::from_millis(500);

fn utf16_to_string(utf16: &[u16]) -> String {
    OsString::from_wide(utf16)
        .to_string_lossy()
        .trim_end_matches('\0')
        .to_string()
}

fn string_to_utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn get_window_class_name(hwnd: HWND) -> String {
    let mut class_name = [0u16; 256];
    let len = GetClassNameW(hwnd, class_name.as_mut_ptr(), class_name.len() as i32);
    if len > 0 {
        utf16_to_string(&class_name[..len as usize])
    } else {
        String::new()
    }
}

unsafe fn get_window_text(hwnd: HWND) -> String {
    let mut text = [0u16; 512];
    let len = GetWindowTextW(hwnd, text.as_mut_ptr(), text.len() as i32);
    if len > 0 {
        utf16_to_string(&text[..len as usize])
    } else {
        String::new()
    }
}

unsafe fn is_tooltip_in_taskbar_area(hwnd: HWND) -> bool {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    
    if GetWindowRect(hwnd, &mut rect) != 0 {
        let screen_height = GetSystemMetrics(SM_CYSCREEN);
        rect.top > screen_height - 200 // Taskbar area
    } else {
        false
    }
}

unsafe fn should_update_tooltip() -> bool {
    if let Some(last_update) = LAST_TOOLTIP_UPDATE {
        if last_update.elapsed() < TOOLTIP_UPDATE_COOLDOWN {
            return false;
        }
    }
    true
}

unsafe fn mark_tooltip_updated() {
    LAST_TOOLTIP_UPDATE = Some(Instant::now());
}

fn get_uptime() -> String {
    unsafe {
        let tick_count = GetTickCount();
        let uptime_seconds = tick_count / 1000;
        let days = uptime_seconds / (24 * 3600);
        let hours = (uptime_seconds % (24 * 3600)) / 3600;
        let minutes = (uptime_seconds % 3600) / 60;

        if days > 0 {
            format!("{}d {}h {}m", days, hours, minutes)
        } else if hours > 0 {
            format!("{}h {}m", hours, minutes)
        } else {
            format!("{}m", minutes)
        }
    }
}

fn get_norwegian_week() -> String {
    let now = chrono::Local::now();
    let naive_date = now.date_naive();
    let iso_week = naive_date.iso_week();
    format!("Uke {}", iso_week.week())
}

unsafe fn modify_tooltip_text(hwnd: HWND) {
    if !should_update_tooltip() {
        return;
    }

    let class_name = get_window_class_name(hwnd);
    if class_name != "tooltips_class32" {
        return;
    }

    if !is_tooltip_in_taskbar_area(hwnd) {
        return;
    }

    let current_text = get_window_text(hwnd);
    
    // Only modify if it looks like a time/date tooltip
    if current_text.is_empty() || 
       (!current_text.contains(":") && 
        !current_text.contains("AM") && 
        !current_text.contains("PM") &&
        !current_text.contains("/") &&
        !current_text.chars().any(|c| c.is_ascii_digit())) {
        return;
    }

    let uptime = get_uptime();
    let week = get_norwegian_week();
    let new_text = format!("{}\nOpptid: {}\n{}", current_text, uptime, week);
    let new_text_utf16 = string_to_utf16(&new_text);

    if SetWindowTextW(hwnd, new_text_utf16.as_ptr()) != 0 {
        mark_tooltip_updated();
        
        // Force redraw
        InvalidateRect(hwnd, ptr::null(), 1);
        UpdateWindow(hwnd);
    }
}

// CBT hook procedure - this will be called in each process
unsafe extern "system" fn cbt_hook_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code == HCBT_CREATEWND {
        let hwnd = wparam as HWND;
        // Check if this is a tooltip window
        let class_name = get_window_class_name(hwnd);
        if class_name == "tooltips_class32" {
            // Schedule tooltip modification after a short delay
            let hwnd_value = hwnd as usize;
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(100));
                
                unsafe {
                    let hwnd = hwnd_value as HWND;
                    //if IsWindow(hwnd) != 0 && IsWindowVisible(hwnd) != 0 
                    {
                        modify_tooltip_text(hwnd);
                    }
                }
            });
        }
    }
    
    CallNextHookEx(ptr::null_mut(), code, wparam, lparam)
}

// Export functions for the main application to call
#[no_mangle]
pub unsafe extern "system" fn InstallHook() -> BOOL {
    if HOOK_INSTALLED.load(Ordering::SeqCst) {
        return 1; // Already installed
    }

    let hook = SetWindowsHookExW(
        WH_CBT,
        Some(cbt_hook_proc),
        DLL_INSTANCE,  // Use the DLL instance instead of null
        0, // Global hook
    );
    
    if !hook.is_null() {
        HOOK_HANDLE.store(hook, Ordering::SeqCst);
        HOOK_INSTALLED.store(true, Ordering::SeqCst);
        1 // Success
    } else {
        0 // Failure
    }
}

#[no_mangle]
pub unsafe extern "system" fn UninstallHook() -> BOOL {
    if !HOOK_INSTALLED.load(Ordering::SeqCst) {
        return 1; // Not installed
    }

    let hook = HOOK_HANDLE.load(Ordering::SeqCst);
    if !hook.is_null() {
        if UnhookWindowsHookEx(hook) != 0 {
            HOOK_HANDLE.store(ptr::null_mut(), Ordering::SeqCst);
            HOOK_INSTALLED.store(false, Ordering::SeqCst);
            1 // Success
        } else {
            0 // Failed to unhook
        }
    } else {
        HOOK_INSTALLED.store(false, Ordering::SeqCst);
        1 // No hook to remove
    }
}

// DLL entry point
#[no_mangle]
pub unsafe extern "system" fn DllMain(
    hinst_dll: HINSTANCE,
    fdw_reason: DWORD,
    _lpv_reserved: *mut std::ffi::c_void,
) -> BOOL {
    match fdw_reason {
        DLL_PROCESS_ATTACH => {
            // Store the DLL instance for the hook
            DLL_INSTANCE = hinst_dll;
            1
        }
        DLL_PROCESS_DETACH => {
            // Cleanup when DLL is unloaded from a process
            1
        }
        _ => 1,
    }
}
