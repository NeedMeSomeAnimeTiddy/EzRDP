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

// "Control admin windows too" (switched on by the person from the tray menu): EzRDP starts as
// administrator when they sign in, through a scheduled task, since the Run key can't start
// programs as administrator. Windows won't let a normal program click or type into a window
// that runs as administrator, so without this the phone can see such a window but not use it.

const TASK: &str = "EzRDP";
static ADMIN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn schtasks(args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("schtasks")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Whether EzRDP starts as administrator (as last checked).
pub fn admin() -> bool {
    ADMIN.load(std::sync::atomic::Ordering::Relaxed)
}

/// Looks for the sign-in task again.
pub fn refresh_admin() -> bool {
    let on = schtasks(&["/query", "/tn", TASK]);
    ADMIN.store(on, std::sync::atomic::Ordering::Relaxed);
    on
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Turns starting as administrator on or off. Only works when running as administrator.
/// Off goes back to the normal "Start with Windows".
pub fn set_admin(on: bool) -> bool {
    let ok = if on {
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
        let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
            (Ok(d), Ok(u)) => format!("{d}\\{u}"),
            (_, Ok(u)) => u,
            _ => return false,
        };
        // Never stopped (the default is after 3 days), and runs on battery too.
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Starts EzRDP as administrator when you sign in, so phones can control admin windows too. Turn off from the EzRDP tray menu.</Description></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author"><Exec><Command>{exe}</Command><Arguments>--background</Arguments></Exec></Actions>
</Task>
"#,
            exe = xml_escape(&exe),
            user = xml_escape(&user)
        );
        // schtasks wants the file in UTF-16.
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
        let path = std::env::temp_dir().join(format!("ezrdp-task-{}.xml", std::process::id()));
        let ok = std::fs::write(&path, bytes).is_ok()
            && schtasks(&["/create", "/tn", TASK, "/xml", &path.display().to_string(), "/f"]);
        let _ = std::fs::remove_file(&path);
        if ok {
            // The task starts it now; a Run entry as well would start a second, normal copy.
            set(false);
        }
        ok
    } else {
        let ok = schtasks(&["/delete", "/tn", TASK, "/f"]);
        if ok {
            set(true);
        }
        ok
    };
    refresh_admin();
    ok
}
