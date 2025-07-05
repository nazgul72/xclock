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
use winapi::um::debugapi::OutputDebugStringA;
use winapi::um::errhandlingapi::GetLastError;
use std::ffi::CString;

// DLL attach/detach constants
const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_PROCESS_DETACH: u32 = 0;

// Global state for the hook
static HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);
static HOOK_HANDLE: AtomicPtr<HHOOK__> = AtomicPtr::new(ptr::null_mut());
static mut DLL_INSTANCE: HINSTANCE = ptr::null_mut();
static mut LAST_TOOLTIP_UPDATE: Option<Instant> = None;
const TOOLTIP_UPDATE_COOLDOWN: Duration = Duration::from_millis(500);

// Debug logging function
unsafe fn debug_log(msg: &str) {
    if let Ok(c_msg) = CString::new(format!("[XClock Hook] {}", msg)) {
        OutputDebugStringA(c_msg.as_ptr());
    }
}

// Debug log with formatting
unsafe fn debug_logf(msg: &str, args: &[&dyn std::fmt::Display]) {
    let formatted = args.iter().enumerate().fold(msg.to_string(), |acc, (i, arg)| {
        acc.replace(&format!("{{{}}}", i), &arg.to_string())
    });
    debug_log(&formatted);
}

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
        let name = utf16_to_string(&class_name[..len as usize]);
        if name == "tooltips_class32" {
            debug_logf("Found tooltip window class for HWND {0}: {1}", &[&(hwnd as usize), &name]);
        }
        name
    } else {
        debug_logf("Failed to get class name for HWND {0}", &[&(hwnd as usize)]);
        String::new()
    }
}

unsafe fn get_window_text(hwnd: HWND) -> String {
    let mut text = [0u16; 512];
    let len = GetWindowTextW(hwnd, text.as_mut_ptr(), text.len() as i32);
    if len > 0 {
        let window_text = utf16_to_string(&text[..len as usize]);
        debug_logf("Window text for HWND {0}: '{1}'", &[&(hwnd as usize), &window_text]);
        window_text
    } else {
        debug_logf("No window text for HWND {0}", &[&(hwnd as usize)]);
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
        let is_in_taskbar = rect.top > screen_height - 200;
        debug_logf("Tooltip position check - HWND {0}: rect({1},{2},{3},{4}), screen_height={5}, in_taskbar={6}", 
                  &[&(hwnd as usize), &rect.left, &rect.top, &rect.right, &rect.bottom, &screen_height, &is_in_taskbar]);
        is_in_taskbar
    } else {
        debug_logf("Failed to get window rect for HWND {0}", &[&(hwnd as usize)]);
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
    debug_logf("modify_tooltip_text called for HWND {0}", &[&(hwnd as usize)]);
    
    if !should_update_tooltip() {
        debug_log("Tooltip modification skipped due to cooldown");
        return;
    }

    let class_name = get_window_class_name(hwnd);
    if class_name != "tooltips_class32" {
        debug_logf("Skipping window - not tooltip class. Class: '{0}'", &[&class_name]);
        return;
    }
    debug_log("Confirmed tooltip class name");

    if !is_tooltip_in_taskbar_area(hwnd) {
        debug_log("Tooltip not in taskbar area - skipping");
        return;
    }
    debug_log("Tooltip is in taskbar area");

    let current_text = get_window_text(hwnd);
    
    // Only modify if it looks like a time/date tooltip
    let has_time_markers = current_text.contains(":") || 
                          current_text.contains("AM") || 
                          current_text.contains("PM") ||
                          current_text.contains("/") ||
                          current_text.chars().any(|c| c.is_ascii_digit());
    
    if current_text.is_empty() || !has_time_markers {
        debug_logf("Skipping tooltip - doesn't look like time/date. Text: '{0}'", &[&current_text]);
        return;
    }
    debug_logf("Confirmed time/date tooltip with text: '{0}'", &[&current_text]);

    let uptime = get_uptime();
    let week = get_norwegian_week();
    let new_text = format!("{}\nOpptid: {}\n{}", current_text, uptime, week);
    debug_logf("Generated new tooltip text: '{0}'", &[&new_text]);
    
    let new_text_utf16 = string_to_utf16(&new_text);

    let result = SetWindowTextW(hwnd, new_text_utf16.as_ptr());
    if result != 0 {
        debug_log("Successfully updated tooltip text");
        mark_tooltip_updated();
        
        // Force redraw
        InvalidateRect(hwnd, ptr::null(), 1);
        UpdateWindow(hwnd);
        debug_log("Tooltip redraw completed");
    } else {
        debug_logf("Failed to set window text for HWND {0}", &[&(hwnd as usize)]);
    }
}

// CBT hook procedure - this will be called in each process
unsafe extern "system" fn cbt_hook_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // Only log for window creation events to reduce noise
    if code == HCBT_CREATEWND {
        let hwnd = wparam as HWND;
        debug_logf("CBT Hook - Window created: HWND {0}", &[&(hwnd as usize)]);
        
        // Check if this is a tooltip window
        let class_name = get_window_class_name(hwnd);
        if class_name == "tooltips_class32" {
            debug_logf("Found tooltip window creation: HWND {0}", &[&(hwnd as usize)]);
            
            // Schedule tooltip modification after a short delay
            let hwnd_value = hwnd as usize;
            std::thread::spawn(move || {
                debug_logf("Starting delayed tooltip modification for HWND {0}", &[&hwnd_value]);
                std::thread::sleep(std::time::Duration::from_millis(100));
                
                unsafe {
                    let hwnd = hwnd_value as HWND;
                    if IsWindow(hwnd) != 0 {
                        debug_logf("Window still valid, proceeding with modification for HWND {0}", &[&hwnd_value]);
                        modify_tooltip_text(hwnd);
                    } else {
                        debug_logf("Window no longer valid for HWND {0}", &[&hwnd_value]);
                    }
                }
            });
        }
    } else if code >= 0 {
        // Log other hook codes at a lower frequency
        static mut HOOK_CALL_COUNT: u32 = 0;
        HOOK_CALL_COUNT += 1;
        if HOOK_CALL_COUNT % 100 == 0 {
            debug_logf("CBT Hook called 100 times, latest code: {0}", &[&code]);
        }
    }
    
    CallNextHookEx(ptr::null_mut(), code, wparam, lparam)
}

