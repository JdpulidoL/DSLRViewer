//! Focus stacking: merge a bracketed sequence into one extended-depth image.
//!
//! Every frame gets a per-pixel "focus weight" (smoothed Laplacian energy).
//! Two merge strategies are offered:
//! * `Weighted` blends frames in proportion to `weight^p` (smooth, forgiving).
//! * `Sharpest` picks the best frame per pixel, then feathers the choice.

use std::path::Path;

use anyhow::{Context, bail};
use image::RgbImage;
use rayon::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Weighted,
    Sharpest,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub method: Method,
    /// Radius (px) of the window used to smooth the focus measure.
    pub smoothing: usize,
    /// Radius (px) used to feather the hard selection in `Sharpest` mode.
    pub feather: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self { method: Method::Weighted, smoothing: 3, feather: 4 }
    }
}

pub fn load_sequence(paths: &[impl AsRef<Path>]) -> anyhow::Result<Vec<RgbImage>> {
    paths
        .iter()
        .map(|p| Ok(image::open(p.as_ref()).with_context(|| format!("reading {}", p.as_ref().display()))?.to_rgb8()))
        .collect()
}

/// Merge `frames` (all the same size). `progress` receives values in 0..=1.
pub fn merge(frames: &[RgbImage], opts: Options, progress: &(dyn Fn(f32) + Sync)) -> anyhow::Result<RgbImage> {
    let Some(first) = frames.first() else { bail!("no images to stack") };
    let (w, h) = (first.width() as usize, first.height() as usize);
    if frames.iter().any(|f| f.dimensions() != first.dimensions()) {
        bail!("all images in a stack must have the same dimensions");
    }
    if frames.len() == 1 {
        return Ok(first.clone());
    }
    let n = frames.len();
    let total_steps = (n * 2) as f32;

    // Pass 1: focus measure per frame.
    let mut measures: Vec<Vec<f32>> = Vec::with_capacity(n);
    for (i, f) in frames.iter().enumerate() {
        let m = box_blur_f32(&laplacian_energy(f), w, h, opts.smoothing);
        measures.push(m);
        progress((i + 1) as f32 / total_steps);
    }

    // Per-pixel weights.
    let weights: Vec<Vec<f32>> = match opts.method {
        Method::Weighted => measures
            .iter()
            .map(|m| m.par_iter().map(|v| v.max(1e-6).powi(4)).collect())
            .collect(),
        Method::Sharpest => {
            let best: Vec<usize> = (0..w * h)
                .into_par_iter()
                .map(|p| (0..n).max_by(|a, b| measures[*a][p].total_cmp(&measures[*b][p])).unwrap_or(0))
                .collect();
            (0..n)
                .map(|k| {
                    let mask: Vec<f32> = best.par_iter().map(|b| if *b == k { 1.0 } else { 0.0 }).collect();
                    box_blur_f32(&mask, w, h, opts.feather)
                })
                .collect()
        }
    };

    // Pass 2: normalised blend.
    let mut out = RgbImage::new(w as u32, h as u32);
    out.par_chunks_mut(3).enumerate().for_each(|(p, px)| {
        let sum: f32 = weights.iter().map(|wk| wk[p]).sum::<f32>().max(1e-12);
        let mut acc = [0f32; 3];
        for (k, f) in frames.iter().enumerate() {
            let wk = weights[k][p] / sum;
            let src = &f.as_raw()[p * 3..p * 3 + 3];
            for c in 0..3 {
                acc[c] += src[c] as f32 * wk;
            }
        }
        for c in 0..3 {
            px[c] = acc[c].round().clamp(0.0, 255.0) as u8;
        }
    });
    progress(1.0);
    Ok(out)
}

fn luma(f: &RgbImage) -> Vec<f32> {
    f.as_raw().par_chunks(3).map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32).collect()
}

fn laplacian_energy(f: &RgbImage) -> Vec<f32> {
    let (w, h) = (f.width() as usize, f.height() as usize);
    let l = luma(f);
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        if y == 0 || y + 1 >= h {
            return;
        }
        for x in 1..w - 1 {
            let c = l[y * w + x];
            let lap = l[y * w + x - 1] + l[y * w + x + 1] + l[(y - 1) * w + x] + l[(y + 1) * w + x] - 4.0 * c;
            row[x] = lap.abs();
        }
    });
    out
}

/// Separable box blur with edge clamping.
fn box_blur_f32(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 {
        return src.to_vec();
    }
    let win = (2 * r + 1) as f32;
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let line = &src[y * w..(y + 1) * w];
        let at = |i: isize| line[i.clamp(0, w as isize - 1) as usize];
        let mut sum: f32 = (-(r as isize)..=r as isize).map(at).sum();
        for (x, cell) in row.iter_mut().enumerate() {
            *cell = sum / win;
            sum += at(x as isize + r as isize + 1) - at(x as isize - r as isize);
        }
    });
    let mut out = vec![0f32; w * h];
    let cols: Vec<Vec<f32>> = (0..w)
        .into_par_iter()
        .map(|x| {
            let at = |i: isize| tmp[(i.clamp(0, h as isize - 1) as usize) * w + x];
            let mut sum: f32 = (-(r as isize)..=r as isize).map(at).sum();
            (0..h)
                .map(|y| {
                    let v = sum / win;
                    sum += at(y as isize + r as isize + 1) - at(y as isize - r as isize);
                    v
                })
                .collect()
        })
        .collect();
    for (x, col) in cols.iter().enumerate() {
        for (y, v) in col.iter().enumerate() {
            out[y * w + x] = *v;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    /// Two frames, each sharp (checkerboard) in a different half, flat elsewhere.
    fn halves() -> (RgbImage, RgbImage) {
        let (w, h) = (96u32, 64u32);
        let board = |x: u32, y: u32| if (x / 2 + y / 2).is_multiple_of(2) { Rgb([220, 40, 40]) } else { Rgb([30, 30, 200]) };
        let flat = Rgb([125, 135, 120]);
        let a = RgbImage::from_fn(w, h, |x, y| if x < w / 2 { board(x, y) } else { flat });
        let b = RgbImage::from_fn(w, h, |x, y| if x >= w / 2 { board(x, y) } else { flat });
        (a, b)
    }

    fn energy(img: &RgbImage, x0: u32, x1: u32) -> f32 {
        let e = laplacian_energy(img);
        let w = img.width();
        let mut s = 0.0;
        for y in 2..img.height() - 2 {
            for x in x0..x1 {
                s += e[(y * w + x) as usize];
            }
        }
        s
    }

    #[test]
    fn merge_keeps_detail_from_both_frames() {
        let (a, b) = halves();
        for method in [Method::Weighted, Method::Sharpest] {
            let out = merge(&[a.clone(), b.clone()], Options { method, ..Options::default() }, &|_| {}).unwrap();
            let left = energy(&out, 4, 40) / energy(&a, 4, 40);
            let right = energy(&out, 56, 92) / energy(&b, 56, 92);
            assert!(left > 0.6, "{method:?} left retained {left}");
            assert!(right > 0.6, "{method:?} right retained {right}");
        }
    }

    #[test]
    fn rejects_mismatched_sizes() {
        let a = RgbImage::new(8, 8);
        let b = RgbImage::new(9, 8);
        assert!(merge(&[a, b], Options::default(), &|_| {}).is_err());
        assert!(merge(&[], Options::default(), &|_| {}).is_err());
    }
}
