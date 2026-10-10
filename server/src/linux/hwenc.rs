//! No graphics-card encoder on Linux yet: the software encoder is always used.

use crate::convert::Yuv;

pub struct HwEncoder {
    pub name: String,
}

impl HwEncoder {
    pub fn new(_w: usize, _h: usize, _fps: u32, _bitrate: u32) -> Result<Self, String> {
        Err("not available on Linux".into())
    }
    pub fn force_keyframe(&mut self) {}
    pub fn set_bitrate(&mut self, _bps: u32) {}
    pub fn encode(&mut self, _yuv: &Yuv) -> Result<Option<(&[u8], bool)>, String> {
        Err("not available on Linux".into())
    }
}
