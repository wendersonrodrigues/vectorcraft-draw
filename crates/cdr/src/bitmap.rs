//! Bitmaps: the document's `LIST bmpt` holds one `bmp ` chunk per bitmap, its data a record in
//! `content/data/Bitmaps.dat`:
//!
//! | bytes | meaning |
//! |---|---|
//! | 0..4 | the bitmap's id, which bitmap objects name |
//! | 4..8 | 1 |
//! | 8.. | a `UI` block: `UI`, 2 zero bytes, its length (u32), 0, then the offsets (u32, from the block's start) of the colour image and of the mask (0: none) |
//!
//! Each image is an `RI` block: `RI`, its length (u32), 0 (u32), the header's length (u32, 78),
//! then u32s: kind (1 colour, 99 mask), 1, width, height, planes (1), bits per pixel, the bytes
//! of one row (padded to 4), the bytes of all rows, 0, then horizontal and vertical resolution.
//! Rows follow the header bottom row first. A colour image of 24 bits is blue, green, red per
//! pixel; a mask of 8 bits is the pixel's opacity (255 opaque).
//!
//! Only those two depths have been seen; others are refused (the caller lists the bitmap as
//! not imported).

use crate::Error;

/// Most pixels one bitmap may hold.
pub const MAX_PIXELS: u64 = 64 << 20;

/// A decoded bitmap: rows top first, four bytes (red, green, blue, alpha) per pixel.
#[derive(Debug, Clone, PartialEq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// The id a `bmp ` record gives its bitmap.
pub(crate) fn id(record: &[u8]) -> Option<u32> {
    u32_at(record, 0)
}

struct Raster<'a> {
    width: usize,
    height: usize,
    bpp: u32,
    stride: usize,
    rows: &'a [u8],
}

fn raster(b: &[u8], at: usize) -> Result<Raster<'_>, Error> {
    let bad = Error::Malformed("bitmap image header");
    if b.get(at..at.checked_add(2).ok_or(bad)?) != Some(b"RI".as_slice()) {
        return Err(bad);
    }
    let field = |i: usize| u32_at(b, at + 14 + 4 * i).ok_or(bad);
    let header = u32_at(b, at + 10).ok_or(bad)? as usize;
    let (width, height, bpp, stride, size) = (field(2)? as usize, field(3)? as usize, field(5)?, field(6)? as usize, field(7)? as usize);
    if width == 0 || height == 0 {
        return Err(Error::Malformed("a bitmap has no pixels"));
    }
    if (width as u64).saturating_mul(height as u64) > MAX_PIXELS {
        return Err(Error::Limit("a bitmap has too many pixels"));
    }
    let row = (width * bpp as usize).div_ceil(8);
    if stride < row || stride.checked_mul(height).is_none_or(|n| n > size) {
        return Err(Error::Malformed("bitmap row layout"));
    }
    let start = at.checked_add(header).ok_or(bad)?;
    let rows = b.get(start..start.checked_add(size).ok_or(bad)?).ok_or(Error::Malformed("bitmap pixels cut short"))?;
    Ok(Raster { width, height, bpp, stride, rows })
}

/// Decode a `bmp ` record.
pub(crate) fn decode(record: &[u8]) -> Result<Bitmap, Error> {
    let bad = Error::Malformed("bitmap record");
    const UI: usize = 8;
    if record.get(UI..UI + 2) != Some(b"UI".as_slice()) {
        return Err(bad);
    }
    let color_at = u32_at(record, UI + 12).ok_or(bad)? as usize;
    let mask_at = u32_at(record, UI + 16).ok_or(bad)? as usize;
    let color = raster(record, UI.checked_add(color_at).ok_or(bad)?)?;
    if color.bpp != 24 {
        return Err(Error::Unsupported("a bitmap colour depth other than 24 bits"));
    }
    let mask = match mask_at {
        0 => None,
        m => {
            let r = raster(record, UI.checked_add(m).ok_or(bad)?)?;
            if r.bpp != 8 || r.width != color.width || r.height != color.height {
                return Err(Error::Unsupported("a bitmap mask other than 8 bits of the image's size"));
            }
            Some(r)
        }
    };
    let (w, h) = (color.width, color.height);
    let mut rgba = Vec::with_capacity(w * h * 4);
    // Stored bottom row first.
    for y in (0..h).rev() {
        let c = color.rows.get(y * color.stride..y * color.stride + w * 3).ok_or(bad)?;
        let a = match &mask {
            Some(m) => Some(m.rows.get(y * m.stride..y * m.stride + w).ok_or(bad)?),
            None => None,
        };
        for (x, &[b, g, r]) in c.as_chunks::<3>().0.iter().enumerate() {
            let alpha = a.and_then(|a| a.get(x)).copied().unwrap_or(255);
            rgba.extend_from_slice(&[r, g, b, alpha]);
        }
    }
    Ok(Bitmap { width: w as u32, height: h as u32, rgba })
}
