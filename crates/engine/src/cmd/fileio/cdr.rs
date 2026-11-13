//! CorelDRAW documents (`.cdr`): the pages, layers, groups, PowerClips, curves, rectangles, text and
//! bitmaps that `vectorcraft-cdr` reads, with their solid and fountain fills, outlines and
//! transparency, as editable VectorCraft art; bitmaps are embedded as PNG. Each page becomes an
//! artboard, side by side 36 pt apart; layers of the same name on different pages become one
//! layer; text becomes point type in the fonts it names. Whatever the reader leaves out is listed
//! in the warnings.

use std::io::Cursor;

use std::sync::Arc;

use kurbo::{Affine, BezPath, Point, Rect, Shape};
use vectorcraft_cdr::model::{self as cd, Fill, Kind, Segment};
use vectorcraft_color::{Color, Gradient, GradientGeom, GradientKind, GradientPaint, GradientStop, Paint};
use vectorcraft_doc::appearance::{Appearance, AppearanceItem, FillLayer, StrokeLayer};
use vectorcraft_doc::geom::{FillRule, PathData};
use vectorcraft_doc::text::{CharStyle, ParaStyle, TextObject, TextRun};
use vectorcraft_doc::{Artboard, Document, ImageObject, LayerColor, Node, NodeKind};

use super::load::err;
use crate::Result;

/// Space between pages, as for Affinity spreads.
const GAP: f64 = 36.0;
/// Widest outline kept, in points.
const MAX_OUTLINE: f64 = 1000.0;

pub(super) fn import(bytes: &[u8]) -> Result<(Document, Vec<String>)> {
    let d = vectorcraft_cdr::read(bytes, vectorcraft_cdr::Limits::default())
        .map_err(|e| err(format!("{e}; export SVG or PDF from CorelDRAW to open it")))?;
    let mut b = Builder { doc: Document::new(0.0, 0.0), warnings: Vec::new(), images: Vec::new() };
    b.document(&d)?;
    b.doc.limit_depth_noting(&mut b.warnings);
    let mut warnings: Vec<String> = d.warnings.iter().map(|w| format!("CorelDRAW: not imported: {w}.")).collect();
    warnings.extend(b.warnings.iter().map(|w| format!("CorelDRAW: {w}.")));
    Ok((b.doc, warnings))
}

struct Builder {
    doc: Document,
    warnings: Vec<String>,
    /// Each of the reader's bitmaps as stored in `doc.images`: its key and pixel size (`None`:
    /// it couldn't be encoded).
    images: Vec<Option<(String, u32, u32)>>,
}

impl Builder {
    fn warn(&mut self, w: &str) {
        if !self.warnings.iter().any(|x| x == w) {
            self.warnings.push(w.to_string());
        }
    }

    fn document(&mut self, d: &cd::Document) -> Result<()> {
        for b in &d.bitmaps {
            let stored = png(b).and_then(|bytes| super::load::raster_image(&bytes).ok());
            let entry = stored.map(|r| {
                self.doc.images.entry(r.key.clone()).or_insert(r.blob);
                (r.key, r.width, r.height)
            });
            if entry.is_none() {
                self.warn("a bitmap that couldn't be stored was left out");
            }
            self.images.push(entry);
        }
        // (name, children bottom first), in the order the names first appear, bottom first.
        let mut layers: Vec<(String, Vec<Arc<Node>>)> = Vec::new();
        let mut artboards = Vec::new();
        let mut x = 0.0;
        for (i, page) in d.pages.iter().enumerate() {
            if !(page.width.is_finite() && page.height.is_finite()) {
                continue;
            }
            artboards.push(Artboard {
                id: i as u32 + 1,
                name: format!("Page {}", i + 1),
                rect: Rect::new(x, 0.0, x + page.width, page.height),
                locked: false,
                show_center_mark: false,
                show_cross_hairs: false,
                show_video_safe_areas: false,
                video_ruler_pixel_aspect_ratio: 1.0,
                color: None,
            });
            for l in &page.layers {
                let art: Vec<Arc<Node>> = l.objects.iter().filter_map(|o| self.node(o, x)).map(Arc::new).collect();
                match layers.iter_mut().find(|(n, _)| *n == l.name) {
                    Some((_, kids)) => kids.extend(art),
                    None => layers.push((l.name.clone(), art)),
                }
            }
            x += page.width + GAP;
        }
        if artboards.is_empty() {
            return Err(err("the CorelDRAW document has no page with a size"));
        }
        self.doc.artboards = artboards;
        self.doc.layers = layers
            .into_iter()
            .enumerate()
            .map(|(i, (name, kids))| {
                let mut n = Node::layer(self.doc.alloc_id(), &name, LayerColor::Preset((i % 9) as u8));
                if let Some(c) = n.children_mut() {
                    *c = kids;
                }
                Arc::new(n)
            })
            .collect();
        if self.doc.layers.is_empty() {
            let n = Node::layer(self.doc.alloc_id(), "Layer 1", LayerColor::Preset(0));
            self.doc.layers.push(Arc::new(n));
        }
        Ok(())
    }

