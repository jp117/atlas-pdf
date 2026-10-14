//! Takeoff marks: the boxes an estimator draws over equipment on a drawing to record whether
//! Atlas quotes it.
//!
//! A mark is a standard Square annotation, so Acrobat, Bluebeam and every other viewer show it
//! with its colour, subject and text. Atlas's own data sits in a private `/AtlasTakeoff`
//! dictionary that other programs keep but ignore:
//!
//! ```text
//! /Subtype /Square  /Subj (Atlas Takeoff: Included)  /C [0 0.62 0.24]  /BS << /W 3 /S /S >>
//! /IC [0 0.62 0.24]  /ca 0.2
//! /Contents (LP-1: provide feed-through lugs)
//! /AtlasTakeoff << /Status /Included /Label (LP-1) /Comment (provide feed-through lugs) >>
//! ```
//!
//! The fill is the status colour at [`FILL_OPACITY`] (PDF 2.0's `/ca`, the opacity of fills) inside
//! a solid border, so the equipment under a box stays readable.
//!
//! `/Contents` is a readable summary that every Atlas edit rewrites. When another program has
//! changed it, the changed text is taken as the comment. A Square annotation without the private
//! dictionary still counts as a mark when its subject is one of the Atlas subjects, so a Bluebeam
//! tool set using those subjects produces marks too.

use pdfcraft_annot::{AnnotError, Meta, NewAnnotation, Rgb, Shape, Style};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};
use serde::Serialize;

/// The three takeoff decisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Green: in Atlas's scope.
    Included,
    /// Yellow: in scope, with a note (a clarification, an exception, a question).
    IncludedWithComments,
    /// Red: not in Atlas's scope.
    Excluded,
}

impl Status {
    pub const ALL: [Status; 3] = [Status::Included, Status::IncludedWithComments, Status::Excluded];

    pub fn label(self) -> &'static str {
        match self {
            Status::Included => "Included",
            Status::IncludedWithComments => "Included w/ comments",
            Status::Excluded => "Excluded",
        }
    }

    /// The `/Status` name in the private dictionary.
    pub fn pdf_name(self) -> &'static str {
        match self {
            Status::Included => "Included",
            Status::IncludedWithComments => "IncludedWithComments",
            Status::Excluded => "Excluded",
        }
    }

    pub fn from_pdf_name(name: &[u8]) -> Option<Status> {
        Status::ALL.into_iter().find(|s| s.pdf_name().as_bytes() == name)
    }

    /// `/Subj`: what Acrobat's and Bluebeam's markup lists show as the markup's type.
    pub fn subject(self) -> &'static str {
        match self {
            Status::Included => "Atlas Takeoff: Included",
            Status::IncludedWithComments => "Atlas Takeoff: Included w/ comments",
            Status::Excluded => "Atlas Takeoff: Excluded",
        }
    }

    pub fn from_subject(subject: &str) -> Option<Status> {
        let subject = subject.trim();
        Status::ALL.into_iter().find(|s| s.subject().eq_ignore_ascii_case(subject))
    }

    pub fn color(self) -> Rgb {
        match self {
            Status::Included => [0.0, 0.62, 0.24],
            // A deep yellow: pure yellow disappears on white paper.
            Status::IncludedWithComments => [1.0, 0.76, 0.0],
            Status::Excluded => [0.89, 0.13, 0.13],
        }
    }

    /// Whether drawing this mark should ask for a comment straight away.
    pub fn wants_comment(self) -> bool {
        self == Status::IncludedWithComments
    }

    /// The key that picks this tool: 1, 2, 3.
    pub fn hotkey(self) -> char {
        match self {
            Status::Included => '1',
            Status::IncludedWithComments => '2',
            Status::Excluded => '3',
        }
    }
}

/// Border width of a mark, in points: heavy enough to read on a busy sheet at any zoom.
pub const BORDER_WIDTH: f64 = 3.0;
/// Opacity of a mark's fill: a tint that leaves the equipment under it readable.
pub const FILL_OPACITY: f64 = 0.2;
/// The longest label, comment or author accepted, in bytes.
pub const MAX_TEXT: usize = 4096;
const MAX_ANNOTATIONS: usize = 200_000;
const KEY: &[u8] = b"AtlasTakeoff";

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum TakeoffError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Annotation(#[from] AnnotError),
}
type Result<T> = std::result::Result<T, TakeoffError>;
fn invalid(s: &str) -> TakeoffError {
    TakeoffError::Invalid(s.into())
}

