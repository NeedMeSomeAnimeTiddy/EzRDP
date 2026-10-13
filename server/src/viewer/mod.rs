//! The EzRDP window: control another PC (Windows or Linux) that runs EzRDP, from this PC.

mod audio;
mod keys;
mod net;
mod store;
mod video;

use eframe::egui::{
    self, Align, Color32, ComboBox, Event, Layout, Order, PointerButton, Pos2, Rect, RichText, TextureHandle,
    TextureOptions, Vec2, ViewportCommand,
};
use net::{ConnError, Found, Session};
use crate::protocol::*;
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use store::{SavedPc, Store};

const QUALITIES: [&str; 3] = ["High", "Balanced", "Data saver"];

type Slot<T> = Arc<Mutex<Option<T>>>;

enum Screen {
    Home,
    Pairing {
        name: String,
        /// The code to compare, and the PC's own name.
        code: Slot<(String, String)>,
        result: Slot<Result<SavedPc, ConnError>>,
        cancel: Slot<TcpStream>,
    },
    Connecting {
        pc: SavedPc,
        result: Slot<Result<Session, ConnError>>,
        /// Set while trying to get a dropped connection back.
        reconnect_until: Option<Instant>,
    },
    Live(Box<Live>),
    Message(String),
}

/// A connected session and what the window needs for it.
struct Live {
    pc: SavedPc,
    session: Session,
    texture: Option<TextureHandle>,
    /// Keys and mouse buttons pressed on the PC, so they can be let go if focus is lost.
    held_keys: Vec<u16>,
    held_buttons: [bool; 3],
    scroll: Vec2,
    last_pos: Option<(f32, f32)>,
    fullscreen: bool,
    image: Rect,
    /// The next paste is typed out on the other PC (from the Keys menu), not sent as Ctrl+V.
    paste_as_text: bool,
}

struct Viewer {
    store: Store,
    found: Arc<Mutex<Vec<Found>>>,
    screen: Screen,
    address: String,
    /// PC being given an internet address, and the text typed so far.
    editing: Option<(String, String)>,
    confirm_forget: Option<String>,
}

/// Opens the window; returns when it is closed.
pub fn run() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("EzRDP")
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([480.0, 320.0]),
        ..Default::default()
    };
    eframe::run_native(
        "EzRDP",
        options,
        Box::new(|cc| {
            let found = Arc::new(Mutex::new(Vec::new()));
            net::discover(found.clone(), cc.egui_ctx.clone(), crate::own_pc_id());
            Ok(Box::new(Viewer {
                store: Store::load(),
                found,
                screen: Screen::Home,
                address: String::new(),
                editing: None,
                confirm_forget: None,
            }))
        }),
    )
}

impl eframe::App for Viewer {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let mut next = None;
        match &mut self.screen {
            Screen::Home => next = self.home(ui),
            Screen::Pairing { .. } => next = self.pairing(ui),
            Screen::Connecting { .. } => next = self.connecting(ui),
            Screen::Live(_) => next = self.live(ui),
            Screen::Message(text) => {
                let text = text.clone();
                egui::CentralPanel::default_margins().show(ui, |ui| {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(text).size(16.0));
                        ui.add_space(16.0);
                        if ui.button("  OK  ").clicked() {
                            next = Some(Screen::Home);
                        }
                    });
                });
            }
        }
        if let Some(screen) = next {
            if matches!(self.screen, Screen::Live(_)) && !matches!(screen, Screen::Live(_)) {
                ctx.send_viewport_cmd(ViewportCommand::Fullscreen(false));
                ctx.send_viewport_cmd(ViewportCommand::Title("EzRDP".into()));
            }
            self.screen = screen;
            ctx.request_repaint();
        }
    }
}

impl Viewer {
    fn live_host(&self, id: &str) -> Option<String> {
        self.found.lock().unwrap().iter().find(|f| f.id == id).map(|f| f.host.clone())
    }

