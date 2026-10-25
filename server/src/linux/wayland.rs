//! Wayland desktops (GNOME, KDE...): apps can't read the screen or press keys directly, so
//! EzRDP asks the desktop through its "remote desktop" portal. The first time, the PC shows
//! a dialog asking to allow it; the answer is remembered (restore token) so later connections
//! don't ask again. The picture arrives through PipeWire; mouse and keys go back through the portal.

use super::capture::{CapError, Monitor};
use crate::convert::Yuv;
use ashpd::desktop::remote_desktop::{Axis, DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions};
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{PersistMode, Session};
use pipewire as pw;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Is this a Wayland session (where the X11 way of reading the screen only gives black)?
pub fn active() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland") || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// A screen the portal shares: PipeWire node, and its place and size in desktop coordinates.
#[derive(Clone)]
struct Screen {
    node: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

enum Cmd {
    Motion { node: u32, x: f64, y: f64 },
    Relative { dx: f64, dy: f64 },
    Button { code: i32, down: bool },
    Scroll { vertical: bool, steps: i32 },
    Key { sym: i32, down: bool },
}

/// One portal session for as long as EzRDP runs (so the person is asked at most once).
pub struct Portal {
    screens: Vec<Screen>,
    fd: OwnedFd,
    cmds: mpsc::Sender<Cmd>,
    dead: Arc<AtomicBool>,
}

static PORTAL: Mutex<(Option<Arc<Portal>>, Option<Instant>)> = Mutex::new((None, None));

fn token_path() -> std::path::PathBuf {
    crate::data_path("portal-token")
}

/// The shared portal session, starting it (and maybe asking the person) if needed.
pub fn portal() -> Result<Arc<Portal>, String> {
    let mut slot = PORTAL.lock().unwrap();
    if let Some(p) = &slot.0 {
        if !p.dead.load(Relaxed) {
            return Ok(p.clone());
        }
        slot.0 = None;
    }
    // Don't pester the person with dialogs: after a refusal, wait before asking again.
    if slot.1.is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
        return Err("sharing this screen was refused or failed on this PC; trying again shortly".into());
    }
    match start() {
        Ok(p) => {
            let p = Arc::new(p);
            slot.0 = Some(p.clone());
            slot.1 = None;
            Ok(p)
        }
        Err(e) => {
            slot.1 = Some(Instant::now());
            Err(e)
        }
    }
}

fn start() -> Result<Portal, String> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(Vec<Screen>, OwnedFd), String>>();
    let (cmds, cmd_rx) = mpsc::channel::<Cmd>();
    let dead = Arc::new(AtomicBool::new(false));
    let dead2 = dead.clone();
    std::thread::spawn(move || {
        let setup = pollster::block_on(async {
            match with_input().await {
                Ok((rd, session, screens, fd)) => Ok((Some((rd, session)), None, screens, fd)),
                // The person said no: don't ask again with another dialog.
                Err(Setup::Refused(e)) => Err(e),
                // This desktop can share the screen but not take mouse and keys (e.g. Sway).
                Err(Setup::Unavailable(why)) => {
                    crate::log!("This desktop doesn't offer remote mouse and keyboard ({why}); sharing the picture only.");
                    match view_only().await {
                        Ok((session, screens, fd)) => Ok((None, Some(session), screens, fd)),
                        Err(Setup::Refused(e) | Setup::Unavailable(e)) => Err(e),
                    }
                }
            }
        });
        let (input, _view_session, screens, fd) = match setup {
            Ok(v) => v,
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
        };
        let Some((rd, session)) = input else {
            let _ = ready_tx.send(Ok((screens, fd)));
            // Keep the picture session open; mouse and keys can't be passed on.
            while cmd_rx.recv().is_ok() {}
            return;
        };
        let _ = ready_tx.send(Ok((screens, fd)));
        // Mouse and keyboard, in order, for as long as EzRDP runs.
        while let Ok(cmd) = cmd_rx.recv() {
            let r = pollster::block_on(send(&rd, &session, cmd));
            if let Err(e) = r {
                crate::log!("Wayland input stopped working ({e}); it will be set up again on the next connection.");
                dead2.store(true, Relaxed);
                break;
            }
        }
    });
    // The person may take a while to answer the dialog.
    let (screens, fd) = ready_rx.recv_timeout(Duration::from_secs(120)).map_err(|_| "no answer from the desktop".to_string())??;
    crate::log!("Screen sharing allowed: {} screen(s)", screens.len());
    Ok(Portal { screens, fd, cmds, dead })
}

