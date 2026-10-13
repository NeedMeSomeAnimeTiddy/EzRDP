//! Screen capture on Linux (X11). Reads the screen through shared memory (MIT-SHM), falling
//! back to plain GetImage. Like on Windows, this only reads: display settings are never changed.

use crate::convert::Yuv;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shm::ConnectionExt as _;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, Window};
use x11rb::rust_connection::RustConnection;

#[derive(Clone, Debug)]
pub struct Monitor {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub primary: bool,
    /// Wayland: the PipeWire stream of this screen (0 on X11).
    pub stream: u32,
}

pub enum CapError {
    /// The screen changed size or the X server went away. Reopen.
    Lost,
    Other(String),
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn connect() -> Result<(RustConnection, Window), String> {
    let (conn, screen) = x11rb::connect(None).map_err(|e| format!("can't open the X display ({e})"))?;
    let root = conn.setup().roots[screen].root;
    Ok((conn, root))
}

fn list(conn: &RustConnection, root: Window) -> Vec<Monitor> {
    let mut out = Vec::new();
    if let Ok(reply) = conn.randr_get_monitors(root, true).map_err(err).and_then(|c| c.reply().map_err(err)) {
        for (i, m) in reply.monitors.iter().enumerate() {
            let name = conn
                .get_atom_name(m.name)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| format!("Display {}", i + 1));
            out.push(Monitor {
                name,
                x: m.x as i32,
                y: m.y as i32,
                w: m.width as i32,
                h: m.height as i32,
                primary: m.primary,
                stream: 0,
            });
        }
    }
    if out.is_empty()
        && let Ok(g) = conn.get_geometry(root).map_err(err).and_then(|c| c.reply().map_err(err))
    {
        out.push(Monitor { name: "Display 1".into(), x: 0, y: 0, w: g.width as i32, h: g.height as i32, primary: true, stream: 0 });
    }
    if !out.iter().any(|m| m.primary)
        && let Some(m) = out.first_mut()
    {
        m.primary = true;
    }
    out
}

pub fn monitors() -> Vec<Monitor> {
    if super::wayland::active() {
        return super::wayland::monitors();
    }
    match connect() {
        Ok((conn, root)) => list(&conn, root),
        Err(_) => Vec::new(),
    }
}

/// A System V shared memory block that the X server writes the picture into.
struct Shm {
    addr: *mut u8,
    size: usize,
    seg: u32,
}

impl Shm {
    fn new(conn: &RustConnection, size: usize) -> Result<Self, String> {
        unsafe {
            let id = libc::shmget(libc::IPC_PRIVATE, size, libc::IPC_CREAT | 0o600);
            if id < 0 {
                return Err("shmget failed".into());
            }
            let addr = libc::shmat(id, std::ptr::null(), 0);
            let mut shm = Shm { addr: std::ptr::null_mut(), size, seg: 0 };
            let attached = (addr as isize != -1)
                .then(|| {
                    shm.addr = addr as *mut u8;
                    let seg = conn.generate_id().ok()?;
                    conn.shm_attach(seg, id as u32, false).ok()?.check().ok()?;
                    Some(seg)
                })
                .flatten();
            // Both sides are attached (or gave up), so mark it for removal: the memory goes
            // away by itself once nobody uses it, even if EzRDP crashes.
            libc::shmctl(id, libc::IPC_RMID, std::ptr::null_mut());
            shm.seg = attached.ok_or("the X server can't use shared memory")?;
            Ok(shm)
        }
    }

    fn bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.addr, self.size) }
    }
}

impl Drop for Shm {
    fn drop(&mut self) {
        unsafe {
            if !self.addr.is_null() {
                libc::shmdt(self.addr as *const libc::c_void);
            }
        }
    }
}

pub struct XSource {
    monitor: Monitor,
    conn: RustConnection,
    root: Window,
    shm: Option<Shm>,
    shm_works: bool,
    /// The newest picture and the one before, to tell whether anything changed.
    cur: Vec<u8>,
    prev: Vec<u8>,
    last_grab: Instant,
    checked: Instant,
}

/// How often the screen is read (X11 has no "something changed" signal we can wait on cheaply).
const GRAB_GAP: Duration = Duration::from_millis(16);

