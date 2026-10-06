//! Fast BGRA -> I420 (YUV 4:2:0, BT.601 limited range) with optional integer downscale.

use openh264::formats::YUVSource;

#[derive(Default)]
pub struct Yuv {
    pub w: usize,
    pub h: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl YUVSource for Yuv {
    fn dimensions(&self) -> (usize, usize) {
        (self.w, self.h)
    }
    fn strides(&self) -> (usize, usize, usize) {
        (self.w, self.w / 2, self.w / 2)
    }
    fn y(&self) -> &[u8] {
        &self.y
    }
    fn u(&self) -> &[u8] {
        &self.u
    }
    fn v(&self) -> &[u8] {
        &self.v
    }
}

/// Output size for a source size and integer downscale factor (always even, as H.264 needs).
pub fn out_size(sw: usize, sh: usize, scale: usize) -> (usize, usize) {
    ((sw / scale) & !1, (sh / scale) & !1)
}

impl Yuv {
    /// Converts a BGRA image (`pitch` bytes per row) into this buffer, shrinking by `scale` (1..=4).
    pub fn convert(&mut self, src: &[u8], pitch: usize, sw: usize, sh: usize, scale: usize) {
        let (w, h) = out_size(sw, sh, scale);
        if w == 0 || h == 0 {
            return;
        }
        self.w = w;
        self.h = h;
        self.y.resize(w * h, 0);
        self.u.resize(w * h / 4, 0);
        self.v.resize(w * h / 4, 0);

        let pairs = h / 2;
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8);
        let pairs_per = pairs.div_ceil(threads);
        let cw = w / 2;

        std::thread::scope(|s| {
            let ys = self.y.chunks_mut(2 * w * pairs_per);
            let us = self.u.chunks_mut(cw * pairs_per);
            let vs = self.v.chunks_mut(cw * pairs_per);
            for (i, ((y, u), v)) in ys.zip(us).zip(vs).enumerate() {
                let first = i * pairs_per;
                s.spawn(move || match scale {
                    1 => rows::<1>(src, pitch, w, first, y, u, v),
                    2 => rows::<2>(src, pitch, w, first, y, u, v),
                    3 => rows::<3>(src, pitch, w, first, y, u, v),
                    _ => rows::<4>(src, pitch, w, first, y, u, v),
                });
            }
        });
    }
}

#[inline(always)]
fn pixel<const S: usize>(src: &[u8], pitch: usize, x: usize, y: usize) -> (i32, i32, i32) {
    if S == 1 {
        let o = y * pitch + x * 4;
        return (src[o] as i32, src[o + 1] as i32, src[o + 2] as i32);
    }
    let (mut b, mut g, mut r) = (0i32, 0i32, 0i32);
    for dy in 0..S {
        let row = (y * S + dy) * pitch + x * S * 4;
        for dx in 0..S {
            let o = row + dx * 4;
            b += src[o] as i32;
            g += src[o + 1] as i32;
            r += src[o + 2] as i32;
        }
    }
    let n = (S * S) as i32;
    (b / n, g / n, r / n)
}

#[inline(always)]
fn luma(b: i32, g: i32, r: i32) -> u8 {
    (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8
}

fn rows<const S: usize>(src: &[u8], pitch: usize, w: usize, first_pair: usize, y: &mut [u8], u: &mut [u8], v: &mut [u8]) {
    let cw = w / 2;
    let count = u.len() / cw;
    for p in 0..count {
        let sy = (first_pair + p) * 2;
        let (y0, y1) = y[p * 2 * w..(p + 1) * 2 * w].split_at_mut(w);
        let urow = &mut u[p * cw..(p + 1) * cw];
        let vrow = &mut v[p * cw..(p + 1) * cw];
        for cx in 0..cw {
            let x = cx * 2;
            let (b0, g0, r0) = pixel::<S>(src, pitch, x, sy);
            let (b1, g1, r1) = pixel::<S>(src, pitch, x + 1, sy);
            let (b2, g2, r2) = pixel::<S>(src, pitch, x, sy + 1);
            let (b3, g3, r3) = pixel::<S>(src, pitch, x + 1, sy + 1);
            y0[x] = luma(b0, g0, r0);
            y0[x + 1] = luma(b1, g1, r1);
            y1[x] = luma(b2, g2, r2);
            y1[x + 1] = luma(b3, g3, r3);
            let b = (b0 + b1 + b2 + b3 + 2) >> 2;
            let g = (g0 + g1 + g2 + g3 + 2) >> 2;
            let r = (r0 + r1 + r2 + r3 + 2) >> 2;
            urow[cx] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
            vrow[cx] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, bgra: [u8; 4]) -> Vec<u8> {
        bgra.iter().copied().cycle().take(w * h * 4).collect()
    }

    #[test]
    fn white_and_black() {
        let mut yuv = Yuv::default();
        yuv.convert(&solid(64, 36, [255, 255, 255, 255]), 64 * 4, 64, 36, 1);
        assert_eq!((yuv.w, yuv.h), (64, 36));
        assert!(yuv.y.iter().all(|&p| p == 235));
        assert!(yuv.u.iter().all(|&p| p == 128));
        yuv.convert(&solid(64, 36, [0, 0, 0, 255]), 64 * 4, 64, 36, 1);
        assert!(yuv.y.iter().all(|&p| p == 16));
    }

    #[test]
    fn downscale_and_odd_sizes() {
        let mut yuv = Yuv::default();
        // Red, odd size, padded pitch, scale 2.
        let (w, h, pitch) = (101, 59, 101 * 4 + 12);
        let mut img = vec![0u8; pitch * h];
        for row in img.chunks_mut(pitch) {
            for px in row[..w * 4].chunks_mut(4) {
                px.copy_from_slice(&[0, 0, 255, 255]);
            }
        }
        yuv.convert(&img, pitch, w, h, 2);
        assert_eq!((yuv.w, yuv.h), (50, 28));
        assert!(yuv.y.iter().all(|&p| p == 82));
        assert!(yuv.v.iter().all(|&p| p == 240));
    }
}
