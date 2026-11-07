//! The part of a CorelDRAW document VectorCraft imports, in points with y down from each page's
//! top-left corner and every transform applied.
//!
//! The tree: `RIFF CDR?` holds `LIST doc ` (document settings: `mcfg` gives the page size) and
//! one `LIST page` per page, the master page first. A page holds `LIST gobj`, which holds the
//! page's layers (`LIST layr`); a layer holds its objects (`LIST obj `) and groups
//! (`LIST grp `, holding objects and groups), topmost first. Every page, layer, group and object
//! has a `LIST lgob` whose `loda` is an argument table ([`crate::args`]) giving its type and
//! arguments; an object's `lgob` also holds `LIST trfl` with its transform (`trfd`).
//!
//! PowerClip: a container object's argument `0x1F45` starts with a u32 that matches the `spnd`
//! (a u32) of one group in the document's `LIST clpt`; that group's objects are the art the
//! container clips, in page coordinates like the page's own objects.
//!
//! Coordinates are integers, 254 000 to the inch (0.1 µm), y up, with the origin at the
//! page's centre. The unit is an inference: it makes the master page's desktop 1800 inches
//! tall and the sample page 98 × 100 cm, both plausible, but no file seen so far states it.

use crate::args::{Args, kind, object};
use crate::package::{Package, is_legacy_riff};
use crate::riff::{self, Node};
use crate::{Error, Limits, style};

