//! Preview extraction from TIFF-based RAW files (Canon CR2 and friends).
//!
//! RAW files embed full-size JPEG previews; their offsets are listed in the TIFF IFDs as
//! strip offsets / JPEG interchange tags. Reading those gives something the UI and the
//! stacker can open without a RAW decoder.

const TAG_STRIP_OFFSETS: u16 = 0x0111;
const TAG_STRIP_BYTE_COUNTS: u16 = 0x0117;
const TAG_JPEG_OFFSET: u16 = 0x0201;
const TAG_JPEG_LENGTH: u16 = 0x0202;
const TAG_SUB_IFDS: u16 = 0x014A;
const MAX_IFDS: usize = 16;

struct Reader<'a> {
    data: &'a [u8],
    little: bool,
}

impl Reader<'_> {
    fn u16(&self, at: usize) -> Option<u16> {
        let b: [u8; 2] = self.data.get(at..at + 2)?.try_into().ok()?;
        Some(if self.little { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let b: [u8; 4] = self.data.get(at..at + 4)?.try_into().ok()?;
        Some(if self.little { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    }

    /// First value of an IFD entry (SHORT or LONG, inline in the entry).
    fn first_value(&self, entry: usize) -> Option<u32> {
        match self.u16(entry + 2)? {
            3 => self.u16(entry + 8).map(u32::from),
            _ => self.u32(entry + 8),
        }
    }
}

/// True for baseline/progressive JPEGs. The RAW sensor data itself is a *lossless* JPEG (SOF3), which
/// no ordinary decoder - and not the `image` crate - can open, so it must never be taken as a preview.
fn is_displayable_jpeg(jpeg: &[u8]) -> bool {
    let mut pos = 2;
    while pos + 4 <= jpeg.len() && jpeg[pos] == 0xFF {
        let marker = jpeg[pos + 1];
        if (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker) {
            return matches!(marker, 0xC0..=0xC2);
        }
        let len = u16::from_be_bytes([jpeg[pos + 2], jpeg[pos + 3]]) as usize;
        pos += 2 + len;
    }
    false
}

/// The largest displayable JPEG that the file's IFDs point to.
pub fn embedded_jpeg(data: &[u8]) -> Option<&[u8]> {
    let little = match data.get(..4)? {
        [b'I', b'I', 42, 0] => true,
        [b'M', b'M', 0, 42] => false,
        _ => return None,
    };
    let r = Reader { data, little };
    let mut pending = vec![r.u32(4)? as usize];
    let mut best: Option<(usize, usize)> = None;
    let mut seen = 0;
    while let Some(ifd) = pending.pop() {
        seen += 1;
        if ifd == 0 || seen > MAX_IFDS {
            continue;
        }
        let Some(count) = r.u16(ifd) else { continue };
        let (mut strip_off, mut strip_len, mut jpeg_off, mut jpeg_len) = (None, None, None, None);
        for i in 0..count as usize {
            let entry = ifd + 2 + i * 12;
            let Some(tag) = r.u16(entry) else { break };
            match tag {
                TAG_STRIP_OFFSETS => strip_off = r.first_value(entry),
                TAG_STRIP_BYTE_COUNTS => strip_len = r.first_value(entry),
                TAG_JPEG_OFFSET => jpeg_off = r.first_value(entry),
                TAG_JPEG_LENGTH => jpeg_len = r.first_value(entry),
                TAG_SUB_IFDS => {
                    // Count 1 stores the offset inline; larger counts point at an array of offsets.
                    let n = r.u32(entry + 4).unwrap_or(0) as usize;
                    if n == 1 {
                        pending.extend(r.u32(entry + 8).map(|o| o as usize));
                    } else if let Some(arr) = r.u32(entry + 8) {
                        pending.extend((0..n.min(8)).filter_map(|k| r.u32(arr as usize + k * 4)).map(|o| o as usize));
                    }
                }
                _ => {}
            }
        }
        for (off, len) in [(strip_off, strip_len), (jpeg_off, jpeg_len)] {
            if let (Some(off), Some(len)) = (off, len) {
                let (off, len) = (off as usize, len as usize);
                let looks_like_jpeg = data.get(off..off + 3) == Some(&[0xFF, 0xD8, 0xFF]) && off + len <= data.len() && is_displayable_jpeg(&data[off..off + len]);
                if looks_like_jpeg && best.is_none_or(|(_, l)| len > l) {
                    best = Some((off, len));
                }
            }
        }
        if let Some(next) = r.u32(ifd + 2 + count as usize * 12) {
            pending.push(next as usize);
        }
    }
    best.map(|(o, l)| &data[o..o + l])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal little-endian TIFF with two IFDs: a small thumbnail JPEG and a larger preview JPEG.
    fn tiff() -> (Vec<u8>, Vec<u8>) {
        let sof = |kind: u8| [0xFF, 0xD8, 0xFF, 0xDB, 0, 2, 0xFF, kind, 0, 2];
        let thumb: Vec<u8> = sof(0xC0).into_iter().chain([0xFF, 0xD9]).collect();
        let preview: Vec<u8> = sof(0xC2).into_iter().chain((0..40).map(|i| i as u8)).chain([0xFF, 0xD9]).collect();
        let mut d = vec![b'I', b'I', 42, 0, 8, 0, 0, 0];
        let ifd = |entries: &[(u16, u16, u32)], next: u32| {
            let mut v = (entries.len() as u16).to_le_bytes().to_vec();
            for (tag, ty, val) in entries {
                v.extend(tag.to_le_bytes());
                v.extend(ty.to_le_bytes());
                v.extend(1u32.to_le_bytes());
                v.extend(val.to_le_bytes());
            }
            v.extend(next.to_le_bytes());
            v
        };
        // Layout: header (8) | IFD0 (2+24+4=30) | IFD1 (30) | thumb | preview
        let (ifd0_at, ifd1_at) = (8u32, 38u32);
        let data_at = 68u32;
        d.extend(ifd(&[(TAG_STRIP_OFFSETS, 4, data_at + thumb.len() as u32), (TAG_STRIP_BYTE_COUNTS, 4, preview.len() as u32)], ifd1_at));
        d.extend(ifd(&[(TAG_JPEG_OFFSET, 4, data_at), (TAG_JPEG_LENGTH, 4, thumb.len() as u32)], 0));
        assert_eq!((d.len() as u32, ifd0_at), (data_at, 8));
        d.extend(&thumb);
        d.extend(&preview);
        (d, preview)
    }

    #[test]
    fn picks_the_largest_embedded_jpeg() {
        let (file, preview) = tiff();
        assert_eq!(embedded_jpeg(&file).unwrap(), &preview[..]);
    }

    #[test]
    fn lossless_raw_jpeg_is_not_a_preview() {
        let lossless = [0xFF, 0xD8, 0xFF, 0xDB, 0, 2, 0xFF, 0xC3, 0, 2, 9, 9];
        let baseline = [0xFF, 0xD8, 0xFF, 0xDB, 0, 2, 0xFF, 0xC0, 0, 2, 9, 9];
        assert!(!is_displayable_jpeg(&lossless));
        assert!(is_displayable_jpeg(&baseline));
    }

    #[test]
    fn rejects_non_tiff_and_bad_offsets() {
        assert!(embedded_jpeg(b"not a tiff at all").is_none());
        let (mut file, _) = tiff();
        file.truncate(60); // offsets now point past the end
        assert!(embedded_jpeg(&file).is_none());
    }
}
