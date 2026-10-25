//! Updates EzRDP from the newest build on GitHub, then restarts it. A connected phone just
//! reconnects by itself a few seconds later.
//!
//! The new program is downloaded next to this one and checked against the sha256 in the
//! release's latest.json. Then this program is renamed to `.old` (Windows allows that while it
//! runs), the new one takes its name and is started. It takes over like any newer copy does. On
//! Windows, if it doesn't take over within 40 seconds, this copy puts the old file back and
//! keeps running, so a broken download can't leave the PC without EzRDP.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Duration;

/// This program's build number, given by GitHub's build (0 for a build made by hand).
pub const BUILD: u32 = parse_build(option_env!("EZRDP_BUILD"));

const fn parse_build(s: Option<&str>) -> u32 {
    let Some(s) = s else { return 0 };
    let b = s.as_bytes();
    let mut n: u32 = 0;
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() {
            return 0;
        }
        n = n * 10 + (b[i] - b'0') as u32;
        i += 1;
    }
    n
}

const DOWNLOADS: &str = "https://github.com/NeedMeSomeAnimeTiddy/EzRDP/releases/latest/download/";

/// Where updates come from (EZRDP_UPDATE_URL can point elsewhere, for testing).
fn downloads() -> String {
    std::env::var("EZRDP_UPDATE_URL").unwrap_or_else(|_| DOWNLOADS.into())
}

#[cfg(windows)]
const PLATFORM: &str = "windows";
#[cfg(not(windows))]
const PLATFORM: &str = "linux";

/// Update by itself when one is out ("auto_update" in ezrdp.conf).
pub static AUTO: AtomicBool = AtomicBool::new(true);
/// An update is being installed right now.
static BUSY: AtomicBool = AtomicBool::new(false);

pub struct Latest {
    pub build: u32,
    file: String,
    sha256: String,
}

/// curl ships with Windows 10 and later and with nearly every Linux.
fn curl() -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let mut c = Command::new(Path::new(&root).join(r"System32\curl.exe"));
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flashing up
        c
    }
    #[cfg(not(windows))]
    Command::new("curl")
}

/// The text of `"key": "value"` (or a number) inside `json`.
fn field<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let at = json.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = json[at..].trim_start().strip_prefix(':')?.trim_start();
    if let Some(s) = rest.strip_prefix('"') {
        s.split('"').next()
    } else {
        rest.split(|c: char| !c.is_ascii_digit()).next()
    }
}

/// Reads latest.json: the newest build and this platform's file in it.
fn parse_latest(json: &str) -> Option<Latest> {
    let build = field(json, "build")?.parse().ok()?;
    let section = &json[json.find(&format!("\"{PLATFORM}\""))?..];
    let section = &section[..section.find('}')?];
    Some(Latest { build, file: field(section, "file")?.to_string(), sha256: field(section, "sha256")?.to_lowercase() })
}

/// Asks GitHub for the newest build. `None` = this one is the newest.
pub fn check() -> Result<Option<Latest>, String> {
    let out = curl()
        .args(["-fsSL", "--max-time", "30", &format!("{}latest.json", downloads())])
        .output()
        .map_err(|e| format!("couldn't run curl ({e})"))?;
    if !out.status.success() {
        return Err("couldn't reach GitHub".into());
    }
    let latest = parse_latest(&String::from_utf8_lossy(&out.stdout)).ok_or("the update list looks wrong")?;
    Ok((latest.build > BUILD).then_some(latest))
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn sha256_hex(path: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let data = std::fs::read(path).ok()?;
    Some(crate::trust::hex(&Sha256::digest(&data)))
}

/// Renames, retrying for a moment: antivirus often holds a new file briefly.
fn rename(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Err(_) if tries < 15 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(200));
            }
            r => return r,
        }
    }
}

/// Downloads `latest`, swaps it in and starts it. On success the new copy takes over and this
/// one is asked to quit (Linux: quits right away). Errors are in plain words for the person.
pub fn install(latest: &Latest) -> Result<(), String> {
    if BUSY.swap(true, Relaxed) {
        return Err("an update is already being installed".into());
    }
    let r = install_inner(latest);
    if r.is_err() {
        BUSY.store(false, Relaxed);
    }
    r
}

