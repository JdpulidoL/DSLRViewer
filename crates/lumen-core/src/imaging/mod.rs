//! Live-view image analysis: decoding, histogram, focus metric and peaking.

pub mod raw;
pub mod stack;

use anyhow::Context;
use rayon::prelude::*;

/// A decoded live-view frame plus everything the UI derives from it.
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    /// R, G, B and luma histograms, 256 bins each (in that order).
    pub histogram: Vec<u32>,
    /// Mean gradient energy of the central region; higher means sharper.
    pub sharpness: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peaking {
    /// Gradient threshold on a 0..=255 scale.
    pub threshold: f32,
    pub color: [u8; 3],
}

pub const HIST_BINS: usize = 256;

pub fn decode_frame(jpeg: &[u8], peaking: Option<Peaking>) -> anyhow::Result<Frame> {
    let img = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg).context("decoding live view JPEG")?;
    let rgb = img.to_rgb8();
    Ok(analyze(rgb.width() as usize, rgb.height() as usize, rgb.as_raw(), peaking))
}

/// Decode any live-view picture a camera driver can produce.
pub fn decode_live(image: &crate::camera::LiveImage, peaking: Option<Peaking>) -> anyhow::Result<Frame> {
    match image {
        crate::camera::LiveImage::Jpeg(bytes) => decode_frame(bytes, peaking),
        crate::camera::LiveImage::Rgb { width, height, pixels } => Ok(analyze(*width, *height, pixels, peaking)),
    }
}

/// Histogram, sharpness and peaking over packed RGB pixels.
fn analyze(w: usize, h: usize, src: &[u8], peaking: Option<Peaking>) -> Frame {

    let luma: Vec<u8> = src.par_chunks(3).map(|p| ((p[0] as u32 * 54 + p[1] as u32 * 183 + p[2] as u32 * 19) >> 8) as u8).collect();

    let mut histogram = vec![0u32; HIST_BINS * 4];
    for (i, p) in src.chunks_exact(3).enumerate() {
        histogram[p[0] as usize] += 1;
        histogram[HIST_BINS + p[1] as usize] += 1;
        histogram[2 * HIST_BINS + p[2] as usize] += 1;
        histogram[3 * HIST_BINS + luma[i] as usize] += 1;
    }

    let sharpness = central_sharpness(&luma, w, h);
    let mut rgba = vec![255u8; w * h * 4];
    rgba.par_chunks_mut(4).zip(src.par_chunks(3)).for_each(|(d, s)| d[..3].copy_from_slice(s));
    if let Some(p) = peaking {
        apply_peaking(&mut rgba, &luma, w, h, p);
    }
    Frame { width: w, height: h, rgba, histogram, sharpness }
}

fn gradient(luma: &[u8], w: usize, x: usize, y: usize) -> f32 {
    let at = |xx: usize, yy: usize| luma[yy * w + xx] as f32;
    (at(x + 1, y) - at(x - 1, y)).abs() + (at(x, y + 1) - at(x, y - 1)).abs()
}

fn central_sharpness(luma: &[u8], w: usize, h: usize) -> f32 {
    if w < 8 || h < 8 {
        return 0.0;
    }
    let (x0, x1, y0, y1) = (w / 4, w * 3 / 4, h / 4, h * 3 / 4);
    let sum: f64 = (y0..y1).into_par_iter().map(|y| (x0..x1).map(|x| gradient(luma, w, x, y) as f64).sum::<f64>()).sum();
    (sum / ((x1 - x0) * (y1 - y0)) as f64) as f32
}

fn apply_peaking(rgba: &mut [u8], luma: &[u8], w: usize, h: usize, p: Peaking) {
    if w < 3 || h < 3 {
        return;
    }
    rgba.par_chunks_mut(w * 4).enumerate().skip(1).take(h - 2).for_each(|(y, row)| {
        for x in 1..w - 1 {
            if gradient(luma, w, x, y) > p.threshold {
                row[x * 4..x * 4 + 3].copy_from_slice(&p.color);
            }
        }
    });
}

/// Decode JPEG bytes into a small RGBA thumbnail whose longest edge is `max_edge`.
pub fn thumbnail(jpeg: &[u8], max_edge: u32) -> anyhow::Result<(usize, usize, Vec<u8>)> {
    let img = image::load_from_memory(jpeg).context("decoding image")?;
    let t = img.thumbnail(max_edge, max_edge).to_rgba8();
    Ok((t.width() as usize, t.height() as usize, t.into_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageEncoder, codecs::jpeg::JpegEncoder};

    fn jpeg_of(img: &image::RgbImage) -> Vec<u8> {
        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, 95)
            .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)
            .unwrap();
        out
    }

    #[test]
    fn sharp_edges_score_higher_and_get_peaked() {
        let flat = image::RgbImage::from_pixel(64, 64, image::Rgb([120, 120, 120]));
        let mut edgy = flat.clone();
        for y in 0..64 {
            for x in 0..64 {
                if (x / 4) % 2 == 0 {
                    edgy.put_pixel(x, y, image::Rgb([240, 240, 240]));
                }
            }
        }
        let peak = Some(Peaking { threshold: 40.0, color: [0, 255, 0] });
        let f = decode_frame(&jpeg_of(&flat), peak).unwrap();
        let e = decode_frame(&jpeg_of(&edgy), peak).unwrap();
        assert!(e.sharpness > f.sharpness * 10.0);
        let green = |fr: &Frame| fr.rgba.chunks(4).filter(|p| p[..3] == [0, 255, 0]).count();
        assert_eq!(green(&f), 0);
        assert!(green(&e) > 200);
        assert_eq!(e.histogram.iter().take(HIST_BINS).sum::<u32>(), 64 * 64);
    }
}