enum Setup {
    /// The portal (or the part needed) isn't there.
    Unavailable(String),
    /// The person at the PC said no, or the dialog failed.
    Refused(String),
}

fn screens_of(streams: &[ashpd::desktop::screencast::Stream]) -> Vec<Screen> {
    streams
        .iter()
        .map(|s| {
            let (x, y) = s.position().unwrap_or((0, 0));
            let (w, h) = s.size().unwrap_or((1920, 1080));
            Screen { node: s.pipe_wire_node_id(), x, y, w, h }
        })
        .collect()
}

fn sources() -> SelectSourcesOptions {
    SelectSourcesOptions::default()
        .set_cursor_mode(CursorMode::Embedded)
        .set_sources(enumflags2::BitFlags::from(SourceType::Monitor))
        .set_multiple(true)
}

/// Screen and mouse/keyboard together (GNOME, KDE).
async fn with_input() -> Result<(RemoteDesktop, Session<RemoteDesktop>, Vec<Screen>, OwnedFd), Setup> {
    let gone = |e: ashpd::Error| Setup::Unavailable(e.to_string());
    let rd = RemoteDesktop::new().await.map_err(gone)?;
    let sc = Screencast::new().await.map_err(gone)?;
    let session = rd.create_session(Default::default()).await.map_err(gone)?;
    let token = std::fs::read_to_string(token_path()).ok();
    rd.select_devices(
        &session,
        SelectDevicesOptions::default()
            .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
            .set_persist_mode(PersistMode::ExplicitlyRevoked)
            .set_restore_token(token.as_deref().map(str::trim)),
    )
    .await
    .map_err(gone)?;
    sc.select_sources(&session, sources()).await.map_err(gone)?;
    crate::log!("Asking this PC's desktop for permission to share the screen (a dialog may appear here)...");
    let started = rd
        .start(&session, None, Default::default())
        .await
        .map_err(|e| Setup::Refused(e.to_string()))?
        .response()
        .map_err(|e| Setup::Refused(format!("screen sharing was not allowed on this PC ({e})")))?;
    if let Some(t) = started.restore_token() {
        let _ = std::fs::write(token_path(), t);
    }
    let screens = screens_of(started.streams());
    if screens.is_empty() {
        return Err(Setup::Refused("the desktop shared no screens".into()));
    }
    let fd = sc.open_pipe_wire_remote(&session, Default::default()).await.map_err(|e| Setup::Refused(e.to_string()))?;
    Ok((rd, session, screens, fd))
}

/// The picture only.
async fn view_only() -> Result<(Session<Screencast>, Vec<Screen>, OwnedFd), Setup> {
    let sc = Screencast::new().await.map_err(|e| Setup::Unavailable(format!("no screen sharing portal ({e})")))?;
    let session = sc.create_session(Default::default()).await.map_err(|e| Setup::Unavailable(e.to_string()))?;
    sc.select_sources(&session, sources()).await.map_err(|e| Setup::Unavailable(e.to_string()))?;
    crate::log!("Asking this PC's desktop for permission to share the screen (a dialog may appear here)...");
    let started = sc
        .start(&session, None, Default::default())
        .await
        .map_err(|e| Setup::Refused(e.to_string()))?
        .response()
        .map_err(|e| Setup::Refused(format!("screen sharing was not allowed on this PC ({e})")))?;
    let screens = screens_of(started.streams());
    if screens.is_empty() {
        return Err(Setup::Refused("the desktop shared no screens".into()));
    }
    let fd = sc.open_pipe_wire_remote(&session, Default::default()).await.map_err(|e| Setup::Refused(e.to_string()))?;
    Ok((session, screens, fd))
}

async fn send(rd: &RemoteDesktop, session: &Session<RemoteDesktop>, cmd: Cmd) -> ashpd::Result<()> {
    let state = |down: bool| if down { KeyState::Pressed } else { KeyState::Released };
    match cmd {
        Cmd::Motion { node, x, y } => rd.notify_pointer_motion_absolute(session, node, x, y, Default::default()).await,
        Cmd::Relative { dx, dy } => rd.notify_pointer_motion(session, dx, dy, Default::default()).await,
        Cmd::Button { code, down } => rd.notify_pointer_button(session, code, state(down), Default::default()).await,
        Cmd::Scroll { vertical, steps } => {
            let axis = if vertical { Axis::Vertical } else { Axis::Horizontal };
            rd.notify_pointer_axis_discrete(session, axis, steps, Default::default()).await
        }
        Cmd::Key { sym, down } => rd.notify_keyboard_keysym(session, sym, state(down), Default::default()).await,
    }
}