    /// `o` with its page moved `dx` points right.
    fn node(&mut self, o: &cd::Object, dx: f64) -> Option<Node> {
        let mut n = match &o.kind {
            Kind::Group(kids) => {
                let children: Vec<Arc<Node>> = kids.iter().filter_map(|k| self.node(k, dx)).map(Arc::new).collect();
                if children.is_empty() {
                    return None;
                }
                Node::new(self.doc.alloc_id(), NodeKind::Group { children, clip: false })
            }
            Kind::Path { subpaths, even_odd } => {
                let bp = path(subpaths, dx)?;
                let bounds = bp.bounding_box();
                let path = PathData::from_bezpath(&bp);
                let rule = if *even_odd { FillRule::EvenOdd } else { FillRule::NonZero };
                let contents: Vec<Arc<Node>> = o.clipped.iter().filter_map(|k| self.node(k, dx)).map(Arc::new).collect();
                // A PowerClip: VectorCraft's clipping path keeps its paint, its fill behind the
                // clipped art and its outline over it, as CorelDRAW draws a container.
                let clipping = !contents.is_empty();
                let mut p = Node::new(self.doc.alloc_id(), NodeKind::Path { path, rule, live: None, clipping, guide: false });
                p.appearance = self.appearance(o, bounds).into();
                if clipping {
                    let mut kids = vec![Arc::new(p)];
                    kids.extend(contents);
                    Node::new(self.doc.alloc_id(), NodeKind::Group { children: kids, clip: true })
                } else {
                    p
                }
            }
            Kind::Image { bitmap, origin, x, y } => {
                let (key, width, height) = self.images.get(*bitmap).cloned().flatten()?;
                let xf = Affine::new([x.x, x.y, y.x, y.y, origin.x + dx, origin.y]);
                if !xf.as_coeffs().iter().all(|v| v.is_finite()) {
                    return None;
                }
                let image = ImageObject { key, width, height, xf, link: None, placement: Default::default(), pixels: None };
                Node::new(self.doc.alloc_id(), NodeKind::Image(Box::new(image)))
            }
            Kind::Text { text, origin, x, y } => {
                let xf = Affine::new([x.x, x.y, y.x, y.y, origin.x + dx, origin.y]);
                if !xf.as_coeffs().iter().all(|v| v.is_finite()) {
                    return None;
                }
                let mut obj = TextObject::point(Point::ZERO, "", CharStyle::default());
                obj.runs = text.runs.iter().map(|r| TextRun::new(r.text.clone(), self.char_style(r, text.tracking))).collect();
                // The last paragraph has no paragraph mark in VectorCraft.
                if let Some(last) = obj.runs.last_mut()
                    && last.text.ends_with('\n')
                {
                    last.text.pop();
                }
                obj.runs.retain(|r| !r.text.is_empty());
                let mut para = ParaStyle::default();
                // CorelDRAW's 100 % line spacing is the first font's Windows line height (OS/2
                // usWinAscent + usWinDescent: 1.361 em for Barlow, and lines 0.56 × 1.36 em apart in
                // the samples); 1.2 em when the font can't be read.
                let height = obj.runs.first().map_or(1.2, |r| win_line_height(&r.style.font_family, &r.style.font_style));
                let leading = text.line_spacing * height * 100.0;
                if leading.is_finite() && (1.0..=500.0).contains(&leading) {
                    para.justification.auto_leading = leading;
                }
                obj.para = para;
                obj.paras = Vec::new();
                obj.xf = xf;
                Node::new(self.doc.alloc_id(), NodeKind::Text(Box::new(obj)))
            }
        };
        n.opacity = if o.opacity.is_finite() { o.opacity.clamp(0.0, 1.0) as f32 } else { 1.0 };
        Some(n)
    }

