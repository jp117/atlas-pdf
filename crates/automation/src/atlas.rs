//! Atlas fork: takeoff tools, so an agent can read an estimator's takeoff and draft one. Geometry
//! is in display points (origin at the top-left of the page as shown), like every other tool;
//! pages and mark indexes are 1-based.

use crate::comments::{DEFAULT_AUTHOR, rect_to_view};
use crate::tools::ToolDef;
use crate::{Args, Automation, Result, ToolError, failed};
use pdfcraft_atlas::takeoff::{self, Mark, MarkText, NewMark, Status};
use pdfcraft_engine::{Document, Edit};
use serde_json::{Value, json};

const STATUSES: &str = "included, included_with_comments or excluded";

fn status_from(name: &str) -> Result<Status> {
    match name {
        "included" => Ok(Status::Included),
        "included_with_comments" => Ok(Status::IncludedWithComments),
        "excluded" => Ok(Status::Excluded),
        other => Err(ToolError::InvalidArgs(format!("unknown status {other:?} ({STATUSES})"))),
    }
}

/// A mark as the tools report it.
fn mark_json(doc: &Document, m: &Mark) -> Value {
    let info = doc.info.pages.get(m.page);
    json!({
        "page": m.page.saturating_add(1),
        "index": m.index.saturating_add(1),
        "sheet": info.map(|p| p.label.clone()),
        "id": m.id,
        "status": m.status,
        "label": m.label,
        "comment": m.comment,
        "author": m.author,
        "rect": info.map(|p| rect_to_view(p, m.rect.map(|v| v as f32))),
    })
}

/// `page` (1-based) as a 0-based filter.
fn page_filter(a: &Args) -> Result<Option<usize>> {
    Ok(a.opt_int("page")?.map(|n| usize::try_from(n.max(1).saturating_sub(1)).unwrap_or(usize::MAX)))
}

impl Automation {
    pub(crate) fn takeoff_list(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let listing = doc.takeoff_marks().map_err(failed)?;
        let page = page_filter(a)?;
        let status = a.opt_str("status")?.map(status_from).transpose()?;
        let marks: Vec<Value> = listing
            .marks
            .iter()
            .filter(|m| page.is_none_or(|p| m.page == p) && status.is_none_or(|s| m.status == s))
            .map(|m| mark_json(doc, m))
            .collect();
        let [included, with_comments, excluded] = listing.counts();
        Ok(json!({
            "count": marks.len(),
            "counts": {"included": included, "included_with_comments": with_comments, "excluded": excluded},
            "marks": marks,
            "truncated": listing.truncated,
        }))
    }

    pub(crate) fn takeoff_add(&mut self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let shown = a.nums::<4>("rect")?.ok_or_else(|| ToolError::InvalidArgs("takeoff_add needs `rect`".into()))?;
        let [x0, y0, x1, y1] = shown;
        if (x1 - x0).abs() < 1.0 || (y1 - y0).abs() < 1.0 || shown.iter().any(|v| v.abs() > 1e9) {
            return Err(ToolError::InvalidArgs("rect must be at least 1 point wide and tall, within 1e9 points".into()));
        }
        let status = status_from(a.str("status")?)?;
        let doc = self.doc(a)?;
        let id = doc.id;
        let info = doc.info.pages.get(page).cloned().ok_or_else(|| failed("no such page"))?;
        let (p, q) = (info.view_to_user(x0 as f32, y0 as f32), info.view_to_user(x1 as f32, y1 as f32));
        let rect = [f64::from(p[0].min(q[0])), f64::from(p[1].min(q[1])), f64::from(p[0].max(q[0])), f64::from(p[1].max(q[1]))];
        let label = match a.opt_str("label")? {
            Some(label) => label.to_owned(),
            // The text inside the box, SHX text included, on one line.
            None => {
                let texts = self.page_texts(id, &[page])?;
                let boxed = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)].map(|v| v as f32);
                let raw = texts.first().map(|t| t.column_text(&t.glyphs_in(boxed))).unwrap_or_default();
                raw.split_whitespace().take(64).collect::<Vec<_>>().join(" ")
            }
        };
        let comment = a.opt_str("comment")?.unwrap_or("").to_owned();
        let author = a.opt_str("author")?.unwrap_or(DEFAULT_AUTHOR).to_owned();
        let mut out = self.apply(a, Edit::AddTakeoffMark(NewMark { page, rect, status, label: label.clone(), comment, author }))?;
        out["label"] = json!(label);
        Ok(out)
    }

    pub(crate) fn takeoff_update(&mut self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let index = usize::try_from(a.int("index")?.saturating_sub(1)).map_err(|_| ToolError::InvalidArgs("index must be at least 1".into()))?;
        let listing = self.doc(a)?.takeoff_marks().map_err(failed)?;
        let mark = listing
            .marks
            .iter()
            .find(|m| m.page == page && m.index == index)
            .ok_or_else(|| failed(format!("there is no takeoff mark {} on page {}", index.saturating_add(1), page.saturating_add(1))))?;
        let text = MarkText {
            status: a.opt_str("status")?.map(status_from).transpose()?.unwrap_or(mark.status),
            label: a.opt_str("label")?.map_or_else(|| mark.label.clone(), str::to_owned),
            comment: a.opt_str("comment")?.map_or_else(|| mark.comment.clone(), str::to_owned),
        };
        self.apply(a, Edit::UpdateTakeoffMark { page, index, text })
    }

    /// Write the marks as CSV, or as JSON when `out` ends in `.json`.
    pub(crate) fn takeoff_export(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let listing = doc.takeoff_marks().map_err(failed)?;
        let out = a.str("out")?;
        let bytes = if out.to_ascii_lowercase().ends_with(".json") {
            let marks: Vec<Value> = listing.marks.iter().map(|m| mark_json(doc, m)).collect();
            let [included, with_comments, excluded] = listing.counts();
            let body = json!({
                "document": doc.name,
                "counts": {"included": included, "included_with_comments": with_comments, "excluded": excluded},
                "marks": marks,
            });
            serde_json::to_vec_pretty(&body).map_err(|e| failed(e.to_string()))?
        } else {
            let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
            takeoff::to_csv(&listing.marks, &|p| labels.get(p).cloned().unwrap_or_default()).into_bytes()
        };
        let target = self.resolve(out, true)?;
        crate::write_atomic(&target, &bytes)?;
        Ok(json!({"path": target, "count": listing.marks.len(), "truncated": listing.truncated}))
    }
}

