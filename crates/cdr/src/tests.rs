use crate::model::{Color, Fill, Kind, Segment};
use crate::synth::{self, curve, loda, object, rectangle, style};
use crate::{Error, Limits, is_cdr, read};

const ID: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
const PT: f64 = 254_000.0 / 72.0;

fn solid(r: u8, g: u8, b: u8) -> Vec<u8> {
    style(&format!(
        r#"{{"transparency":{{}},"fill":{{"type":"1","primaryColor":"RGB255,USER,{r},{g},{b},100,00000000-0000-0000-0000-000000000000","winding":"0"}},"outline":{{"width":"0","color":"RGB255,USER,0,0,0,100,0"}}}}"#
    ))
}

fn triangle(style_: Vec<u8>) -> synth::Chunk {
    let nodes = [(0, 0, 0x0c), (1_000_000, 0, 0x40), (0, 1_000_000, 0x48)];
    object(loda(3, &[(0x1e, curve(&nodes)), (0xc9, style_)]), ID)
}

#[test]
fn sniffs_packages() {
    let f = synth::simple(vec![]);
    assert!(is_cdr(&f));
    assert!(!is_cdr(b"PK\x03\x04 not a corel package"));
    assert!(!is_cdr(b"RIFF\0\0\0\0CDRA"));
}

#[test]
fn reads_a_triangle_on_the_page() {
    let d = read(&synth::simple(vec![triangle(solid(255, 0, 0))]), Limits::default()).unwrap();
    assert_eq!(d.pages.len(), 1, "the master page isn't a page");
    let p = &d.pages[0];
    assert!((p.width - 9_800_000.0 / PT).abs() < 1e-9 && (p.height - 10_000_000.0 / PT).abs() < 1e-9);
    assert_eq!(p.layers.len(), 1, "guide layers are left out");
    assert_eq!(p.layers[0].name, "Layer 1");
    let o = &p.layers[0].objects[0];
    assert_eq!(o.fill, Some(Fill::Solid(Color::Rgb(1.0, 0.0, 0.0))));
    assert_eq!(o.outline, None, "zero-width outlines are none");
    let Kind::Path { subpaths, even_odd } = &o.kind else { panic!("not a path") };
    assert!(even_odd);
    let s = &subpaths[0];
    assert!(s.closed);
    // The origin is the page centre and y points up.
    assert!((s.start.x - p.width / 2.0).abs() < 1e-9 && (s.start.y - p.height / 2.0).abs() < 1e-9);
    let Segment::Line(q) = s.segments[1] else { panic!("not a line") };
    assert!((q.y - (p.height / 2.0 - 1_000_000.0 / PT)).abs() < 1e-9, "y up becomes y down");
}

#[test]
fn cubic_nodes_take_the_two_controls_before_them() {
    let nodes = [(0, 0, 0x0c), (100, 0, 0xc0), (200, 100, 0xc0), (200, 200, 0x88)];
    let f = synth::simple(vec![object(loda(3, &[(0x1e, curve(&nodes)), (0xc9, solid(0, 0, 0))]), ID)]);
    let d = read(&f, Limits::default()).unwrap();
    let Kind::Path { subpaths, .. } = &d.pages[0].layers[0].objects[0].kind else { panic!() };
    assert!(matches!(subpaths[0].segments[..], [Segment::Cubic(..)]));
    assert!(subpaths[0].closed);
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
}

#[test]
fn rectangles_are_their_size_through_their_transform() {
    let m = [0.5, 0.0, 1000.0, 0.0, 0.5, -2000.0];
    let f = synth::simple(vec![object(loda(1, &[(0x1e, rectangle(4000.0, -6000.0)), (0xc9, solid(0, 0, 255))]), m)]);
    let d = read(&f, Limits::default()).unwrap();
    let p = &d.pages[0];
    let Kind::Path { subpaths, .. } = &p.layers[0].objects[0].kind else { panic!() };
    let s = &subpaths[0];
    assert_eq!(s.segments.len(), 3);
    let Segment::Line(far) = s.segments[1] else { panic!() };
    assert!((far.x - (p.width / 2.0 + 3000.0 / PT)).abs() < 1e-9);
    assert!((far.y - (p.height / 2.0 + 5000.0 / PT)).abs() < 1e-9);
}

