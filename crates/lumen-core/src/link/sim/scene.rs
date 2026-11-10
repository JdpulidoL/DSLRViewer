//! Procedural optical scene used by the virtual camera.
//!
//! The scene is a stack of depth layers. Rendering blurs each layer by its
//! distance from the focus plane (scaled by aperture), then applies exposure,
//! white balance and sensor noise, so every camera control visibly matters.

use image::{ImageEncoder, RgbImage, codecs::jpeg::JpegEncoder};
use rayon::prelude::*;

/// Full range of the virtual focus ring.
pub const FOCUS_MAX: i32 = 1000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Exposure {
    pub iso: f32,
    pub shutter_s: f32,
    pub f_number: f32,
    pub bias_ev: f32,
    /// Per-channel white balance gain.
    pub wb: [f32; 3],
}

impl Exposure {
    fn gain(&self) -> f32 {
        // Reference: ISO 400, 1/125 s, f/5.6.
        (self.iso / 400.0) * (self.shutter_s / 0.008) * (5.6 / self.f_number).powi(2) * 2f32.powf(self.bias_ev)
    }
}

struct Layer {
    /// Premultiplied RGBA8.
    px: Vec<u8>,
    depth: f32,
}

struct LayerSet {
    w: usize,
    h: usize,
    layers: Vec<Layer>,
    /// Cached blur result for the last (focus, aperture) pair.
    cache: Option<((i32, u32), Vec<f32>)>,
}

pub struct Scene {
    live: LayerSet,
    full: Option<LayerSet>,
    full_size: (usize, usize),
    frame: u32,
}

impl Scene {
    pub fn new(live_size: (usize, usize), full_size: (usize, usize)) -> Self {
        Self { live: build_layers(live_size.0, live_size.1), full: None, full_size, frame: 0 }
    }

    pub fn render_live(&mut self, focus: i32, exp: &Exposure, jpeg_quality: u8) -> Vec<u8> {
        self.frame = self.frame.wrapping_add(1);
        let t = self.frame;
        let img = render(&mut self.live, focus, exp, t);
        let img = overlay_marker(img, t);
        encode_jpeg(&img, jpeg_quality)
    }

    pub fn render_full(&mut self, focus: i32, exp: &Exposure, jpeg_quality: u8) -> (Vec<u8>, (u32, u32)) {
        let (w, h) = self.full_size;
        let set = self.full.get_or_insert_with(|| build_layers(w, h));
        self.frame = self.frame.wrapping_add(7919);
        let img = render(set, focus, exp, self.frame);
        let dims = (img.width(), img.height());
        (encode_jpeg(&img, jpeg_quality), dims)
    }
}

fn encode_jpeg(img: &RgbImage, quality: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.as_raw().len() / 8);
    JpegEncoder::new_with_quality(&mut out, quality)
        .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)
        .expect("in-memory JPEG encoding cannot fail");
    out
}

fn blur_radius(focus: i32, depth: f32, f_number: f32, width: usize) -> usize {
    let dist = (focus as f32 / FOCUS_MAX as f32 - depth).abs();
    let scale = width as f32 / 1920.0;
    let r = dist * 46.0 * (2.8 / f_number) * scale;
    if r < 0.6 { 0 } else { r.round() as usize }
}

fn render(set: &mut LayerSet, focus: i32, exp: &Exposure, frame: u32) -> RgbImage {
    let key = (focus, exp.f_number.to_bits());
    let (w, h) = (set.w, set.h);
    if set.cache.as_ref().map(|c| c.0) != Some(key) {
        let mut acc = vec![0f32; w * h * 4];
        for layer in &set.layers {
            let r = blur_radius(focus, layer.depth, exp.f_number, w);
            let mut px = layer.px.clone();
            if r > 0 {
                box_blur(&mut px, w, h, r);
                box_blur(&mut px, w, h, (r / 2).max(1));
            }
            // Premultiplied "over" compositing.
            acc.par_chunks_mut(4).zip(px.par_chunks(4)).for_each(|(dst, src)| {
                let a = src[3] as f32 / 255.0;
                for c in 0..3 {
                    dst[c] = src[c] as f32 / 255.0 + dst[c] * (1.0 - a);
                }
                dst[3] = a + dst[3] * (1.0 - a);
            });
        }
        set.cache = Some((key, acc));
    }
    let base = &set.cache.as_ref().unwrap().1;
    let gain = exp.gain();
    let noise_amp = 0.012 * (exp.iso / 100.0).sqrt() / gain.clamp(0.5, 2.0).sqrt();
    let mut out = RgbImage::new(w as u32, h as u32);
    out.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let seed = (y as u32).wrapping_mul(73856093) ^ (x as u32).wrapping_mul(19349663) ^ frame.wrapping_mul(83492791);
            let n = (hash(seed) as f32 / u32::MAX as f32 - 0.5) * 2.0 * noise_amp;
            let p = &base[(y * w + x) * 4..];
            for c in 0..3 {
                let v = (p[c] * gain * exp.wb[c] + n).clamp(0.0, 1.0);
                // Simple display gamma.
                row[x * 3 + c] = (v.powf(1.0 / 1.15) * 255.0 + 0.5) as u8;
            }
        }
    });
    out
}

