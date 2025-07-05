#![allow(unsafe_op_in_unsafe_fn)]

use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use winapi::shared::minwindef::{BOOL, HMODULE};
use winapi::um::libloaderapi::{FreeLibrary, GetProcAddress, LoadLibraryW};
use winapi::um::winuser::*;

// Global variables for thread communication
static RUNNING: AtomicBool = AtomicBool::new(false);
static mut HOOK_DLL: HMODULE = ptr::null_mut();

// Function pointers for DLL functions
type InstallHookFn = unsafe extern "system" fn() -> BOOL;
type UninstallHookFn = unsafe extern "system" fn() -> BOOL;

fn to_wide_string(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn load_hook_dll() -> Result<(), Box<dyn std::error::Error>> {
    if !HOOK_DLL.is_null() {
        return Ok(()); // Already loaded
    }

    let dll_name = to_wide_string("xclock_hook.dll");
    HOOK_DLL = LoadLibraryW(dll_name.as_ptr());
    
    if HOOK_DLL.is_null() {
        return Err("Failed to load xclock_hook.dll".into());
    }

    Ok(())
}

unsafe fn unload_hook_dll() {
    if !HOOK_DLL.is_null() {
        FreeLibrary(HOOK_DLL);
        HOOK_DLL = ptr::null_mut();
    }
}

unsafe fn call_dll_function<T>(func_name: &str) -> Result<T, Box<dyn std::error::Error>> 
where
    T: Copy,
{
    if HOOK_DLL.is_null() {
        return Err("DLL not loaded".into());
    }

    let func_name_cstr = std::ffi::CString::new(func_name)?;
    let func_ptr = GetProcAddress(HOOK_DLL, func_name_cstr.as_ptr());
    
    if func_ptr.is_null() {
        return Err(format!("Function {} not found in DLL", func_name).into());
    }

    match func_name {
        "InstallHook" => {
            let install_hook: InstallHookFn = std::mem::transmute(func_ptr);
            let result = install_hook();
            Ok(*((&result) as *const BOOL as *const T))
        }
        "UninstallHook" => {
            let uninstall_hook: UninstallHookFn = std::mem::transmute(func_ptr);
            let result = uninstall_hook();
            Ok(*((&result) as *const BOOL as *const T))
        }
        _ => Err(format!("Unknown function: {}", func_name).into()),
    }
}

pub fn start_monitoring() -> Result<(), Box<dyn std::error::Error>> {
    if RUNNING.load(Ordering::SeqCst) {
        return Err("Monitoring is already running".into());
    }

    unsafe {
        load_hook_dll()?;
        
        let result: BOOL = call_dll_function("InstallHook")?;
        if result == 0 {
            return Err("Failed to install hook in DLL".into());
        }

        RUNNING.store(true, Ordering::SeqCst);
        println!("Global hook installed via DLL - monitoring tooltip creation across all processes");
    }

    Ok(())
}

pub fn stop_monitoring() {
    RUNNING.store(false, Ordering::SeqCst);
    
    unsafe {
        if !HOOK_DLL.is_null() {
            let _result: Result<BOOL, _> = call_dll_function("UninstallHook");
            unload_hook_dll();
            println!("Hook removed and DLL unloaded");
        }
    }
}

pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

pub fn message_loop() -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let mut msg = std::mem::zeroed();
        while RUNNING.load(Ordering::SeqCst) {
            let result = GetMessageW(&mut msg, ptr::null_mut(), 0, 0);
            if result == -1 {
                return Err("GetMessage failed".into());
            }
            if result == 0 {
                break; // WM_QUIT
            }
            
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