#[test]
fn objects_are_listed_topmost_first() {
    let f = synth::simple(vec![triangle(solid(1, 1, 1)), triangle(solid(2, 2, 2))]);
    let d = read(&f, Limits::default()).unwrap();
    let fills: Vec<_> = d.pages[0].layers[0].objects.iter().map(|o| o.fill.clone()).collect();
    let c = |v: u8| Some(Fill::Solid(Color::Rgb(f64::from(v) / 255.0, f64::from(v) / 255.0, f64::from(v) / 255.0)));
    assert_eq!(fills, vec![c(2), c(1)], "bottom first after reading");
}

#[test]
fn transparency_outlines_and_cmyk() {
    let j = r#"{"transparency":{"uniformTransparency":"0.25"},"fill":{"type":"1","primaryColor":"CMYK,USER,0,100,50,0,100,0"},"outline":{"width":"25400","color":"CMYK255,USER,0,0,0,255,100,0"}}"#;
    let d = read(&synth::simple(vec![triangle(style(j))]), Limits::default()).unwrap();
    let o = &d.pages[0].layers[0].objects[0];
    assert!((o.opacity - 0.75).abs() < 1e-12);
    assert_eq!(o.fill, Some(Fill::Solid(Color::Cmyk(0.0, 1.0, 0.5, 0.0))));
    let out = o.outline.as_ref().unwrap();
    assert!((out.width - 25400.0 / PT).abs() < 1e-9);
    assert_eq!(out.color, Color::Cmyk(0.0, 0.0, 0.0, 1.0));
}

#[test]
fn fountain_fills_keep_both_colours() {
    let j = r#"{"fill":{"type":"2","fountainType":"2","primaryColor":"RGB255,USER,0,6,114,100,0","secondaryColor":"RGB255,USER,0,117,255,100,0","angle":"0","xCenterOffset":"43","yCenterOffset":"-25"}}"#;
    let d = read(&synth::simple(vec![triangle(style(j))]), Limits::default()).unwrap();
    let Some(Fill::Fountain(g)) = &d.pages[0].layers[0].objects[0].fill else { panic!() };
    assert_eq!(g.kind, 2);
    assert_eq!(g.center, (43.0, -25.0));
}

#[test]
fn text_and_bitmaps_are_reported() {
    let f = synth::simple(vec![object(loda(4, &[]), ID), object(loda(5, &[]), ID), object(loda(5, &[]), ID)]);
    let d = read(&f, Limits::default()).unwrap();
    assert!(d.warnings.contains(&"text without its characters".to_string()), "{:?}", d.warnings);
    assert!(d.warnings.contains(&"bitmaps whose placement couldn't be read (2)".to_string()), "{:?}", d.warnings);
}

#[test]
fn groups_nest() {
    let f = synth::simple(vec![synth::list(b"grp ", vec![triangle(solid(0, 0, 0)), triangle(solid(0, 0, 0))])]);
    let d = read(&f, Limits::default()).unwrap();
    let Kind::Group(kids) = &d.pages[0].layers[0].objects[0].kind else { panic!() };
    assert_eq!(kids.len(), 2);
}

#[test]
fn older_riff_files_are_refused_plainly() {
    assert!(matches!(read(b"RIFF\x04\0\0\0CDR9", Limits::default()), Err(Error::Unsupported(_))));
}

#[test]
fn deep_groups_hit_the_limit() {
    let mut c = triangle(solid(0, 0, 0));
    for _ in 0..100 {
        c = synth::list(b"grp ", vec![c]);
    }
    assert!(matches!(read(&synth::simple(vec![c]), Limits::default()), Err(Error::Limit(_))));
}

#[test]
fn hostile_bytes_never_panic() {
    let f = synth::simple(vec![triangle(solid(9, 9, 9)), object(loda(1, &[(0x1e, rectangle(1.0, 1.0))]), ID)]);
    // Every truncation, and every single-byte change at a stride.
    for n in 0..f.len() {
        let _ = read(&f[..n], Limits::default());
    }
    for i in (0..f.len()).step_by(3) {
        for v in [0x00, 0x7f, 0xff] {
            let mut g = f.clone();
            g[i] = v;
            let _ = read(&g, Limits::default());
        }
    }
    let mut bad = curve(&[]);
    bad[..4].copy_from_slice(&i32::MAX.to_le_bytes());
    let _ = read(&synth::simple(vec![object(loda(3, &[(0x1e, bad)]), ID)]), Limits::default());
}