impl XSource {
    pub fn open(index: usize) -> Result<Self, String> {
        let (conn, root) = connect()?;
        let mons = list(&conn, root);
        let monitor = mons.get(index).or(mons.first()).cloned().ok_or("no screens found")?;
        let size = monitor.w as usize * monitor.h as usize * 4;
        let shm = match Shm::new(&conn, size) {
            Ok(s) => Some(s),
            Err(e) => {
                crate::log!("Shared-memory capture unavailable ({e}); using slower capture");
                None
            }
        };
        Ok(Self { monitor, conn, root, shm, shm_works: false, cur: Vec::new(), prev: Vec::new(), last_grab: Instant::now() - GRAB_GAP, checked: Instant::now() })
    }

    pub fn monitor(&self) -> &Monitor {
        &self.monitor
    }

    fn grab(&mut self) -> Result<(), CapError> {
        let m = &self.monitor;
        let (x, y, w, h) = (m.x as i16, m.y as i16, m.w as u16, m.h as u16);
        if let Some(shm) = &self.shm {
            let reply = self
                .conn
                .shm_get_image(self.root, x, y, w, h, !0, ImageFormat::Z_PIXMAP.into(), shm.seg, 0)
                .map_err(|e| CapError::Other(err(e)))?
                .reply();
            match reply {
                Ok(r) if r.depth >= 24 => {
                    self.shm_works = true;
                    self.cur.clear();
                    self.cur.extend_from_slice(shm.bytes());
                    return Ok(());
                }
                Ok(r) => return Err(CapError::Other(format!("unsupported screen colour depth {}", r.depth))),
                // Usually the screen changed size, so the monitor no longer fits.
                Err(_) if self.shm_works => return Err(CapError::Lost),
                Err(e) => {
                    crate::log!("Shared-memory capture failed ({e}); using slower capture");
                    self.shm = None;
                }
            }
        }
        let r = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, self.root, x, y, w, h, !0)
            .map_err(|e| CapError::Other(err(e)))?
            .reply()
            .map_err(|_| CapError::Lost)?;
        if r.depth < 24 || r.data.len() < w as usize * h as usize * 4 {
            return Err(CapError::Other(format!("unsupported screen colour depth {}", r.depth)));
        }
        self.cur = r.data;
        Ok(())
    }

    /// Waits up to `timeout_ms` for the next picture. `Ok(true)` means the screen changed and
    /// `yuv` holds the new picture.
    pub fn frame(&mut self, timeout_ms: u32, yuv: &mut Yuv, scale: usize) -> Result<bool, CapError> {
        let wait = GRAB_GAP.saturating_sub(self.last_grab.elapsed());
        if wait > Duration::from_millis(timeout_ms as u64) {
            std::thread::sleep(Duration::from_millis(timeout_ms as u64));
            return Ok(false);
        }
        std::thread::sleep(wait);
        self.last_grab = Instant::now();

        // Now and then, check the screen layout hasn't changed under us.
        if self.checked.elapsed() > Duration::from_secs(2) {
            self.checked = Instant::now();
            let now = list(&self.conn, self.root);
            let m = &self.monitor;
            if !now.iter().any(|n| (n.x, n.y, n.w, n.h) == (m.x, m.y, m.w, m.h)) {
                return Err(CapError::Lost);
            }
        }

        std::mem::swap(&mut self.cur, &mut self.prev);
        self.grab()?;
        if self.cur == self.prev {
            return Ok(false);
        }
        let m = &self.monitor;
        yuv.convert(&self.cur, m.w as usize * 4, m.w as usize, m.h as usize, scale);
        Ok(true)
    }
}

/// X11 or Wayland capture, whichever this desktop uses.
pub enum Source {
    X(XSource),
    Wayland(super::wayland::Capture),
}

impl Source {
    pub fn open(index: usize) -> Result<Self, String> {
        if super::wayland::active() {
            return super::wayland::Capture::open(index).map(Source::Wayland);
        }
        XSource::open(index).map(Source::X)
    }

    pub fn monitor(&self) -> &Monitor {
        match self {
            Source::X(s) => s.monitor(),
            Source::Wayland(s) => s.monitor(),
        }
    }

    pub fn frame(&mut self, timeout_ms: u32, yuv: &mut Yuv, scale: usize) -> Result<bool, CapError> {
        match self {
            Source::X(s) => s.frame(timeout_ms, yuv, scale),
            Source::Wayland(s) => s.frame(timeout_ms, yuv, scale),
        }
    }
}
