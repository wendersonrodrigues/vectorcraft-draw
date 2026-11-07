//! Reader for CorelDRAW documents (`.cdr`) saved as a ZIP package (`mimetype`
//! `application/x-vnd.corel.zcf.draw.document+zip`): the package, the RIFF chunk tree in
//! `content/root.dat` with its references into the data streams, and the subset of the document
//! model VectorCraft imports (pages, layers, groups, curves, rectangles, solid and fountain fills,
//! outlines, uniform transparency).
//!
//! Clean room: every layout here was worked out from the bytes of CorelDRAW files their owners
//! saved and shared for this purpose, and checked against what those files record about
//! themselves (each object's bounding box). No other reader's code was consulted. See the crate
//! README for the evidence and the exact supported subset. Nothing here writes CorelDRAW files.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::fmt;

mod args;
pub mod model;
mod package;
mod riff;
mod style;
#[cfg(any(test, feature = "synth"))]
pub mod synth;

pub use model::{Document, read};
pub use package::{MIMETYPE, is_cdr, is_legacy_riff};

/// Why a file could not be read. Messages are short reasons; callers add advice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Malformed(&'static str),
    Unsupported(&'static str),
    Limit(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(s) => write!(f, "damaged CorelDRAW file: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported CorelDRAW file: {s}"),
            Self::Limit(s) => write!(f, "CorelDRAW file exceeds a safety limit: {s}"),
        }
    }
}

impl std::error::Error for Error {}

/// Caps on what a file may make the reader do.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Bytes any one package member may inflate to.
    pub max_member: usize,
    /// Members the package may list.
    pub max_members: usize,
    /// Chunks the RIFF tree may hold.
    pub max_chunks: usize,
    /// How deep lists may nest.
    pub max_depth: usize,
    /// Path nodes in the whole document.
    pub max_nodes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_member: 256 << 20, max_members: 4096, max_chunks: 2_000_000, max_depth: 64, max_nodes: 10_000_000 }
    }
}

#[cfg(test)]
mod tests;
