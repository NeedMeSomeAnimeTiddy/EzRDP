//! PCs this computer has paired with, and a few settings, kept in a small text file:
//! %APPDATA%\EzRDP\viewer.txt on Windows, ~/.config/ezrdp/viewer.txt on Linux.

use crate::trust::{hex, unhex};
use std::path::PathBuf;

#[derive(Clone)]
pub struct SavedPc {
    /// The PC's id (hex).
    pub id: String,
    pub name: String,
    /// This computer's id and key on that PC.
    pub device_id: [u8; 16],
    pub key: [u8; 32],
    pub home: String,
    pub internet: String,
    /// Address typed by the person (e.g. a DDNS name), tried after the home address.
    pub manual: String,
    pub port: u16,
}

impl SavedPc {
    /// Addresses to try, best first: found on this network now, last home address, typed, internet.
    pub fn addresses(&self, live: Option<&str>) -> Vec<(String, std::time::Duration)> {
        use std::time::Duration;
        let mut list: Vec<(String, Duration)> = Vec::new();
        let mut add = |host: &str, ms| {
            if !host.is_empty() && !list.iter().any(|(h, _)| h == host) {
                list.push((host.to_string(), Duration::from_millis(ms)));
            }
        };
        add(live.unwrap_or(""), 6000);
        add(&self.home, 1500);
        add(&self.manual, 6000);
        add(&self.internet, 6000);
        list
    }
}

pub struct Store {
    path: PathBuf,
    pub pcs: Vec<SavedPc>,
    /// 0 high, 1 balanced, 2 data saver.
    pub quality: u8,
    pub sound: bool,
}

fn dir() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("EzRDP"));
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|c| c.join("ezrdp"));
    base.unwrap_or_else(std::env::temp_dir)
}

fn field(s: &str) -> String {
    if s.is_empty() { "-".into() } else { s.to_string() }
}

fn unfield(s: &str) -> String {
    if s == "-" { String::new() } else { s.to_string() }
}

impl Store {
    pub fn load() -> Self {
        Self::load_from(dir().join("viewer.txt"))
    }

    fn load_from(path: PathBuf) -> Self {
        let mut store = Store { path, pcs: Vec::new(), quality: 0, sound: true };
        let text = std::fs::read_to_string(&store.path).unwrap_or_default();
        for line in text.lines() {
            let mut f = line.split(' ');
            match f.next() {
                Some("quality") => store.quality = f.next().and_then(|v| v.parse().ok()).unwrap_or(0).min(2),
                Some("sound") => store.sound = f.next() != Some("off"),
                Some("pc") => {
                    let pc = (|| {
                        Some(SavedPc {
                            id: f.next()?.to_string(),
                            device_id: unhex(f.next()?)?,
                            key: unhex(f.next()?)?,
                            port: f.next()?.parse().ok()?,
                            home: unfield(f.next()?),
                            internet: unfield(f.next()?),
                            manual: unfield(f.next()?),
                            name: f.collect::<Vec<_>>().join(" "),
                        })
                    })();
                    store.pcs.extend(pc);
                }
                _ => {}
            }
        }
        store
    }

    pub fn save(&self) {
        let mut text = String::from("# EzRDP viewer: PCs paired with this computer. Keep this file private.\n");
        text += &format!("quality {}\nsound {}\n", self.quality, if self.sound { "on" } else { "off" });
        for p in &self.pcs {
            let name: String = p.name.chars().filter(|c| !c.is_control()).collect();
            text += &format!(
                "pc {} {} {} {} {} {} {} {}\n",
                p.id,
                hex(&p.device_id),
                hex(&p.key),
                p.port,
                field(&p.home),
                field(&p.internet),
                field(p.manual.trim()),
                name
            );
        }
        if let Some(d) = self.path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = self.path.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
            }
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }

    pub fn find(&self, id: &str) -> Option<&SavedPc> {
        self.pcs.iter().find(|p| p.id == id)
    }

    pub fn put(&mut self, pc: SavedPc) {
        match self.pcs.iter_mut().find(|p| p.id == pc.id) {
            Some(p) => *p = pc,
            None => self.pcs.push(pc),
        }
        self.save();
    }

    pub fn remove(&mut self, id: &str) {
        self.pcs.retain(|p| p.id != id);
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load() {
        let path = std::env::temp_dir().join(format!("ezrdp-viewer-{}.txt", std::process::id()));
        let mut s = Store::load_from(path.clone());
        s.quality = 2;
        s.put(SavedPc {
            id: "ab".repeat(16),
            name: "Office PC 2".into(),
            device_id: [1; 16],
            key: [2; 32],
            home: "192.168.1.9".into(),
            internet: String::new(),
            manual: "me.example.org".into(),
            port: 7270,
        });
        let t = Store::load_from(path.clone());
        assert_eq!(t.quality, 2);
        let p = &t.pcs[0];
        assert_eq!((p.name.as_str(), p.home.as_str(), p.internet.as_str()), ("Office PC 2", "192.168.1.9", ""));
        assert_eq!((p.key, p.device_id, p.port), ([2; 32], [1; 16], 7270));
        assert_eq!(p.addresses(Some("10.0.0.4")).len(), 3);
        let _ = std::fs::remove_file(path);
    }
}