    /// A run's character style; `tracking` in thousandths of an em.
    fn char_style(&mut self, r: &vectorcraft_cdr::text::TextRun, tracking: f64) -> CharStyle {
        let mut st = CharStyle { font_style: font_style(r.weight, r.italic), ..CharStyle::default() };
        if !r.font.is_empty() {
            st.font_family = r.font.clone();
            // CorelDRAW names some faces as families ("Barlow Medium"); where only the shorter
            // family is installed, its last word is the style.
            let db = vectorcraft_text::FontDb::global();
            if db.styles(&r.font).is_empty()
                && let Some((family, word)) = r.font.rsplit_once(' ')
                && STYLE_WORDS.iter().any(|w| w.eq_ignore_ascii_case(word))
                && !db.styles(family).is_empty()
            {
                st.font_family = family.to_string();
                st.font_style = if r.italic { format!("{word} Italic") } else { word.to_string() };
            }
        }
        st.size = if r.size.is_finite() { r.size.clamp(0.1, 1296.0) } else { 12.0 };
        st.tracking = if tracking.is_finite() { tracking.clamp(-1000.0, 10000.0) } else { 0.0 };
        st.fill = match &r.fill {
            Some(Fill::Solid(c)) => Paint::solid(color(*c)),
            Some(Fill::Fountain(g)) => {
                self.warn("fountain fills on text import as their first colour");
                Paint::solid(color(g.from))
            }
            None => Paint::None,
        };
        if let Some(o) = &r.outline
            && o.width.is_finite()
            && o.width > 0.0
        {
            st.stroke = Paint::solid(color(o.color));
            st.stroke_width = o.width.min(MAX_OUTLINE);
        }
        st
    }

    fn appearance(&mut self, o: &cd::Object, bounds: Rect) -> Appearance {
        let mut items = Vec::new();
        if let Some(f) = &o.fill {
            items.push(AppearanceItem::Fill(FillLayer::new(self.paint(f, bounds))));
        }
        if let Some(s) = &o.outline
            && s.width.is_finite()
        {
            items.push(AppearanceItem::Stroke(StrokeLayer::new(Paint::solid(color(s.color)), s.width.clamp(0.0, MAX_OUTLINE))));
        }
        Appearance { items, ..Appearance::default() }
    }

