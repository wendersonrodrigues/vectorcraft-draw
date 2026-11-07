//! The ZIP package recent CorelDRAW versions save: a stored `mimetype` member first, then
//! `content/root.dat` (the chunk tree), `content/dataFileList.dat` (the data streams' names, one
//! per line) and `content/data/*` (the streams the tree's chunks point into).
//!
//! Only what that needs is read, from the ZIP format as PKWARE's public APPNOTE describes it:
//! the end of central directory record, central directory entries, local headers, and stored or
//! deflated members. ZIP64, encryption and multi-disk archives are refused.

use std::io::Read;

use crate::{Error, Limits};

/// The `mimetype` member's content in the packages read here.
pub const MIMETYPE: &str = "application/x-vnd.corel.zcf.draw.document+zip";
/// What every CorelDRAW package's `mimetype` starts with (other applications of the suite too).
const MIMETYPE_PREFIX: &[u8] = b"application/x-vnd.corel.";

const LOCAL: u32 = 0x0403_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const END: u32 = 0x0605_4b50;

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// Whether `bytes` is a CorelDRAW drawing package: a ZIP whose first member is a `mimetype`
/// naming a Corel document.
pub fn is_cdr(bytes: &[u8]) -> bool {
    if u32_at(bytes, 0) != Some(LOCAL) {
        return false;
    }
    let (Some(n), Some(e)) = (u16_at(bytes, 26), u16_at(bytes, 28)) else { return false };
    if bytes.get(30..30 + usize::from(n)) != Some(b"mimetype".as_slice()) {
        return false;
    }
    let at = 30 + usize::from(n) + usize::from(e);
    bytes.get(at..at + MIMETYPE_PREFIX.len()) == Some(MIMETYPE_PREFIX)
}

/// Whether `bytes` is an older CorelDRAW file that is one RIFF file (`RIFF` … `CDR?`), not a
/// package. Those aren't read yet.
pub fn is_legacy_riff(bytes: &[u8]) -> bool {
    bytes.get(..4) == Some(b"RIFF".as_slice()) && bytes.get(8..11) == Some(b"CDR".as_slice())
}

struct Entry {
    name: String,
    method: u16,
    compressed: usize,
    size: usize,
    local: usize,
}

/// A package's members, read on demand.
pub(crate) struct Package<'a> {
    zip: &'a [u8],
    entries: Vec<Entry>,
    limits: Limits,
}

impl<'a> Package<'a> {
    pub(crate) fn open(zip: &'a [u8], limits: Limits) -> Result<Self, Error> {
        // The end record is in the last 22 + 65535 (largest comment) bytes.
        let lo = zip.len().saturating_sub(22 + 0xffff);
        let end = (lo..zip.len().saturating_sub(21)).rev().find(|&i| u32_at(zip, i) == Some(END)).ok_or(Error::Malformed("no ZIP directory"))?;
        let count = usize::from(u16_at(zip, end + 10).ok_or(Error::Malformed("ZIP directory"))?);
        let dir = u32_at(zip, end + 16).ok_or(Error::Malformed("ZIP directory"))? as usize;
        if count == 0xffff || dir == 0xffff_ffff {
            return Err(Error::Unsupported("ZIP64 package"));
        }
        if count > limits.max_members {
            return Err(Error::Limit("too many package members"));
        }
        let mut entries = Vec::with_capacity(count);
        let mut at = dir;
        for _ in 0..count {
            if u32_at(zip, at) != Some(CENTRAL) {
                return Err(Error::Malformed("ZIP directory entry"));
            }
            let field = |o: usize| u16_at(zip, at + o).ok_or(Error::Malformed("ZIP directory entry"));
            let long = |o: usize| u32_at(zip, at + o).map(|v| v as usize).ok_or(Error::Malformed("ZIP directory entry"));
            if field(8)? & 1 != 0 {
                return Err(Error::Unsupported("encrypted package"));
            }
            let method = field(10)?;
            let (compressed, size, local) = (long(20)?, long(24)?, long(42)?);
            let (n, e, c) = (usize::from(field(28)?), usize::from(field(30)?), usize::from(field(32)?));
            let name = zip.get(at + 46..at + 46 + n).ok_or(Error::Malformed("ZIP directory entry"))?;
            entries.push(Entry { name: String::from_utf8_lossy(name).into_owned(), method, compressed, size, local });
            at += 46 + n + e + c;
        }
        Ok(Self { zip, entries, limits })
    }

    /// The member called `name`, inflated; `None` when there is none.
    pub(crate) fn read(&self, name: &str) -> Result<Option<Vec<u8>>, Error> {
        let Some(e) = self.entries.iter().find(|e| e.name == name) else { return Ok(None) };
        if e.size > self.limits.max_member {
            return Err(Error::Limit("a package member is too large"));
        }
        if u32_at(self.zip, e.local) != Some(LOCAL) {
            return Err(Error::Malformed("ZIP member header"));
        }
        let n = usize::from(u16_at(self.zip, e.local + 26).ok_or(Error::Malformed("ZIP member header"))?);
        let x = usize::from(u16_at(self.zip, e.local + 28).ok_or(Error::Malformed("ZIP member header"))?);
        let start = e.local + 30 + n + x;
        let raw = self
            .zip
            .get(start..start.checked_add(e.compressed).ok_or(Error::Malformed("ZIP member"))?)
            .ok_or(Error::Malformed("ZIP member cut short"))?;
        match e.method {
            0 => Ok(Some(raw.to_vec())),
            8 => {
                let mut out = Vec::with_capacity(e.size.min(self.limits.max_member));
                let cap = self.limits.max_member as u64 + 1;
                flate2::read::DeflateDecoder::new(raw).take(cap).read_to_end(&mut out).map_err(|_| Error::Malformed("ZIP member data"))?;
                if out.len() > self.limits.max_member {
                    return Err(Error::Limit("a package member is too large"));
                }
                Ok(Some(out))
            }
            _ => Err(Error::Unsupported("package member compression")),
        }
    }
}
