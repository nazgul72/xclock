#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::OsStr;
use std::iter::once;
use std::os::windows::ffi::OsStrExt;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Mutex, OnceLock};

use chrono::Datelike;
use winapi::ctypes::c_int;
use winapi::shared::minwindef::{LPARAM, LRESULT, UINT, WPARAM};
use winapi::shared::windef::{HBRUSH, HWND, POINT, RECT};
use winapi::um::libloaderapi::GetModuleHandleW;
use winapi::um::sysinfoapi::GetTickCount;
use winapi::um::winuser::*;
use winapi::um::wingdi::*;

// Newtype wrapper for HWND to allow Send/Sync implementations
#[derive(Copy, Clone)]
struct SafeHwnd(HWND);

// SAFETY: HWND is a pointer type and is safe to send/sync between threads for our usage.
unsafe impl Send for SafeHwnd {}
unsafe impl Sync for SafeHwnd {}

// Global state using thread-safe primitives
static HOOK_HANDLE: AtomicPtr<winapi::shared::windef::HHOOK__> = AtomicPtr::new(ptr::null_mut());
static TOOLTIP_WINDOW: AtomicPtr<winapi::shared::windef::HWND__> = AtomicPtr::new(ptr::null_mut());
static CLOCK_WINDOWS: OnceLock<Mutex<Vec<SafeHwnd>>> = OnceLock::new();
static RUNNING: AtomicBool = AtomicBool::new(true);
static TOOLTIP_VISIBLE: AtomicBool = AtomicBool::new(false);
static LAST_MOUSE_POS: OnceLock<Mutex<POINT>> = OnceLock::new();

const TOOLTIP_CLASS_NAME: &str = "ClockHoverTooltip";

fn to_wide_string(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(once(0)).collect()
}

fn from_wide_string(wide: &[u16]) -> String {
    String::from_utf16_lossy(wide).trim_end_matches('\0').to_string()
}

unsafe fn get_window_class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    let len = GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
    if len > 0 {
        from_wide_string(&buffer[..len as usize])
    } else {
        "Unknown".to_string()
    }
}

// Generate tooltip text with uptime and Norwegian week number
fn generate_tooltip_text() -> String {
    // Get current time info
    let now = std::time::SystemTime::now();
    let boot_time = std::time::SystemTime::now() - std::time::Duration::from_millis(unsafe { GetTickCount() } as u64);
    let uptime = now.duration_since(boot_time).unwrap_or_default();
    
    // Calculate Norwegian week number using chrono
    let local_now = chrono::Local::now();
    
    // Norwegian week numbering follows ISO 8601:
    // Week 1 is the first week with at least 4 days in the new year
    // Week starts on Monday
    let week_number = local_now.iso_week().week();
    let year = local_now.iso_week().year();
    
    // Format uptime nicely
    let uptime_secs = uptime.as_secs();
    let days = uptime_secs / 86400;
    let hours = (uptime_secs % 86400) / 3600;
    let minutes = (uptime_secs % 3600) / 60;
    
    let uptime_text = if days > 0 {
        format!("{}d {}h {}m", days, hours, minutes)
    } else if hours > 0 {
        format!("{}h {}m", hours, minutes)
    } else {
        format!("{}m", minutes)
    };
    
    format!(
        "Uptime: {}\nWeek {}, {} (NO)",
        uptime_text,
        week_number,
        year
    )
}

