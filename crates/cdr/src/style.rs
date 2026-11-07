//! Object styles: the style argument holds a u32 (1), the JSON's length, then a JSON object
//! whose `fill`, `outline` and `transparency` members describe the paint. Values are strings.
//!
//! Colours are written `MODEL,PALETTE,components…,tint,id`:
//! `RGB255` (three 0–255 components), `CMYK` (four 0–100) and `CMYK255` (four 0–255).
//! Fill `type` is 0 (none), 1 (uniform) or 2 (fountain); fountain fills give `fountainType`
//! (1 and 2 seen), `primaryColor`, `secondaryColor`, `angle` and centre offsets in percent.

use serde_json::{Map, Value};

use crate::model::{Color, Fill, Fountain, Outline};

/// Most bytes of style JSON parsed for one object.
const MAX_JSON: usize = 1 << 20;

pub(crate) struct Style {
    pub fill: Option<Fill>,
    pub outline: Option<Outline>,
    /// 0 (opaque) to 1 (invisible).
    pub transparency: f64,
    /// What the style asked for that isn't read.
    pub skipped: Vec<&'static str>,
}

pub(crate) fn parse(arg: &[u8]) -> Option<Map<String, Value>> {
    let len = u32::from_le_bytes(arg.get(4..8)?.try_into().ok()?) as usize;
    if len > MAX_JSON {
        return None;
    }
    let text = arg.get(8..8 + len.min(arg.len().saturating_sub(8)))?;
    let text = text.split(|&b| b == 0).next().unwrap_or(text);
    match serde_json::from_slice(text).ok()? {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

fn s<'v>(m: &'v Map<String, Value>, k: &str) -> Option<&'v str> {
    m.get(k).and_then(Value::as_str)
}

fn num(m: &Map<String, Value>, k: &str) -> Option<f64> {
    s(m, k).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite())
}

pub(crate) fn color(text: &str) -> Option<Color> {
    let mut parts = text.split(',');
    let model = parts.next()?;
    let _palette = parts.next()?;
    let mut next = || parts.next().and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite());
    let clamp = |v: f64, max: f64| (v / max).clamp(0.0, 1.0);
    match model {
        "RGB255" => Some(Color::Rgb(clamp(next()?, 255.0), clamp(next()?, 255.0), clamp(next()?, 255.0))),
        "CMYK" => Some(Color::Cmyk(clamp(next()?, 100.0), clamp(next()?, 100.0), clamp(next()?, 100.0), clamp(next()?, 100.0))),
        "CMYK255" => Some(Color::Cmyk(clamp(next()?, 255.0), clamp(next()?, 255.0), clamp(next()?, 255.0), clamp(next()?, 255.0))),
        _ => None,
    }
}

/// The paint of `json`; `units_per_point` converts outline widths.
pub(crate) fn style(json: &Map<String, Value>, units_per_point: f64) -> Style {
    let mut skipped = Vec::new();
    let fill = json.get("fill").and_then(Value::as_object).and_then(|f| match s(f, "type") {
        Some("1") => match s(f, "primaryColor").and_then(color) {
            Some(c) => Some(Fill::Solid(c)),
            None => {
                skipped.push("a fill colour model");
                None
            }
        },
        Some("2") => {
            let (Some(from), Some(to)) = (s(f, "primaryColor").and_then(color), s(f, "secondaryColor").and_then(color)) else {
                skipped.push("a fountain fill colour model");
                return None;
            };
            if f.get("intermediateColors").and_then(Value::as_array).is_some_and(|a| !a.is_empty()) {
                skipped.push("fountain fill intermediate colours");
            }
            Some(Fill::Fountain(Fountain {
                kind: num(f, "fountainType").unwrap_or(1.0) as u32,
                from,
                to,
                angle: num(f, "angle").unwrap_or(0.0),
                center: (num(f, "xCenterOffset").unwrap_or(0.0), num(f, "yCenterOffset").unwrap_or(0.0)),
            }))
        }
        Some("0") | None => None,
        Some(_) => {
            skipped.push("pattern, texture or PostScript fills");
            None
        }
    });
    let outline = json.get("outline").and_then(Value::as_object).and_then(|o| {
        let w = num(o, "width").unwrap_or(0.0);
        if w <= 0.0 || units_per_point <= 0.0 {
            return None;
        }
        let c = s(o, "color").and_then(color)?;
        Some(Outline { width: w / units_per_point, color: c })
    });
    let transparency =
        json.get("transparency").and_then(Value::as_object).and_then(|t| num(t, "uniformTransparency")).map(|v| v.clamp(0.0, 1.0)).unwrap_or(0.0);
    Style { fill, outline, transparency, skipped }
}
