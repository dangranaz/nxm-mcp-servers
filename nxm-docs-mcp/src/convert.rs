//! Document conversion core.
//!
//! Two directions, both dependency-light and offline:
//! - [`pdf_to_md`]: extract text from a PDF into Markdown (best-effort, text layer only).
//! - [`md_to_html`]: render Markdown to a standalone HTML document.
//!
//! This module has no knowledge of MCP or JSON-RPC — it is a plain library so it
//! can be unit-tested and reused independently.

use anyhow::{Context, Result};
use lopdf::Object;
use pulldown_cmark::{html, Options, Parser};
use std::collections::BTreeMap;
use std::path::Path;

use crate::md_to_typst;
use crate::world::NxmWorld;

/// A line of text extracted from a PDF page with the font size it was set in.
#[derive(Debug)]
struct Line {
    text: String,
    font_size: f32,
}

/// How to turn raw string bytes shown by a font into Unicode text.
/// Borrows font bytes from `doc`, hence the lifetime.
enum TextDecoder<'a> {
    /// ToUnicode CMap: cid (big-endian, `cid_bytes` wide) -> Unicode text.
    /// `widths` maps cid -> glyph advance in 1000-unit em space (for accurate
    /// intra-line spacing); `default_width` is used for cids not in the map.
    ToUnicode {
        map: BTreeMap<u32, String>,
        cid_bytes: u8,
        widths: BTreeMap<u32, f32>,
        default_width: f32,
    },
    /// lopdf's built-in encoding (simple fonts, WinAnsi, etc.).
    Lopdf(lopdf::Encoding<'a>),
}

impl<'a> TextDecoder<'a> {
    /// Decode raw bytes to text. On an undecodable font entry, returns `None`
    /// so callers can drop the segment rather than abort the whole page.
    fn decode(&self, bytes: &[u8]) -> Option<String> {
        match self {
            TextDecoder::ToUnicode { map, cid_bytes, .. } => {
                let step = *cid_bytes as usize;
                let mut out = String::new();
                for chunk in bytes.chunks(step) {
                    let mut cid: u32 = 0;
                    for &b in chunk {
                        cid = (cid << 8) | b as u32;
                    }
                    if let Some(s) = map.get(&cid) {
                        out.push_str(s);
                    }
                }
                (!out.is_empty()).then_some(out)
            }
            TextDecoder::Lopdf(enc) => lopdf::Document::decode_text(enc, bytes).ok(),
        }
    }

    /// Total advance width of `bytes` in em units (1.0 == one font size), so the
    /// caller can multiply by the font size to get the drawn width. Returns
    /// `None` for encodings we can't measure precisely.
    fn measure(&self, bytes: &[u8]) -> Option<f32> {
        match self {
            TextDecoder::ToUnicode {
                cid_bytes,
                widths,
                default_width,
                ..
            } => {
                let step = *cid_bytes as usize;
                let mut total = 0.0_f32;
                for chunk in bytes.chunks(step) {
                    let mut cid: u32 = 0;
                    for &b in chunk {
                        cid = (cid << 8) | b as u32;
                    }
                    total += widths.get(&cid).copied().unwrap_or(*default_width);
                }
                Some(total / 1000.0)
            }
            TextDecoder::Lopdf(_) => None,
        }
    }
}

/// Build a decoder for a font dictionary. Prefers a ToUnicode CMap when the
/// font has one (and we can parse it); otherwise falls back to lopdf's encoding.
///
/// lopdf's own `get_font_encoding` bails on ToUnicode CMaps that declare
/// `/CMapType 0` (which Typst emits), so we parse those ourselves — the
/// bfchar/bfrange sections are just hex pairs.
fn font_decoder<'a>(doc: &'a lopdf::Document, font: &'a lopdf::Dictionary) -> Option<TextDecoder<'a>> {
    // Prefer our own ToUnicode decode when a ToUnicode CMap is present and parses.
    if let Some(parsed) = try_tounicode(doc, font) {
        return Some(parsed);
    }
    // Simple fonts / no ToUnicode: let lopdf handle the base encoding.
    font.get_font_encoding(doc).ok().map(TextDecoder::Lopdf)
}

fn try_tounicode<'a>(doc: &'a lopdf::Document, font: &lopdf::Dictionary) -> Option<TextDecoder<'a>> {
    let id = font
        .get(b"ToUnicode")
        .and_then(Object::as_reference)
        .ok()?;
    let Object::Stream(stream) = doc.get_object(id).ok()? else {
        return None;
    };
    let bytes = stream.decompressed_content().ok()?;
    let map = parse_tounicode(&bytes)?;
    let (widths, default_width) = cid_widths(doc, font);
    Some(TextDecoder::ToUnicode {
        map,
        cid_bytes: 2,
        widths,
        default_width,
    })
}

