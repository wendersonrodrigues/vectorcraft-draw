//! Argument tables (`loda` chunks, also layers' and pages'): a header, a table of argument
//! offsets and a table of argument kinds, then the arguments.
//!
//! | bytes | meaning |
//! |---|---|
//! | 0..4 | length of the whole table |
//! | 4..8 | number of arguments `n` |
//! | 8..12 | where the `n` offsets (u32) start |
//! | 12..16 | where the `n` kinds (u32) start |
//! | 16..20 | the object's type |
//!
//! The kinds are listed in the reverse order of the offsets: the last kind is the first offset's.
//! An argument runs from its offset to the next offset or table, whichever is nearest.

use crate::Error;

/// Most arguments a table may list.
const MAX_ARGS: usize = 4096;

pub(crate) struct Args<'a> {
    pub(crate) object: u32,
    args: Vec<(u32, &'a [u8])>,
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

impl<'a> Args<'a> {
    pub(crate) fn parse(d: &'a [u8]) -> Result<Self, Error> {
        let bad = Error::Malformed("argument table");
        let total = (u32_at(d, 0).ok_or(bad)? as usize).min(d.len());
        let n = u32_at(d, 4).ok_or(bad)? as usize;
        if n > MAX_ARGS {
            return Err(Error::Limit("an argument table is too long"));
        }
        let (po, pt) = (u32_at(d, 8).ok_or(bad)? as usize, u32_at(d, 12).ok_or(bad)? as usize);
        let object = u32_at(d, 16).ok_or(bad)?;
        let mut offs = Vec::with_capacity(n);
        let mut kinds = Vec::with_capacity(n);
        for i in 0..n {
            offs.push(u32_at(d, po + 4 * i).ok_or(bad)? as usize);
            kinds.push(u32_at(d, pt + 4 * i).ok_or(bad)?);
        }
        kinds.reverse();
        let mut bounds: Vec<usize> = offs.iter().copied().chain([po, pt, total]).collect();
        bounds.sort_unstable();
        bounds.dedup();
        let mut args = Vec::with_capacity(n);
        for (&o, &k) in offs.iter().zip(&kinds) {
            let end = bounds.iter().copied().find(|&b| b > o).unwrap_or(total).min(total);
            if let Some(a) = d.get(o..end) {
                args.push((k, a));
            }
        }
        Ok(Self { object, args })
    }

    pub(crate) fn get(&self, kind: u32) -> Option<&'a [u8]> {
        self.args.iter().find(|(k, _)| *k == kind).map(|(_, a)| *a)
    }
}

/// Argument kinds seen so far.
pub(crate) mod kind {
    /// An object's geometry: a curve's nodes, a rectangle's size.
    pub const GEOMETRY: u32 = 0x1e;
    /// An object's style, as JSON.
    pub const STYLE: u32 = 0xc9;
    /// A layer's name, UTF-16.
    pub const NAME: u32 = 0x3e8;
}

/// Object types seen so far.
pub(crate) mod object {
    pub const LAYER: u32 = 0x00;
    pub const RECTANGLE: u32 = 0x01;
    pub const CURVE: u32 = 0x03;
    pub const TEXT: u32 = 0x04;
    pub const BITMAP: u32 = 0x05;
    pub const GRID_LAYER: u32 = 0x0b;
    pub const GUIDE_LAYER: u32 = 0x0c;
    pub const DESKTOP_LAYER: u32 = 0x11;
}