/// The style every mark of a status is drawn with: a solid border filled with the same colour.
/// The fill is see-through only once [`write_fields`] adds `/ca`.
pub fn style(status: Status) -> Style {
    Style { color: status.color(), opacity: 1.0, width: BORDER_WIDTH, fill: Some(status.color()) }
}

/// A mark to add.
#[derive(Clone, Debug, PartialEq)]
pub struct NewMark {
    /// 0-based page index.
    pub page: usize,
    /// PDF user space.
    pub rect: [f64; 4],
    pub status: Status,
    /// What the box is: a panel name, an equipment tag.
    pub label: String,
    pub comment: String,
    pub author: String,
}

/// What a mark says, for [`update`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkText {
    pub status: Status,
    pub label: String,
    pub comment: String,
}

/// A mark read back from the document.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Mark {
    /// 0-based page index.
    pub page: usize,
    /// Index in the page's `/Annots` (what the comment edits address).
    pub index: usize,
    /// `/NM`.
    pub id: String,
    pub status: Status,
    pub label: String,
    pub comment: String,
    pub author: String,
    /// PDF user space, normalised (x0 ≤ x1, y0 ≤ y1).
    pub rect: [f64; 4],
}

/// Every mark in a document.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Listing {
    pub marks: Vec<Mark>,
    /// The annotation limit stopped the scan early.
    pub truncated: bool,
}

impl Listing {
    /// How many marks have each status, in [`Status::ALL`] order.
    pub fn counts(&self) -> [usize; 3] {
        let mut out = [0usize; 3];
        for mark in &self.marks {
            let slot = match mark.status {
                Status::Included => &mut out[0],
                Status::IncludedWithComments => &mut out[1],
                Status::Excluded => &mut out[2],
            };
            *slot = slot.saturating_add(1);
        }
        out
    }
}

/// The readable `/Contents` for a label and comment.
pub fn summary(label: &str, comment: &str) -> String {
    match (label.trim(), comment.trim()) {
        ("", "") => String::new(),
        (label, "") => label.to_owned(),
        ("", comment) => comment.to_owned(),
        (label, comment) => format!("{label}: {comment}"),
    }
}

fn check_text(label: &str, comment: &str, author: &str) -> Result<()> {
    if label.len() > MAX_TEXT || comment.len() > MAX_TEXT || author.len() > MAX_TEXT {
        return Err(invalid("takeoff labels, comments and authors must be at most 4096 bytes"));
    }
    Ok(())
}

fn text(d: &Dict, key: &[u8]) -> String {
    d.get(key).and_then(Object::as_string).map(PdfString::to_text).unwrap_or_default()
}

fn annot(doc: &Document, page: usize, index: usize) -> Result<(ObjRef, Dict)> {
    let p = pdfcraft_model::pages(doc).get(page).cloned().ok_or(AnnotError::NoSuchPage(page))?;
    let missing = || TakeoffError::Annotation(AnnotError::NoSuchAnnotation { page, index });
    let list = doc.resolve(p.dict.get(b"Annots").ok_or_else(missing)?);
    let o = list.as_array().and_then(|a| a.get(index)).ok_or_else(missing)?;
    let r = o.as_ref().ok_or_else(|| invalid("inline annotations can't be edited"))?;
    let d = doc.get(r).as_dict().cloned().ok_or_else(|| invalid("invalid annotation dictionary"))?;
    Ok((r, d))
}

/// Write the Atlas fields: subject, colours, summary and the private dictionary.
fn write_fields(d: &mut Dict, status: Status, label: &str, comment: &str) {
    let [r, g, b] = status.color();
    let colour = || Object::Array(vec![Object::Real(r), Object::Real(g), Object::Real(b)]);
    d.set(b"Subj".to_vec(), PdfString::text(status.subject()));
    d.set(b"C".to_vec(), colour());
    d.set(b"IC".to_vec(), colour());
    d.set(b"ca".to_vec(), Object::Real(FILL_OPACITY));
    d.set(b"Contents".to_vec(), PdfString::text(&summary(label, comment)));
    let mut private = Dict::new();
    private.set(b"Status".to_vec(), Object::name(status.pdf_name()));
    private.set(b"Label".to_vec(), PdfString::text(label.trim()));
    private.set(b"Comment".to_vec(), PdfString::text(comment.trim()));
    d.set(KEY.to_vec(), Object::Dict(private));
}

