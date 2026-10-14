//! Atlas fork: AutoCAD SHX text.
//!
//! AutoCAD draws text in SHX fonts as vector strokes, which no extractor can read back. So the
//! text is still searchable, its PDF export adds an invisible annotation over each string, with
//! `/T (AutoCAD SHX Text)` and the string as `/Contents`. On electrical drawings that is most of
//! the useful text (equipment tags, ratings, panel names) and hundreds or thousands of
//! annotations per sheet. They are drawing text, not comments: [`is_shx_author`] keeps them out
//! of the comment list, and [`append_page_text`] adds them to the page's text so find, copy, the
//! text tools and "text inside this box" all see them.

use crate::text::{PageText, TextGlyph};
use hayro::hayro_interpret::hayro_syntax::object::{Array, Dict, Rect as PdfRect, String as PdfString};
use hayro::hayro_interpret::hayro_syntax::page::Page;
use kurbo::{Affine, Point, Rect, Vec2};

/// The `/T` AutoCAD gives its SHX text annotations.
pub const SHX_AUTHOR: &str = "AutoCAD SHX Text";
/// At most this many SHX strings are read from one page (the largest real sheets have a few
/// thousand).
const MAX_STRINGS: usize = 100_000;
/// At most this many characters are kept from one string.
const MAX_CHARS: usize = 2_000;

/// Whether an annotation author marks AutoCAD SHX text.
pub fn is_shx_author(author: &str) -> bool {
    author.trim() == SHX_AUTHOR
}

/// Append the page's SHX strings to `text`, one line each, after the content's own text.
/// `initial` maps PDF user space to the page's view space (`Page::initial_transform(true)`).
pub(crate) fn append_page_text(text: &mut PageText, page: &Page<'_>, initial: Affine) {
    let Some(annots) = page.raw().get::<Array<'_>>(b"Annots") else { return };
    let mut strings = 0usize;
    for annot in annots.iter::<Dict<'_>>() {
        let Some(author) = annot.get::<PdfString<'_>>(b"T") else { continue };
        if !is_shx_author(&decode(&author)) {
            continue;
        }
        strings = strings.saturating_add(1);
        if strings > MAX_STRINGS {
            break;
        }
        let (Some(contents), Some(rect)) = (annot.get::<PdfString<'_>>(b"Contents"), annot.get::<PdfRect>(b"Rect")) else { continue };
        push_string(text, &decode(&contents), Rect::new(rect.x0, rect.y0, rect.x1, rect.y1), initial);
    }
}

fn decode(s: &PdfString<'_>) -> String {
    pdfcraft_cos::PdfString::literal(s.as_bytes()).to_text()
}

/// Lay `s` out evenly across `rect` (user space): left to right when the box is wider than
/// tall, else bottom to top (AutoCAD's text rotated a quarter turn anticlockwise).
fn push_string(text: &mut PageText, s: &str, rect: Rect, initial: Affine) {
    let chars: Vec<char> = s.chars().take(MAX_CHARS).collect();
    let rect = rect.abs();
    let finite = [rect.x0, rect.y0, rect.x1, rect.y1].iter().all(|v| v.is_finite());
    if !finite || rect.width() <= 0.0 || rect.height() <= 0.0 || chars.iter().all(|c| c.is_whitespace()) {
        return;
    }
    let vertical = rect.height() > rect.width();
    let step = if vertical { rect.height() } else { rect.width() } / chars.len() as f64;
    let along = if vertical { Vec2::new(0.0, 1.0) } else { Vec2::new(1.0, 0.0) };
    let direction = to_view_direction(initial, along);
    let line = text.line_of.last().map_or(0, |l| l.saturating_add(1));
    let mut gap = false;
    let mut first = true;
    for (i, c) in chars.iter().enumerate() {
        if c.is_whitespace() {
            gap = true;
            continue;
        }
        let a = step * i as f64;
        let b = step * (i as f64 + 1.0);
        // Corners in user space: the edge towards the next line first, in the writing direction.
        let corners = if vertical {
            [Point::new(rect.x1, rect.y0 + a), Point::new(rect.x1, rect.y0 + b), Point::new(rect.x0, rect.y0 + b), Point::new(rect.x0, rect.y0 + a)]
        } else {
            [Point::new(rect.x0 + a, rect.y0), Point::new(rect.x0 + b, rect.y0), Point::new(rect.x0 + b, rect.y1), Point::new(rect.x0 + a, rect.y1)]
        };
        let quad = corners.map(|p| {
            let v = initial * p;
            [v.x as f32, v.y as f32]
        });
        let xs = quad.map(|q| q[0]);
        let ys = quad.map(|q| q[1]);
        let min = |v: [f32; 4]| v.into_iter().fold(f32::INFINITY, f32::min);
        let max = |v: [f32; 4]| v.into_iter().fold(f32::NEG_INFINITY, f32::max);
        text.glyphs.push(TextGlyph { text: c.to_string(), rect: [min(xs), min(ys), max(xs), max(ys)], quad, direction });
        text.line_of.push(line);
        text.space_before.push(gap && !first);
        gap = false;
        first = false;
    }
}

