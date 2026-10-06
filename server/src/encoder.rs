//! H.264 encoding tuned for low-latency screen sharing.

use crate::convert::Yuv;
use openh264::OpenH264API;
use openh264::encoder::{
    BitRate, Complexity, Encoder, EncoderConfig, FrameRate, FrameType, RateControlMode, UsageType, VuiConfig,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Quality {
    /// Frames wider than this are shrunk by an integer factor.
    pub max_width: usize,
    pub bitrate: u32,
    pub fps: u32,
}

pub fn quality(level: u8) -> Quality {
    match level {
        0 => Quality { max_width: 2560, bitrate: 14_000_000, fps: 60 },
        2 => Quality { max_width: 1280, bitrate: 1_500_000, fps: 30 },
        _ => Quality { max_width: 1920, bitrate: 6_000_000, fps: 60 },
    }
}

pub fn scale_for(width: usize, max_width: usize) -> usize {
    width.div_ceil(max_width).clamp(1, 4)
}

pub struct H264 {
    enc: Encoder,
    out: Vec<u8>,
}

impl H264 {
    pub fn new(q: Quality) -> Result<Self, openh264::Error> {
        let config = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(q.bitrate))
            .max_frame_rate(FrameRate::from_hz(q.fps as f32))
            .complexity(Complexity::Low)
            .skip_frames(true)
            .adaptive_quantization(false)
            .background_detection(false)
            .vui(VuiConfig::bt601());
        let enc = Encoder::with_api_config(OpenH264API::from_source(), config)?;
        Ok(Self { enc, out: Vec::with_capacity(512 * 1024) })
    }

    pub fn force_keyframe(&mut self) {
        self.enc.force_intra_frame();
    }

    /// Changes the target bitrate on the fly (used to back off when the network is slow).
    pub fn set_bitrate(&mut self, bps: u32) {
        #[repr(C)]
        struct BitrateInfo {
            layer: i32,
            bitrate: i32,
        }
        const SPATIAL_LAYER_ALL: i32 = 4;
        const ENCODER_OPTION_BITRATE: i32 = 5;
        let mut info = BitrateInfo { layer: SPATIAL_LAYER_ALL, bitrate: bps as i32 };
        unsafe {
            self.enc.raw_api().set_option(ENCODER_OPTION_BITRATE, (&raw mut info).cast());
        }
    }

    /// Returns `(annex-b bytes, is_keyframe)`, or `None` if the encoder skipped the frame.
    pub fn encode(&mut self, yuv: &Yuv) -> Result<Option<(&[u8], bool)>, openh264::Error> {
        let bs = self.enc.encode(yuv)?;
        let kind = bs.frame_type();
        if matches!(kind, FrameType::Skip | FrameType::Invalid) {
            return Ok(None);
        }
        self.out.clear();
        bs.write_vec(&mut self.out);
        if self.out.is_empty() {
            return Ok(None);
        }
        Ok(Some((&self.out, matches!(kind, FrameType::IDR | FrameType::I))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_keyframe_with_sps() {
        let mut yuv = Yuv::default();
        let img: Vec<u8> = (0..320 * 240 * 4).map(|i| (i * 7 % 251) as u8).collect();
        yuv.convert(&img, 320 * 4, 320, 240, 1);
        let mut enc = H264::new(quality(1)).unwrap();
        let (data, key) = enc.encode(&yuv).unwrap().unwrap();
        assert!(key);
        assert_eq!(&data[..4], &[0, 0, 0, 1]);
        assert_eq!(data[4] & 0x1f, 7, "first NAL should be SPS");
    }

    #[test]
    fn scale_choice() {
        assert_eq!(scale_for(1920, 1920), 1);
        assert_eq!(scale_for(2560, 1920), 2);
        assert_eq!(scale_for(3840, 2560), 2);
        assert_eq!(scale_for(1920, 1280), 2);
    }
}