    fn start_connect(&self, pc: SavedPc, ctx: &egui::Context, reconnect_until: Option<Instant>) -> Screen {
        let result: Slot<Result<Session, ConnError>> = Arc::default();
        let live = self.live_host(&pc.id);
        let quality = self.store.quality;
        {
            let (pc, result, ctx) = (pc.clone(), result.clone(), ctx.clone());
            std::thread::spawn(move || {
                if reconnect_until.is_some() {
                    std::thread::sleep(Duration::from_secs(1));
                }
                let r = Session::connect(&pc, live.as_deref(), quality, ctx.clone());
                *result.lock().unwrap() = Some(r);
                ctx.request_repaint();
            });
        }
        Screen::Connecting { pc, result, reconnect_until }
    }

    fn start_pairing(&self, host: String, port: u16, name: String, ctx: &egui::Context) -> Screen {
        let (code, result, cancel): (Slot<(String, String)>, Slot<_>, Slot<TcpStream>) = Default::default();
        {
            let (code, result, cancel, ctx) = (code.clone(), result.clone(), cancel.clone(), ctx.clone());
            std::thread::spawn(move || {
                let r = net::pair(&host, port, &cancel, |c, name| {
                    *code.lock().unwrap() = Some((c, name));
                    ctx.request_repaint();
                });
                *result.lock().unwrap() = Some(r);
                ctx.request_repaint();
            });
        }
        Screen::Pairing { name, code, result, cancel }
    }

    // ---------- Home: list of PCs ----------

