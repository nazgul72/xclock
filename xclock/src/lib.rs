#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::OsStr;
use std::iter::once;
use std::os::windows::ffi::OsStrExt;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Mutex, OnceLock};

use winapi::ctypes::c_int;
use winapi::shared::minwindef::{LPARAM, LRESULT, UINT, WPARAM};
use winapi::shared::windef::{HBRUSH, HWND, POINT, RECT};
use winapi::um::libloaderapi::GetModuleHandleW;

use winapi::um::sysinfoapi::GetTickCount;
use winapi::um::winuser::*;

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

const TOOLTIP_CLASS_NAME: &str = "ClockHoverTooltip";

fn to_wide_string(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(once(0)).collect()
}

fn from_wide_string(wide: &[u16]) -> String {
    String::from_utf16_lossy(wide).trim_end_matches('\0').to_string()
}

unsafe fn get_window_class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    if len > 0 {
        from_wide_string(&buffer[..len as usize])
    } else {
        "Unknown".to_string()
    }
}

// Find the Windows 11 clock area (TrayNotifyWnd)
unsafe fn find_all_clock_windows() -> Vec<HWND> {
    let mut candidates = Vec::new();
    
    println!("Searching for Windows 11 clock area...");
    
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
            candidates.push(notification_area);
        } else {
            println!("WARNING: TrayNotifyWnd not found in taskbar");
        }
    } else {
        println!("WARNING: Shell_TrayWnd taskbar not found");
    }
    
    // Also check secondary taskbar for multi-monitor setups
    let secondary_taskbar = FindWindowW(to_wide_string("Shell_SecondaryTrayWnd").as_ptr(), ptr::null());
    if !secondary_taskbar.is_null() {
        println!("Found secondary taskbar: Shell_SecondaryTrayWnd");
        
        let secondary_notification = FindWindowExW(
            secondary_taskbar,
            ptr::null_mut(),
            to_wide_string("TrayNotifyWnd").as_ptr(),
            ptr::null(),
        );
        
        if !secondary_notification.is_null() {
            println!("Found secondary notification area: TrayNotifyWnd -> {:?}", secondary_notification);
            candidates.push(secondary_notification);
        }
    }
    
    println!("Found {} clock areas", candidates.len());
    candidates
}

// Check if point is inside the TrayNotifyWnd clock area
unsafe fn is_point_in_any_clock(x: i32, y: i32) -> bool {
    // Check known TrayNotifyWnd windows
    if let Some(clock_windows) = CLOCK_WINDOWS.get() {
        if let Ok(windows) = clock_windows.lock() {
            for &clock_hwnd in windows.iter() {
                if clock_hwnd.0.is_null() {
                    continue;
                }
                
                let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                if GetWindowRect(clock_hwnd.0, &mut rect) != 0 {
                    if x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom {
                        return true;
                    }
                }
            }
        }
    }
    
    // Fallback: Check if we're over TrayNotifyWnd directly
    let point = POINT { x, y };
    let hwnd = WindowFromPoint(point);
    
    if !hwnd.is_null() {
        let class_name = get_window_class_name(hwnd);
        
        // Only trigger for TrayNotifyWnd
        if class_name == "TrayNotifyWnd" {
            return true;
        }
    }
    
    false
}

unsafe fn show_tooltip(x: i32, y: i32) {
    let current_tooltip = TOOLTIP_WINDOW.load(Ordering::SeqCst);
    if !current_tooltip.is_null() {
        return;
    }

    let class_name = to_wide_string(TOOLTIP_CLASS_NAME);
    let window_name = to_wide_string("Extended Clock Info");
    
    let tooltip = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        class_name.as_ptr(),
        window_name.as_ptr(),
        WS_POPUP | WS_BORDER,
        x + 15,
        y - 120,
        280,
        100,
        ptr::null_mut(),
        ptr::null_mut(),
        GetModuleHandleW(ptr::null()),
        ptr::null_mut(),
    );

    if !tooltip.is_null() {
        TOOLTIP_WINDOW.store(tooltip, Ordering::SeqCst);
        ShowWindow(tooltip, SW_SHOW);
        UpdateWindow(tooltip);
    }
}

unsafe fn hide_tooltip() {
    let current_tooltip = TOOLTIP_WINDOW.load(Ordering::SeqCst);
    if !current_tooltip.is_null() {
        DestroyWindow(current_tooltip);
        TOOLTIP_WINDOW.store(ptr::null_mut(), Ordering::SeqCst);
    }
}

unsafe extern "system" fn mouse_hook_proc(
    code: c_int,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 && wparam as u32 == WM_MOUSEMOVE {
        let mouse_struct = *(lparam as *const MOUSEHOOKSTRUCT);
        let x = mouse_struct.pt.x;
        let y = mouse_struct.pt.y;

        if is_point_in_any_clock(x, y) {
            show_tooltip(x, y);
        } else {
            hide_tooltip();
        }
    }

    let hook = HOOK_HANDLE.load(Ordering::SeqCst);
    CallNextHookEx(hook, code, wparam, lparam)
}

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
            
            // Get current time info
            let now = std::time::SystemTime::now();
            let duration = now.duration_since(std::time::UNIX_EPOCH).unwrap();
            
            let text = format!(
                "Unix Time: {}\nUptime: {}s\nTick Count: {}",
                duration.as_secs(),
                duration.as_secs(),
                GetTickCount()
            );
            
            let text_wide = to_wide_string(&text);
            let mut rect = RECT { left: 10, top: 10, right: 270, bottom: 90 };
            
            DrawTextW(hdc, text_wide.as_ptr(), -1, &mut rect, DT_LEFT | DT_TOP | DT_WORDBREAK);
            EndPaint(hwnd, &ps);
            0
        }
        WM_DESTROY => 0,
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn register_tooltip_class() -> bool {
    let class_name = to_wide_string(TOOLTIP_CLASS_NAME);
    
    let wc = WNDCLASSW {
        style: 0,
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
        // Find all potential clock windows and store them
        let clock_windows = find_all_clock_windows();
        let _ = CLOCK_WINDOWS.set(Mutex::new(clock_windows.clone().into_iter().map(|hwnd| SafeHwnd(hwnd)).collect::<Vec<_>>()));
        
        if clock_windows.is_empty() {
            println!("WARNING: No clock windows found!");
            println!("The hook will still work but may not detect the clock area correctly.");
        } else {
            println!("Monitoring {} potential clock windows", clock_windows.len());
        }

        if !register_tooltip_class() {
            return Err("Failed to register tooltip window class".to_string());
        }

        if !install_hook() {
            return Err("Failed to install mouse hook".to_string());
        }

        RUNNING.store(true, Ordering::SeqCst);
        println!("Hook installed successfully. Hover over the system clock area...");
        Ok(())
    }
}

pub fn stop_clock_hook() {
    unsafe {
        hide_tooltip();
        remove_hook();
    }
    RUNNING.store(false, Ordering::SeqCst);
    println!("Clock hook stopped.");
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