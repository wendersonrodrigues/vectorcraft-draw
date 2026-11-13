//! Text objects (object type 4) hold a `txsm` chunk:
//!
//! - a header whose matrix (six f64 from byte 0x31) repeats the object's own transform;
//! - styles, each a u32 length and a JSON object, with some bytes between them (a language tag
//!   among them). The first is the base style: `character` (`latin.font`, `latin.weight`,
//!   `latin.italic`, `latin.size` in document units, `fill`, `outline`) and `paragraph`
//!   (`interLineSpacing` and `interCharSpacing`, millionths). The others are character styles
//!   whose `character` members replace the base style's;
//! - then the characters: a u32 count, 8 bytes per character (bytes 2..4, a u16, name the style
//!   it uses, 0 for the base style), a u32 byte length and the text, one byte per character in
//!   Windows-1252. `\r` ends a paragraph.
//!
//! The origin of text space, the object's (0, 0), is where the first line's baseline starts:
//! every text object's recorded bounding box starts there and rises 0.73 em above it.
//!
//! Weights are powers of four: 64 for regular, 256 for families named Medium, 1024 for
//! SemiBold, 4096 for bold faces; they are read as CSS weights 100 (1) to 900 (65536).

use serde_json::{Map, Value};

use crate::Error;
use crate::model::{Fill, Outline};
use crate::style;

/// Most bytes of one text chunk.
const MAX_TXSM: usize = 16 << 20;
/// Most styles one text chunk may hold.
const MAX_STYLES: usize = 4096;
/// Most characters one text object may hold.
const MAX_CHARS: usize = 1 << 20;
/// Most bytes of JSON tried while looking for one text object's styles: every `{` after a
/// plausible length is a candidate, so hostile data could otherwise make the search quadratic.
const MAX_TRIED: usize = 32 << 20;

/// A run of characters in one style.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    /// Paragraphs end with `\n`.
    pub text: String,
    pub font: String,
    /// CSS weight, 100 to 900.
    pub weight: u16,
    pub italic: bool,
    /// Points.
    pub size: f64,
    pub fill: Option<Fill>,
    pub outline: Option<Outline>,
}

/// A text object's characters and paragraph spacing.
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub runs: Vec<TextRun>,
    /// Line spacing, 1 for CorelDRAW's 100 %.
    pub line_spacing: f64,
    /// Character spacing in thousandths of an em (inferred from CorelDRAW's percentage of a space,
    /// taken as a quarter em).
    pub tracking: f64,
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// The JSON styles in `t`, in order, and where the last one ends.
fn styles(t: &[u8]) -> (Vec<Map<String, Value>>, usize) {
    let mut out = Vec::new();
    let mut end = 0usize;
    let mut at = 4usize;
    let mut tried = 0usize;
    while out.len() < MAX_STYLES && tried < MAX_TRIED {
        let Some(i) = t.get(at..).and_then(|s| s.iter().position(|&b| b == b'{')) else { break };
        let start = at + i;
        at = start + 1;
        let Some(len) = u32_at(t, start - 4).map(|l| l as usize) else { continue };
        if len < 2 || len > t.len() - start {
            continue;
        }
        let Some(raw) = t.get(start..start + len) else { continue };
        tried = tried.saturating_add(len);
        let raw = raw.split(|&b| b == 0).next().unwrap_or(raw);
        if let Ok(Value::Object(m)) = serde_json::from_slice::<Value>(raw) {
            out.push(m);
            end = start + len;
            at = end;
        }
    }
    (out, end)
}

/// Windows-1252 to Unicode.
fn cp1252(b: u8) -> char {
    const HIGH: [u16; 32] = [
        0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x8D, 0x017D, 0x8F, 0x90, 0x2018,
        0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178,
    ];
    match b {
        0x80..=0x9F => HIGH.get(usize::from(b - 0x80)).and_then(|&u| char::from_u32(u32::from(u))).unwrap_or('\u{FFFD}'),
        _ => char::from(b),
    }
}

fn obj<'m>(m: &'m Map<String, Value>, k: &str) -> Option<&'m Map<String, Value>> {
    m.get(k).and_then(Value::as_object)
}