impl Portal {
    fn cmd(&self, c: Cmd) {
        let _ = self.cmds.send(c);
    }

    fn screen(&self, m: &Monitor) -> Option<&Screen> {
        self.screens.iter().find(|s| s.node == m.stream).or(self.screens.first())
    }

    /// Moves to 0..1 fractions of a screen.
    pub fn move_abs(&self, m: &Monitor, fx: f32, fy: f32) {
        if let Some(s) = self.screen(m) {
            // Same scale the pointer is reported back in (pixel / size).
            let x = (fx.clamp(0.0, 1.0) as f64 * s.w as f64).min((s.w - 1).max(0) as f64);
            let y = (fy.clamp(0.0, 1.0) as f64 * s.h as f64).min((s.h - 1).max(0) as f64);
            self.cmd(Cmd::Motion { node: s.node, x, y });
        }
    }

    /// Moves by fractions of the screen size.
    pub fn move_rel(&self, m: &Monitor, fx: f32, fy: f32) {
        if let Some(s) = self.screen(m) {
            self.cmd(Cmd::Relative { dx: fx as f64 * s.w as f64, dy: fy as f64 * s.h as f64 });
        }
    }

    /// 0 left, 1 right, 2 middle (Linux button codes BTN_LEFT...).
    pub fn button(&self, b: u8, down: bool) {
        let code = match b {
            0 => 0x110,
            1 => 0x111,
            2 => 0x112,
            _ => return,
        };
        self.cmd(Cmd::Button { code, down });
    }

    /// Notches: positive = down / right.
    pub fn scroll(&self, vertical: bool, steps: i32) {
        if steps != 0 {
            self.cmd(Cmd::Scroll { vertical, steps });
        }
    }

    pub fn key(&self, sym: u32, down: bool) {
        self.cmd(Cmd::Key { sym: sym as i32, down });
    }
}

pub fn monitors() -> Vec<Monitor> {
    match portal() {
        Ok(p) => p
            .screens
            .iter()
            .enumerate()
            .map(|(i, s)| Monitor {
                name: format!("Screen {}", i + 1),
                x: s.x,
                y: s.y,
                w: s.w,
                h: s.h,
                primary: i == 0,
                stream: s.node,
            })
            .collect(),
        Err(e) => {
            crate::log!("{e}");
            Vec::new()
        }
    }
}

/// The newest picture from PipeWire.
#[derive(Default)]
struct Frame {
    data: Vec<u8>,
    w: usize,
    h: usize,
    stride: usize,
    /// Red and blue swapped (RGBx instead of BGRx).
    rgb: bool,
    seq: u64,
}

struct Shared {
    frame: Mutex<Frame>,
    new_frame: Condvar,
    stop: AtomicBool,
}

