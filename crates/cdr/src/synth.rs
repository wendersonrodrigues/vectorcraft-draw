//! Builders for small synthetic CorelDRAW packages, for tests and fuzz seeds. They write only
//! the structures this crate reads, as [`crate::package`], [`crate::riff`] and [`crate::args`]
//! describe them; they are not a CorelDRAW exporter. Member CRCs are left zero (the reader
//! doesn't check them).

/// A chunk to write.
pub enum Chunk {
    List(&'static [u8; 4], Vec<Chunk>),
    Leaf(&'static [u8; 4], Vec<u8>),
}

pub fn list(kind: &'static [u8; 4], children: Vec<Chunk>) -> Chunk {
    Chunk::List(kind, children)
}

pub fn leaf(id: &'static [u8; 4], data: Vec<u8>) -> Chunk {
    Chunk::Leaf(id, data)
}

/// An argument table of `object` type with `(kind, bytes)` arguments.
pub fn loda(object: u32, args: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let n = args.len();
    let header = 20;
    let po = header;
    let pt = po + 4 * n;
    let mut body = Vec::new();
    let mut offs = Vec::new();
    let start = pt + 4 * n;
    for (_, a) in args {
        offs.push((start + body.len()) as u32);
        body.extend_from_slice(a);
    }
    let total = (start + body.len()) as u32;
    let mut out = Vec::new();
    for v in [total, n as u32, po as u32, pt as u32, object] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for o in &offs {
        out.extend_from_slice(&o.to_le_bytes());
    }
    // Kinds are stored in reverse.
    for (k, _) in args.iter().rev() {
        out.extend_from_slice(&k.to_le_bytes());
    }
    out.extend_from_slice(&body);
    out
}

/// A curve's geometry argument: `(x, y, node type)` nodes.
pub fn curve(nodes: &[(i32, i32, u8)]) -> Vec<u8> {
    let mut out = (nodes.len() as u32).to_le_bytes().to_vec();
    for (x, y, _) in nodes {
        out.extend_from_slice(&x.to_le_bytes());
        out.extend_from_slice(&y.to_le_bytes());
    }
    out.extend(nodes.iter().map(|n| n.2));
    out
}

/// A rectangle's geometry argument: width and height, then zeros.
pub fn rectangle(w: f64, h: f64) -> Vec<u8> {
    let mut out = w.to_le_bytes().to_vec();
    out.extend_from_slice(&h.to_le_bytes());
    out.resize(128, 0);
    out
}

/// A style argument holding `json`.
pub fn style(json: &str) -> Vec<u8> {
    let mut out = 1u32.to_le_bytes().to_vec();
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(json.as_bytes());
    out
}

/// A layer name argument.
pub fn name(s: &str) -> Vec<u8> {
    let mut out: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
    out.extend_from_slice(&[0, 0]);
    out
}

/// A `trfd` holding `(a, b, tx, c, d, ty)`.
pub fn trfd(m: [f64; 6]) -> Vec<u8> {
    let mut out = vec![0u8; 40];
    out[..4].copy_from_slice(&96u32.to_le_bytes());
    for v in m {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.resize(96, 0);
    out
}

/// An `mcfg` giving a page `w` × `h` units.
pub fn mcfg(w: i32, h: i32) -> Vec<u8> {
    let mut out = vec![0u8; 0x40];
    out[0x0c..0x10].copy_from_slice(&w.to_le_bytes());
    out[0x10..0x14].copy_from_slice(&h.to_le_bytes());
    out
}

/// An object: its argument table and transform.
pub fn object(loda: Vec<u8>, m: [f64; 6]) -> Chunk {
    list(b"obj ", vec![list(b"lgob", vec![leaf(b"loda", loda), list(b"trfl", vec![leaf(b"trfd", trfd(m))])])])
}

/// A layer of `object` type named `name`, holding `children` (topmost first).
pub fn layer(object: u32, name_: &str, children: Vec<Chunk>) -> Chunk {
    let mut c = vec![list(b"lgob", vec![leaf(b"loda", loda(object, &[(0x3e8, name(name_))]))])];
    c.extend(children);
    list(b"layr", c)
}

/// A page holding `layers` (topmost first).
pub fn page(layers: Vec<Chunk>) -> Chunk {
    list(b"page", vec![list(b"gobj", layers)])
}

/// The chunk tree `CDRP` holding `children`, with every leaf's data moved to one stream.
pub fn root(children: Vec<Chunk>) -> (Vec<u8>, Vec<u8>) {
    let mut stream = Vec::new();
    let mut body = b"CDRP".to_vec();
    for c in &children {
        write(c, &mut body, &mut stream);
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    (out, stream)
}

fn write(c: &Chunk, out: &mut Vec<u8>, stream: &mut Vec<u8>) {
    match c {
        Chunk::List(kind, kids) => {
            let mut body = kind.to_vec();
            for k in kids {
                write(k, &mut body, stream);
            }
            out.extend_from_slice(b"LIST");
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&body);
            if body.len() % 2 == 1 {
                out.push(0);
            }
        }
        Chunk::Leaf(id, data) => {
            out.extend_from_slice(*id);
            out.extend_from_slice(&16u32.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(stream.len() as u32).to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            stream.extend_from_slice(data);
        }
    }
}

/// A stored ZIP of `files`, in order.
pub fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let at = out.len() as u32;
        let header = |sig: u32, central_: bool| {
            let mut h = sig.to_le_bytes().to_vec();
            if central_ {
                h.extend_from_slice(&20u16.to_le_bytes());
            }
            h.extend_from_slice(&20u16.to_le_bytes()); // version needed
            h.extend_from_slice(&0u16.to_le_bytes()); // flags
            h.extend_from_slice(&0u16.to_le_bytes()); // stored
            h.extend_from_slice(&0u32.to_le_bytes()); // time, date
            h.extend_from_slice(&0u32.to_le_bytes()); // crc
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes()); // extra
            h
        };
        out.extend(header(0x0403_4b50, false));
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.extend(header(0x0201_4b50, true));
        central.extend_from_slice(&[0u8; 6]); // comment length, disk, internal attributes
        central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        central.extend_from_slice(&at.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let dir = out.len() as u32;
    let n = files.len() as u16;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&dir.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// A whole package from the top-level chunks of its tree.
pub fn package(children: Vec<Chunk>) -> Vec<u8> {
    let (root, stream) = root(children);
    zip(&[
        ("mimetype", crate::MIMETYPE.as_bytes()),
        ("content/root.dat", &root),
        ("content/dataFileList.dat", b"data1.dat"),
        ("content/data/data1.dat", &stream),
    ])
}

/// A one-page document 9.8 × 10 million units (98 × 100 cm) with a document settings list and
/// `objects` (topmost first) on one layer called "Layer 1".
pub fn simple(objects: Vec<Chunk>) -> Vec<u8> {
    package(vec![
        list(b"doc ", vec![leaf(b"mcfg", mcfg(9_800_000, 10_000_000))]),
        page(vec![layer(0x0c, "Guides", vec![])]),
        page(vec![layer(0x00, "Layer 1", objects)]),
    ])
}