/// CorelDRAW files their owners lent for testing, kept out of the repository: set
/// `VECTORCRAFT_CDR_SAMPLES` to a folder of them.
#[test]
fn real_files() {
    let Ok(dir) = std::env::var("VECTORCRAFT_CDR_SAMPLES") else { return };
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("cdr")) {
            let bytes = std::fs::read(&p).unwrap();
            let d = read(&bytes, Limits::default()).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            let objects: usize = d.pages.iter().flat_map(|p| &p.layers).map(|l| l.objects.len()).sum();
            eprintln!(
                "{}: {} pages, {objects} top-level objects, {} bitmaps, warnings {:?}",
                p.display(),
                d.pages.len(),
                d.bitmaps.len(),
                d.warnings
            );
            assert!(objects > 0, "{}", p.display());
        }
    }
}

#[test]
fn powerclip_containers_take_their_contents_from_clpt() {
    use synth::{layer, leaf, list, mcfg, package, page};
    let mut link = 7u32.to_le_bytes().to_vec();
    link.extend_from_slice(&[0; 12]);
    let container = object(loda(1, &[(0x1e, rectangle(1000.0, 1000.0)), (0xc9, solid(0, 0, 0)), (0x1f45, link)]), ID);
    let f = package(vec![
        list(
            b"doc ",
            vec![
                leaf(b"mcfg", mcfg(9_800_000, 10_000_000)),
                list(b"clpt", vec![list(b"grp ", vec![leaf(b"spnd", 7u32.to_le_bytes().to_vec()), triangle(solid(255, 0, 0))])]),
            ],
        ),
        page(vec![]),
        page(vec![layer(0, "Layer 1", vec![container])]),
    ]);
    let d = read(&f, Limits::default()).unwrap();
    let o = &d.pages[0].layers[0].objects[0];
    assert_eq!(o.clipped.len(), 1);
    assert_eq!(o.clipped[0].fill, Some(Fill::Solid(Color::Rgb(1.0, 0.0, 0.0))));
}

fn bitmap_doc(alpha: bool, id_in_object: i32) -> Vec<u8> {
    use synth::{bitmap_record, image, layer, leaf, list, mcfg, package, page};
    // 2 × 2 pixels, stored bottom row first: bottom row red, green; top row blue, white.
    let bgr = [vec![0, 0, 255, 0, 255, 0], vec![255, 0, 0, 255, 255, 255]];
    let a = [vec![255, 128], vec![0, 255]];
    let rec = bitmap_record(6, 2, 2, &bgr, alpha.then_some(&a[..]));
    let obj = object(loda(5, &[(0x1e, image([0, 2_000, 4_000, 0], 2, 2, id_in_object))]), [1.0, 0.0, 1_000.0, 0.0, 1.0, 0.0]);
    package(vec![
        list(b"bmpt", vec![leaf(b"bmp ", rec)]),
        list(b"doc ", vec![leaf(b"mcfg", mcfg(9_800_000, 10_000_000))]),
        page(vec![]),
        page(vec![layer(0, "Layer 1", vec![obj])]),
    ])
}

#[test]
fn bitmaps_are_read_top_row_first_with_their_mask() {
    let d = read(&bitmap_doc(true, 6), Limits::default()).unwrap();
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    let b = &d.bitmaps[0];
    assert_eq!((b.width, b.height), (2, 2));
    // Top row: blue (alpha 0), white; bottom row: red, green (alpha 128).
    assert_eq!(b.rgba, vec![0, 0, 255, 0, 255, 255, 255, 255, 255, 0, 0, 255, 0, 255, 0, 128]);
    let p = &d.pages[0];
    let Kind::Image { bitmap, origin, x, y } = &p.layers[0].objects[0].kind else { panic!("not an image") };
    assert_eq!(*bitmap, 0);
    // The box's top-left corner (0, 2000) moved 1000 right, y up to y down from the page's top.
    assert!((origin.x - (p.width / 2.0 + 1_000.0 / PT)).abs() < 1e-9);
    assert!((origin.y - (p.height / 2.0 - 2_000.0 / PT)).abs() < 1e-9);
    assert!((x.x - 2_000.0 / PT).abs() < 1e-9 && x.y.abs() < 1e-12, "one pixel right is half the box's width");
    assert!(y.x.abs() < 1e-12 && (y.y - 1_000.0 / PT).abs() < 1e-9, "one pixel down is half its height, downwards");
}