// Find all potential clock windows
unsafe fn find_all_clock_windows() -> Vec<HWND> {
    let mut candidates = Vec::new();
    
    println!("Searching for Windows 11 clock control...");
    
    // Find the main taskbar
    let taskbar = FindWindowW(to_wide_string("Shell_TrayWnd").as_ptr(), ptr::null());
    if !taskbar.is_null() {
        println!("Found main taskbar: Shell_TrayWnd");
        
        // Find the notification area (TrayNotifyWnd) which contains the clock
        let notification_area = FindWindowExW(
            taskbar,
            ptr::null_mut(),
            to_wide_string("TrayNotifyWnd").as_ptr(),
            ptr::null(),
        );
        
        if !notification_area.is_null() {
            println!("Found notification area: TrayNotifyWnd -> {:?}", notification_area);
            
            // Search for actual clock controls
            let clock_classes = ["TrayClockWClass", "ClockWClass", "DigitalClockWClass"];
            
            for clock_class in &clock_classes {
                let clock = FindWindowExW(
                    notification_area,
                    ptr::null_mut(),
                    to_wide_string(clock_class).as_ptr(),
                    ptr::null(),
                );
                
                if !clock.is_null() {
                    println!("Found clock control: {} -> {:?}", clock_class, clock);
                    candidates.push(clock);
                }
            }
            
            // If no specific clock control found, use notification area as fallback
            if candidates.is_empty() {
                println!("No specific clock control found, using notification area");
                candidates.push(notification_area);
            }
        }
    }
    
    println!("Found {} clock controls", candidates.len());
    candidates
}

// Check if point is inside any clock control
unsafe fn is_point_in_any_clock(x: i32, y: i32) -> bool {
    if let Some(clock_windows) = CLOCK_WINDOWS.get() {
        if let Ok(windows) = clock_windows.lock() {
            for &clock_hwnd in windows.iter() {
                if clock_hwnd.0.is_null() {
                    continue;
                }
                
                let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                if GetWindowRect(clock_hwnd.0, &mut rect) != 0 {
                    let class_name = get_window_class_name(clock_hwnd.0);
                    
                    // If it's a specific clock control, use exact bounds
                    if class_name.contains("Clock") || class_name.contains("Time") {
                        if x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom {
                            return true;
                        }
                    }
                    // If it's TrayNotifyWnd (fallback), be more restrictive
                    else if class_name == "TrayNotifyWnd" {
                        // Only trigger in the right portion of TrayNotifyWnd (where clock usually is)
                        let width = rect.right - rect.left;
                        let clock_area_left = rect.left + (width * 2 / 3);  // Right third
                        
                        if x >= clock_area_left && x <= rect.right && 
                           y >= rect.top && y <= rect.bottom {
                            return true;
                        }
                    }
                }
            }
        }
    }
    
    false
}

// Hide any existing native tooltips in the area
unsafe fn hide_native_tooltips() {
    // Find and hide tooltip windows that might be showing
    let tooltip_classes = ["tooltips_class32", "SysTooltip32"];
    
    for tooltip_class in &tooltip_classes {
        let mut hwnd = FindWindowW(to_wide_string(tooltip_class).as_ptr(), ptr::null());
        while !hwnd.is_null() {
            if IsWindowVisible(hwnd) != 0 {
                // Check if this tooltip is for the clock area
                let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                if GetWindowRect(hwnd, &mut rect) != 0 {
                    // Hide tooltips that appear in the taskbar area
                    if rect.bottom > 1000 {  // Assuming taskbar is at bottom
                        ShowWindow(hwnd, SW_HIDE);
                    }
                }
            }
            
            hwnd = FindWindowExW(ptr::null_mut(), hwnd, to_wide_string(tooltip_class).as_ptr(), ptr::null());
        }
    }
}

