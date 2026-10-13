//! "Start with Windows": a value under HKCU\...\Run (no admin needed).

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::*;
use windows::core::{HSTRING, w};

const RUN: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

fn open() -> Option<HKEY> {
    let mut key = HKEY::default();
    let r = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN, None, KEY_READ | KEY_WRITE, &mut key) };
    (r == ERROR_SUCCESS).then_some(key)
}

pub fn enabled() -> bool {
    let Some(key) = open() else { return false };
    let r = unsafe { RegQueryValueExW(key, w!("EzRDP"), None, None, None, None) };
    unsafe {
        let _ = RegCloseKey(key);
    }
    r == ERROR_SUCCESS
}

pub fn set(on: bool) {
    let Some(key) = open() else { return };
    unsafe {
        if on {
            let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
            // Started with Windows: the tray only, no window.
            let cmd = HSTRING::from(format!("\"{exe}\" --background"));
            let bytes = std::slice::from_raw_parts(cmd.as_ptr() as *const u8, (cmd.len() + 1) * 2);
            let _ = RegSetValueExW(key, w!("EzRDP"), None, REG_SZ, Some(bytes));
        } else {
            let _ = RegDeleteValueW(key, w!("EzRDP"));
        }
        let _ = RegCloseKey(key);
    }
}