/// Read the glyph advance widths of a Type0/CID font: the descendant font's
/// `/W` array (cid -> width in 1000-unit em space) and `/DW` default. Returns
/// an empty map + a sane default (500) when the font omits them.
fn cid_widths(doc: &lopdf::Document, font: &lopdf::Dictionary) -> (BTreeMap<u32, f32>, f32) {
    let mut widths = BTreeMap::new();
    let mut default_width = 500.0_f32;

    let desc = font
        .get(b"DescendantFonts")
        .ok()
        .and_then(|o| resolve(doc, o))
        .and_then(|o| match o {
            Object::Array(a) => a.first().and_then(|f| resolve(doc, f)),
            _ => None,
        })
        .and_then(|o| o.as_dict().ok());
    let Some(desc) = desc else {
        return (widths, default_width);
    };

    if let Some(dw) = desc.get(b"DW").ok().and_then(number) {
        default_width = dw;
    }

    // `/W` is a mix of `c [w1 w2 ...]` (widths for c, c+1, ...) and
    // `cFirst cLast w` (one width for the whole range).
    let w = desc.get(b"W").ok().and_then(|o| resolve(doc, o));
    if let Some(Object::Array(items)) = w {
        let mut k = 0;
        while k < items.len() {
            let Some(first) = number(&items[k]).map(|f| f as u32) else {
                k += 1;
                continue;
            };
            match items.get(k + 1) {
                Some(Object::Array(list)) => {
                    for (offset, wobj) in list.iter().enumerate() {
                        if let Some(w) = number(wobj) {
                            widths.insert(first + offset as u32, w);
                        }
                    }
                    k += 2;
                }
                Some(obj) => {
                    let last = number(obj).map(|f| f as u32).unwrap_or(first);
                    let w = items.get(k + 2).and_then(number).unwrap_or(default_width);
                    for cid in first..=last {
                        widths.insert(cid, w);
                    }
                    k += 3;
                }
                None => break,
            }
        }
    }

    (widths, default_width)
}

/// Resolve an object through one indirect reference, if present.
fn resolve<'a>(doc: &'a lopdf::Document, obj: &'a Object) -> Option<&'a Object> {
    match obj {
        Object::Reference(r) => doc.get_object(*r).ok(),
        other => Some(other),
    }
}

/// Parse a ToUnicode CMap's `beginbfchar`/`beginbfrange` sections into a
/// cid -> Unicode mapping. Returns `None` if nothing decodable is found.
fn parse_tounicode(cmap: &[u8]) -> Option<BTreeMap<u32, String>> {
    // CMaps are almost always ASCII; tolerate stray non-UTF-8 bytes so a single
    // odd byte does not sink the whole map.
    let text = String::from_utf8_lossy(cmap);
    let tokens = tokenize_cmap(&text);

    let mut map = BTreeMap::new();
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::BeginBfChar => {
                i += 1;
                i = parse_bfchar(&tokens, i, &mut map);
            }
            Token::BeginBfRange => {
                i += 1;
                i = parse_bfrange(&tokens, i, &mut map);
            }
            _ => i += 1,
        }
    }
    if map.is_empty() {
        None
    } else {
        Some(map)
    }
}

/// A CMap token: only the pieces we care about for bf sections.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    /// `<..>` hex string, stored without the angle brackets.
    Hex(String),
    /// A `[ <..> <..> ... ]` destination array of hex strings.
    Array(Vec<String>),
    BeginBfChar,
    EndBfChar,
    BeginBfRange,
    EndBfRange,
}

/// Tokenize a CMap by scanning `<..>` hex strings and `[..]` arrays directly,
/// so packed entries like `<0003><0020>` (no whitespace) parse correctly.
/// Everything else is scanned as a bareword; only the `begin/end bf*` keywords
/// are kept, other barewords are dropped.
fn tokenize_cmap(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'<' {
            // Hex string up to the next '>'.
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'>' {
                j += 1;
            }
            let hex: String = text[start..j.min(bytes.len())]
                .chars()
                .filter(|ch| ch.is_ascii_hexdigit())
                .collect();
            tokens.push(Token::Hex(hex));
            i = j + 1;
        } else if c == b'[' {
            // Array of hex strings until ']'.
            let mut arr = Vec::new();
            i += 1;
            while i < bytes.len() && bytes[i] != b']' {
                if bytes[i] == b'<' {
                    let start = i + 1;
                    let mut j = start;
                    while j < bytes.len() && bytes[j] != b'>' {
                        j += 1;
                    }
                    let hex: String = text[start..j.min(bytes.len())]
                        .chars()
                        .filter(|ch| ch.is_ascii_hexdigit())
                        .collect();
                    arr.push(hex);
                    i = j + 1;
                } else {
                    i += 1;
                }
            }
            tokens.push(Token::Array(arr));
            i += 1; // skip ']'
        } else {
            // Bareword: read until whitespace or a delimiter.
            let start = i;
            while i < bytes.len()
                && !bytes[i].is_ascii_whitespace()
                && bytes[i] != b'<'
                && bytes[i] != b'['
                && bytes[i] != b']'
            {
                i += 1;
            }
            match &text[start..i] {
                "beginbfchar" => tokens.push(Token::BeginBfChar),
                "endbfchar" => tokens.push(Token::EndBfChar),
                "beginbfrange" => tokens.push(Token::BeginBfRange),
                "endbfrange" => tokens.push(Token::EndBfRange),
                _ => {}
            }
        }
    }
    tokens
}