// Show our custom tooltip
unsafe fn show_tooltip(x: i32, y: i32) {
    // Don't show multiple tooltips
    if TOOLTIP_VISIBLE.load(Ordering::SeqCst) {
        return;
    }
    
    // Hide any native tooltips first
    hide_native_tooltips();
    
    let current_tooltip = TOOLTIP_WINDOW.load(Ordering::SeqCst);
    if !current_tooltip.is_null() {
        return;
    }

    let class_name = to_wide_string(TOOLTIP_CLASS_NAME);
    let window_name = to_wide_string("Extended Clock Info");
    let tooltip_text = generate_tooltip_text();
    
    // Calculate tooltip size based on text
    let text_lines = tooltip_text.lines().count() as i32;
    let max_line_length = tooltip_text.lines().map(|l| l.len()).max().unwrap_or(0) as i32;
    
    let tooltip_width = std::cmp::max(250, max_line_length * 8);
    let tooltip_height = std::cmp::max(60, text_lines * 16 + 20);
    
    // Position tooltip near cursor but avoid screen edges
    let screen_width = GetSystemMetrics(SM_CXSCREEN);
    
    let mut tooltip_x = x + 15;
    let mut tooltip_y = y - tooltip_height - 10;
    
    // Adjust if tooltip would go off screen
    if tooltip_x + tooltip_width > screen_width {
        tooltip_x = x - tooltip_width - 15;
    }
    if tooltip_y < 0 {
        tooltip_y = y + 25;
    }
    
    // Create tooltip window
    let tooltip = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        class_name.as_ptr(),
        window_name.as_ptr(),
        WS_POPUP,
        tooltip_x,
        tooltip_y,
        tooltip_width,
        tooltip_height,
        ptr::null_mut(),
        ptr::null_mut(),
        GetModuleHandleW(ptr::null()),
        ptr::null_mut(),
    );

    if !tooltip.is_null() {
        TOOLTIP_WINDOW.store(tooltip, Ordering::SeqCst);
        TOOLTIP_VISIBLE.store(true, Ordering::SeqCst);
        ShowWindow(tooltip, SW_SHOW);
        UpdateWindow(tooltip);
        
        // Set a timer to hide the tooltip after some time
        SetTimer(tooltip, 1, 5000, None); // Hide after 5 seconds
    }
}

// Hide our custom tooltip
unsafe fn hide_tooltip() {
    let current_tooltip = TOOLTIP_WINDOW.load(Ordering::SeqCst);
    if !current_tooltip.is_null() {
        DestroyWindow(current_tooltip);
        TOOLTIP_WINDOW.store(ptr::null_mut(), Ordering::SeqCst);
        TOOLTIP_VISIBLE.store(false, Ordering::SeqCst);
    }
}

// Mouse hook procedure
unsafe extern "system" fn mouse_hook_proc(
    code: c_int,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 {
        match wparam as u32 {
            WM_MOUSEMOVE => {
                let mouse_struct = *(lparam as *const MOUSEHOOKSTRUCT);
                let x = mouse_struct.pt.x;
                let y = mouse_struct.pt.y;

                // Update last mouse position
                if let Some(last_pos) = LAST_MOUSE_POS.get() {
                    if let Ok(mut pos) = last_pos.lock() {
                        pos.x = x;
                        pos.y = y;
                    }
                }

                if is_point_in_any_clock(x, y) {
                    // Show tooltip after delay
                    if !TOOLTIP_VISIBLE.load(Ordering::SeqCst) {
                        // Small delay before showing tooltip (to mimic native behavior)
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        
                        // Check if mouse is still in the clock area
                        if is_point_in_any_clock(x, y) {
                            show_tooltip(x, y);
                        }
                    }
                } else {
                    // Hide tooltip when mouse leaves clock area
                    hide_tooltip();
                }
            }
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
                // Hide tooltip on any mouse click
                hide_tooltip();
            }
            _ => {}
        }
    }

    let hook = HOOK_HANDLE.load(Ordering::SeqCst);
    CallNextHookEx(hook, code, wparam, lparam)
}