/// Receives one screen's pictures on a PipeWire thread.
pub struct Capture {
    monitor: Monitor,
    shared: Arc<Shared>,
    seen: u64,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Capture {
    pub fn open(index: usize) -> Result<Self, String> {
        let p = portal()?;
        let screen = p.screens.get(index).or(p.screens.first()).cloned().ok_or("no screens shared")?;
        let fd = p.fd.as_fd().try_clone_to_owned().map_err(|e| e.to_string())?;
        let shared = Arc::new(Shared { frame: Mutex::new(Frame::default()), new_frame: Condvar::new(), stop: AtomicBool::new(false) });
        let thread = {
            let shared = shared.clone();
            let node = screen.node;
            std::thread::spawn(move || {
                if let Err(e) = run_stream(fd, node, &shared) {
                    crate::log!("Wayland screen capture failed: {e}");
                }
                shared.stop.store(true, Relaxed);
                shared.new_frame.notify_all();
            })
        };
        let mut cap = Self {
            monitor: Monitor {
                name: format!("Screen {}", index + 1),
                x: screen.x,
                y: screen.y,
                w: screen.w,
                h: screen.h,
                primary: index == 0,
                stream: screen.node,
            },
            shared,
            seen: 0,
            thread: Some(thread),
        };
        // The video size must match the real pixels (which differ from the screen size when scaled).
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut f = cap.shared.frame.lock().unwrap();
        while f.seq == 0 && !cap.shared.stop.load(Relaxed) && Instant::now() < deadline {
            f = cap.shared.new_frame.wait_timeout(f, Duration::from_millis(100)).unwrap().0;
        }
        if f.seq == 0 {
            return Err("no picture from the desktop".into());
        }
        cap.monitor.w = f.w as i32;
        cap.monitor.h = f.h as i32;
        drop(f);
        Ok(cap)
    }

    pub fn monitor(&self) -> &Monitor {
        &self.monitor
    }

    pub fn frame(&mut self, timeout_ms: u32, yuv: &mut Yuv, scale: usize) -> Result<bool, CapError> {
        let mut f = self.shared.frame.lock().unwrap();
        if f.seq == self.seen {
            f = self.shared.new_frame.wait_timeout(f, Duration::from_millis(timeout_ms as u64)).unwrap().0;
        }
        if self.shared.stop.load(Relaxed) {
            return Err(CapError::Lost);
        }
        if f.seq == self.seen {
            return Ok(false);
        }
        if (f.w as i32, f.h as i32) != (self.monitor.w, self.monitor.h) {
            return Err(CapError::Lost); // the screen changed size
        }
        self.seen = f.seq;
        if f.rgb {
            // Rare: swap to the BGRA order the converter expects.
            let mut copy = f.data.clone();
            for px in copy.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            yuv.convert(&copy, f.stride, f.w, f.h, scale);
        } else {
            yuv.convert(&f.data, f.stride, f.w, f.h, scale);
        }
        Ok(true)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.shared.stop.store(true, Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run_stream(fd: OwnedFd, node: u32, shared: &Arc<Shared>) -> Result<(), String> {
    use pw::spa;
    use spa::param::video::{VideoFormat, VideoInfoRaw};
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
    let core = context.connect_fd_rc(fd, None).map_err(|e| e.to_string())?;
    let stream = pw::stream::StreamBox::new(
        &core,
        "ezrdp-screen",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|e| e.to_string())?;

    let sink = shared.clone();
    let _listener = stream
        .add_local_listener_with_user_data(VideoInfoRaw::default())
        .param_changed(|_, info, id, param| {
            if let Some(param) = param
                && id == spa::param::ParamType::Format.as_raw()
            {
                let _ = info.parse(param);
            }
        })
        .process(move |stream, info| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let datas = buffer.datas_mut();
            let Some(d) = datas.first_mut() else { return };
            let size = info.size();
            let (w, h) = (size.width as usize, size.height as usize);
            let chunk = d.chunk();
            let stride = if chunk.stride() > 0 { chunk.stride() as usize } else { w * 4 };
            let offset = chunk.offset() as usize;
            let Some(bytes) = d.data() else { return };
            if w == 0 || h == 0 || bytes.len() < offset + stride * h {
                return; // an empty "nothing changed" buffer, or not mapped
            }
            let rgb = matches!(info.format(), VideoFormat::RGBx | VideoFormat::RGBA);
            let mut f = sink.frame.lock().unwrap();
            f.data.clear();
            f.data.extend_from_slice(&bytes[offset..offset + stride * h]);
            (f.w, f.h, f.stride, f.rgb) = (w, h, stride, rgb);
            f.seq += 1;
            drop(f);
            sink.new_frame.notify_all();
        })
        .register()
        .map_err(|e| e.to_string())?;

    let obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(spa::param::format::FormatProperties::MediaType, Id, spa::param::format::MediaType::Video),
        spa::pod::property!(spa::param::format::FormatProperties::MediaSubtype, Id, spa::param::format::MediaSubtype::Raw),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle { width: 1920, height: 1080 },
            spa::utils::Rectangle { width: 1, height: 1 },
            spa::utils::Rectangle { width: 8192, height: 8192 }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: 60, denom: 1 },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction { num: 240, denom: 1 }
        ),
    );
    let bytes = spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &spa::pod::Value::Object(obj))
        .map_err(|e| format!("{e:?}"))?
        .0
        .into_inner();
    let mut params = [spa::pod::Pod::from_bytes(&bytes).ok_or("bad format")?];
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| e.to_string())?;

    while !shared.stop.load(Relaxed) {
        mainloop.loop_().iterate(pw::loop_::Timeout::Finite(Duration::from_millis(50)));
    }
    Ok(())
}