/// Add a mark; returns its index in the page's `/Annots`.
pub fn add(doc: &mut Document, new: &NewMark, meta: &Meta) -> Result<usize> {
    check_text(&new.label, &new.comment, &new.author)?;
    let annotation = NewAnnotation {
        page: new.page,
        shape: Shape::Rectangle { rect: new.rect },
        style: style(new.status),
        contents: summary(&new.label, &new.comment),
        author: new.author.clone(),
    };
    let index = pdfcraft_annot::add_annotation(doc, &annotation, meta)?;
    let (r, mut d) = annot(doc, new.page, index)?;
    write_fields(&mut d, new.status, &new.label, &new.comment);
    doc.update_dict(r, |target| *target = d).map_err(|e| invalid(&e.to_string()))?;
    pdfcraft_annot::set_appearance(doc, r)?;
    Ok(index)
}

/// Change a mark's status, label and comment, and redraw it in the status colour.
pub fn update(doc: &mut Document, page: usize, index: usize, change: &MarkText, meta: &Meta) -> Result<()> {
    check_text(&change.label, &change.comment, "")?;
    let (r, mut d) = annot(doc, page, index)?;
    if read_status(&d).is_none() {
        return Err(invalid("that comment is not a takeoff mark"));
    }
    write_fields(&mut d, change.status, &change.label, &change.comment);
    if let Some(date) = &meta.date {
        d.set(b"M".to_vec(), PdfString::literal(date.as_bytes().to_vec()));
    }
    doc.update_dict(r, |target| *target = d).map_err(|e| invalid(&e.to_string()))?;
    pdfcraft_annot::set_appearance(doc, r)?;
    Ok(())
}

/// A Square annotation's takeoff status: the private dictionary's, else the subject's.
fn read_status(d: &Dict) -> Option<Status> {
    if d.name(b"Subtype") != Some(b"Square") {
        return None;
    }
    let private = d.get(KEY).and_then(Object::as_dict);
    private.and_then(|p| p.name(b"Status")).and_then(Status::from_pdf_name).or_else(|| Status::from_subject(&text(d, b"Subj")))
}

fn read_rect(d: &Dict) -> Option<[f64; 4]> {
    let a = d.get(b"Rect")?.as_array()?;
    let v: Vec<f64> = a.iter().map(Object::as_f64).collect::<Option<_>>()?;
    let [x0, y0, x1, y1] = <[f64; 4]>::try_from(v).ok()?;
    if ![x0, y0, x1, y1].iter().all(|n| n.is_finite()) {
        return None;
    }
    Some([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)])
}

fn read_mark(d: &Dict, page: usize, index: usize) -> Option<Mark> {
    let status = read_status(d)?;
    let rect = read_rect(d)?;
    let private = d.get(KEY).and_then(Object::as_dict);
    let label = private.map(|p| text(p, b"Label")).unwrap_or_default();
    let stored = private.map(|p| text(p, b"Comment")).unwrap_or_default();
    let contents = text(d, b"Contents");
    let comment = if private.is_some() && contents == summary(&label, &stored) {
        stored
    } else {
        // Edited elsewhere (or never written by Atlas): the visible text is the comment.
        let prefix = format!("{}: ", label.trim());
        let edited = if label.trim().is_empty() { contents.as_str() } else { contents.strip_prefix(&prefix).unwrap_or(&contents) };
        if edited.trim() == label.trim() { String::new() } else { edited.trim().to_owned() }
    };
    Some(Mark { page, index, id: text(d, b"NM"), status, label, comment, author: text(d, b"T"), rect })
}

/// Every takeoff mark, page by page in `/Annots` order.
pub fn list(doc: &Document) -> Listing {
    let mut out = Listing::default();
    let mut seen = 0usize;
    for (page, p) in pdfcraft_model::pages(doc).iter().enumerate() {
        let Some(annots) = p.dict.get(b"Annots") else { continue };
        let annots = doc.resolve(annots);
        let Some(annots) = annots.as_array() else { continue };
        for (index, a) in annots.iter().enumerate() {
            seen = seen.saturating_add(1);
            if seen > MAX_ANNOTATIONS {
                out.truncated = true;
                return out;
            }
            let o = doc.resolve(a);
            if let Some(mark) = o.as_dict().and_then(|d| read_mark(d, page, index)) {
                out.marks.push(mark);
            }
        }
    }
    out
}

/// The marks as CSV (RFC 4180), one row per mark. `sheet` names a 0-based page (its page label
/// or sheet number; empty when unknown).
pub fn to_csv(marks: &[Mark], sheet: &dyn Fn(usize) -> String) -> String {
    let mut out = String::from("Page,Sheet,Status,Label,Comment,Author\r\n");
    for m in marks {
        let row =
            [m.page.saturating_add(1).to_string(), sheet(m.page), m.status.label().to_owned(), m.label.clone(), m.comment.clone(), m.author.clone()];
        let cells: Vec<String> = row.iter().map(|c| csv_cell(c)).collect();
        out.push_str(&cells.join(","));
        out.push_str("\r\n");
    }
    out
}