fn schema(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

/// The Atlas tools, appended to the tool table.
pub(crate) fn tools() -> Vec<ToolDef> {
    let doc = || json!({ "type": "integer", "minimum": 1, "description": "Document id from doc_open or doc_list." });
    let page = || json!({ "type": "integer", "minimum": 1, "description": "1-based page number." });
    let status = || json!({ "type": "string", "enum": ["included", "included_with_comments", "excluded"] });
    let rect = || {
        json!({ "type": "array", "items": { "type": "number" }, "minItems": 4, "maxItems": 4,
            "description": "[x0, y0, x1, y1] in points from the top-left of the page as displayed." })
    };
    let text = || json!({ "type": "string", "maxLength": takeoff::MAX_TEXT });
    let def = |name, title, description, read_only, input_schema| ToolDef {
        name,
        title,
        description,
        input_schema,
        read_only,
        destructive: false,
        command: None,
    };
    vec![
        def(
            "takeoff_list",
            "List takeoff marks",
            "Atlas takeoff marks: the boxes an estimator drew over equipment, each Included (green), Included with comments (yellow) or Excluded (red), with its page, 1-based index, sheet label, label, comment, author and rectangle. counts covers the whole document; page and status filter the marks.",
            true,
            schema(json!({"doc": doc(), "page": page(), "status": status()}), &["doc"]),
        ),
        def(
            "takeoff_add",
            "Add a takeoff mark",
            "Draw an Atlas takeoff box over equipment. Without label, the label is the text inside the box (AutoCAD SHX text included). Saved as a standard Square annotation with subject \"Atlas Takeoff: …\", so other PDF viewers show it. Undoable.",
            false,
            schema(
                json!({"doc": doc(), "page": page(), "rect": rect(), "status": status(), "label": text(), "comment": text(), "author": {"type": "string"}}),
                &["doc", "page", "rect", "status"],
            ),
        ),
        def(
            "takeoff_update",
            "Change a takeoff mark",
            "Change a takeoff mark's status, label or comment (omitted ones stay) and redraw it in its status colour. index is the 1-based index from takeoff_list. Undoable. Delete a mark with comment_delete.",
            false,
            schema(
                json!({"doc": doc(), "page": page(), "index": {"type": "integer", "minimum": 1}, "status": status(), "label": text(), "comment": text()}),
                &["doc", "page", "index"],
            ),
        ),
        def(
            "takeoff_export",
            "Export the takeoff",
            "Atomically write every takeoff mark as spreadsheet-safe CSV (page, sheet, status, label, comment, author), or as JSON with counts and rectangles when out ends in .json.",
            false,
            schema(json!({"doc": doc(), "out": {"type": "string", "description": "A file path (relative to --root when set)."}}), &["doc", "out"]),
        ),
    ]
}
