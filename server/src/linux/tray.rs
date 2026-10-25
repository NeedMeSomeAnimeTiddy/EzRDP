//! The icon in the Linux panel (StatusNotifierItem: KDE, Ubuntu, Cinnamon, XFCE... On plain
//! GNOME it needs the "AppIndicator" extension). Same menu as the Windows tray.

use crate::state::App;
use ksni::blocking::TrayMethods;
use ksni::menu::{CheckmarkItem, StandardItem, SubMenu};
use ksni::{Icon, MenuItem, ToolTip};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, OnceLock};

struct Tray {
    /// None when this PC can't be controlled (the reason is in `problem`).
    app: Option<Arc<App>>,
    problem: Option<String>,
}

static HANDLE: OnceLock<ksni::blocking::Handle<Tray>> = OnceLock::new();

fn status(t: &Tray) -> String {
    match (&t.app, &t.problem) {
        (Some(app), _) if app.paused() => "Connections paused".into(),
        (Some(app), _) => match app.connected.lock().unwrap().as_deref() {
            Some(name) => format!("{name} is controlling this PC"),
            None => "Ready: others can control this PC".into(),
        },
        (None, Some(p)) => p.clone(),
        (None, None) => "This PC can't be controlled".into(),
    }
}

fn label<T>(text: impl Into<String>) -> MenuItem<T> {
    StandardItem { label: text.into(), enabled: false, ..Default::default() }.into()
}

fn item<T>(text: impl Into<String>, f: impl Fn(&mut T) + Send + 'static) -> MenuItem<T> {
    StandardItem { label: text.into(), activate: Box::new(f), ..Default::default() }.into()
}

/// A small monitor: blue, green while someone is connected, grey while paused.
fn icon(t: &Tray) -> Icon {
    let (r, g, b) = match &t.app {
        Some(app) if app.paused() => (150, 150, 150),
        Some(app) if app.connected.lock().unwrap().is_some() => (60, 190, 90),
        Some(_) => (70, 140, 230),
        None => (200, 80, 80),
    };
    let size = 32;
    let mut data = vec![0u8; size * size * 4];
    let mut put = |x: usize, y: usize, c: (u8, u8, u8)| {
        let o = (y * size + x) * 4;
        data[o..o + 4].copy_from_slice(&[255, c.0, c.1, c.2]);
    };
    let dark = (30, 40, 60);
    for y in 4..23 {
        for x in 2..30 {
            let edge = y < 6 || y > 20 || x < 4 || x > 27;
            put(x, y, if edge { dark } else { (r, g, b) });
        }
    }
    for y in 23..27 {
        for x in 13..19 {
            put(x, y, dark);
        }
    }
    for x in 8..24 {
        put(x, 27, dark);
        put(x, 28, dark);
    }
    Icon { width: size as i32, height: size as i32, data }
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "ezrdp".into()
    }

    fn title(&self) -> String {
        "EzRDP".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![icon(self)]
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip { title: "EzRDP".into(), description: status(self), ..Default::default() }
    }

    /// Clicking the icon opens the window.
    fn activate(&mut self, _x: i32, _y: i32) {
        crate::open_window();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut menu = vec![
            label(status(self)),
            MenuItem::Separator,
            item("Control another PC...", |_: &mut Self| crate::open_window()),
        ];
        if let Some(app) = &self.app {
            let paused = app.paused();
            menu.push(item(if paused { "Resume connections" } else { "Pause connections" }, |t: &mut Self| {
                if let Some(app) = &t.app {
                    let now = !app.paused();
                    app.paused.store(now, Relaxed);
                    if now {
                        crate::server::pause_all();
                    }
                }
            }));

            let devices = app.trusted.lock().unwrap().devices.clone();
            let mut trusted: Vec<MenuItem<Self>> = devices
                .iter()
                .map(|d| {
                    let id = d.id;
                    item(format!("Remove {}", d.name), move |t: &mut Self| {
                        if let Some(app) = &t.app {
                            app.trusted.lock().unwrap().remove(&id);
                            crate::server::drop_device(app, &id);
                        }
                    })
                })
                .collect();
            if trusted.is_empty() {
                trusted.push(label("None yet: pair from the phone or the other PC"));
            }
            menu.push(SubMenu { label: "Trusted devices".into(), submenu: trusted, ..Default::default() }.into());

            let home = crate::server::lan_ip().map(|i| i.to_string()).unwrap_or_else(|| "unknown".into());
            let net = app.public_ip.lock().unwrap().map(|i| i.to_string()).unwrap_or_else(|| "unknown".into());
            menu.push(
                SubMenu {
                    label: "Connection info".into(),
                    submenu: vec![
                        label(format!("PC name: {}", app.pc_name)),
                        label(format!("Home address: {home}")),
                        label(format!("Internet address: {net}")),
                        label(format!("Port: {}", app.port)),
                        label(format!("EzRDP build {} (protocol {})", crate::update::BUILD, crate::protocol::VERSION)),
                    ],
                    ..Default::default()
                }
                .into(),
            );
        }
        menu.push(
            CheckmarkItem {
                label: "Start when I sign in".into(),
                checked: crate::linux_autostart_enabled(),
                activate: Box::new(|_: &mut Self| crate::linux_autostart(!crate::linux_autostart_enabled())),
                ..Default::default()
            }
            .into(),
        );
        menu.push(item("Check for updates", |_: &mut Self| {
            std::thread::spawn(|| match crate::update::check() {
                Ok(None) => crate::notify_linux("Up to date", &format!("You have the newest version (build {}).", crate::update::BUILD)),
                Ok(Some(latest)) => {
                    crate::notify_linux("Updating", &format!("Installing build {}. EzRDP restarts by itself.", latest.build));
                    if let Err(e) = crate::update::install(&latest) {
                        crate::notify_linux("Couldn't update", &e);
                    }
                }
                Err(e) => crate::notify_linux("Couldn't check for updates", &e),
            });
        }));
        menu.push(
            CheckmarkItem {
                label: "Update automatically".into(),
                checked: crate::update::AUTO.load(Relaxed),
                activate: Box::new(|_: &mut Self| crate::set_auto_update(!crate::update::AUTO.load(Relaxed))),
                ..Default::default()
            }
            .into(),
        );
        menu.push(MenuItem::Separator);
        menu.push(item("Quit EzRDP", |_: &mut Self| std::process::exit(0)));
        menu
    }
}

/// Shows the icon. `problem` says why this PC can't be controlled, if it can't.
pub fn run(app: Option<Arc<App>>, problem: Option<String>) {
    let tray = Tray { app: app.clone(), problem };
    match tray.spawn() {
        Ok(handle) => {
            let _ = HANDLE.set(handle);
            if let Some(app) = app {
                *app.on_change.lock().unwrap() = Some(Box::new(refresh));
            }
        }
        Err(e) => crate::log!("No tray icon ({e}). On GNOME, install the \"AppIndicator\" extension to see it."),
    }
}

/// Redraws the icon and menu after something changed.
pub fn refresh() {
    if let Some(h) = HANDLE.get() {
        h.update(|_| {});
    }
}
