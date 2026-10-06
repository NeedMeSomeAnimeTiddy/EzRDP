//! Screen capture with DXGI Desktop Duplication. This only reads what's on screen:
//! it never changes display modes, so monitors stay on and untouched.

use crate::convert::Yuv;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::core::Interface;

#[derive(Clone, Debug)]
pub struct Monitor {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub primary: bool,
}

pub enum CapError {
    /// Duplication was lost (resolution change, UAC prompt, lock screen...). Recreate.
    Lost,
    Other(windows::core::Error),
}

impl From<windows::core::Error> for CapError {
    fn from(e: windows::core::Error) -> Self {
        if e.code() == DXGI_ERROR_ACCESS_LOST || e.code() == DXGI_ERROR_INVALID_CALL {
            CapError::Lost
        } else {
            CapError::Other(e)
        }
    }
}

/// All attached monitors, in a stable order (adapter, then output).
fn outputs() -> windows::core::Result<Vec<(IDXGIAdapter1, IDXGIOutput1, Monitor)>> {
    let mut list = Vec::new();
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut a = 0;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            a += 1;
            let mut o = 0;
            while let Ok(output) = adapter.EnumOutputs(o) {
                o += 1;
                let desc = output.GetDesc()?;
                if !desc.AttachedToDesktop.as_bool() {
                    continue;
                }
                let r = desc.DesktopCoordinates;
                let n = list.len() + 1;
                let mon = Monitor {
                    name: format!("Display {n}"),
                    x: r.left,
                    y: r.top,
                    w: r.right - r.left,
                    h: r.bottom - r.top,
                    primary: r.left == 0 && r.top == 0,
                };
                list.push((adapter.clone(), output.cast::<IDXGIOutput1>()?, mon));
            }
        }
    }
    Ok(list)
}

pub fn monitors() -> Vec<Monitor> {
    outputs().map(|l| l.into_iter().map(|(_, _, m)| m).collect()).unwrap_or_default()
}

pub struct Capturer {
    pub monitor: Monitor,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    rotation: DXGI_MODE_ROTATION,
    rotated: Vec<u8>,
    /// The first frame after starting always holds the full screen, even if nothing changed.
    fresh: bool,
}

impl Capturer {
    pub fn new(index: usize) -> windows::core::Result<Self> {
        let mut list = outputs()?;
        if list.is_empty() {
            return Err(windows::core::Error::from(DXGI_ERROR_NOT_FOUND));
        }
        let (adapter, output, monitor) = list.swap_remove(index.min(list.len() - 1));
        unsafe {
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.unwrap();
            let dup = output.DuplicateOutput(&device)?;
            let rotation = dup.GetDesc().Rotation;
            Ok(Self {
                monitor,
                device,
                context: context.unwrap(),
                dup,
                staging: None,
                rotation,
                rotated: Vec::new(),
                fresh: true,
            })
        }
    }

    /// Waits up to `timeout_ms` for a screen change and converts it into `yuv`.
    /// Returns `Ok(false)` if nothing on screen changed.
    pub fn frame(&mut self, timeout_ms: u32, yuv: &mut Yuv, scale: usize) -> Result<bool, CapError> {
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res = None;
            if let Err(e) = self.dup.AcquireNextFrame(timeout_ms, &mut info, &mut res) {
                if e.code() == DXGI_ERROR_WAIT_TIMEOUT {
                    return Ok(false);
                }
                return Err(e.into());
            }
            let result = self.copy(info, res, yuv, scale);
            let _ = self.dup.ReleaseFrame();
            result
        }
    }

    unsafe fn copy(
        &mut self,
        info: DXGI_OUTDUPL_FRAME_INFO,
        res: Option<IDXGIResource>,
        yuv: &mut Yuv,
        scale: usize,
    ) -> Result<bool, CapError> {
        // Only the mouse moved: nothing new to encode.
        if info.LastPresentTime == 0 && !self.fresh {
            return Ok(false);
        }
        self.fresh = false;
        let Some(res) = res else { return Ok(false) };
        unsafe {
            let tex: ID3D11Texture2D = res.cast()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            tex.GetDesc(&mut desc);

            if !matches!(self.staging, Some((_, w, h)) if w == desc.Width && h == desc.Height) {
                let sd = D3D11_TEXTURE2D_DESC {
                    Width: desc.Width,
                    Height: desc.Height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: desc.Format,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                };
                let mut t = None;
                self.device.CreateTexture2D(&sd, None, Some(&mut t))?;
                self.staging = Some((t.unwrap(), desc.Width, desc.Height));
            }
            let staging = &self.staging.as_ref().unwrap().0;
            self.context.CopyResource(staging, &tex);

            let mut map = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut map))?;
            let pitch = map.RowPitch as usize;
            let (w, h) = (desc.Width as usize, desc.Height as usize);
            let data = std::slice::from_raw_parts(map.pData as *const u8, pitch * h);
            match rotate(self.rotation, data, pitch, w, h, &mut self.rotated) {
                Some((dw, dh)) => yuv.convert(&self.rotated, dw * 4, dw, dh, scale),
                None => yuv.convert(data, pitch, w, h, scale),
            }
            self.context.Unmap(staging, 0);
        }
        Ok(true)
    }
}