/// A user-space direction as a unit vector in view space.
fn to_view_direction(initial: Affine, v: Vec2) -> [f32; 2] {
    let [a, b, c, d, _, _] = initial.as_coeffs();
    let (x, y) = (a * v.x + c * v.y, b * v.x + d * v.y);
    let len = x.hypot(y);
    if len.is_finite() && len > 0.0 { [(x / len) as f32, (y / len) as f32] } else { [1.0, 0.0] }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 200 × 100 pt page: user space y-up, view space y-down.
    fn initial() -> Affine {
        Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, 100.0])
    }

    #[test]
    fn the_author_must_be_autocads() {
        assert!(is_shx_author("AutoCAD SHX Text"));
        assert!(is_shx_author(" AutoCAD SHX Text "));
        assert!(!is_shx_author("autocad shx text"));
        assert!(!is_shx_author("Estimator"));
    }

    #[test]
    fn horizontal_strings_spread_across_their_box_with_word_gaps() {
        let mut text = PageText::default();
        push_string(&mut text, "60A SW", Rect::new(10.0, 20.0, 70.0, 30.0), initial());
        assert_eq!(text.plain_text(), "60A SW");
        assert_eq!(text.find("60a sw"), vec![0..5]);
        assert_eq!(text.space_before, [false, false, false, true, false]);
        assert_eq!(text.glyphs[0].rect, [10.0, 70.0, 20.0, 80.0]);
        assert_eq!(text.glyphs[0].direction, [1.0, 0.0]);
        // Each string is its own line, after what is already there.
        push_string(&mut text, "MDP", Rect::new(100.0, 20.0, 130.0, 30.0), initial());
        assert_eq!(text.line_of, [0, 0, 0, 0, 0, 1, 1, 1]);
    }

    #[test]
    fn tall_boxes_read_bottom_to_top() {
        let mut text = PageText::default();
        push_string(&mut text, "1600A", Rect::new(10.0, 10.0, 20.0, 60.0), initial());
        assert_eq!(text.glyphs.len(), 5);
        // The first character is at the bottom of the box (largest y in view space).
        assert_eq!(text.glyphs[0].rect, [10.0, 80.0, 20.0, 90.0]);
        assert_eq!(text.glyphs[4].rect, [10.0, 40.0, 20.0, 50.0]);
        assert_eq!(text.glyphs[0].direction, [0.0, -1.0]);
    }

    /// A one-page drawing: an SHX string over the equipment, and a real reviewer's comment.
    fn drawing() -> std::sync::Arc<Vec<u8>> {
        use lopdf::{Document, Object, Stream, dictionary};
        let mut doc = Document::with_version("1.7");
        let pages = doc.new_object_id();
        let shx = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Square", "F" => 64,
            "Rect" => vec![100.into(), 50.into(), 220.into(), 62.into()],
            "T" => Object::string_literal("AutoCAD SHX Text"), "Contents" => Object::string_literal("1600A D.S.#1"),
        });
        let comment = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Square",
            "Rect" => vec![10.into(), 10.into(), 40.into(), 40.into()],
            "T" => Object::string_literal("Engineer"), "Contents" => Object::string_literal("Revise and resubmit"),
        });
        let content = doc.add_object(Stream::new(dictionary! {}, Vec::new()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages, "Contents" => content,
            "MediaBox" => vec![0.into(), 0.into(), 300.into(), 200.into()],
            "Annots" => vec![Object::Reference(shx), Object::Reference(comment)],
        });
        doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1}.into());
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
        doc.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        std::sync::Arc::new(bytes)
    }

    #[test]
    fn shx_annotations_are_searchable_text_and_not_comments() {
        use hayro::hayro_interpret::InterpreterSettings;
        use hayro::hayro_interpret::hayro_syntax::Pdf;
        let bytes = drawing();
        let pdf = Pdf::new(bytes.clone()).unwrap();
        let text = crate::text::extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
        assert_eq!(text.plain_text(), "1600A D.S.#1");
        assert_eq!(text.find("d.s.#1").len(), 1);
        // In view space (y down on a 200 pt page): the box spans y 138..150.
        assert_eq!(text.glyphs[0].rect, [100.0, 138.0, 110.0, 150.0]);
        let info = crate::inspect(bytes, None).unwrap();
        let authors: Vec<Option<&str>> = info.annotations.iter().map(|a| a.author.as_deref()).collect();
        assert_eq!(authors, [Some("Engineer")]);
    }

    #[test]
    fn degenerate_input_adds_nothing() {
        let mut text = PageText::default();
        for (s, rect) in [
            ("", Rect::new(0.0, 0.0, 10.0, 10.0)),
            ("   ", Rect::new(0.0, 0.0, 10.0, 10.0)),
            ("A", Rect::new(0.0, 0.0, 0.0, 10.0)),
            ("A", Rect::new(0.0, 0.0, f64::NAN, 10.0)),
            ("A", Rect::new(0.0, 0.0, f64::INFINITY, 10.0)),
        ] {
            push_string(&mut text, s, rect, initial());
        }
        assert!(text.is_empty());
        // Corners given the wrong way round are fine.
        push_string(&mut text, "A", Rect::new(10.0, 10.0, 0.0, 0.0), initial());
        assert_eq!(text.glyphs.len(), 1);
    }
}