/// Units in an inch (see the module documentation).
pub const UNITS_PER_INCH: f64 = 254_000.0;
const UNITS_PER_POINT: f64 = UNITS_PER_INCH / 72.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Color {
    /// 0 to 1.
    Rgb(f64, f64, f64),
    /// 0 to 1.
    Cmyk(f64, f64, f64, f64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fountain {
    /// CorelDRAW's fountain type: 1 and 2 seen (2 has its centre offsets in use).
    pub kind: u32,
    pub from: Color,
    pub to: Color,
    /// Degrees.
    pub angle: f64,
    /// Percent of the object's size from its centre, y up.
    pub center: (f64, f64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Fill {
    Solid(Color),
    Fountain(Fountain),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outline {
    /// Points.
    pub width: f64,
    pub color: Color,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    Line(Point),
    Cubic(Point, Point, Point),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Subpath {
    pub start: Point,
    pub segments: Vec<Segment>,
    pub closed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Group(Vec<Object>),
    Path { subpaths: Vec<Subpath>, even_odd: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub kind: Kind,
    pub fill: Option<Fill>,
    pub outline: Option<Outline>,
    /// 0 to 1.
    pub opacity: f64,
    /// A PowerClip container's contents, bottom first, clipped to its path (its fill is behind
    /// them, its outline over them).
    pub clipped: Vec<Object>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub name: String,
    /// Bottom first.
    pub objects: Vec<Object>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    /// Points.
    pub width: f64,
    pub height: f64,
    /// Bottom first.
    pub layers: Vec<Layer>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    pub pages: Vec<Page>,
    /// What was left out or approximated, one short phrase each.
    pub warnings: Vec<String>,
}

fn i32_at(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn f64_at(b: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(b.get(at..at.checked_add(8)?)?.try_into().ok()?)).filter(|v| v.is_finite())
}

/// Read a CorelDRAW package.
pub fn read(bytes: &[u8], limits: Limits) -> Result<Document, Error> {
    if is_legacy_riff(bytes) {
        return Err(Error::Unsupported("older CorelDRAW files saved as one RIFF file aren't read yet"));
    }
    let pkg = Package::open(bytes, limits)?;
    let root = pkg.read("content/root.dat")?.ok_or(Error::Unsupported("no content/root.dat (a CorelDRAW version not read yet)"))?;
    let list = pkg.read("content/dataFileList.dat")?.unwrap_or_default();
    let names: Vec<String> = String::from_utf8_lossy(&list).lines().map(str::to_owned).collect();
    if names.len() > limits.max_members {
        return Err(Error::Limit("too many data streams"));
    }
    let mut owned = Vec::with_capacity(names.len());
    for n in &names {
        // A stream that is listed but missing reads as empty: chunks pointing into it fail.
        owned.push(pkg.read(&format!("content/data/{n}"))?.unwrap_or_default());
    }
    let streams: Vec<&[u8]> = owned.iter().map(Vec::as_slice).collect();
    let tree = riff::parse(&root, &streams, limits)?;
    Builder { limits, nodes: 0, warnings: Warnings::default(), page: (0.0, 0.0), clips: Vec::new() }.document(&tree)
}

#[derive(Default)]
struct Warnings(Vec<(String, usize)>);

impl Warnings {
    fn add(&mut self, w: &str) {
        match self.0.iter_mut().find(|(s, _)| s == w) {
            Some((_, n)) => *n += 1,
            None => self.0.push((w.to_string(), 1)),
        }
    }

    fn into_vec(self) -> Vec<String> {
        self.0.into_iter().map(|(w, n)| if n > 1 { format!("{w} ({n})") } else { w }).collect()
    }
}

/// An affine map `(a, b, tx, c, d, ty)`: `x' = a x + b y + tx`, `y' = c x + d y + ty`.
type Affine = [f64; 6];
const IDENTITY: Affine = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

struct Builder<'t, 'a> {
    limits: Limits,
    /// Curve nodes and objects made so far (PowerClips can repeat art).
    nodes: usize,
    warnings: Warnings,
    /// The page size in units.
    page: (f64, f64),
    /// PowerClip contents: each `LIST clpt` group by its `spnd`.
    clips: Vec<(u32, &'t Node<'a>)>,
}

/// A PowerClip container's argument naming its contents.
const POWERCLIP: u32 = 0x1f45;

impl<'t, 'a> Builder<'t, 'a> {
    fn count(&mut self, n: usize) -> Result<(), Error> {
        self.nodes = self.nodes.saturating_add(n);
        if self.nodes > self.limits.max_nodes {
            return Err(Error::Limit("too many objects and curve nodes"));
        }
        Ok(())
    }

    fn document(mut self, tree: &'t Node<'a>) -> Result<Document, Error> {
        if let Some(clpt) = tree.list(b"doc ").and_then(|d| d.list(b"clpt")) {
            for g in clpt.children().iter().filter(|c| c.is_list(b"grp ")) {
                if let Some(id) = g.leaf(b"spnd").and_then(|s| s.get(..4)).and_then(|s| s.try_into().ok()).map(u32::from_le_bytes) {
                    self.clips.push((id, g));
                }
            }
        }
        self.page = tree
            .list(b"doc ")
            .and_then(|d| d.leaf(b"mcfg"))
            .and_then(|m| Some((i32_at(m, 0x0c)?, i32_at(m, 0x10)?)))
            .map(|(w, h)| (f64::from(w), f64::from(h)))
            .unwrap_or((0.0, 0.0));
        let pages: Vec<&Node<'_>> = tree.children().iter().filter(|c| c.is_list(b"page")).collect();
        let Some((master, pages)) = pages.split_first() else { return Err(Error::Malformed("the document has no pages")) };
        let master_layers = self.layers(master)?;
        let mut out = Vec::new();
        for p in pages {
            let mut layers =
                master_layers.iter().map(|l| Layer { name: format!("Master: {}", l.name), objects: l.objects.clone() }).collect::<Vec<_>>();
            layers.extend(self.layers(p)?);
            if self.page.0 <= 0.0 || self.page.1 <= 0.0 {
                return Err(Error::Malformed("the page has no size"));
            }
            out.push(Page { width: self.page.0 / UNITS_PER_POINT, height: self.page.1 / UNITS_PER_POINT, layers });
        }
        if out.is_empty() {
            return Err(Error::Malformed("the document has no pages"));
        }
        Ok(Document { pages: out, warnings: self.warnings.into_vec() })
    }

    /// A page's drawing layers, bottom first.
    fn layers(&mut self, page: &Node<'_>) -> Result<Vec<Layer>, Error> {
        let mut out = Vec::new();
        for gobj in page.children().iter().filter(|c| c.is_list(b"gobj")) {
            for layr in gobj.children().iter().filter(|c| c.is_list(b"layr")) {
                let a = layr.list(b"lgob").and_then(|l| l.leaf(b"loda")).map(Args::parse).transpose()?;
                let ty = a.as_ref().map(|a| a.object).unwrap_or(object::LAYER);
                let objects = self.objects(layr, 0)?;
                if matches!(ty, object::GUIDE_LAYER | object::DESKTOP_LAYER | object::GRID_LAYER) {
                    if !objects.is_empty() && ty == object::DESKTOP_LAYER {
                        self.warnings.add("objects on the desktop, outside the pages");
                    }
                    continue;
                }
                let name = a
                    .as_ref()
                    .and_then(|a| a.get(kind::NAME))
                    .map(utf16)
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| format!("Layer {}", out.len() + 1));
                out.push(Layer { name, objects });
            }
        }
        out.reverse();
        Ok(out)
    }

    /// The objects and groups listed in `parent`, bottom first.
    fn objects(&mut self, parent: &Node<'_>, depth: usize) -> Result<Vec<Object>, Error> {
        if depth > self.limits.max_depth {
            return Err(Error::Limit("groups nest too deeply"));
        }
        let mut out = Vec::new();
        for c in parent.children() {
            if c.is_list(b"grp ") {
                let kids = self.objects(c, depth + 1)?;
                if !kids.is_empty() {
                    out.push(Object { kind: Kind::Group(kids), fill: None, outline: None, opacity: 1.0, clipped: Vec::new() });
                }
            } else if c.is_list(b"obj ")
                && let Some(o) = self.object(c, depth)?
            {
                out.push(o);
            }
        }
        out.reverse();
        Ok(out)
    }

    fn object(&mut self, obj: &Node<'_>, depth: usize) -> Result<Option<Object>, Error> {
        self.count(1)?;
        let Some(lgob) = obj.list(b"lgob") else { return Ok(None) };
        let Some(loda) = lgob.leaf(b"loda") else { return Ok(None) };
        let a = Args::parse(loda)?;
        let geometry = a.get(kind::GEOMETRY);
        let points: Vec<(f64, f64, u8)> = match (a.object, geometry) {
            (object::CURVE, Some(g)) => self.curve(g)?,
            (object::RECTANGLE, Some(g)) => {
                let (Some(w), Some(h)) = (f64_at(g, 0), f64_at(g, 8)) else {
                    self.warnings.add("a rectangle without a size");
                    return Ok(None);
                };
                vec![(0.0, 0.0, 0x0c), (w, 0.0, 0x40), (w, h, 0x40), (0.0, h, 0x48)]
            }
            (object::TEXT, _) => {
                self.warnings.add("text");
                return Ok(None);
            }
            (object::BITMAP, _) => {
                self.warnings.add("bitmaps");
                return Ok(None);
            }
            (t, _) => {
                self.warnings.add(&format!("objects of a kind not read yet (type {t})"));
                return Ok(None);
            }
        };
        let m = lgob.list(b"trfl").and_then(|t| t.leaf(b"trfd")).map(transform).unwrap_or(Some(IDENTITY));
        let Some(m) = m else {
            self.warnings.add("an object whose transform couldn't be read");
            return Ok(None);
        };
        let subpaths = self.subpaths(&points, m);
        if subpaths.is_empty() {
            return Ok(None);
        }
        let json = a.get(kind::STYLE).and_then(style::parse);
        let st = json.as_ref().map(|j| style::style(j, UNITS_PER_POINT));
        let even_odd = json.as_ref().and_then(|j| j.get("fill")).and_then(|f| f.get("winding")).and_then(|w| w.as_str()) != Some("1");
        let (fill, outline, opacity) = match st {
            Some(s) => {
                for w in &s.skipped {
                    self.warnings.add(w);
                }
                (s.fill, s.outline, 1.0 - s.transparency)
            }
            None => (None, None, 1.0),
        };
        let clipped = match a.get(POWERCLIP).and_then(|p| p.get(..4)).and_then(|p| p.try_into().ok()).map(u32::from_le_bytes) {
            Some(id) => match self.clips.iter().find(|(i, _)| *i == id).map(|(_, g)| *g) {
                Some(g) => self.objects(g, depth + 1)?,
                None => {
                    self.warnings.add("PowerClip contents that couldn't be found");
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        Ok(Some(Object { kind: Kind::Path { subpaths, even_odd }, fill, outline, opacity, clipped }))
    }

    /// A curve's nodes: a count, that many (x, y) i32 pairs, then that many type bytes.
    fn curve(&mut self, g: &[u8]) -> Result<Vec<(f64, f64, u8)>, Error> {
        let n = i32_at(g, 0).ok_or(Error::Malformed("curve"))?;
        let n = usize::try_from(n).map_err(|_| Error::Malformed("curve node count"))?;
        self.count(n)?;
        let types = g.get(4 + 8 * n..4 + 9 * n).ok_or(Error::Malformed("curve cut short"))?;
        let mut out = Vec::with_capacity(n);
        for (i, &t) in types.iter().enumerate() {
            let (Some(x), Some(y)) = (i32_at(g, 4 + 8 * i), i32_at(g, 8 + 8 * i)) else { return Err(Error::Malformed("curve cut short")) };
            out.push((f64::from(x), f64::from(y), t));
        }
        Ok(out)
    }

    /// Nodes to subpaths in page points. A node's top two bits give its kind: 00 starts a
    /// subpath, 01 is a line to it, 10 a cubic to it whose two control points are the 11 nodes
    /// just before; bit 3 on a line or cubic closes the subpath.
    fn subpaths(&mut self, nodes: &[(f64, f64, u8)], m: Affine) -> Vec<Subpath> {
        let (w, h) = self.page;
        let to_page = |x: f64, y: f64| {
            let (px, py) = (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5]);
            Point { x: (px + w / 2.0) / UNITS_PER_POINT, y: (h / 2.0 - py) / UNITS_PER_POINT }
        };
        let mut out: Vec<Subpath> = Vec::new();
        let mut controls: Vec<Point> = Vec::new();
        let mut broken = false;
        for &(x, y, t) in nodes {
            let p = to_page(x, y);
            match t & 0xc0 {
                0x00 => {
                    controls.clear();
                    out.push(Subpath { start: p, segments: Vec::new(), closed: false });
                }
                0xc0 => controls.push(p),
                class => {
                    let Some(s) = out.last_mut() else {
                        broken = true;
                        continue;
                    };
                    if class == 0x80 {
                        if let [c1, c2] = controls[..] {
                            s.segments.push(Segment::Cubic(c1, c2, p));
                        } else {
                            broken = true;
                            s.segments.push(Segment::Line(p));
                        }
                    } else {
                        s.segments.push(Segment::Line(p));
                    }
                    controls.clear();
                    if t & 0x08 != 0 {
                        s.closed = true;
                    }
                }
            }
        }
        if broken {
            self.warnings.add("curve segments with unexpected control points, drawn as lines");
        }
        out.retain(|s| !s.segments.is_empty());
        out
    }
}

/// A `trfd`: 40 header bytes, then six f64 `(a, b, tx, c, d, ty)`.
fn transform(d: &[u8]) -> Option<Affine> {
    let mut m = [0.0; 6];
    for (i, v) in m.iter_mut().enumerate() {
        *v = f64_at(d, 40 + 8 * i)?;
    }
    Some(m)
}

fn utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).take_while(|&u| u != 0).collect();
    String::from_utf16_lossy(&units)
}