/// Turns a rotated (portrait / flipped) monitor's image upright. Returns the upright size,
/// or `None` if the monitor isn't rotated.
fn rotate(rot: DXGI_MODE_ROTATION, src: &[u8], pitch: usize, w: usize, h: usize, out: &mut Vec<u8>) -> Option<(usize, usize)> {
    let (dw, dh) = match rot {
        DXGI_MODE_ROTATION_ROTATE90 | DXGI_MODE_ROTATION_ROTATE270 => (h, w),
        DXGI_MODE_ROTATION_ROTATE180 => (w, h),
        _ => return None,
    };
    out.resize(dw * dh * 4, 0);
    for y in 0..dh {
        for x in 0..dw {
            let (sx, sy) = match rot {
                DXGI_MODE_ROTATION_ROTATE90 => (dh - 1 - y, x),
                DXGI_MODE_ROTATION_ROTATE270 => (y, dw - 1 - x),
                _ => (dw - 1 - x, dh - 1 - y),
            };
            let s = sy * pitch + sx * 4;
            let d = (y * dw + x) * 4;
            out[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    Some((dw, dh))
}

/// Backup capture using plain GDI screen copies. Slower than Desktop Duplication, but works
/// where that isn't available (some laptops with two GPUs, virtual machines, remote sessions).
pub struct GdiCapturer {
    pub monitor: Monitor,
    screen: windows::Win32::Graphics::Gdi::HDC,
    mem: windows::Win32::Graphics::Gdi::HDC,
    bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
    old: windows::Win32::Graphics::Gdi::HGDIOBJ,
    bits: *const u8,
    prev: Vec<u8>,
}

// The GDI handles are only ever used from the video thread that owns this.
unsafe impl Send for GdiCapturer {}

impl GdiCapturer {
    pub fn new(index: usize) -> windows::core::Result<Self> {
        use windows::Win32::Graphics::Gdi::*;
        let list = monitors();
        let monitor = list
            .get(index.min(list.len().saturating_sub(1)))
            .cloned()
            .ok_or_else(|| windows::core::Error::from(DXGI_ERROR_NOT_FOUND))?;
        unsafe {
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: monitor.w,
                    biHeight: -monitor.h, // top-down rows
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(Some(mem), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) => b,
                Err(e) => {
                    let _ = DeleteDC(mem);
                    ReleaseDC(None, screen);
                    return Err(e);
                }
            };
            let old = SelectObject(mem, bitmap.into());
            Ok(Self { monitor, screen, mem, bitmap, old, bits: bits as *const u8, prev: Vec::new() })
        }
    }

    /// Copies the screen; returns `Ok(false)` if it looks the same as last time.
    pub fn frame(&mut self, yuv: &mut Yuv, scale: usize) -> Result<bool, CapError> {
        use windows::Win32::Graphics::Gdi::*;
        let (w, h) = (self.monitor.w as usize, self.monitor.h as usize);
        unsafe {
            BitBlt(
                self.mem,
                0,
                0,
                w as i32,
                h as i32,
                Some(self.screen),
                self.monitor.x,
                self.monitor.y,
                SRCCOPY | CAPTUREBLT,
            )
            .map_err(|_| CapError::Lost)?;
            let data = std::slice::from_raw_parts(self.bits, w * h * 4);
            if self.prev.as_slice() == data {
                std::thread::sleep(std::time::Duration::from_millis(16));
                return Ok(false);
            }
            self.prev.clear();
            self.prev.extend_from_slice(data);
            yuv.convert(data, w * 4, w, h, scale);
        }
        Ok(true)
    }
}

impl Drop for GdiCapturer {
    fn drop(&mut self) {
        use windows::Win32::Graphics::Gdi::*;
        unsafe {
            SelectObject(self.mem, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.mem);
            ReleaseDC(None, self.screen);
        }
    }
}

/// Whichever capture method is working.
pub enum Source {
    Fast(Capturer),
    Backup(GdiCapturer, std::time::Instant, usize),
}

impl Source {
    /// Prefers Desktop Duplication; falls back to GDI if it's unavailable.
    pub fn open(index: usize) -> Result<Self, String> {
        match Capturer::new(index) {
            Ok(c) => Ok(Source::Fast(c)),
            Err(e) => match GdiCapturer::new(index) {
                Ok(g) => {
                    static NOTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                    if !NOTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        crate::log!("Fast screen capture unavailable ({e}); using backup capture.");
                    }
                    Ok(Source::Backup(g, std::time::Instant::now(), index))
                }
                Err(e2) => Err(format!("{e}; backup: {e2}")),
            },
        }
    }

    pub fn monitor(&self) -> &Monitor {
        match self {
            Source::Fast(c) => &c.monitor,
            Source::Backup(g, _, _) => &g.monitor,
        }
    }

    pub fn frame(&mut self, timeout_ms: u32, yuv: &mut Yuv, scale: usize) -> Result<bool, CapError> {
        match self {
            Source::Fast(c) => c.frame(timeout_ms, yuv, scale),
            Source::Backup(g, since, index) => {
                // Every few seconds, see if fast capture works again (e.g. after the lock screen).
                if since.elapsed() > std::time::Duration::from_secs(5) {
                    *since = std::time::Instant::now();
                    if Capturer::new(*index).is_ok() {
                        return Err(CapError::Lost);
                    }
                }
                g.frame(yuv, scale)
            }
        }
    }
}