fn csv_cell(value: &str) -> String {
    // A leading = + - @ makes spreadsheets run the cell as a formula; a quote mark keeps it text.
    let value = if value.starts_with(['=', '+', '-', '@']) { format!("'{value}") } else { value.to_owned() };
    if value.contains([',', '"', '\n', '\r']) { format!("\"{}\"", value.replace('"', "\"\"")) } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_cos::{SaveOptions, write_full, write_incremental};
    use std::sync::Arc;

    fn fixture(extra_annots: &[&str]) -> Document {
        let annots: Vec<String> = (0..extra_annots.len()).map(|i| format!("{} 0 R", i + 5)).collect();
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 2592 1728] >>".into(),
            format!("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Annots [{}] >>", annots.join(" ")),
            "<< /Length 0 >>\nstream\n\nendstream".into(),
        ];
        objects.extend(extra_annots.iter().map(|s| s.to_string()));
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for o in offsets {
            bytes.extend(format!("{o:010} 00000 n \n").as_bytes());
        }
        bytes.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
        Document::open(Arc::new(bytes)).unwrap()
    }

    fn meta(id: &str) -> Meta {
        Meta { date: Some("D:20261014120000Z".into()), id: id.into() }
    }

    fn mark(status: Status, label: &str, comment: &str) -> NewMark {
        NewMark { page: 0, rect: [100.0, 200.0, 40.0, 150.0], status, label: label.into(), comment: comment.into(), author: "Estimator".into() }
    }

    fn reopen(doc: &Document) -> Document {
        Document::open(Arc::new(write_full(doc, &SaveOptions::default()).unwrap())).unwrap()
    }

    #[test]
    fn marks_round_trip_through_a_saved_file() {
        let mut doc = fixture(&[]);
        add(&mut doc, &mark(Status::Included, "LP-1", ""), &meta("a")).unwrap();
        add(&mut doc, &mark(Status::IncludedWithComments, "MDP", "provide feed-through lugs"), &meta("b")).unwrap();
        add(&mut doc, &mark(Status::Excluded, "", "by utility"), &meta("c")).unwrap();
        let listing = list(&reopen(&doc));
        assert!(!listing.truncated);
        assert_eq!(listing.counts(), [1, 1, 1]);
        let m = &listing.marks[1];
        assert_eq!((m.page, m.index, m.id.as_str()), (0, 1, "b"));
        assert_eq!(m.status, Status::IncludedWithComments);
        assert_eq!((m.label.as_str(), m.comment.as_str(), m.author.as_str()), ("MDP", "provide feed-through lugs", "Estimator"));
        // Normalised from the reversed corners we passed in.
        assert_eq!(m.rect, [40.0, 150.0, 100.0, 200.0]);
        assert_eq!(listing.marks[2].label, "");
        assert_eq!(listing.marks[2].comment, "by utility");
    }

    #[test]
    fn marks_are_square_annotations_other_viewers_understand() {
        let mut doc = fixture(&[]);
        let index = add(&mut doc, &mark(Status::Excluded, "T-1", "by others"), &meta("x")).unwrap();
        let (_, d) = annot(&doc, 0, index).unwrap();
        assert_eq!(d.name(b"Subtype"), Some(&b"Square"[..]));
        assert_eq!(text(&d, b"Subj"), "Atlas Takeoff: Excluded");
        assert_eq!(text(&d, b"Contents"), "T-1: by others");
        assert_eq!(text(&d, b"T"), "Estimator");
        assert!(d.get(b"AP").is_some(), "has an appearance stream");
        // A see-through fill in the status colour: the equipment under the box stays readable.
        let colour = |key: &[u8]| -> Vec<f64> { d.get(key).unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect() };
        assert_eq!(colour(b"IC"), Status::Excluded.color().to_vec());
        assert_eq!(d.get(b"ca").and_then(Object::as_f64), Some(FILL_OPACITY));
        assert!(d.get(b"CA").is_none(), "the border stays solid");
    }

    #[test]
    fn update_changes_status_colour_and_text_and_survives_an_incremental_save() {
        let mut doc = fixture(&[]);
        let index = add(&mut doc, &mark(Status::Included, "LP-2", ""), &meta("u")).unwrap();
        let change = MarkText { status: Status::IncludedWithComments, label: "LP-2".into(), comment: "NEMA 3R".into() };
        update(&mut doc, 0, index, &change, &meta("ignored")).unwrap();
        let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
        let reopened = Document::open(Arc::new(bytes)).unwrap();
        let m = &list(&reopened).marks[0];
        assert_eq!((m.status, m.comment.as_str(), m.id.as_str()), (Status::IncludedWithComments, "NEMA 3R", "u"));
        let (_, d) = annot(&reopened, 0, index).unwrap();
        for key in [&b"C"[..], b"IC"] {
            let c: Vec<f64> = d.get(key).unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect();
            assert_eq!(c, Status::IncludedWithComments.color().to_vec(), "{}", String::from_utf8_lossy(key));
        }
    }

    #[test]
    fn update_refuses_ordinary_comments_and_missing_annotations() {
        let mut doc = fixture(&["<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /Subj (Rectangle) >>"]);
        let change = MarkText { status: Status::Excluded, label: String::new(), comment: String::new() };
        assert!(matches!(update(&mut doc, 0, 0, &change, &meta("m")), Err(TakeoffError::Invalid(_))));
        assert!(matches!(update(&mut doc, 0, 9, &change, &meta("m")), Err(TakeoffError::Annotation(AnnotError::NoSuchAnnotation { .. }))));
        assert!(matches!(update(&mut doc, 4, 0, &change, &meta("m")), Err(TakeoffError::Annotation(AnnotError::NoSuchPage(4)))));
    }

    #[test]
    fn foreign_marks_are_read_by_subject_and_edited_text_wins() {
        let mut doc = fixture(&[
            // Drawn in another program with an Atlas subject, no private data.
            "<< /Type /Annot /Subtype /Square /Rect [10 10 50 50] /Subj (atlas takeoff: excluded) /Contents (Fire pump by others) >>",
            // Ordinary comments are not marks.
            "<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /Subj (Rectangle) >>",
            "<< /Type /Annot /Subtype /Circle /Rect [0 0 10 10] /Subj (Atlas Takeoff: Included) >>",
            "<< /Type /Annot /Subtype /Square /Rect [0 0 10] /Subj (Atlas Takeoff: Included) >>",
        ]);
        let index = add(&mut doc, &mark(Status::Included, "LP-1", "spare"), &meta("e")).unwrap();
        // Someone edits the text in Acrobat.
        let (r, _) = annot(&doc, 0, index).unwrap();
        doc.update_dict(r, |d| d.set(b"Contents".to_vec(), PdfString::text("LP-1: spare, add 2 breakers"))).unwrap();
        let marks = list(&doc).marks;
        assert_eq!(marks.len(), 2);
        assert_eq!((marks[0].status, marks[0].comment.as_str()), (Status::Excluded, "Fire pump by others"));
        assert_eq!(marks[1].comment, "spare, add 2 breakers");
    }

    #[test]
    fn oversized_text_is_refused() {
        let mut doc = fixture(&[]);
        let long = "x".repeat(MAX_TEXT + 1);
        assert!(matches!(add(&mut doc, &mark(Status::Included, &long, ""), &meta("l")), Err(TakeoffError::Invalid(_))));
        assert!(list(&doc).marks.is_empty());
    }

    #[test]
    fn summary_joins_label_and_comment() {
        assert_eq!(summary(" LP-1 ", ""), "LP-1");
        assert_eq!(summary("", " note "), "note");
        assert_eq!(summary("LP-1", "note"), "LP-1: note");
        assert_eq!(summary("  ", "  "), "");
    }

    #[test]
    fn statuses_map_both_ways() {
        for s in Status::ALL {
            assert_eq!(Status::from_pdf_name(s.pdf_name().as_bytes()), Some(s));
            assert_eq!(Status::from_subject(s.subject()), Some(s));
        }
        assert_eq!(Status::from_pdf_name(b"Maybe"), None);
        assert_eq!(Status::from_subject("Rectangle"), None);
        assert_eq!(Status::ALL.map(Status::hotkey), ['1', '2', '3']);
    }

    #[test]
    fn csv_quotes_and_defuses_formulas() {
        let m = |label: &str, comment: &str| Mark {
            page: 2,
            index: 0,
            id: String::new(),
            status: Status::Excluded,
            label: label.into(),
            comment: comment.into(),
            author: "A".into(),
            rect: [0.0; 4],
        };
        let csv = to_csv(&[m("LP-1", "by others, see E-401"), m("=1+1", "say \"no\"")], &|p| format!("E-{p}"));
        let lines: Vec<&str> = csv.split("\r\n").collect();
        assert_eq!(lines[0], "Page,Sheet,Status,Label,Comment,Author");
        assert_eq!(lines[1], "3,E-2,Excluded,LP-1,\"by others, see E-401\",A");
        assert_eq!(lines[2], "3,E-2,Excluded,'=1+1,\"say \"\"no\"\"\",A");
    }
}