    /// `f` on an object whose bounds are `bounds`.
    ///
    /// A radial fountain (type 2) has its second colour at its centre and its first at its edge,
    /// a radius of half the object's longer side, its centre moved by the centre offsets (percent
    /// of the object's size, y up). That reading matches the previews CorelDRAW saves in its files;
    /// a linear one (type 1) runs from its first colour to its second at its angle.
    fn paint(&mut self, f: &Fill, bounds: Rect) -> Paint {
        match f {
            Fill::Solid(c) => Paint::solid(color(*c)),
            Fill::Fountain(g) => {
                let finite = |v: f64| if v.is_finite() { v } else { 0.0 };
                let mut gp = match g.kind {
                    2 => {
                        let stops = vec![GradientStop::new(0.0, color(g.to)), GradientStop::new(1.0, color(g.from))];
                        let mut gp = GradientPaint::new(Gradient::new(GradientKind::Radial, stops));
                        let c = bounds.center();
                        let (ox, oy) = (finite(g.center.0).clamp(-100.0, 100.0), finite(g.center.1).clamp(-100.0, 100.0));
                        let start = Point::new(c.x + ox / 100.0 * bounds.width(), c.y - oy / 100.0 * bounds.height());
                        let r = (bounds.width().max(bounds.height()) / 2.0).max(1e-6);
                        gp.geom = Some(GradientGeom { start, end: Point::new(start.x + r, start.y), aspect: 1.0, focal: None });
                        gp
                    }
                    k => {
                        if k != 1 {
                            self.warn("conical, square and other fountain fills import as linear gradients");
                        }
                        let stops = vec![GradientStop::new(0.0, color(g.from)), GradientStop::new(1.0, color(g.to))];
                        GradientPaint::new(Gradient::new(GradientKind::Linear, stops))
                    }
                };
                gp.angle = finite(g.angle);
                Paint::Gradient(Box::new(gp))
            }
        }
    }
}

fn color(c: cd::Color) -> Color {
    let f = |v: f64| if v.is_finite() { v.clamp(0.0, 1.0) as f32 } else { 0.0 };
    match c {
        cd::Color::Rgb(r, g, b) => Color::Rgb { r: f(r), g: f(g), b: f(b) },
        cd::Color::Cmyk(c, m, y, k) => Color::Cmyk { c: f(c), m: f(m), y: f(y), k: f(k) },
    }
}

/// The subpaths as a path, `dx` points right; `None` when a coordinate isn't finite.
fn path(subpaths: &[cd::Subpath], dx: f64) -> Option<BezPath> {
    let pt = |p: cd::Point| (p.x.is_finite() && p.y.is_finite()).then(|| Point::new(p.x + dx, p.y));
    let mut bp = BezPath::new();
    for s in subpaths {
        bp.move_to(pt(s.start)?);
        for seg in &s.segments {
            match *seg {
                Segment::Line(p) => bp.line_to(pt(p)?),
                Segment::Cubic(a, b, p) => bp.curve_to(pt(a)?, pt(b)?, pt(p)?),
            }
        }
        if s.closed {
            bp.close_path();
        }
    }
    Some(bp)
}

/// A bitmap as PNG bytes.
fn png(b: &vectorcraft_cdr::bitmap::Bitmap) -> Option<Vec<u8>> {
    let buf = image::RgbaImage::from_raw(b.width, b.height, b.rgba.clone())?;
    let mut out = Vec::new();
    buf.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
    Some(out)
}

/// Style names a CorelDRAW family name may end with.
const STYLE_WORDS: &[&str] = &["Thin", "ExtraLight", "Light", "Book", "Regular", "Medium", "SemiBold", "Bold", "ExtraBold", "Black", "Heavy"];

/// The face name of a CSS weight.
fn font_style(weight: u16, italic: bool) -> String {
    let w = match weight {
        ..=150 => "Thin",
        151..=250 => "ExtraLight",
        251..=350 => "Light",
        351..=450 => "Regular",
        451..=550 => "Medium",
        551..=650 => "SemiBold",
        651..=750 => "Bold",
        751..=850 => "ExtraBold",
        _ => "Black",
    };
    match (w, italic) {
        ("Regular", true) => "Italic".into(),
        (w, true) => format!("{w} Italic"),
        (w, false) => w.into(),
    }
}

/// A face's Windows line height in ems, 1.2 when it can't be found.
fn win_line_height(family: &str, style: &str) -> f64 {
    let Some(face) = vectorcraft_text::FontDb::global().face(family, style) else { return 1.2 };
    let upem = face.units_per_em();
    face.win_line_height().map(|h| h / upem).filter(|h| h.is_finite() && (0.5..=3.0).contains(h)).unwrap_or(1.2)
}