#[test]
fn bitmaps_without_a_mask_are_opaque_and_missing_ones_are_reported() {
    let d = read(&bitmap_doc(false, 6), Limits::default()).unwrap();
    assert!(d.bitmaps[0].rgba.chunks(4).all(|p| p[3] == 255));
    let d = read(&bitmap_doc(false, 9), Limits::default()).unwrap();
    assert!(d.bitmaps.is_empty() && d.pages[0].layers[0].objects.is_empty());
    assert!(d.warnings.iter().any(|w| w.contains("pixels are missing")), "{:?}", d.warnings);
}

#[test]
fn hostile_bitmaps_never_panic() {
    let f = bitmap_doc(true, 6);
    for i in 0..f.len() {
        for v in [0x00, 0x7f, 0xff] {
            let mut g = f.clone();
            g[i] = v;
            let _ = read(&g, Limits::default());
        }
    }
}

const BASE_STYLE: &str = r#"{"character":{"latin":{"font":"Barlow","weight":"64","italic":"0","size":"254000"},"fill":{"type":"1","primaryColor":"RGB255,USER,0,22,131,100,0"}},"paragraph":{"interLineSpacing":"560000","interCharSpacing":"-40000"}}"#;
const BOLD_WHITE: &str = r#"{"character":{"latin":{"weight":"4096"},"fill":{"type":"1","primaryColor":"RGB255,USER,255,255,255,100,0"}}}"#;

#[test]
fn text_runs_take_their_character_styles() {
    use synth::{text_object, txsm};
    let t = txsm(&[BASE_STYLE, "{\"character\":{}}", BOLD_WHITE], b"m\xeas\rok", &[0, 0, 2, 2, 2, 2]);
    let d = read(&synth::simple(vec![text_object(t, [1.0, 0.0, 1_000.0, 0.0, 1.0, 2_000.0])]), Limits::default()).unwrap();
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    let p = &d.pages[0];
    let Kind::Text { text, origin, x, y } = &p.layers[0].objects[0].kind else { panic!("not text") };
    assert_eq!(text.runs.len(), 2);
    assert_eq!(text.runs[0].text, "mê");
    assert_eq!(text.runs[1].text, "s\nok", "\\r ends a paragraph");
    assert_eq!((text.runs[0].font.as_str(), text.runs[0].weight, text.runs[1].weight), ("Barlow", 400, 700));
    assert_eq!(text.runs[1].font, "Barlow", "an override keeps the base's other members");
    assert!((text.runs[0].size - 72.0).abs() < 1e-9, "254000 units are an inch");
    assert_eq!(text.runs[1].fill, Some(Fill::Solid(Color::Rgb(1.0, 1.0, 1.0))));
    assert!((text.line_spacing - 0.56).abs() < 1e-12 && (text.tracking + 10.0).abs() < 1e-12);
    // The first baseline starts at the object's origin; text space is y down in points.
    assert!((origin.x - (p.width / 2.0 + 1_000.0 / PT)).abs() < 1e-9 && (origin.y - (p.height / 2.0 - 2_000.0 / PT)).abs() < 1e-9);
    assert!((x.x - 1.0).abs() < 1e-9 && (y.y - 1.0).abs() < 1e-9 && x.y.abs() < 1e-12 && y.x.abs() < 1e-12);
}

#[test]
fn hostile_text_never_panics() {
    use synth::{text_object, txsm};
    let f = synth::simple(vec![text_object(txsm(&[BASE_STYLE, BOLD_WHITE], b"abc", &[0, 1, 9]), ID)]);
    for i in 0..f.len() {
        for v in [0x00, 0x7b, 0xff] {
            let mut g = f.clone();
            g[i] = v;
            let _ = read(&g, Limits::default());
        }
    }
    // A chunk full of braces after plausible lengths stays bounded.
    let mut t = vec![0u8; 8];
    for _ in 0..20_000 {
        t.extend_from_slice(&1_000_000u32.to_le_bytes());
        t.push(b'{');
    }
    let _ = read(&synth::simple(vec![text_object(t, ID)]), Limits::default());
}
