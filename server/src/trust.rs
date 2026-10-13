//! The list of phones this PC trusts (stored next to the exe in `trusted.txt`).

use std::path::PathBuf;

#[derive(Clone)]
pub struct Device {
    pub id: [u8; 16],
    pub key: [u8; 32],
    pub name: String,
}

pub struct Trusted {
    path: PathBuf,
    pub devices: Vec<Device>,
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// The file holds every device's key, so only this user (and admins) may read or change it.
#[cfg(unix)]
fn keep_private(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(windows)]
fn keep_private(path: &std::path::Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Ok(user) = std::env::var("USERNAME") else { return };
    // Drop the inherited "all users can change this" rights; keep this user, SYSTEM and admins.
    let _ = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r", &format!("{user}:F"), "*S-1-5-18:F", "*S-1-5-32-544:F"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
}

#[cfg(not(any(unix, windows)))]
fn keep_private(_: &std::path::Path) {}

impl Trusted {
    pub fn load(path: PathBuf) -> Self {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let devices = text
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| {
                let mut parts = l.splitn(3, ' ');
                Some(Device {
                    id: unhex(parts.next()?)?,
                    key: unhex(parts.next()?)?,
                    name: parts.next().unwrap_or("Phone").to_string(),
                })
            })
            .collect();
        if path.exists() {
            keep_private(&path);
        }
        Self { path, devices }
    }

    fn save(&self) {
        let mut text = String::from("# Phones trusted by EzRDP: id key name. Delete a line to un-trust a phone.\n");
        for d in &self.devices {
            text += &format!("{} {} {}\n", hex(&d.id), hex(&d.key), d.name);
        }
        let _ = std::fs::write(&self.path, text);
        keep_private(&self.path);
    }

    pub fn find(&self, id: &[u8]) -> Option<&Device> {
        self.devices.iter().find(|d| d.id == id)
    }

    pub fn add(&mut self, name: &str, key: [u8; 32]) -> [u8; 16] {
        let mut id = [0u8; 16];
        getrandom::fill(&mut id).expect("no random source");
        let name: String = name.chars().filter(|c| !c.is_control()).take(40).collect();
        self.devices.push(Device { id, key, name: if name.is_empty() { "Phone".into() } else { name } });
        self.save();
        id
    }

    pub fn remove(&mut self, id: &[u8; 16]) {
        self.devices.retain(|d| &d.id != id);
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_reload_remove() {
        let path = std::env::temp_dir().join(format!("ezrdp-trust-{}.txt", std::process::id()));
        let mut t = Trusted::load(path.clone());
        let id = t.add("Pixel\n 8", [5; 32]);
        let t2 = Trusted::load(path.clone());
        assert_eq!(t2.find(&id).unwrap().name, "Pixel 8");
        assert_eq!(t2.find(&id).unwrap().key, [5; 32]);
        t.remove(&id);
        assert!(Trusted::load(path.clone()).devices.is_empty());
        let _ = std::fs::remove_file(path);
    }
}
