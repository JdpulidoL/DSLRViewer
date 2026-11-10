//! Live-view payloads.
//!
//! * Nikon and Canon (photo mode) deliver a JPEG somewhere inside the payload.
//! * A Canon EOS in video mode delivers a record list whose type-9 record holds an
//!   uncompressed 4:2:2 frame (verified on a Rebel T5i: 720x480, UYVY byte order,
//!   full-range 8-bit luma, chroma stored as *signed* bytes centred on 0).

use rayon::prelude::*;

/// One live-view picture, ready for analysis.
#[derive(Debug, Clone)]
pub enum LiveImage {
    Jpeg(Vec<u8>),
    Rgb { width: usize, height: usize, pixels: Vec<u8> },
}

const RECORD_YUV: u32 = 9;
/// Bytes between the start of a YUV record and its first pixel.
const YUV_HEADER: usize = 40;

fn le32(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

/// Interpret a Canon `GetViewFinderData` payload.
pub fn parse_canon(data: &[u8]) -> Option<LiveImage> {
    parse_yuv_record(data).or_else(|| extract_jpeg(data).map(|j| LiveImage::Jpeg(j.to_vec())))
}

/// Locate the JPEG inside a vendor payload (which carries a header of varying length).
pub fn extract_jpeg(data: &[u8]) -> Option<&[u8]> {
    let start = data.windows(3).position(|w| w == [0xFF, 0xD8, 0xFF])?;
    let end = data.windows(2).rposition(|w| w == [0xFF, 0xD9]).map(|p| p + 2)?;
    (end > start).then(|| &data[start..end])
}

fn parse_yuv_record(data: &[u8]) -> Option<LiveImage> {
    let mut at = 0;
    while let (Some(size), Some(kind)) = (le32(data, at), le32(data, at + 4)) {
        let size = size as usize;
        if size < 8 || at + size > data.len() {
            return None;
        }
        if kind == RECORD_YUV {
            let (w, h) = (le32(data, at + 12)? as usize, le32(data, at + 16)? as usize);
            let start = at + YUV_HEADER;
            let len = w.checked_mul(h)?.checked_mul(2)?;
            if w == 0 || h == 0 || w % 2 != 0 || w > 8192 || h > 8192 || start + len > at + size {
                return None;
            }
            return Some(LiveImage::Rgb { width: w, height: h, pixels: uyvy_to_rgb(&data[start..start + len]) });
        }
        at += size;
    }
    None
}

/// UYVY (U Y0 V Y1) with signed chroma to packed RGB, BT.601 full range.
pub fn uyvy_to_rgb(src: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; src.len() / 4 * 6];
    out.par_chunks_mut(6).zip(src.par_chunks(4)).for_each(|(dst, px)| {
        let u = px[0] as i8 as f32;
        let v = px[2] as i8 as f32;
        for (k, y) in [px[1], px[3]].into_iter().enumerate() {
            let y = y as f32;
            let rgb = [y + 1.402 * v, y - 0.344_136 * u - 0.714_136 * v, y + 1.772 * u];
            for (c, val) in rgb.into_iter().enumerate() {
                dst[k * 3 + c] = val.clamp(0.0, 255.0) as u8;
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yuv_payload(w: usize, h: usize, fill: [u8; 4]) -> Vec<u8> {
        let mut data = Vec::new();
        // Leading header record (type 0xFFFFFFFF), as the camera sends it.
        data.extend_from_slice(&44u32.to_le_bytes());
        data.extend_from_slice(&u32::MAX.to_le_bytes());
        data.extend_from_slice(&[0; 36]);
        let size = YUV_HEADER + w * h * 2;
        data.extend_from_slice(&(size as u32).to_le_bytes());
        data.extend_from_slice(&RECORD_YUV.to_le_bytes());
        data.extend_from_slice(&0x1000_0422u32.to_le_bytes());
        data.extend_from_slice(&(w as u32).to_le_bytes());
        data.extend_from_slice(&(h as u32).to_le_bytes());
        data.extend_from_slice(&[0; 8]);
        data.extend_from_slice(&(w as u32).to_le_bytes());
        data.extend_from_slice(&(h as u32).to_le_bytes());
        data.extend_from_slice(&0x0002_0003u32.to_le_bytes());
        for _ in 0..w * h / 2 {
            data.extend_from_slice(&fill);
        }
        data.extend_from_slice(&[0u8; 12]); // trailing metadata
        data
    }

    #[test]
    fn decodes_signed_chroma_uyvy() {
        // Neutral chroma, mid luma -> grey.
        let LiveImage::Rgb { width, height, pixels } = parse_canon(&yuv_payload(4, 2, [0, 100, 0, 100])).unwrap() else { panic!() };
        assert_eq!((width, height, pixels.len()), (4, 2, 24));
        assert!(pixels.iter().all(|p| *p == 100));
        // Strong positive V (signed +60) pushes red up and green down.
        let LiveImage::Rgb { pixels, .. } = parse_canon(&yuv_payload(2, 2, [0, 100, 60, 100])).unwrap() else { panic!() };
        assert!(pixels[0] > 150 && pixels[1] < 100);
    }

    #[test]
    fn falls_back_to_jpeg_and_rejects_garbage() {
        let mut data = vec![0u8; 40];
        data.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 0xFF, 0xD9]);
        assert!(matches!(parse_canon(&data), Some(LiveImage::Jpeg(j)) if j.len() == 9));
        assert!(parse_canon(&[1, 2, 3, 4]).is_none());
        // A truncated YUV record must not panic or be accepted.
        let mut bad = yuv_payload(4, 2, [0; 4]);
        bad.truncate(80);
        assert!(parse_yuv_record(&bad).is_none());
    }

    #[test]
    fn finds_jpeg_after_vendor_header() {
        let mut data = vec![0u8; 40];
        data.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 0xFF, 0xD9, 0, 0, 0]);
        assert_eq!(extract_jpeg(&data).unwrap(), &[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 0xFF, 0xD9]);
        assert!(extract_jpeg(&[1, 2, 3]).is_none());
    }
}