/// Consume `<src><dst>` pairs until `endbfchar`. Returns the index after the
/// section.
fn parse_bfchar(tokens: &[Token], mut i: usize, map: &mut BTreeMap<u32, String>) -> usize {
    while i < tokens.len() {
        match &tokens[i] {
            Token::EndBfChar => return i + 1,
            Token::Hex(src) => {
                // dst follows; it may be a hex string or (rarely) an array.
                if let Some(next) = tokens.get(i + 1) {
                    if let Some(cid) = hex_to_u32(src) {
                        match next {
                            Token::Hex(dst) => {
                                map.insert(cid, hex_to_string(dst));
                                i += 2;
                                continue;
                            }
                            Token::Array(items) => {
                                let s: String = items.iter().map(|h| hex_to_string(h)).collect();
                                map.insert(cid, s);
                                i += 2;
                                continue;
                            }
                            _ => {}
                        }
                    }
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    i
}

/// Consume `<lo><hi><dst>` or `<lo><hi>[<d0><d1>...]` entries until
/// `endbfrange`. Returns the index after the section.
fn parse_bfrange(tokens: &[Token], mut i: usize, map: &mut BTreeMap<u32, String>) -> usize {
    while i < tokens.len() {
        match &tokens[i] {
            Token::EndBfRange => return i + 1,
            Token::Hex(lo) => {
                let (hi, dst) = match (tokens.get(i + 1), tokens.get(i + 2)) {
                    (Some(Token::Hex(hi)), Some(dst)) => (hi, dst),
                    _ => {
                        i += 1;
                        continue;
                    }
                };
                let (Some(lo_cid), Some(hi_cid)) = (hex_to_u32(lo), hex_to_u32(hi)) else {
                    i += 1;
                    continue;
                };
                match dst {
                    // <lo><hi><base>: cid n maps to base + (n - lo).
                    Token::Hex(base_hex) => {
                        let base_units = hex_to_utf16_units(base_hex);
                        for (offset, cid) in (lo_cid..=hi_cid).enumerate() {
                            let mut units = base_units.clone();
                            if let Some(last) = units.last_mut() {
                                *last = last.wrapping_add(offset as u16);
                            }
                            map.insert(cid, units_to_string(&units));
                        }
                    }
                    // <lo><hi>[<d0><d1>...]: explicit destination per cid.
                    Token::Array(items) => {
                        for (idx, cid) in (lo_cid..=hi_cid).enumerate() {
                            if let Some(h) = items.get(idx) {
                                map.insert(cid, hex_to_string(h));
                            }
                        }
                    }
                    _ => {}
                }
                i += 3;
            }
            _ => i += 1,
        }
    }
    i
}

/// Parse a hex string (already stripped of `<>`) into a code value.
fn hex_to_u32(hex: &str) -> Option<u32> {
    if hex.is_empty() {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

/// Hex string -> sequence of UTF-16 code units.
fn hex_to_utf16_units(hex: &str) -> Vec<u16> {
    // Pad to an even number of nibbles, then to whole 16-bit units.
    let mut cleaned: String = hex.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if cleaned.len() % 4 != 0 {
        // Left-pad so the value stays aligned to 16-bit units.
        let pad = 4 - (cleaned.len() % 4);
        cleaned = "0".repeat(pad) + &cleaned;
    }
    (0..cleaned.len())
        .step_by(4)
        .filter_map(|k| u16::from_str_radix(&cleaned[k..k + 4], 16).ok())
        .collect()
}

/// UTF-16 code units -> Rust String (handles surrogate pairs).
fn units_to_string(units: &[u16]) -> String {
    let mut out = String::with_capacity(units.len());
    let mut it = units.iter();
    while let Some(&u) = it.next() {
        if (0xD800..0xDC00).contains(&u) {
            if let Some(&lo) = it.next() {
                if (0xDC00..0xE000).contains(&lo) {
                    let c = 0x10000 + ((u as u32 - 0xD800) << 10) + (lo as u32 - 0xDC00);
                    out.push(char::from_u32(c).unwrap_or('\u{FFFD}'));
                    continue;
                }
                out.push(char::from_u32(u as u32).unwrap_or('\u{FFFD}'));
                out.push(char::from_u32(lo as u32).unwrap_or('\u{FFFD}'));
                continue;
            }
        }
        out.push(char::from_u32(u as u32).unwrap_or('\u{FFFD}'));
    }
    out
}

/// Hex string -> Rust String via UTF-16 code units.
fn hex_to_string(hex: &str) -> String {
    units_to_string(&hex_to_utf16_units(hex))
}



/// Extract the text layer of a PDF and return it as Markdown.
///
/// Best-effort heuristics:
/// - lines set in a font noticeably larger than the page's median body size are
///   promoted to Markdown headings (`#` / `##`);
/// - pages are separated by a horizontal rule.
///
/// It does **not** perform OCR (scanned/image-only PDFs will yield little or
/// no text) and does not attempt to reconstruct complex layout (tables,
/// multi-column).
pub fn pdf_to_md(pdf_bytes: &[u8]) -> Result<String> {
    let doc = lopdf::Document::load_mem(pdf_bytes)
        .context("failed to parse PDF (is the file a valid PDF?)")?;

    let mut out = String::new();
    // `get_pages` returns a map of page-number -> object id, ordered by page number.
    let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
    let total = pages.len();

    for (idx, &page_number) in pages.iter().enumerate() {
        let lines = page_lines(&doc, page_number).unwrap_or_else(|| {
            // Fallback for pages whose content stream we can't walk.
            fallback_page_lines(&doc, page_number)
        });
        let body_size = median_font_size(&lines);
        for line in &lines {
            if line.text.is_empty() {
                out.push('\n');
                continue;
            }
            let level = heading_level(line.font_size, body_size);
            match level {
                Some(level) => {
                    out.push('\n');
                    out.push_str(&"#".repeat(level));
                    out.push(' ');
                    out.push_str(&line.text);
                    out.push_str("\n\n");
                }
                None => {
                    out.push_str(&line.text);
                    out.push('\n');
                }
            }
        }

        // Separate pages with a horizontal rule, except after the last page.
        if idx + 1 < total && !out.trim_end().is_empty() {
            out.push_str("\n---\n\n");
        }
    }
    let result = out.trim_end().to_string() + "\n";
    Ok(result)
}

/// Convenience wrapper: read a PDF file from disk and convert it to Markdown.
pub fn pdf_file_to_md(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read PDF file: {}", path.display()))?;
    pdf_to_md(&bytes)
}

/// Walk a page's content stream and collect text lines with their font sizes.
///
/// Mirrors lopdf's `extract_text`: text is decoded through each font's encoding
/// (handles ToUnicode CMaps in subset fonts, which is what Typst-produced PDFs
/// use). On top of that it captures the font size set by the matching `Tf` so
/// headings can be detected from relative text size. Returns `None` when the
/// page has no showable text, so callers can fall back.
fn page_lines(doc: &lopdf::Document, page_number: u32) -> Option<Vec<Line>> {
    let page_id = *doc.get_pages().get(&page_number)?;
    let content = doc.get_and_decode_page_content(page_id).ok()?;

    // Name(\.) -> text decoder. Only keep fonts we can actually decode.
    let decoders: BTreeMap<Vec<u8>, TextDecoder<'_>> = doc
        .get_page_fonts(page_id)
        .ok()?
        .into_iter()
        .filter_map(|(name, font)| font_decoder(doc, font).map(|it| (name, it)))
        .collect();

    // Collect positioned text runs, then group them into visual lines by their
    // baseline Y. Typst (and many engines) emit one `Tj` per word wrapped in its
    // own `BT..ET`, so operator-based line breaks alone shatter every word onto
    // its own line. Positions recover the real layout.
    //
    // Positions are tracked with full affine matrices: the graphics-state CTM
    // (from `cm`, with a `q`/`Q` save stack) and the text matrices `Tm`/`Tlm`.
    // A run's device position is `CTM · Tm · (0,0)`, so page flips (`cm` with
    // d = -1) and scaling are honored — a plain translation model collapses
    // such pages onto a single line.
    let mut runs: Vec<TextRun> = Vec::new();
    let mut font_size = 0.0_f32;
    let mut current_decoder: Option<&TextDecoder<'_>> = None;
    let mut leading = 0.0_f32;

    let mut ctm = Mat::IDENTITY;
    let mut ctm_stack: Vec<Mat> = Vec::new();
    // Text matrix (`tm`) and text line matrix (`tlm`), reset at each `BT`.
    let mut tm = Mat::IDENTITY;
    let mut tlm = Mat::IDENTITY;

    for op in &content.operations {
        match op.operator.as_str() {
            "q" => ctm_stack.push(ctm),
            "Q" => {
                if let Some(m) = ctm_stack.pop() {
                    ctm = m;
                }
            }
            "cm" => {
                if let Some(m) = Mat::from_operands(&op.operands) {
                    // New CTM = old CTM · cm.
                    ctm = ctm.mul(&m);
                }
            }
            "BT" => {
                tm = Mat::IDENTITY;
                tlm = Mat::IDENTITY;
            }
            "Tf" => {
                if let (Some(name), Some(size)) = (
                    op.operands.first().and_then(|o| Object::as_name(o).ok()),
                    op.operands.get(1).and_then(number),
                ) {
                    current_decoder = decoders.get(name);
                    font_size = size;
                }
            }
            "TL" => {
                if let Some(l) = op.operands.first().and_then(number) {
                    leading = l;
                }
            }
            "Tm" => {
                if let Some(m) = Mat::from_operands(&op.operands) {
                    tm = m;
                    tlm = m;
                }
            }
            "Td" | "TD" => {
                let (dx, dy) = (
                    op.operands.first().and_then(number).unwrap_or(0.0),
                    op.operands.get(1).and_then(number).unwrap_or(0.0),
                );
                if op.operator == "TD" {
                    leading = -dy;
                }
                // Tlm = translate(dx,dy) · Tlm; Tm = Tlm.
                tlm = Mat::translate(dx, dy).mul(&tlm);
                tm = tlm;
            }
            "T*" => {
                tlm = Mat::translate(0.0, -leading).mul(&tlm);
                tm = tlm;
            }
            "Tj" | "TJ" | "'" | "\"" => {
                if matches!(op.operator.as_str(), "'" | "\"") {
                    // These move to the next line before showing text.
                    tlm = Mat::translate(0.0, -leading).mul(&tlm);
                    tm = tlm;
                }
                let operands: &[Object] = match op.operator.as_str() {
                    "\"" => op.operands.get(2..).unwrap_or(&[]),
                    "'" => std::slice::from_ref(op.operands.last().unwrap_or(&Object::Null)),
                    _ => &op.operands,
                };
                let Some(decoded) =
                    current_decoder.and_then(|dec| decode_operands(dec, operands))
                else {
                    continue;
                };
                let text = decoded.replace(|c: char| c.is_control(), "");
                if text.is_empty() {
                    continue;
                }
                // The run's true drawn width, computed from the font's glyph
                // widths (falling back to a rough per-glyph estimate for
                // encodings we can't measure). Accurate advance is what lets us
                // tell a real inter-word space from tight kerning.
                let advance_em = current_decoder
                    .and_then(|dec| measure_operands(dec, operands))
                    .unwrap_or_else(|| decoded.chars().count() as f32 * 0.5);
                let advance = advance_em * font_size;

                // Device position of the run's origin: CTM · Tm · (0,0).
                let render = ctm.mul(&tm);
                let (dev_x, dev_y) = render.apply(0.0, 0.0);
                // Width in device space along the text x-axis (magnitude of the
                // transformed x basis vector times the text advance).
                let (bx, by) = render.apply(advance, 0.0);
                let dev_w = ((bx - dev_x).powi(2) + (by - dev_y).powi(2)).sqrt();

                runs.push(TextRun {
                    x: dev_x,
                    y: dev_y,
                    width: dev_w,
                    text,
                    font_size,
                });
                // Advance the text matrix along its own x-axis by the run width.
                tm = tm.mul(&Mat::translate(advance, 0.0));
            }
            _ => {}
        }
    }

    if runs.is_empty() {
        return None;
    }
    Some(group_runs_into_lines(runs))
}

/// A 2-D affine transform stored as the six PDF matrix components
/// `[a b c d e f]`, mapping `(x, y)` to `(a·x + c·y + e, b·x + d·y + f)`.
#[derive(Clone, Copy)]
struct Mat {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Mat {
    const IDENTITY: Mat = Mat {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn translate(x: f32, y: f32) -> Mat {
        Mat {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: x,
            f: y,
        }
    }

    /// Build from `[a b c d e f]` operands (as in `cm`/`Tm`).
    fn from_operands(ops: &[Object]) -> Option<Mat> {
        let v: Vec<f32> = ops.iter().filter_map(number).collect();
        if v.len() < 6 {
            return None;
        }
        Some(Mat {
            a: v[0],
            b: v[1],
            c: v[2],
            d: v[3],
            e: v[4],
            f: v[5],
        })
    }

    /// Matrix product `self · other` (apply `other` first, then `self`).
    fn mul(&self, other: &Mat) -> Mat {
        Mat {
            a: other.a * self.a + other.b * self.c,
            b: other.a * self.b + other.b * self.d,
            c: other.c * self.a + other.d * self.c,
            d: other.c * self.b + other.d * self.d,
            e: other.e * self.a + other.f * self.c + self.e,
            f: other.e * self.b + other.f * self.d + self.f,
        }
    }

    /// Transform a point.
    fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }
}

/// A single positioned text run (usually one word) in text space.
struct TextRun {
    x: f32,
    y: f32,
    /// Drawn advance width of the run, in the same units as `x`.
    width: f32,
    text: String,
    font_size: f32,
}

/// Group positioned runs into visual lines: sort by Y (top to bottom) then X
/// (left to right), merge runs sharing a baseline, and emit blank `Line`s where
/// the vertical gap indicates a paragraph break.
fn group_runs_into_lines(mut runs: Vec<TextRun>) -> Vec<Line> {
    // Sort top-to-bottom (larger Y first), then left-to-right.
    runs.sort_by(|a, b| {
        b.y.partial_cmp(&a.y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut lines: Vec<Line> = Vec::new();
    let mut cur_text = String::new();
    let mut cur_y = f32::NAN;
    let mut cur_size = 0.0_f32;
    let mut prev_end_x = 0.0_f32;

    // Collapse internal runs of whitespace to single spaces and trim ends.
    let flush = |cur_text: &str| -> String {
        cur_text.split_whitespace().collect::<Vec<_>>().join(" ")
    };

    for run in runs {
        let same_line = !cur_y.is_nan() && (cur_y - run.y).abs() <= (run.font_size.max(1.0) * 0.5);
        if same_line {
            // Insert a space when there is a visible gap between runs. The
            // threshold is a fraction of the font size: small enough to keep
            // words that wrapped onto the same baseline separated, large enough
            // not to split tight kerning inside a single word.
            let gap = run.x - prev_end_x;
            let ends_space = cur_text.ends_with(char::is_whitespace);
            let starts_space = run.text.starts_with(char::is_whitespace);
            let ends_bullet = cur_text.ends_with(['•', '·', '‣', '▪', '-']);
            // Some layouts butt two logically separate tokens right against each
            // other with no space glyph and no positional gap (e.g. a job title
            // immediately followed by a right-aligned date: "ENGINEER18/03/..").
            // A transition from a letter to a digit across a run boundary is a
            // reliable signal to insert a separating space.
            // Some layouts butt two logically separate tokens right against
            // each other with no space glyph and no positional gap (e.g. a job
            // title immediately followed by a right-aligned date:
            // "ENGINEER18/03/.."). Insert a separating space only when a letter
            // is followed by a *date/number-like* run — at least two leading
            // digits, or a digit group with a date separator — so genuine
            // alphanumeric tokens ("J2EE", "Java8", "H2") are left intact.
            let boundary = cur_text.chars().last().is_some_and(|c| c.is_alphabetic())
                && looks_like_number_or_date(&run.text);
            let need_space = !ends_space
                && !starts_space
                && (ends_bullet || boundary || gap > run.font_size * 0.28);
            if !cur_text.is_empty() && need_space {
                cur_text.push(' ');
            }
            cur_text.push_str(&run.text);
            cur_size = cur_size.max(run.font_size);
        } else {
            // Flush the finished line.
            let text = flush(&cur_text);
            if !text.is_empty() {
                lines.push(Line {
                    text,
                    font_size: cur_size,
                });
                // A large vertical gap between lines starts a new paragraph.
                if !cur_y.is_nan() && (cur_y - run.y).abs() > run.font_size.max(1.0) * 1.6 {
                    lines.push(Line {
                        text: String::new(),
                        font_size: 0.0,
                    });
                }
            }
            cur_text = run.text.clone();
            cur_y = run.y;
            cur_size = run.font_size;
        }
        prev_end_x = run.x + run.width;
        if !same_line {
            cur_y = run.y;
        }
    }
    let text = flush(&cur_text);
    if !text.is_empty() {
        lines.push(Line {
            text,
            font_size: cur_size,
        });
    }
    lines
}

/// Whether a run begins like a standalone number or date (e.g. "18/03/2018",
/// "2023", "31/12"), as opposed to an alphanumeric token that happens to start
/// with a digit (e.g. "2EE" in "J2EE"). Used to decide whether a letter->digit
/// run boundary needs a separating space.
fn looks_like_number_or_date(s: &str) -> bool {
    let s = s.trim_start();
    // Must start with a digit.
    if !s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return false;
    }
    let leading_digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    let after = s.chars().nth(leading_digits);
    let has_date_sep = matches!(after, Some('/') | Some('-') | Some('.') | Some(':'));
    // A single digit immediately followed by a letter (e.g. "2EE") is an
    // alphanumeric token, not a number.
    let digit_then_letter = leading_digits == 1 && matches!(after, Some(c) if c.is_alphabetic());
    !digit_then_letter && (leading_digits >= 2 || has_date_sep)
}

/// Decode the text-showing operands of `Tj`/`TJ` through a font decoder.
/// Returns `None` if no decodable string was produced.
fn decode_operands(decoder: &TextDecoder, operands: &[Object]) -> Option<String> {
    let mut text = String::new();
    for operand in operands {
        match operand {
            Object::String(bytes, _) => {
                if let Some(s) = decoder.decode(bytes) {
                    text.push_str(&s);
                }
            }
            Object::Array(arr) => {
                if let Some(s) = decode_operands(decoder, arr) {
                    text.push_str(&s);
                }
            }
            Object::Integer(i) => {
                // Large negative adjustments in a TJ array denote a space.
                if *i < -100 {
                    text.push(' ');
                }
            }
            Object::Real(f)
                if *f < -100.0 => {
                    text.push(' ');
                }
            _ => {}
        }
    }
    (!text.is_empty()).then_some(text)
}

/// Total advance width of `Tj`/`TJ` operands in em units (1.0 == one font
/// size). Mirrors `decode_operands` but sums glyph widths and applies the
/// numeric position adjustments of a `TJ` array (which are in thousandths of
/// an em, subtracted from the advance). Returns `None` if the decoder can't
/// measure the font.
fn measure_operands(decoder: &TextDecoder, operands: &[Object]) -> Option<f32> {
    let mut em = 0.0_f32;
    let mut measured_any = false;
    for operand in operands {
        match operand {
            Object::String(bytes, _) => {
                let w = decoder.measure(bytes)?;
                em += w;
                measured_any = true;
            }
            Object::Array(arr) => {
                em += measure_operands(decoder, arr)?;
                measured_any = true;
            }
            // TJ adjustments: positive values move left (reduce advance),
            // negative move right (add advance); both are /1000 em.
            Object::Integer(i) => em -= *i as f32 / 1000.0,
            Object::Real(f) => em -= *f / 1000.0,
            _ => {}
        }
    }
    measured_any.then_some(em.max(0.0))
}

/// Last-resort extraction when the content stream can't be walked.
fn fallback_page_lines(doc: &lopdf::Document, page_number: u32) -> Vec<Line> {
    doc.extract_text(&[page_number])
        .unwrap_or_default()
        .lines()
        .map(|l| Line {
            text: normalize_extracted_text(l),
            font_size: 0.0,
        })
        .collect()
}

fn number(obj: &Object) -> Option<f32> {
    match obj {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(f) => Some(*f),
        _ => None,
    }
}

/// The document's body-text font size: the size most characters are set in.
/// Uses the mode (by total character count) rather than the median, so a
/// document with many headings relative to body lines still resolves the true
/// body size instead of drifting toward a heading size.
fn median_font_size(lines: &[Line]) -> f32 {
    // Bucket sizes (rounded to 0.5pt) weighted by how much text they carry.
    let mut weight: BTreeMap<u32, usize> = BTreeMap::new();
    for l in lines {
        if l.text.is_empty() || l.font_size <= 0.0 {
            continue;
        }
        let key = (l.font_size * 2.0).round() as u32;
        *weight.entry(key).or_insert(0) += l.text.chars().count();
    }
    let Some((&key, _)) = weight.iter().max_by_key(|(_, &w)| w) else {
        return 0.0;
    };
    key as f32 / 2.0
}

/// Decide the Markdown heading level from a font size relative to body size.
/// Two fixed bands; tune the ratios if output quality on real documents needs it.
fn heading_level(size: f32, body: f32) -> Option<usize> {
    if size <= 0.0 || body <= 0.0 {
        return None;
    }
    let ratio = size / body;
    if ratio >= 1.35 {
        Some(1)
    } else if ratio >= 1.12 {
        Some(2)
    } else {
        None
    }
}

/// Collapse the noisy whitespace that PDF text extraction often produces.
fn normalize_extracted_text(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        // Collapse runs of internal whitespace to single spaces.
        let collapsed = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
        lines.push(collapsed);
    }
    // Drop leading/trailing empty lines but keep single blank lines between blocks.
    let joined = lines.join("\n");
    joined.trim().to_string()
}

/// Render a Markdown string to a standalone HTML document.
///
/// A true Markdown→PDF conversion requires a PDF rendering engine (layout,
/// fonts, pagination) which is intentionally out of scope for this small,
/// dependency-light tool. Producing clean HTML lets any browser or `wkhtmltopdf`
/// / "Print to PDF" step complete the last mile without pulling a heavy native
/// rendering stack into this crate.
#[allow(dead_code)]
pub fn md_to_html(markdown: &str, title: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(markdown, options);
    let mut body = String::new();
    html::push_html(&mut body, parser);

    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<title>{title}</title>\n\
<style>body{{max-width:48rem;margin:2rem auto;padding:0 1rem;\
font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;\
line-height:1.6}}pre{{background:#f5f5f5;padding:1rem;overflow:auto}}\
code{{font-family:ui-monospace,monospace}}table{{border-collapse:collapse}}\
td,th{{border:1px solid #ddd;padding:.4rem .6rem}}</style>\n\
</head>\n<body>\n{body}</body>\n</html>\n",
        title = escape_html(title),
        body = body,
    )
}

/// Convenience wrapper: read a Markdown file from disk and render it to HTML.
#[allow(dead_code)]
pub fn md_file_to_html(path: &Path) -> Result<String> {
    let markdown = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read Markdown file: {}", path.display()))?;
    let title = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    Ok(md_to_html(&markdown, title))
}

/// Minimal HTML escaping for text placed into element content / title.
#[allow(dead_code)]
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Convert a Markdown string to a native PDF document.
///
/// Uses Typst as the typesetting engine. The pipeline is:
/// Markdown → Typst markup → Typst compilation → PDF export.
pub fn md_to_pdf(markdown: &str) -> Result<Vec<u8>> {
    let typst_src = md_to_typst::convert(markdown);
    let world = NxmWorld::new(&typst_src);
    let warned = typst::compile(&world);
    let document = warned.output.map_err(|errors| {
        let msgs: Vec<String> = errors.iter().map(|e| format!("{:?}", e)).collect();
        anyhow::anyhow!("Typst compilation failed:\n{}", msgs.join("\n"))
    })?;
    let pdf_bytes = typst_pdf::pdf(&document, &Default::default())
        .map_err(|errors| {
        let msgs: Vec<String> = errors.iter().map(|e| format!("{:?}", e)).collect();
            anyhow::anyhow!("PDF export failed:\n{}", msgs.join("\n"))
        })?;
    Ok(pdf_bytes)
}

/// Convenience wrapper: read a Markdown file from disk and convert it to PDF.
pub fn md_file_to_pdf(path: &Path) -> Result<Vec<u8>> {
    let markdown = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read Markdown file: {}", path.display()))?;
    md_to_pdf(&markdown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md_to_html_renders_heading_and_paragraph() {
        let html = md_to_html("# Title\n\nHello **world**.", "doc");
        assert!(html.contains("<h1>Title</h1>"));
        assert!(html.contains("<strong>world</strong>"));
        assert!(html.contains("<!DOCTYPE html>"));
    }

    #[test]
    fn md_to_html_supports_tables() {
        let md = "| a | b |\n|---|---|\n| 1 | 2 |";
        let html = md_to_html(md, "t");
        assert!(html.contains("<table>"));
        assert!(html.contains("<td>1</td>"));
    }

    #[test]
    fn md_to_html_escapes_title() {
        let html = md_to_html("x", "a<b>&c");
        assert!(html.contains("<title>a&lt;b&gt;&amp;c</title>"));
    }

    #[test]
    fn normalize_collapses_whitespace() {
        let got = normalize_extracted_text("  hello    world  \n\n  foo \t bar ");
        assert_eq!(got, "hello world\n\nfoo bar");
    }

    #[test]
    fn pdf_to_md_rejects_garbage() {
        let err = pdf_to_md(b"not a pdf at all").unwrap_err();
        assert!(err.to_string().contains("PDF"));
    }

    /// Round-trip: Markdown → native PDF (Typst) → Markdown. Headings and
    /// multi-page separation must survive.
    #[test]
    fn md_pdf_md_roundtrip_keeps_headings_and_pages() {
        let md = "# Big Title\n\nSome body text here.\n\n## A Subheading\n\n\\pagebreak More text on page two.\n";
        let pdf = md_to_pdf(md).expect("md_to_pdf");
        let back = pdf_to_md(&pdf).expect("pdf_to_md");

        assert!(back.contains("# Big Title"), "h1 lost:\n{back}");
        assert!(back.contains("## A Subheading"), "h2 lost:\n{back}");
        assert!(back.contains("Some body text here"), "body lost:\n{back}");
        assert!(back.contains("More text on page two"), "page 2 lost:\n{back}");
        assert!(back.contains("---"), "page separator missing:\n{back}");
        // Body text must NOT be promoted to a heading.
        assert!(!back.contains("# Some body text"), "body promoted:\n{back}");
    }

    /// Certain roundtrip on CV-like content: a realistic document (name,
    /// sections, multi-word sentences, contact details, bullet list) must
    /// survive MD -> native PDF (Typst, Type0/Identity-H fonts with ToUnicode
    /// CMaps — the exact structure that defeats lopdf's own extractor and real
    /// Europass CVs) -> MD, with words kept together on their lines.
    #[test]
    fn md_pdf_md_roundtrip_complex_cv_like_document() {
        let md = "\
# Daniele Oddo

Results-oriented IT professional with comprehensive experience across the public sector.

## About Me

I specialize in J2EE technologies and open-source frameworks such as the Spring Framework and JPA.

## Skills

- Java, J2EE, Spring Boot, Spring Cloud
- Angular, TypeScript, JavaScript
- Docker, GitLab, PostgreSQL
";
        let pdf = md_to_pdf(md).expect("md_to_pdf");
        // Sanity: this really is the complex font path (lopdf can't extract it).
        assert!(pdf.starts_with(b"%PDF-"), "not a PDF");

        let back = pdf_to_md(&pdf).expect("pdf_to_md");

        // Headings survive at the right level.
        assert!(back.contains("# Daniele Oddo"), "name/h1 lost:\n{back}");
        assert!(back.contains("## About Me"), "About Me h2 lost:\n{back}");
        assert!(back.contains("## Skills"), "Skills h2 lost:\n{back}");

        // Multi-word sentences stay intact on their line (words not shattered
        // one-per-line). Long paragraphs may wrap across visual lines, so assert
        // on phrase segments that share a line rather than the whole sentence.
        assert!(
            back.contains("Results-oriented IT professional with comprehensive experience"),
            "intro sentence broken:\n{back}"
        );
        assert!(
            back.contains("across the public sector"),
            "intro tail broken:\n{back}"
        );
        assert!(
            back.contains("I specialize in J2EE technologies and open-source frameworks"),
            "about sentence broken:\n{back}"
        );

        // Skill items survive as real multi-word text, with tightly-kerned
        // tokens (e.g. "J2EE") kept intact thanks to glyph-width-accurate
        // spacing.
        assert!(back.contains("Java, J2EE, Spring Boot, Spring Cloud"), "skill 1 lost:\n{back}");
        assert!(back.contains("Angular, TypeScript, JavaScript"), "skill 2 lost:\n{back}");
        assert!(back.contains("Docker, GitLab, PostgreSQL"), "skill 3 lost:\n{back}");

        // No word-per-line shattering: the extracted text must not contain a
        // long run of single-word lines.
        let single_word_lines = back
            .lines()
            .filter(|l| {
                let t = l.trim().trim_start_matches('#').trim();
                !t.is_empty() && !t.contains(' ') && t.len() < 12
            })
            .count();
        assert!(
            single_word_lines < 6,
            "too many single-word lines ({single_word_lines}), text is shattered:\n{back}"
        );
    }

    /// A plain-text-only document (no headings) must not gain spurious headings.
    #[test]
    fn pdf_to_md_plain_text_no_headings() {
        let md = "Just a paragraph of plain text with no headings at all.\n";
        let pdf = md_to_pdf(md).expect("md_to_pdf");
        let back = pdf_to_md(&pdf).expect("pdf_to_md");
        assert!(!back.contains("# "), "spurious heading:\n{back}");
        assert!(back.contains("Just a paragraph"), "text lost:\n{back}");
    }

    #[test]
    fn heading_level_thresholds() {
        assert_eq!(heading_level(24.0, 12.0), Some(1)); // 2.0x
        assert_eq!(heading_level(15.0, 12.0), Some(2)); // 1.25x
        assert_eq!(heading_level(13.0, 12.0), None);    // ~1.08x
        assert_eq!(heading_level(12.0, 0.0), None);     // unknown body size
    }

    #[test]
    fn number_or_date_detection() {
        // Dates / numbers that must be split off a preceding word.
        assert!(looks_like_number_or_date("18/03/2018"));
        assert!(looks_like_number_or_date("2023"));
        assert!(looks_like_number_or_date("31/12"));
        assert!(looks_like_number_or_date("01/01/2023 - CURRENT"));
        // Alphanumeric tokens that must NOT be split.
        assert!(!looks_like_number_or_date("2EE"));       // the tail of J2EE
        assert!(!looks_like_number_or_date("8"));          // Java8 tail
        assert!(!looks_like_number_or_date("x"));
        assert!(!looks_like_number_or_date("word"));
    }
}