/// A small in-focus-plane marker that moves every frame so the live view is visibly alive.
fn overlay_marker(mut img: RgbImage, frame: u32) -> RgbImage {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let phase = frame as f32 * 0.045;
    let cx = w * (0.5 + 0.42 * phase.sin());
    let cy = h * 0.93;
    let r = h * 0.012;
    let (x0, x1) = ((cx - r - 2.0).max(0.0) as u32, ((cx + r + 2.0).min(w - 1.0)) as u32);
    let (y0, y1) = ((cy - r - 2.0).max(0.0) as u32, ((cy + r + 2.0).min(h - 1.0)) as u32);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            let a = (r + 1.0 - d).clamp(0.0, 1.0);
            if a > 0.0 {
                let p = img.get_pixel_mut(x, y);
                for (c, t) in p.0.iter_mut().zip([255.0, 190.0, 60.0]) {
                    *c = (*c as f32 * (1.0 - a) + t * a) as u8;
                }
            }
        }
    }
    img
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

/// Separable box blur on premultiplied RGBA8 with edge clamping.
fn box_blur(px: &mut [u8], w: usize, h: usize, r: usize) {
    let win = (2 * r + 1) as u32;
    // Horizontal pass, rows in parallel.
    px.par_chunks_mut(w * 4).for_each(|row| {
        let src = row.to_vec();
        for c in 0..4 {
            let at = |i: isize| src[(i.clamp(0, w as isize - 1) as usize) * 4 + c] as u32;
            let mut sum: u32 = (-(r as isize)..=r as isize).map(at).sum();
            for x in 0..w {
                row[x * 4 + c] = (sum / win) as u8;
                sum = sum + at(x as isize + r as isize + 1) - at(x as isize - r as isize);
            }
        }
    });
    // Vertical pass, columns processed through a transposed scratch buffer.
    let mut cols = vec![0u8; px.len()];
    cols.par_chunks_mut(h * 4).enumerate().for_each(|(x, col)| {
        for y in 0..h {
            col[y * 4..y * 4 + 4].copy_from_slice(&px[(y * w + x) * 4..(y * w + x) * 4 + 4]);
        }
        let src = col.to_vec();
        for c in 0..4 {
            let at = |i: isize| src[(i.clamp(0, h as isize - 1) as usize) * 4 + c] as u32;
            let mut sum: u32 = (-(r as isize)..=r as isize).map(at).sum();
            for y in 0..h {
                col[y * 4 + c] = (sum / win) as u8;
                sum = sum + at(y as isize + r as isize + 1) - at(y as isize - r as isize);
            }
        }
    });
    for x in 0..w {
        for y in 0..h {
            px[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&cols[(x * h + y) * 4..(x * h + y) * 4 + 4]);
        }
    }
}

// ---------------------------------------------------------------- drawing

struct Canvas {
    w: usize,
    h: usize,
    s: f32,
    /// Straight (non-premultiplied) RGBA8.
    px: Vec<u8>,
}