    fn home(&mut self, ui: &mut egui::Ui) -> Option<Screen> {
        let ctx = ui.ctx().clone();
        let mut next = None;
        let found = self.found.lock().unwrap().clone();
        egui::CentralPanel::default_margins().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("EzRDP");
                ui.label("Control another PC that runs EzRDP.");
                ui.add_space(12.0);

                ui.label(RichText::new("Your PCs").strong().size(16.0));
                if self.store.pcs.is_empty() {
                    ui.label("None yet. Pick a PC found on this network below to pair with it.");
                }
                let mut forget = None;
                let mut save = None;
                let mut connect = None;
                let mut stop_editing = false;
                let pcs = self.store.pcs.clone();
                for pc in &pcs {
                    let here = found.iter().find(|f| f.id == pc.id);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("🖥  {}", pc.name)).size(15.0));
                        if ui.button("Connect").clicked() {
                            connect = Some(pc.clone());
                        }
                        match here {
                            Some(f) if f.paused => ui.label(RichText::new("paused").weak()),
                            Some(_) => ui.label(RichText::new("on this network").weak()),
                            None => ui.label(RichText::new("not seen on this network").weak()),
                        };
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if self.confirm_forget.as_deref() == Some(&pc.id) {
                                if ui.button("Cancel").clicked() {
                                    self.confirm_forget = None;
                                }
                                if ui.button(RichText::new("Yes, forget it").color(Color32::LIGHT_RED)).clicked() {
                                    forget = Some(pc.id.clone());
                                }
                            } else {
                                ui.menu_button("More…", |ui| {
                                    if ui.button("Set internet address…").clicked() {
                                        self.editing = Some((pc.id.clone(), pc.manual.clone()));
                                        ui.close();
                                    }
                                    if ui.button("Forget this PC").clicked() {
                                        self.confirm_forget = Some(pc.id.clone());
                                        ui.close();
                                    }
                                });
                            }
                        });
                    });
                    if let Some((id, text)) = &mut self.editing
                        && *id == pc.id
                    {
                        ui.horizontal(|ui| {
                            ui.label("Internet address (e.g. a DDNS name):");
                            ui.text_edit_singleline(text);
                            if ui.button("Save").clicked() {
                                let mut p = pc.clone();
                                p.manual = text.trim().to_string();
                                save = Some(p);
                            }
                            if ui.button("Cancel").clicked() {
                                stop_editing = true;
                            }
                        });
                    }
                }
                if let Some(id) = forget {
                    self.store.remove(&id);
                    self.confirm_forget = None;
                }
                if let Some(p) = save {
                    self.store.put(p);
                    stop_editing = true;
                }
                if stop_editing {
                    self.editing = None;
                }
                if let Some(pc) = connect {
                    next = Some(self.start_connect(pc, &ctx, None));
                }

                ui.add_space(16.0);
                ui.label(RichText::new("Other PCs on this network").strong().size(16.0));
                let others: Vec<&Found> = found.iter().filter(|f| self.store.find(&f.id).is_none()).collect();
                if others.is_empty() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Looking… Start EzRDP on the other PC (same network).");
                    });
                }
                for f in others {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("🖥  {}", f.name)).size(15.0));
                        ui.label(RichText::new(&f.host).weak());
                        if f.version != VERSION {
                            ui.label(RichText::new("different EzRDP version: update both").color(Color32::LIGHT_RED));
                        } else if f.paused {
                            ui.label(RichText::new("paused").weak());
                        } else if ui.button("Pair").clicked() {
                            next = Some(self.start_pairing(f.host.clone(), f.port, f.name.clone(), &ctx));
                        }
                    });
                }

                ui.add_space(16.0);
                ui.label(RichText::new("Pair by address").strong());
                ui.horizontal(|ui| {
                    ui.label("Home network address of the PC:");
                    ui.add(egui::TextEdit::singleline(&mut self.address).hint_text("192.168.1.20").desired_width(160.0));
                    let addr = self.address.trim().to_string();
                    if ui.add_enabled(!addr.is_empty(), egui::Button::new("Pair")).clicked() {
                        let (host, port) = match addr.rsplit_once(':') {
                            Some((h, p)) if !h.contains(':') && p.parse::<u16>().is_ok() => (h.to_string(), p.parse().unwrap()),
                            _ => (addr.clone(), DEFAULT_PORT),
                        };
                        next = Some(self.start_pairing(host, port, addr, &ctx));
                        self.address.clear();
                    }
                });

                ui.add_space(16.0);
                ui.label(RichText::new("Settings").strong());
                ui.horizontal(|ui| {
                    ui.label("Picture quality:");
                    let mut q = self.store.quality;
                    for (i, name) in QUALITIES.iter().enumerate() {
                        ui.selectable_value(&mut q, i as u8, *name);
                    }
                    if q != self.store.quality {
                        self.store.quality = q;
                        self.store.save();
                    }
                });
                let mut sound = self.store.sound;
                if ui.checkbox(&mut sound, "Play the other PC's sound here").changed() {
                    self.store.sound = sound;
                    self.store.save();
                }

                ui.add_space(20.0);
                ui.label(
                    RichText::new(
                        "While EzRDP runs on this PC, your phone or another PC can control it too \
                         (pair them the same way).",
                    )
                    .weak(),
                );
            });
        });
        // Keep "on this network" fresh.
        ctx.request_repaint_after(Duration::from_secs(2));
        next
    }

    // ---------- Pairing ----------

    fn pairing(&mut self, ui: &mut egui::Ui) -> Option<Screen> {
        let Screen::Pairing { name, code, result, cancel } = &self.screen else { return None };
        if let Some(r) = result.lock().unwrap().take() {
            return Some(match r {
                Ok(pc) => {
                    let screen = self.start_connect(pc.clone(), ui.ctx(), None);
                    self.store.put(pc);
                    screen
                }
                Err(e) => Screen::Message(e.advice(name)),
            });
        }
        let mut next = None;
        let code = code.lock().unwrap().clone();
        let name = code.as_ref().map_or(name.clone(), |(_, n)| n.clone());
        egui::CentralPanel::default_margins().show(ui, |ui| {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.heading(format!("Pairing with {name}"));
                ui.add_space(16.0);
                match code {
                    Some((code, _)) => {
                        ui.label(format!("{name} now asks whether to allow this computer. Check it shows this code:"));
                        ui.add_space(8.0);
                        ui.label(RichText::new(code).size(40.0).strong().monospace());
                        ui.add_space(8.0);
                        ui.label("If the codes match, click Yes on that PC.");
                    }
                    None => {
                        ui.spinner();
                    }
                }
                ui.add_space(16.0);
                if ui.button("Cancel").clicked() {
                    if let Some(s) = cancel.lock().unwrap().take() {
                        let _ = s.shutdown(Shutdown::Both);
                    }
                    next = Some(Screen::Home);
                }
            });
        });
        next
    }

    // ---------- Connecting ----------

    fn connecting(&mut self, ui: &mut egui::Ui) -> Option<Screen> {
        let ctx = ui.ctx().clone();
        let Screen::Connecting { pc, result, reconnect_until } = &self.screen else { return None };
        let (pc, reconnect_until) = (pc.clone(), *reconnect_until);
        let done = result.lock().unwrap().take();
        match done {
            Some(Ok(session)) => {
                ctx.send_viewport_cmd(ViewportCommand::Title(format!("{} - EzRDP", pc.name)));
                session.send(C_SET_AUDIO, vec![self.store.sound as u8]);
                return Some(Screen::Live(Box::new(Live {
                    pc,
                    session,
                    texture: None,
                    held_keys: Vec::new(),
                    held_buttons: [false; 3],
                    scroll: Vec2::ZERO,
                    last_pos: None,
                    fullscreen: false,
                    image: Rect::NOTHING,
                    paste_as_text: false,
                })));
            }
            Some(Err(e)) => {
                return Some(match reconnect_until {
                    Some(until) if Instant::now() < until && matches!(e, ConnError::Io(_)) => {
                        self.start_connect(pc, &ctx, reconnect_until)
                    }
                    _ => Screen::Message(e.advice(&pc.name)),
                });
            }
            None => {}
        }
        let mut next = None;
        egui::CentralPanel::default_margins().show(ui, |ui| {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.spinner();
                ui.add_space(8.0);
                ui.label(if reconnect_until.is_some() {
                    format!("Connection to {} was lost. Reconnecting…", pc.name)
                } else {
                    format!("Connecting to {}…", pc.name)
                });
                ui.add_space(16.0);
                if ui.button("Cancel").clicked() {
                    next = Some(Screen::Home);
                }
            });
        });
        next
    }

    // ---------- Connected ----------

    fn live(&mut self, ui: &mut egui::Ui) -> Option<Screen> {
        let ctx = ui.ctx().clone();
        let Screen::Live(live) = &mut self.screen else { return None };

        // The connection ended: say why, or try to get it back.
        let ended = live.session.state.lock().unwrap().ended.take();
        if let Some((bye, _error)) = ended {
            let name = live.pc.name.clone();
            return Some(match bye {
                Some(BYE_PAUSED) => Screen::Message(format!("Connections were paused on {name}.")),
                Some(BYE_REMOVED) => Screen::Message(format!("{name} no longer trusts this computer.")),
                Some(_) => Screen::Message(format!("Someone at {name} ended the session.")),
                None => {
                    let pc = live.pc.clone();
                    self.start_connect(pc, &ctx, Some(Instant::now() + Duration::from_secs(30)))
                }
            });
        }
        // Remember the PC's current addresses for next time.
        let addresses = live.session.state.lock().unwrap().addresses.take();
        if let Some((home, internet, port)) = addresses {
            let pc = &mut live.pc;
            if !home.is_empty() {
                pc.home = home;
            }
            if !internet.is_empty() {
                pc.internet = internet;
            }
            pc.port = port;
            self.store.put(pc.clone());
        }

        let Screen::Live(live) = &mut self.screen else { return None };
        if let Some(img) = live.session.video.take() {
            match &mut live.texture {
                Some(t) => t.set(img, TextureOptions::LINEAR),
                None => live.texture = Some(ctx.load_texture("screen", img, TextureOptions::LINEAR)),
            }
        }

        // Keyboard goes to the other PC, not to the buttons in the bar.
        let events = ctx.input_mut(|i| {
            let all = i.events.clone();
            i.events.retain(|e| !matches!(e, Event::Key { .. } | Event::Text(_) | Event::Copy | Event::Cut | Event::Paste(_)));
            all
        });
        if let Some(id) = ctx.memory(|m| m.focused()) {
            ctx.memory_mut(|m| m.surrender_focus(id));
        }

        let mut next = None;
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let show_bar = !live.fullscreen || pointer.is_some_and(|p| p.y < 40.0);
        if show_bar {
            egui::Panel::top("bar").show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("✖ Disconnect").clicked() {
                        next = Some(Screen::Home);
                    }
                    ui.separator();
                    ui.label(RichText::new(&live.pc.name).strong());
                    let (monitors, current) = {
                        let s = live.session.state.lock().unwrap();
                        (s.monitors.clone(), s.monitor)
                    };
                    if monitors.len() > 1 {
                        let label = |m: &net::MonitorInfo| {
                            format!("{} ({}×{}){}", m.name, m.w, m.h, if m.primary { ", main" } else { "" })
                        };
                        let selected = monitors.iter().find(|m| m.index == current).map(label).unwrap_or_default();
                        ComboBox::from_id_salt("screen").selected_text(selected).show_ui(ui, |ui| {
                            for m in &monitors {
                                if ui.selectable_label(m.index == current, label(m)).clicked() {
                                    live.session.send(C_SELECT_MONITOR, vec![m.index]);
                                }
                            }
                        });
                    }
                    let mut q = self.store.quality;
                    ComboBox::from_id_salt("quality").selected_text(QUALITIES[q as usize]).show_ui(ui, |ui| {
                        for (i, name) in QUALITIES.iter().enumerate() {
                            ui.selectable_value(&mut q, i as u8, *name);
                        }
                    });
                    if q != self.store.quality {
                        self.store.quality = q;
                        self.store.save();
                        live.session.send(C_SET_QUALITY, vec![q]);
                    }
                    let mut sound = self.store.sound;
                    if ui.checkbox(&mut sound, "Sound").changed() {
                        self.store.sound = sound;
                        self.store.save();
                        live.session.send(C_SET_AUDIO, vec![sound as u8]);
                    }
                    ui.menu_button("Keys", |ui| {
                        let combos: [(&str, &[u16]); 7] = [
                            ("Windows key / Start", &[keys::WIN]),
                            ("Alt+Tab", &[keys::ALT, 0x09]),
                            ("Alt+F4", &[keys::ALT, 0x73]),
                            ("Task Manager (Ctrl+Shift+Esc)", &[keys::CTRL, keys::SHIFT, 0x1B]),
                            ("Lock (Windows+L)", &[keys::WIN, 0x4C]),
                            ("Show desktop (Windows+D)", &[keys::WIN, 0x44]),
                            ("Print Screen", &[0x2C]),
                        ];
                        for (name, combo) in combos {
                            if ui.button(name).clicked() {
                                for &k in combo {
                                    live.session.key(k, true);
                                }
                                for &k in combo.iter().rev() {
                                    live.session.key(k, false);
                                }
                                ui.close();
                            }
                        }
                        ui.separator();
                        if ui.button("Type the text copied on this computer").clicked() {
                            live.paste_as_text = true;
                            ctx.send_viewport_cmd(ViewportCommand::RequestPaste);
                            ui.close();
                        }
                    });
                    let full = if live.fullscreen { "Leave full screen" } else { "Full screen" };
                    if ui.button(full).clicked() {
                        live.fullscreen = !live.fullscreen;
                        ctx.send_viewport_cmd(ViewportCommand::Fullscreen(live.fullscreen));
                    }
                    if live.fullscreen {
                        ui.label(RichText::new("Move the mouse to the top edge to see this bar.").weak());
                    }
                });
            });
        }

        egui::CentralPanel::no_frame().frame(egui::Frame::NONE.fill(Color32::BLACK)).show(ui, |ui| {
            let area = ui.available_rect_before_wrap();
            match &live.texture {
                Some(tex) => {
                    let size = tex.size_vec2();
                    let scale = (area.width() / size.x).min(area.height() / size.y);
                    live.image = Rect::from_center_size(area.center(), size * scale);
                    ui.painter().image(
                        tex.id(),
                        live.image,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                None => {
                    live.image = Rect::NOTHING;
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new("Waiting for the picture…").color(Color32::GRAY));
                    });
                }
            }
        });

        handle_input(live, &events, &ctx);
        next
    }
}