fn num(m: Option<&Map<String, Value>>, k: &str) -> Option<f64> {
    m?.get(k)?.as_str()?.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// The CSS weight of a CorelDRAW weight.
fn css_weight(w: f64) -> u16 {
    if !(w.is_finite() && w >= 1.0) {
        return 400;
    }
    let k = (w.log2() / 2.0).round().clamp(0.0, 8.0);
    100 + 100 * k as u16
}

/// The style a character with style index `i` uses: the base's `character`, its members
/// replaced by style `i`'s.
fn character(styles: &[Map<String, Value>], i: usize) -> Map<String, Value> {
    let mut c = styles.first().and_then(|b| obj(b, "character")).cloned().unwrap_or_default();
    if i > 0
        && let Some(o) = styles.get(i).and_then(|s| obj(s, "character"))
    {
        for (k, v) in o {
            match (c.get_mut(k), v) {
                (Some(Value::Object(base)), Value::Object(over)) => {
                    for (k2, v2) in over {
                        base.insert(k2.clone(), v2.clone());
                    }
                }
                _ => {
                    c.insert(k.clone(), v.clone());
                }
            }
        }
    }
    c
}

/// Read a `txsm` chunk; `units_per_point` converts sizes.
pub(crate) fn read(t: &[u8], units_per_point: f64) -> Result<Text, Error> {
    if t.len() > MAX_TXSM {
        return Err(Error::Limit("a text object is too large"));
    }
    let bad = Error::Malformed("text characters");
    let (styles, end) = styles(t);
    if styles.is_empty() {
        return Err(Error::Malformed("text without a style"));
    }
    let n = u32_at(t, end).ok_or(bad)? as usize;
    if n > MAX_CHARS {
        return Err(Error::Limit("a text object has too many characters"));
    }
    let records = t.get(end + 4..end + 4 + 8 * n).ok_or(bad)?;
    let len = u32_at(t, end + 4 + 8 * n).ok_or(bad)? as usize;
    let bytes = t.get(end + 8 + 8 * n..end + 8 + 8 * n + len.min(MAX_CHARS)).ok_or(bad)?;
    // One byte per character; if the counts disagree every character takes the base style.
    let index = |i: usize| -> usize {
        if len != n {
            return 0;
        }
        records.get(8 * i + 2..8 * i + 4).and_then(|b| b.try_into().ok()).map(u16::from_le_bytes).map(usize::from).unwrap_or(0)
    };
    let mut runs: Vec<(usize, String)> = Vec::new();
    for (i, &b) in bytes.iter().enumerate() {
        let c = match b {
            b'\r' | b'\n' => '\n',
            0 => continue,
            b => cp1252(b),
        };
        let s = index(i);
        match runs.last_mut() {
            Some((last, text)) if *last == s => text.push(c),
            _ => runs.push((s, c.to_string())),
        }
    }
    let base_para = styles.first().and_then(|b| obj(b, "paragraph"));
    let line_spacing = num(base_para, "interLineSpacing").map(|v| v / 1e6).filter(|v| *v > 0.0).unwrap_or(1.0);
    let tracking = num(base_para, "interCharSpacing").map(|v| v / 4000.0).unwrap_or(0.0);
    let runs = runs
        .into_iter()
        .map(|(s, text)| {
            let c = character(&styles, s);
            let latin = obj(&c, "latin");
            let st = style::style(&c, units_per_point);
            TextRun {
                text,
                font: latin.and_then(|l| l.get("font")).and_then(Value::as_str).unwrap_or_default().to_string(),
                weight: css_weight(num(latin, "weight").unwrap_or(64.0)),
                italic: num(latin, "italic").is_some_and(|v| v != 0.0),
                size: num(latin, "size").map(|v| v / units_per_point).filter(|v| *v > 0.0).unwrap_or(12.0),
                fill: st.fill,
                outline: st.outline,
            }
        })
        .collect();
    Ok(Text { runs, line_spacing, tracking })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_are_powers_of_four() {
        assert_eq!([1.0, 16.0, 64.0, 256.0, 1024.0, 4096.0, 65536.0].map(css_weight), [100, 300, 400, 500, 600, 700, 900]);
        assert_eq!(css_weight(f64::NAN), 400);
    }

    #[test]
    fn windows_1252() {
        assert_eq!(cp1252(0xEA), 'ê');
        assert_eq!(cp1252(0x80), '€');
        assert_eq!(cp1252(b'a'), 'a');
    }
}