// Tooltip window procedure
unsafe extern "system" fn tooltip_window_proc(
    hwnd: HWND,
    msg: UINT,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT {
                hdc: ptr::null_mut(),
                fErase: 0,
                rcPaint: RECT { left: 0, top: 0, right: 0, bottom: 0 },
                fRestore: 0,
                fIncUpdate: 0,
                rgbReserved: [0; 32],
            };
            
            let hdc = BeginPaint(hwnd, &mut ps);
            
            // Get window rect for drawing
            let mut window_rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetClientRect(hwnd, &mut window_rect);
            
            // Draw native-style border
            let border_pen = CreatePen(PS_SOLID as i32, 1, 0x808080);
            let old_pen = SelectObject(hdc, border_pen as *mut _);
            let old_brush = SelectObject(hdc, GetStockObject(NULL_BRUSH as i32));
            
            Rectangle(hdc, 0, 0, window_rect.right, window_rect.bottom);
            
            SelectObject(hdc, old_pen);
            SelectObject(hdc, old_brush);
            DeleteObject(border_pen as *mut _);
            
            // Draw text
            let text = generate_tooltip_text();
            let text_wide = to_wide_string(&text);
            let mut text_rect = RECT { 
                left: 8, 
                top: 8, 
                right: window_rect.right - 8, 
                bottom: window_rect.bottom - 8 
            };
            
            // Set text color and background
            SetTextColor(hdc, 0x000000); // Black text
            SetBkMode(hdc, TRANSPARENT as i32);
            
            DrawTextW(hdc, text_wide.as_ptr(), -1, &mut text_rect, DT_LEFT | DT_TOP | DT_WORDBREAK);
            EndPaint(hwnd, &ps);
            0
        }
        WM_TIMER => {
            // Hide tooltip when timer expires
            hide_tooltip();
            0
        }
        WM_DESTROY => 0,
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// Register tooltip window class
unsafe fn register_tooltip_class() -> bool {
    let class_name = to_wide_string(TOOLTIP_CLASS_NAME);
    
    let wc = WNDCLASSW {
        style: CS_DROPSHADOW,
        lpfnWndProc: Some(tooltip_window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: GetModuleHandleW(ptr::null()),
        hIcon: ptr::null_mut(),
        hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
        hbrBackground: (COLOR_INFOBK + 1) as HBRUSH,
        lpszMenuName: ptr::null(),
        lpszClassName: class_name.as_ptr(),
    };

    RegisterClassW(&wc) != 0
}

// Install mouse hook
unsafe fn install_hook() -> bool {
    let hook = SetWindowsHookExW(
        WH_MOUSE_LL,
        Some(mouse_hook_proc),
        GetModuleHandleW(ptr::null()),
        0,
    );

    if !hook.is_null() {
        HOOK_HANDLE.store(hook, Ordering::SeqCst);
        true
    } else {
        false
    }
}

// Remove mouse hook
unsafe fn remove_hook() {
    let hook = HOOK_HANDLE.load(Ordering::SeqCst);
    if !hook.is_null() {
        UnhookWindowsHookEx(hook);
        HOOK_HANDLE.store(ptr::null_mut(), Ordering::SeqCst);
    }
}

// Public API for the xclock library
pub fn start_clock_hook() -> Result<(), String> {
    unsafe {
        // Initialize last mouse position
        let _ = LAST_MOUSE_POS.set(Mutex::new(POINT { x: 0, y: 0 }));
        
        // Find all potential clock windows and store them
        let clock_windows = find_all_clock_windows();
        let _ = CLOCK_WINDOWS.set(Mutex::new(clock_windows.clone().into_iter().map(|hwnd| SafeHwnd(hwnd)).collect::<Vec<_>>()));
        
        if clock_windows.is_empty() {
            return Err("No clock windows found! Cannot install tooltip replacement.".to_string());
        } else {
            println!("Found {} potential clock windows", clock_windows.len());
        }

        if !register_tooltip_class() {
            return Err("Failed to register tooltip window class".to_string());
        }

        if !install_hook() {
            return Err("Failed to install mouse hook".to_string());
        }

        RUNNING.store(true, Ordering::SeqCst);
        println!("Tooltip replacement installed successfully. Native tooltips will be replaced.");
        println!("Hover over the system clock to see extended information!");
        Ok(())
    }
}

pub fn stop_clock_hook() {
    unsafe {
        hide_tooltip();
        remove_hook();
    }
    RUNNING.store(false, Ordering::SeqCst);
    println!("Clock tooltip replacement stopped.");
}

pub fn is_hook_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

pub fn process_messages() -> bool {
    unsafe {
        let mut msg = MSG {
            hwnd: ptr::null_mut(),
            message: 0,
            wParam: 0,
            lParam: 0,
            time: 0,
            pt: POINT { x: 0, y: 0 },
        };

        let result = PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE);
        
        if result != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
            
            if msg.message == WM_QUIT {
                return false;
            }
        }
        
        true
    }
}