/// Sends this computer's mouse and keyboard to the other PC.
fn handle_input(live: &mut Live, events: &[Event], ctx: &egui::Context) {
    let s = &live.session;
    let image = live.image;
    // Only the picture counts, not menus or the bar drawn over it.
    let on_picture =
        |p: Pos2| image.contains(p) && ctx.layer_id_at(p).is_none_or(|l| l.order == Order::Background);
    let to_pc = |p: Pos2| {
        (
            ((p.x - image.min.x) / image.width()).clamp(0.0, 1.0),
            ((p.y - image.min.y) / image.height()).clamp(0.0, 1.0),
        )
    };
    let shortcut = |held: &[u16]| held.iter().any(|k| matches!(*k, keys::CTRL | 0xA3 | keys::ALT | keys::WIN | 0x5C));
    let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
    let mut lost_focus = false;

    for e in events {
        match e {
            Event::PointerMoved(p) => {
                let dragging = live.held_buttons.iter().any(|b| *b);
                if image.is_positive() && (dragging || on_picture(*p)) {
                    let pos = to_pc(*p);
                    if live.last_pos != Some(pos) {
                        live.last_pos = Some(pos);
                        s.mouse_abs(pos.0, pos.1);
                    }
                }
            }
            Event::PointerButton { pos, button, pressed, .. } => {
                let b = match button {
                    PointerButton::Primary => 0,
                    PointerButton::Secondary => 1,
                    PointerButton::Middle => 2,
                    _ => continue,
                };
                if *pressed && image.is_positive() && on_picture(*pos) {
                    let p = to_pc(*pos);
                    s.mouse_abs(p.0, p.1);
                    s.button(b as u8, true);
                    live.held_buttons[b] = true;
                } else if !*pressed && live.held_buttons[b] {
                    let p = to_pc(*pos);
                    s.mouse_abs(p.0, p.1);
                    s.button(b as u8, false);
                    live.held_buttons[b] = false;
                }
            }
            Event::MouseWheel { unit, delta, .. } => {
                if !ctx.input(|i| i.pointer.hover_pos()).is_some_and(on_picture) {
                    continue;
                }
                // 120 = one notch of a mouse wheel.
                let per = match unit {
                    egui::MouseWheelUnit::Line => 120.0,
                    egui::MouseWheelUnit::Page => 360.0,
                    egui::MouseWheelUnit::Point => 3.0,
                };
                live.scroll += *delta * per;
                let (dy, dx) = (live.scroll.y.trunc(), -live.scroll.x.trunc());
                if dy != 0.0 || dx != 0.0 {
                    live.scroll -= Vec2::new(-dx, dy);
                    s.scroll(dy as i32, dx as i32);
                }
            }
            Event::WindowFocused(false) => lost_focus = true,
            Event::Key { key, physical_key, pressed, .. } if focused => {
                let Some((vk, kind)) = keys::vk(*key).or_else(|| physical_key.and_then(keys::vk)) else { continue };
                let send = match kind {
                    keys::Kind::Modifier | keys::Kind::Special => true,
                    // Plain typing goes as text (right for any keyboard layout); shortcuts as keys.
                    keys::Kind::Printable => !*pressed || shortcut(&live.held_keys),
                };
                if *pressed && send {
                    if matches!(kind, keys::Kind::Modifier) && live.held_keys.contains(&vk) {
                        continue; // key repeat
                    }
                    if !live.held_keys.contains(&vk) {
                        live.held_keys.push(vk);
                    }
                    s.key(vk, true);
                } else if !*pressed && live.held_keys.contains(&vk) {
                    live.held_keys.retain(|k| *k != vk);
                    s.key(vk, false);
                }
            }
            Event::Text(t) if focused && !shortcut(&live.held_keys) => s.text(t),
            Event::Paste(text) if live.paste_as_text => {
                live.paste_as_text = false;
                s.text(text);
            }
            // Ctrl+C / X / V are reported as copy/cut/paste; pass them on as the keys they were.
            Event::Copy | Event::Cut | Event::Paste(_) if focused => {
                let ctrl = live.held_keys.iter().any(|k| matches!(*k, keys::CTRL | 0xA3));
                let vk = match (e, ctrl) {
                    (Event::Copy, true) => 0x43,
                    (Event::Copy, false) => 0x2D, // Ctrl+Insert
                    (Event::Cut, true) => 0x58,
                    (Event::Cut, false) => 0x2E, // Shift+Delete
                    (_, true) => 0x56,
                    (_, false) => 0x2D, // Shift+Insert
                };
                s.key(vk, true);
                s.key(vk, false);
            }
            _ => {}
        }
    }
    if lost_focus {
        release_all(live);
    }
}

fn release_all(live: &mut Live) {
    for vk in std::mem::take(&mut live.held_keys) {
        live.session.key(vk, false);
    }
    for b in 0..3 {
        if live.held_buttons[b] {
            live.session.button(b as u8, false);
            live.held_buttons[b] = false;
        }
    }
}
