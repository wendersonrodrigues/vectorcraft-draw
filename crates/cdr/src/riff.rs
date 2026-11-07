//! The chunk tree in `content/root.dat`: a RIFF file (`RIFF` size `CDR?`) of `LIST` chunks and
//! leaf chunks. In the packages read here every leaf holds 16 bytes that say where its data is:
//!
//! | bytes | meaning |
//! |---|---|
//! | 0..4 | data stream: an index into `dataFileList.dat`, or `0xFFFFFFFF` for data held inline |
//! | 4..8 | length of the data |
//! | 8..12 | offset of the data in that stream; inline data starts here instead (at most 8 bytes) |
//! | 12..16 | zero |

use crate::{Error, Limits};

/// One chunk of the tree, its data resolved.
pub(crate) enum Node<'a> {
    List { kind: [u8; 4], children: Vec<Node<'a>> },
    Leaf { id: [u8; 4], data: &'a [u8] },
}

impl<'a> Node<'a> {
    pub(crate) fn is_list(&self, k: &[u8; 4]) -> bool {
        matches!(self, Node::List { kind, .. } if kind == k)
    }

    pub(crate) fn children(&self) -> &[Node<'a>] {
        match self {
            Node::List { children, .. } => children,
            Node::Leaf { .. } => &[],
        }
    }

    /// The first leaf `id` among the children.
    pub(crate) fn leaf(&self, id: &[u8; 4]) -> Option<&'a [u8]> {
        self.children().iter().find_map(|c| match c {
            Node::Leaf { id: i, data } if i == id => Some(*data),
            _ => None,
        })
    }

    /// The first list `kind` among the children.
    pub(crate) fn list(&self, kind: &[u8; 4]) -> Option<&Node<'a>> {
        self.children().iter().find(|c| c.is_list(kind))
    }
}

fn tag(b: &[u8], at: usize) -> Option<[u8; 4]> {
    b.get(at..at.checked_add(4)?)?.try_into().ok()
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(tag(b, at)?))
}

struct Reader<'s, 'a> {
    root: &'a [u8],
    streams: &'s [&'a [u8]],
    limits: Limits,
    chunks: usize,
}

/// The tree of `root`, the RIFF file's `CDR?` form, its leaves resolved against `streams`.
pub(crate) fn parse<'a>(root: &'a [u8], streams: &[&'a [u8]], limits: Limits) -> Result<Node<'a>, Error> {
    if tag(root, 0) != Some(*b"RIFF") || root.get(8..11) != Some(b"CDR".as_slice()) {
        return Err(Error::Malformed("the chunk tree isn't a CorelDRAW RIFF file"));
    }
    let size = u32_at(root, 4).ok_or(Error::Malformed("chunk tree header"))? as usize;
    let end = size.saturating_add(8).min(root.len());
    let kind = tag(root, 8).ok_or(Error::Malformed("chunk tree header"))?;
    let mut r = Reader { root, streams, limits, chunks: 0 };
    let children = r.list(12, end, 0)?;
    Ok(Node::List { kind, children })
}

impl<'a> Reader<'_, 'a> {
    fn list(&mut self, mut at: usize, end: usize, depth: usize) -> Result<Vec<Node<'a>>, Error> {
        if depth > self.limits.max_depth {
            return Err(Error::Limit("lists nest too deeply"));
        }
        let mut out = Vec::new();
        while at.saturating_add(8) <= end {
            self.chunks += 1;
            if self.chunks > self.limits.max_chunks {
                return Err(Error::Limit("too many chunks"));
            }
            let id = tag(self.root, at).ok_or(Error::Malformed("chunk header"))?;
            let size = u32_at(self.root, at + 4).ok_or(Error::Malformed("chunk header"))? as usize;
            let body = at + 8;
            let stop = body.checked_add(size).ok_or(Error::Malformed("chunk size"))?;
            if stop > end {
                return Err(Error::Malformed("a chunk runs past its list"));
            }
            if &id == b"LIST" || &id == b"RIFF" {
                let kind = tag(self.root, body).ok_or(Error::Malformed("list header"))?;
                let children = self.list(body + 4, stop, depth + 1)?;
                out.push(Node::List { kind, children });
            } else {
                let data = self.resolve(self.root.get(body..stop).ok_or(Error::Malformed("chunk"))?)?;
                out.push(Node::Leaf { id, data });
            }
            // Chunks are padded to an even length.
            at = stop.saturating_add(size & 1);
        }
        Ok(out)
    }

    fn resolve(&self, r: &'a [u8]) -> Result<&'a [u8], Error> {
        let (Some(stream), Some(len)) = (u32_at(r, 0), u32_at(r, 4)) else { return Ok(r) };
        let len = len as usize;
        if r.len() != 16 {
            // Not a reference: the data itself.
            return Ok(r);
        }
        if stream == u32::MAX {
            return r.get(8..8 + len.min(8)).ok_or(Error::Malformed("inline chunk data"));
        }
        let off = u32_at(r, 8).ok_or(Error::Malformed("chunk reference"))? as usize;
        let s = self.streams.get(stream as usize).ok_or(Error::Malformed("a chunk names a missing data stream"))?;
        s.get(off..off.checked_add(len).ok_or(Error::Malformed("chunk reference"))?).ok_or(Error::Malformed("a chunk points past its data stream"))
    }
}