// Export functions for the main application to call
#[no_mangle]
pub unsafe extern "system" fn InstallHook() -> BOOL {
    debug_log("InstallHook called");
    
    if HOOK_INSTALLED.load(Ordering::SeqCst) {
        debug_log("Hook already installed");
        return 1; // Already installed
    }

    debug_logf("Installing CBT hook with DLL instance: {0}", &[&(DLL_INSTANCE as usize)]);
    let hook = SetWindowsHookExW(
        WH_CBT,
        Some(cbt_hook_proc),
        DLL_INSTANCE,  // Use the DLL instance instead of null
        0, // Global hook
    );
    
    if !hook.is_null() {
        HOOK_HANDLE.store(hook, Ordering::SeqCst);
        HOOK_INSTALLED.store(true, Ordering::SeqCst);
        debug_logf("Hook installed successfully with handle: {0}", &[&(hook as usize)]);
        1 // Success
    } else {
        let error = GetLastError();
        debug_logf("Failed to install hook, error code: {0}", &[&error]);
        0 // Failure
    }
}

#[no_mangle]
pub unsafe extern "system" fn UninstallHook() -> BOOL {
    debug_log("UninstallHook called");
    
    if !HOOK_INSTALLED.load(Ordering::SeqCst) {
        debug_log("Hook not installed");
        return 1; // Not installed
    }

    let hook = HOOK_HANDLE.load(Ordering::SeqCst);
    if !hook.is_null() {
        debug_logf("Attempting to uninstall hook with handle: {0}", &[&(hook as usize)]);
        if UnhookWindowsHookEx(hook) != 0 {
            HOOK_HANDLE.store(ptr::null_mut(), Ordering::SeqCst);
            HOOK_INSTALLED.store(false, Ordering::SeqCst);
            debug_log("Hook uninstalled successfully");
            1 // Success
        } else {
            let error = GetLastError();
            debug_logf("Failed to uninstall hook, error code: {0}", &[&error]);
            0 // Failed to unhook
        }
    } else {
        debug_log("No hook handle to remove");
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
            debug_logf("DLL attached to process, instance: {0}", &[&(hinst_dll as usize)]);
            1
        }
        DLL_PROCESS_DETACH => {
            // Cleanup when DLL is unloaded from a process
            debug_log("DLL detaching from process");
            1
        }
        _ => {
            debug_logf("DLL entry point called with reason: {0}", &[&fdw_reason]);
            1
        }
    }
}
