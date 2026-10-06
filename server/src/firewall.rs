//! Makes sure Windows Firewall lets the phone in. Windows often marks home networks as
//! "Public", where new apps are blocked by default, which shows up as a connection timeout.

use std::os::windows::process::CommandExt;
use std::process::Command;

const RULE: &str = "EzRDP";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn netsh(args: &[&str]) -> Option<(bool, String)> {
    let out = Command::new("netsh").args(args).creation_flags(CREATE_NO_WINDOW).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    Some((out.status.success(), text))
}

pub enum Status {
    Ok,
    /// Adding the rule needs administrator rights.
    NeedsAdmin,
    Failed(String),
}

/// Adds an allow rule for this exe (TCP for connections, UDP for being found on Wi-Fi;
/// all network types), replacing any stale one.
pub fn allow() -> Status {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    // Already set up for this exact exe?
    if let Some((true, text)) = netsh(&["advfirewall", "firewall", "show", "rule", &format!("name={RULE}"), "verbose"])
        && text.to_lowercase().contains(&exe.to_lowercase())
    {
        return Status::Ok;
    }
    let _ = netsh(&["advfirewall", "firewall", "delete", "rule", &format!("name={RULE}")]);
    match netsh(&[
        "advfirewall",
        "firewall",
        "add",
        "rule",
        &format!("name={RULE}"),
        "dir=in",
        "action=allow",
        &format!("program={exe}"),
        "profile=any",
        "enable=yes",
    ]) {
        Some((true, _)) => Status::Ok,
        Some((false, text)) if text.to_lowercase().contains("elevation") || text.contains("administrator") => {
            Status::NeedsAdmin
        }
        Some((false, text)) => Status::Failed(text.trim().to_string()),
        None => Status::Failed("netsh not found".into()),
    }
}
