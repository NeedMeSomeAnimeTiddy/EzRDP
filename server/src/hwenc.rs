//! H.264 encoding on the graphics card (NVIDIA, AMD or Intel) through Windows Media Foundation.
//! Much faster than encoding on the processor, which cuts delay. If anything here fails,
//! the caller falls back to the software encoder.

#![allow(non_upper_case_globals)]

use crate::convert::Yuv;
use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::VARIANT_TRUE;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Variant::*;
use windows::core::{GUID, HRESULT, Interface, PWSTR};

/// Media Foundation functions, loaded at run time: Windows "N" editions don't have mfplat.dll,
/// and linking to it directly would stop EzRDP from starting there at all.
struct Mf {
    startup: unsafe extern "system" fn(u32, u32) -> HRESULT,
    enum_ex: unsafe extern "system" fn(
        GUID,
        u32,
        *const MFT_REGISTER_TYPE_INFO,
        *const MFT_REGISTER_TYPE_INFO,
        *mut *mut Option<IMFActivate>,
        *mut u32,
    ) -> HRESULT,
    media_type: unsafe extern "system" fn(*mut Option<IMFMediaType>) -> HRESULT,
    memory_buffer: unsafe extern "system" fn(u32, *mut Option<IMFMediaBuffer>) -> HRESULT,
    sample: unsafe extern "system" fn(*mut Option<IMFSample>) -> HRESULT,
}

fn mf() -> Result<&'static Mf, String> {
    static MF: std::sync::OnceLock<Option<Mf>> = std::sync::OnceLock::new();
    MF.get_or_init(|| unsafe {
        use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        use windows::core::{s, w};
        let lib = LoadLibraryW(w!("mfplat.dll")).ok()?;
        Some(Mf {
            startup: std::mem::transmute(GetProcAddress(lib, s!("MFStartup"))?),
            enum_ex: std::mem::transmute(GetProcAddress(lib, s!("MFTEnumEx"))?),
            media_type: std::mem::transmute(GetProcAddress(lib, s!("MFCreateMediaType"))?),
            memory_buffer: std::mem::transmute(GetProcAddress(lib, s!("MFCreateMemoryBuffer"))?),
            sample: std::mem::transmute(GetProcAddress(lib, s!("MFCreateSample"))?),
        })
    })
    .as_ref()
    .ok_or_else(|| "Windows media features not installed".to_string())
}

fn out<T>(f: impl FnOnce(*mut Option<T>) -> HRESULT, what: &str) -> Result<T, String> {
    let mut v = None;
    f(&mut v).ok().map_err(|e| err(what, e))?;
    v.ok_or_else(|| format!("{what}: nothing returned"))
}

fn new_media_type() -> Result<IMFMediaType, String> {
    let mf = mf()?;
    out(|p| unsafe { (mf.media_type)(p) }, "media type")
}

fn new_buffer(len: u32) -> Result<IMFMediaBuffer, String> {
    let mf = mf()?;
    out(|p| unsafe { (mf.memory_buffer)(len, p) }, "buffer")
}

fn new_sample() -> Result<IMFSample, String> {
    let mf = mf()?;
    out(|p| unsafe { (mf.sample)(p) }, "sample")
}

pub struct HwEncoder {
    activate: IMFActivate,
    mft: IMFTransform,
    events: Option<IMFMediaEventGenerator>,
    codec: Option<ICodecAPI>,
    pub name: String,
    w: usize,
    h: usize,
    /// "Ready for a frame" signals received but not used yet (async encoders only).
    need_input: u32,
    provides_samples: bool,
    out_size: u32,
    /// SPS/PPS, added in front of keyframes that don't carry them.
    header: Vec<u8>,
    out: Vec<u8>,
    nv12: Vec<u8>,
    pts: i64,
    frame_time: i64,
}

fn variant_u32(v: u32) -> VARIANT {
    let mut var = VARIANT::default();
    unsafe {
        let inner = &mut *var.Anonymous.Anonymous;
        inner.vt = VT_UI4;
        inner.Anonymous.ulVal = v;
    }
    var
}

fn variant_bool(v: bool) -> VARIANT {
    let mut var = VARIANT::default();
    unsafe {
        let inner = &mut *var.Anonymous.Anonymous;
        inner.vt = VT_BOOL;
        inner.Anonymous.boolVal = if v { VARIANT_TRUE } else { Default::default() };
    }
    var
}