#[allow(clippy::too_many_arguments)] // plain drawing primitives; a parameter struct would only obscure call sites
impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        Self { w, h, s: w as f32 / 1920.0, px: vec![0; w * h * 4] }
    }

    fn blend(&mut self, x: usize, y: usize, rgb: [u8; 3], a: f32) {
        if a <= 0.0 || x >= self.w || y >= self.h {
            return;
        }
        let i = (y * self.w + x) * 4;
        let da = self.px[i + 3] as f32 / 255.0;
        let oa = a + da * (1.0 - a);
        for (dst, src) in self.px[i..i + 3].iter_mut().zip(rgb) {
            *dst = ((src as f32 * a + *dst as f32 * da * (1.0 - a)) / oa.max(1e-6)) as u8;
        }
        self.px[i + 3] = (oa * 255.0) as u8;
    }

    fn fill(&mut self, f: impl Fn(f32, f32) -> [u8; 3]) {
        for y in 0..self.h {
            for x in 0..self.w {
                let c = f(x as f32 / self.w as f32, y as f32 / self.h as f32);
                let i = (y * self.w + x) * 4;
                self.px[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
    }

    /// Signed-distance based shape painter over a bounding box (design units).
    fn shape(&mut self, bbox: (f32, f32, f32, f32), rgb: [u8; 3], alpha: f32, sdf: impl Fn(f32, f32) -> f32) {
        let s = self.s;
        let (x0, y0, x1, y1) = (
            (bbox.0 * s).max(0.0) as usize,
            (bbox.1 * s).max(0.0) as usize,
            ((bbox.2 * s) as usize + 1).min(self.w),
            ((bbox.3 * s) as usize + 1).min(self.h),
        );
        for y in y0..y1 {
            for x in x0..x1 {
                let d = sdf(x as f32 / s, y as f32 / s) * s;
                self.blend(x, y, rgb, (0.5 - d).clamp(0.0, 1.0) * alpha);
            }
        }
    }

    fn circle(&mut self, cx: f32, cy: f32, r: f32, rgb: [u8; 3], alpha: f32) {
        self.shape((cx - r - 2.0, cy - r - 2.0, cx + r + 2.0, cy + r + 2.0), rgb, alpha, move |x, y| {
            ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r
        });
    }

    fn ring(&mut self, cx: f32, cy: f32, r: f32, thick: f32, rgb: [u8; 3], alpha: f32) {
        self.shape((cx - r - thick, cy - r - thick, cx + r + thick, cy + r + thick), rgb, alpha, move |x, y| {
            (((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r).abs() - thick / 2.0
        });
    }

    fn round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, rad: f32, rgb: [u8; 3], alpha: f32) {
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        self.shape((x - 2.0, y - 2.0, x + w + 2.0, y + h + 2.0), rgb, alpha, move |px, py| {
            let qx = (px - cx).abs() - (w / 2.0 - rad);
            let qy = (py - cy).abs() - (h / 2.0 - rad);
            (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - rad
        });
    }

    /// Fine parallel stripes clipped to a rectangle: gives focus something to resolve.
    fn stripes(&mut self, x: f32, y: f32, w: f32, h: f32, pitch: f32, vertical: bool, rgb: [u8; 3]) {
        let n = (if vertical { w } else { h } / pitch) as usize;
        for i in (0..n).step_by(2) {
            let o = i as f32 * pitch;
            if vertical {
                self.round_rect(x + o, y, pitch, h, 0.0, rgb, 1.0);
            } else {
                self.round_rect(x, y + o, w, pitch, 0.0, rgb, 1.0);
            }
        }
    }

    fn into_premultiplied(mut self) -> Vec<u8> {
        for p in self.px.chunks_exact_mut(4) {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
        self.px
    }
}

fn build_layers(w: usize, h: usize) -> LayerSet {
    // Far: backdrop with bokeh lights and fine diagonal-free grid lines.
    let mut far = Canvas::new(w, h);
    far.fill(|u, v| {
        let t = v;
        [(18.0 + 30.0 * (1.0 - t) + 10.0 * u) as u8, (28.0 + 48.0 * (1.0 - t)) as u8, (44.0 + 60.0 * (1.0 - t)) as u8]
    });
    for i in 0..26 {
        let s = hash(i * 977 + 13);
        let (x, y) = ((s % 1920) as f32, ((s >> 11) % 780) as f32);
        let r = 10.0 + ((s >> 20) % 34) as f32;
        let tint = [[255, 214, 140], [140, 220, 255], [255, 160, 200], [190, 255, 190]][(s % 4) as usize];
        far.circle(x, y, r, tint, 0.55);
    }
    for i in 0..48 {
        far.stripes(0.0, 790.0 + i as f32 * 10.0, 1920.0, 1.0, 1.0, false, [70, 120, 150]);
    }
    far.stripes(0.0, 780.0, 1920.0, 500.0, 6.0, true, [34, 62, 84]);

    // Mid: resolution chart with rings, bars and colour patches.
    let mut mid = Canvas::new(w, h);
    mid.round_rect(560.0, 190.0, 800.0, 720.0, 36.0, [236, 238, 240], 1.0);
    for k in 1..=6 {
        mid.ring(960.0, 470.0, 40.0 * k as f32, 5.0 - k as f32 * 0.4, [20, 24, 30], 1.0);
    }
    mid.circle(960.0, 470.0, 14.0, [230, 70, 60], 1.0);
    for (i, pitch) in [14.0, 10.0, 7.0, 5.0, 3.5].into_iter().enumerate() {
        let x = 600.0 + i as f32 * 150.0;
        mid.stripes(x, 730.0, 120.0, 100.0, pitch, true, [20, 24, 30]);
        mid.stripes(x, 840.0, 120.0, 50.0, pitch, false, [20, 24, 30]);
    }
    for (i, c) in [[230, 70, 60], [250, 180, 40], [60, 180, 110], [50, 130, 230], [150, 90, 220]].into_iter().enumerate() {
        mid.round_rect(610.0 + i as f32 * 144.0, 226.0, 120.0, 36.0, 10.0, c, 1.0);
    }

    // Near: textured foreground objects on both sides.
    let mut near = Canvas::new(w, h);
    near.round_rect(70.0, 520.0, 380.0, 640.0, 90.0, [214, 150, 70], 1.0);
    near.stripes(110.0, 580.0, 300.0, 520.0, 12.0, false, [120, 70, 28]);
    near.round_rect(1500.0, 640.0, 360.0, 520.0, 70.0, [90, 160, 120], 1.0);
    near.stripes(1540.0, 700.0, 280.0, 400.0, 9.0, true, [38, 84, 60]);
    for k in 0..7 {
        near.circle(1680.0 + (k as f32 - 3.0) * 26.0, 590.0 - (k as f32 - 3.0).abs() * 16.0, 30.0, [248, 210, 90], 1.0);
    }

    let layer = |c: Canvas, depth: f32| Layer { px: c.into_premultiplied(), depth };
    LayerSet { w, h, layers: vec![layer(far, 0.9), layer(mid, 0.5), layer(near, 0.12)], cache: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exp() -> Exposure {
        Exposure { iso: 400.0, shutter_s: 0.008, f_number: 5.6, bias_ev: 0.0, wb: [1.0; 3] }
    }

    fn luma_mean(jpeg: &[u8]) -> f32 {
        let img = image::load_from_memory(jpeg).unwrap().to_luma8();
        img.as_raw().iter().map(|v| *v as f32).sum::<f32>() / img.as_raw().len() as f32
    }

    #[test]
    fn exposure_changes_brightness() {
        let mut s = Scene::new((320, 214), (640, 427));
        let dark = luma_mean(&s.render_live(500, &Exposure { iso: 100.0, ..exp() }, 85));
        let bright = luma_mean(&s.render_live(500, &Exposure { iso: 1600.0, ..exp() }, 85));
        assert!(bright > dark + 10.0, "dark={dark} bright={bright}");
    }

    #[test]
    fn focus_changes_sharpness() {
        let mut s = Scene::new((320, 214), (640, 427));
        let sharp = |jpeg: &[u8]| {
            let g = image::load_from_memory(jpeg).unwrap().to_luma8();
            let (w, h) = g.dimensions();
            let mut e = 0f64;
            for y in 0..h - 1 {
                for x in 0..w - 1 {
                    let p = g.get_pixel(x, y)[0] as f64;
                    e += (p - g.get_pixel(x + 1, y)[0] as f64).abs() + (p - g.get_pixel(x, y + 1)[0] as f64).abs();
                }
            }
            e
        };
        let on_mid = sharp(&s.render_live(500, &exp(), 90));
        let off_mid = sharp(&s.render_live(950, &exp(), 90));
        assert!(on_mid > off_mid, "on={on_mid} off={off_mid}");
    }
}