fn install_inner(latest: &Latest) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let new = with_suffix(&exe, ".new");
    let old = with_suffix(&exe, ".old");
    crate::log!("Updating to build {} (this is build {BUILD})", latest.build);

    let ok = curl()
        .args(["-fsSL", "--max-time", "600", "-o"])
        .arg(&new)
        .arg(format!("{}{}", downloads(), latest.file))
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        let _ = std::fs::remove_file(&new);
        let folder = exe.parent().map(|p| p.display().to_string()).unwrap_or_default();
        return Err(if std::fs::write(&new, b"").is_err() {
            format!("EzRDP isn't allowed to change files in {folder}. Move EzRDP to a folder of your own.")
        } else {
            let _ = std::fs::remove_file(&new);
            "the download didn't finish. Check the internet connection.".into()
        });
    }
    if sha256_hex(&new).as_deref() != Some(latest.sha256.as_str()) {
        let _ = std::fs::remove_file(&new);
        return Err("the download was damaged. Try again.".into());
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755));
    }

    let _ = std::fs::remove_file(&old);
    rename(&exe, &old).map_err(|e| format!("couldn't swap the program file ({e})"))?;
    if let Err(e) = rename(&new, &exe) {
        let _ = rename(&old, &exe);
        return Err(format!("couldn't swap the program file ({e})"));
    }

    // Started from this program, the new copy runs the same way (as administrator if this one is).
    let child = Command::new(&exe).args(["--updated", "--background"]).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            put_back(&exe, &old);
            return Err(format!("the new version didn't start ({e})"));
        }
    };

    #[cfg(not(windows))]
    {
        // The new copy asks a running EzRDP to quit; if that's this one, it goes now.
        let _ = &mut child;
        crate::log!("Restarting as build {}", latest.build);
        std::process::exit(0);
    }
    #[cfg(windows)]
    {
        // The new copy asks this one to quit as soon as it's up. If that doesn't happen, it failed.
        let (exe, old) = (exe.clone(), old.clone());
        std::thread::spawn(move || {
            for _ in 0..40 {
                std::thread::sleep(Duration::from_secs(1));
                if let Ok(Some(_)) = child.try_wait() {
                    break; // it quit or crashed
                }
            }
            let _ = child.kill();
            put_back(&exe, &old);
            BUSY.store(false, Relaxed);
            crate::tray::notify("Update didn't work", "The new version didn't start, so EzRDP stays on this one.");
        });
        Ok(())
    }
}

/// Undoes the swap: the new file becomes `.bad`, the old one gets its name back.
fn put_back(exe: &Path, old: &Path) {
    crate::log!("The new version didn't start: going back to build {BUILD}");
    let bad = with_suffix(exe, ".bad");
    let _ = std::fs::remove_file(&bad);
    let _ = rename(exe, &bad);
    let _ = rename(old, exe);
}

/// Called by a copy started with `--updated` once it's running: the old file is no longer needed.
pub fn finished() {
    crate::log!("Updated: now build {BUILD}");
    std::thread::spawn(|| {
        // Not straight away: on Windows the old copy may still be closing.
        std::thread::sleep(Duration::from_secs(20));
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::fs::remove_file(with_suffix(&exe, ".old"));
            let _ = std::fs::remove_file(with_suffix(&exe, ".bad"));
        }
    });
}

/// Checks now and then and installs updates by itself, but never while a phone or PC is
/// connected: then it waits until they've gone, so nobody's work is cut off.
pub fn run_auto(app: std::sync::Arc<crate::state::App>, tell: fn(&str, &str)) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(90));
        let mut told = 0;
        loop {
            let mut wait = Duration::from_secs(6 * 3600);
            if AUTO.load(Relaxed)
                && let Ok(Some(latest)) = check()
            {
                if app.connected.lock().unwrap().is_none() {
                    if let Err(e) = install(&latest) {
                        crate::log!("Automatic update failed: {e}");
                    }
                } else {
                    if told != latest.build {
                        told = latest.build;
                        let text = "An EzRDP update for the PC is ready. It installs by itself after you disconnect.";
                        crate::session::notice(text);
                        tell("Update ready", text);
                    }
                    wait = Duration::from_secs(60);
                }
            }
            std::thread::sleep(wait);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_update_list() {
        let json = r#"{
  "build": 1534567,
  "windows": { "file": "ezrdp-server.exe", "sha256": "AB12" },
  "linux": { "file": "ezrdp-linux", "sha256": "cd34" },
  "android": { "file": "EzRDP.apk", "sha256": "ef56" }
}"#;
        let l = parse_latest(json).unwrap();
        assert_eq!(l.build, 1534567);
        #[cfg(windows)]
        assert_eq!((l.file.as_str(), l.sha256.as_str()), ("ezrdp-server.exe", "ab12"));
        #[cfg(not(windows))]
        assert_eq!((l.file.as_str(), l.sha256.as_str()), ("ezrdp-linux", "cd34"));
    }

    #[test]
    fn build_numbers() {
        assert_eq!(parse_build(Some("1534567")), 1534567);
        assert_eq!(parse_build(Some("x1")), 0);
        assert_eq!(parse_build(None), 0);
    }
}