fn err(what: &str, e: windows::core::Error) -> String {
    format!("{what}: {e}")
}

fn has_nal(data: &[u8], kind: u8) -> bool {
    openh264::nal_units(data).any(|n| {
        let skip = n.iter().take_while(|&&b| b == 0).count() + 1;
        n.get(skip).is_some_and(|b| b & 0x1f == kind)
    })
}

impl HwEncoder {
    /// Finds a graphics-card encoder and checks that it really produces video.
    pub fn new(w: usize, h: usize, fps: u32, bitrate: u32) -> Result<Self, String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            (mf()?.startup)(MF_VERSION, MFSTARTUP_LITE).ok().map_err(|e| err("MFStartup", e))?;
        }
        // For testing on machines without one (e.g. Wine), allow the built-in software encoder.
        let flags = if std::env::var_os("EZRDP_MF_SOFTWARE").is_some() {
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER
        } else {
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER
        };
        let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
        let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
        let activates: Vec<IMFActivate> = unsafe {
            let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            (mf()?.enum_ex)(MFT_CATEGORY_VIDEO_ENCODER, flags.0 as u32, &input, &output, &mut list, &mut count)
                .ok()
                .map_err(|e| err("MFTEnumEx", e))?;
            if list.is_null() {
                return Err("no graphics-card video encoder".into());
            }
            let items = std::slice::from_raw_parts_mut(list, count as usize).iter_mut().filter_map(|a| a.take()).collect();
            CoTaskMemFree(Some(list as *const _));
            items
        };
        if activates.is_empty() {
            return Err("no graphics-card video encoder".into());
        }
        let mut errors = Vec::new();
        for act in activates {
            let name = friendly_name(&act);
            match Self::open(act.clone(), name.clone(), w, h, fps, bitrate) {
                Ok(mut enc) => match enc.self_test() {
                    Ok(()) => return Ok(enc),
                    Err(e) => errors.push(format!("{name}: {e}")),
                },
                Err(e) => {
                    unsafe {
                        let _ = act.ShutdownObject();
                    }
                    errors.push(format!("{name}: {e}"));
                }
            }
        }
        Err(errors.join("; "))
    }

    fn open(activate: IMFActivate, name: String, w: usize, h: usize, fps: u32, bitrate: u32) -> Result<Self, String> {
        unsafe {
            let mft: IMFTransform = activate.ActivateObject().map_err(|e| err("activate", e))?;
            let attrs = mft.GetAttributes().ok();
            let is_async = attrs.as_ref().and_then(|a| a.GetUINT32(&MF_TRANSFORM_ASYNC).ok()).unwrap_or(0) != 0;
            if let Some(a) = &attrs {
                if is_async {
                    a.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1).map_err(|e| err("unlock", e))?;
                }
                let _ = a.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            let codec: Option<ICodecAPI> = mft.cast().ok();
            let set_codec = |codec: &Option<ICodecAPI>| {
                if let Some(c) = codec {
                    let _ = c.SetValue(&CODECAPI_AVLowLatencyMode, &variant_bool(true));
                    let _ = c.SetValue(&CODECAPI_AVEncCommonRateControlMode, &variant_u32(eAVEncCommonRateControlMode_CBR.0 as u32));
                    let _ = c.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &variant_u32(bitrate));
                    let _ = c.SetValue(&CODECAPI_AVEncMPVDefaultBPictureCount, &variant_u32(0));
                    // A fresh full picture every 10 seconds, in case anything got lost.
                    let _ = c.SetValue(&CODECAPI_AVEncMPVGOPSize, &variant_u32(fps * 10));
                }
            };
            set_codec(&codec);

            let size = ((w as u64) << 32) | h as u64;
            let rate = ((fps as u64) << 32) | 1;
            let out_type = new_media_type()?;
            let _ = out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video);
            let _ = out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264);
            let _ = out_type.SetUINT32(&MF_MT_AVG_BITRATE, bitrate);
            let _ = out_type.SetUINT64(&MF_MT_FRAME_SIZE, size);
            let _ = out_type.SetUINT64(&MF_MT_FRAME_RATE, rate);
            let _ = out_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1);
            let _ = out_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32);
            let _ = out_type.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32);
            mft.SetOutputType(0, &out_type, 0).map_err(|e| err("output type", e))?;

            let in_type = new_media_type()?;
            let _ = in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video);
            let _ = in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12);
            let _ = in_type.SetUINT64(&MF_MT_FRAME_SIZE, size);
            let _ = in_type.SetUINT64(&MF_MT_FRAME_RATE, rate);
            let _ = in_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1);
            let _ = in_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32);
            mft.SetInputType(0, &in_type, 0).map_err(|e| err("input type", e))?;
            // Some encoders forget settings when the formats are set, so set them again.
            set_codec(&codec);

            let info = mft.GetOutputStreamInfo(0).map_err(|e| err("stream info", e))?;
            let provides_samples = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
            let out_size = info.cbSize.max((w * h * 3 / 2) as u32);
            let header = mft
                .GetOutputCurrentType(0)
                .ok()
                .and_then(|t| {
                    let n = t.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER).ok()?;
                    let mut b = vec![0u8; n as usize];
                    t.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut b, None).ok()?;
                    Some(b)
                })
                .filter(|b| b.starts_with(&[0, 0, 1]) || b.starts_with(&[0, 0, 0, 1]))
                .unwrap_or_default();

            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).map_err(|e| err("begin", e))?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).map_err(|e| err("start", e))?;
            let events = if is_async { Some(mft.cast().map_err(|e| err("events", e))?) } else { None };
            Ok(Self {
                activate,
                mft,
                events,
                codec,
                name,
                w,
                h,
                need_input: 0,
                provides_samples,
                out_size,
                header,
                out: Vec::new(),
                nv12: Vec::new(),
                pts: 0,
                frame_time: 10_000_000 / fps.max(1) as i64,
            })
        }
    }

    /// Encodes two grey frames; the encoder must give back a keyframe.
    fn self_test(&mut self) -> Result<(), String> {
        let mut yuv = Yuv {
            w: self.w,
            h: self.h,
            y: vec![128; self.w * self.h],
            u: vec![128; self.w * self.h / 4],
            v: vec![128; self.w * self.h / 4],
        };
        for _ in 0..3 {
            if let Some((data, key)) = self.encode(&yuv)? {
                if key && has_nal(data, 7) {
                    return Ok(());
                }
                return Err("first picture wasn't a keyframe".into());
            }
            yuv.y[0] ^= 1;
        }
        Err("no output".into())
    }

    pub fn force_keyframe(&mut self) {
        if let Some(c) = &self.codec {
            unsafe {
                let _ = c.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &variant_u32(1));
            }
        }
    }

    pub fn set_bitrate(&mut self, bps: u32) {
        if let Some(c) = &self.codec {
            unsafe {
                let _ = c.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &variant_u32(bps));
            }
        }
    }

    fn sample(&mut self, yuv: &Yuv) -> Result<IMFSample, String> {
        let (w, h) = (self.w, self.h);
        if yuv.w != w || yuv.h != h {
            return Err("picture size changed".into());
        }
        // I420 -> NV12: same brightness plane, colour planes interleaved.
        self.nv12.clear();
        self.nv12.extend_from_slice(&yuv.y);
        self.nv12.extend(yuv.u.iter().zip(&yuv.v).flat_map(|(&u, &v)| [u, v]));
        unsafe {
            let buf = new_buffer(self.nv12.len() as u32)?;
            let mut p = std::ptr::null_mut();
            buf.Lock(&mut p, None, None).map_err(|e| err("lock", e))?;
            std::ptr::copy_nonoverlapping(self.nv12.as_ptr(), p, self.nv12.len());
            let _ = buf.Unlock();
            buf.SetCurrentLength(self.nv12.len() as u32).map_err(|e| err("length", e))?;
            let s = new_sample()?;
            s.AddBuffer(&buf).map_err(|e| err("add buffer", e))?;
            let _ = s.SetSampleTime(self.pts);
            let _ = s.SetSampleDuration(self.frame_time);
            self.pts += self.frame_time;
            Ok(s)
        }
    }

    /// Waits for the next event from an async encoder. `None` = nothing before the deadline.
    fn next_event(&self, deadline: Instant) -> Result<Option<MF_EVENT_TYPE>, String> {
        let events = self.events.as_ref().unwrap();
        let start = Instant::now();
        loop {
            match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(ev) => {
                    let t = unsafe { ev.GetType() }.map_err(|e| err("event", e))?;
                    return Ok(Some(MF_EVENT_TYPE(t as i32)));
                }
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                    // Spin briefly (output usually comes within a few ms), then nap.
                    if start.elapsed() < Duration::from_millis(3) {
                        std::thread::yield_now();
                    } else {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
                Err(e) => return Err(err("event", e)),
            }
        }
    }

    /// Pulls one finished picture into `self.out`. Returns false if none was ready.
    fn pull(&mut self) -> Result<Option<bool>, String> {
        unsafe {
            let sample = if self.provides_samples {
                None
            } else {
                let b = new_buffer(self.out_size)?;
                let s = new_sample()?;
                s.AddBuffer(&b).map_err(|e| err("add buffer", e))?;
                Some(s)
            };
            let mut bufs = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(sample),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            let r = self.mft.ProcessOutput(0, &mut bufs, &mut status);
            let sample = ManuallyDrop::take(&mut bufs[0].pSample);
            drop(ManuallyDrop::take(&mut bufs[0].pEvents));
            match r {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    if let Ok(t) = self.mft.GetOutputAvailableType(0, 0) {
                        let _ = self.mft.SetOutputType(0, &t, 0);
                    }
                    return Ok(None);
                }
                Err(e) => return Err(err("output", e)),
            }
            let Some(sample) = sample else { return Ok(None) };
            let buf = sample.ConvertToContiguousBuffer().map_err(|e| err("contiguous", e))?;
            let mut p = std::ptr::null_mut();
            let mut len = 0u32;
            buf.Lock(&mut p, None, Some(&mut len)).map_err(|e| err("lock", e))?;
            let data = std::slice::from_raw_parts(p, len as usize);
            self.out.clear();
            let key = sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0 || has_nal(data, 5);
            if key && !has_nal(data, 7) {
                self.out.extend_from_slice(&self.header);
            }
            self.out.extend_from_slice(data);
            let _ = buf.Unlock();
            // Remember the first SPS/PPS so later keyframes can carry them too.
            if key && self.header.is_empty() {
                self.header = openh264::nal_units(&self.out)
                    .filter(|n| has_nal(n, 7) || has_nal(n, 8))
                    .flatten()
                    .copied()
                    .collect();
            }
            Ok(Some(key))
        }
    }

    /// Returns `(annex-b bytes, is_keyframe)`, or `None` if no picture came out this time.
    pub fn encode(&mut self, yuv: &Yuv) -> Result<Option<(&[u8], bool)>, String> {
        let sample = self.sample(yuv)?;
        let deadline = Instant::now() + Duration::from_millis(150);
        let mut got = None;
        if self.events.is_some() {
            while self.need_input == 0 {
                match self.next_event(deadline)? {
                    Some(METransformNeedInput) => self.need_input += 1,
                    Some(METransformHaveOutput) => {
                        // A late picture from the last frame. Sending only the newest would leave
                        // a gap, so ask for a fresh full picture instead.
                        if self.pull()?.is_some() {
                            self.force_keyframe();
                        }
                    }
                    Some(_) => {}
                    None => return Ok(None), // encoder is busy; drop this frame
                }
            }
            unsafe { self.mft.ProcessInput(0, &sample, 0) }.map_err(|e| err("input", e))?;
            self.need_input -= 1;
            while got.is_none() {
                match self.next_event(deadline)? {
                    Some(METransformNeedInput) => self.need_input += 1,
                    Some(METransformHaveOutput) => got = self.pull()?,
                    Some(_) => {}
                    None => break,
                }
            }
        } else {
            unsafe { self.mft.ProcessInput(0, &sample, 0) }.map_err(|e| err("input", e))?;
            while let Some(k) = self.pull()? {
                got = Some(k);
                if !self.out.is_empty() {
                    break;
                }
            }
        }
        Ok(got.filter(|_| !self.out.is_empty()).map(|k| (self.out.as_slice(), k)))
    }
}

fn friendly_name(act: &IMFActivate) -> String {
    unsafe {
        let mut p = PWSTR::null();
        let mut len = 0u32;
        if act.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut p, &mut len).is_err() {
            return "video encoder".into();
        }
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

impl Drop for HwEncoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            let _ = self.activate.ShutdownObject();
        }
    }
}
