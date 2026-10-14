//! The takeoff tools: drag a box over equipment to mark it Included (green), Included w/ comments
//! (yellow) or Excluded (red). The box is labelled with the text inside it (SHX text included), so
//! "LP-1" or "1600A D.S.#1" comes along for free. Comments belong to the quote, not to boxes, so
//! every box is drawn in one drag, yellow included. Clicking a box (without dragging) selects it, for Delete or a 1/2/3 re-mark.

use super::AtlasState;
use crate::QuickTool;
use crate::canvas::{DocView, PageXform};
use egui::{Color32, Pos2, Rect, Stroke};
use pdfcraft_atlas::takeoff::{Mark, NewMark, Status};
use pdfcraft_engine::{DocId, Document, Edit};
use pdfcraft_render::text::PageText;

/// A box being dragged, in the page's view space (points, y down).
pub(crate) struct Drag {
    doc: DocId,
    page: usize,
    start: [f32; 2],
    end: [f32; 2],
}

/// The smallest box, in screen pixels, that counts as a drag rather than a stray click.
const MIN_BOX_PX: f32 = 6.0;
/// The longest automatic label, in characters.
const MAX_LABEL_CHARS: usize = 120;

pub(crate) fn color(status: Status) -> Color32 {
    let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let [r, g, b] = status.color();
    Color32::from_rgb(c(r), c(g), c(b))
}

/// One page's takeoff input; called for every visible page.
#[allow(clippy::too_many_arguments)]
pub(crate) fn page(
    ui: &egui::Ui,
    resp: &egui::Response,
    doc: &Document,
    view: &mut DocView,
    xf: &PageXform,
    page: usize,
    state: &mut AtlasState,
    tool: QuickTool,
    author: &str,
) {
    if let QuickTool::Takeoff(status) = tool {
        draw_input(ui, resp, doc, view, xf, page, state, status, author);
    }
}

fn screen(xf: &PageXform, v: [f32; 2]) -> Pos2 {
    xf.norm_to_screen(v[0] / xf.pw, v[1] / xf.ph)
}

#[allow(clippy::too_many_arguments)]
fn draw_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    doc: &Document,
    view: &mut DocView,
    xf: &PageXform,
    page: usize,
    state: &mut AtlasState,
    status: Status,
    author: &str,
) {
    let Some(info) = doc.info.pages.get(page) else { return };
    let over = ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| xf.rect.contains(p));
    if !doc.allows_annotation() {
        if over {
            ui.ctx().set_cursor_icon(egui::CursorIcon::NotAllowed);
        }
        return;
    }
    let id = view.id;
    // The box under the pointer, if any: a click selects it.
    let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|p| xf.rect.contains(*p));
    let hit = pointer.and_then(|p| {
        let (vx, vy) = xf.screen_to_view(p);
        let [x, y] = info.view_to_user(vx, vy);
        let marks = state.listing(id, doc).map(|l| l.marks.as_slice()).unwrap_or_default();
        mark_at(marks, page, f64::from(x), f64::from(y))
    });
    if over {
        let dragging = state.drag.as_ref().is_some_and(|d| d.doc == id && d.page == page);
        let cursor = if hit.is_some() && !dragging { egui::CursorIcon::PointingHand } else { egui::CursorIcon::Crosshair };
        ui.ctx().set_cursor_icon(cursor);
    }
    if resp.clicked() && pointer.is_some() {
        view.comments.selected = hit.map(|index| (page, index));
    }
    if resp.drag_started()
        && let Some(origin) = ui.input(|i| i.pointer.press_origin()).filter(|p| xf.rect.contains(*p))
    {
        let (x, y) = xf.screen_to_view(origin);
        state.drag = Some(Drag { doc: id, page, start: [x, y], end: [x, y] });
    }
    let ours = |d: &Drag| d.doc == id && d.page == page;
    if let Some(d) = state.drag.as_mut().filter(|d| ours(d)) {
        if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
            let (x, y) = xf.screen_to_view(xf.rect.clamp(p));
            d.end = [x, y];
        }
        let r = Rect::from_two_pos(screen(xf, d.start), screen(xf, d.end));
        ui.painter().rect_filled(r, 0.0, color(status).gamma_multiply(0.12));
        ui.painter().rect_stroke(r, 0.0, Stroke::new(2.0, color(status)), egui::StrokeKind::Middle);
    }
    if !resp.drag_stopped() {
        return;
    }
    let Some(d) = state.drag.take_if(|d| ours(d)) else { return };
    let r = Rect::from_two_pos(screen(xf, d.start), screen(xf, d.end));
    if r.width() < MIN_BOX_PX || r.height() < MIN_BOX_PX {
        return;
    }
    let boxed = [d.start[0].min(d.end[0]), d.start[1].min(d.end[1]), d.start[0].max(d.end[0]), d.start[1].max(d.end[1])];
    let a = info.view_to_user(boxed[0], boxed[1]);
    let b = info.view_to_user(boxed[2], boxed[3]);
    let rect = [f64::from(a[0].min(b[0])), f64::from(a[1].min(b[1])), f64::from(a[0].max(b[0])), f64::from(a[1].max(b[1]))];
    let label = view.page_text(page).map(|t| label_inside(&t, boxed)).unwrap_or_default();
    let mark = NewMark { page, rect, status, label, comment: String::new(), author: author.to_owned() };
    view.pending_edit = Some(Edit::AddTakeoffMark(mark));
}

/// The `/Annots` index of the takeoff mark on `page` under the user-space point (x, y): the
/// smallest one containing it, so a box drawn inside a bigger box can still be picked.
fn mark_at(marks: &[Mark], page: usize, x: f64, y: f64) -> Option<usize> {
    let area = |m: &Mark| (m.rect[2] - m.rect[0]) * (m.rect[3] - m.rect[1]);
    marks
        .iter()
        .filter(|m| m.page == page && m.rect[0] <= x && x <= m.rect[2] && m.rect[1] <= y && y <= m.rect[3])
        .min_by(|a, b| area(a).total_cmp(&area(b)))
        .map(|m| m.index)
}

/// The text inside `rect` (view space) on one line, for a mark's label.
fn label_inside(text: &PageText, rect: [f32; 4]) -> String {
    let raw = text.column_text(&text.glyphs_in(rect));
    let mut label = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some((cut, _)) = label.char_indices().nth(MAX_LABEL_CHARS) {
        label.truncate(cut);
    }
    label
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_render::text::TextGlyph;

    fn glyph(c: char, x: f32) -> TextGlyph {
        let rect = [x, 10.0, x + 5.0, 20.0];
        TextGlyph { text: c.to_string(), rect, quad: [[x, 20.0], [x + 5.0, 20.0], [x + 5.0, 10.0], [x, 10.0]], direction: [1.0, 0.0] }
    }

    #[test]
    fn labels_take_the_text_inside_the_box_on_one_line() {
        let mut text = PageText::default();
        for (i, c) in "LP-1 MDP".chars().filter(|c| !c.is_whitespace()).enumerate() {
            text.glyphs.push(glyph(c, i as f32 * 5.0));
            text.line_of.push(0);
            text.space_before.push(i == 4);
        }
        assert_eq!(label_inside(&text, [0.0, 0.0, 100.0, 30.0]), "LP-1 MDP");
        assert_eq!(label_inside(&text, [0.0, 0.0, 19.0, 30.0]), "LP-1");
        assert_eq!(label_inside(&text, [500.0, 0.0, 600.0, 30.0]), "");
    }

    #[test]
    fn a_click_picks_the_smallest_box_under_it_on_its_page() {
        let mark = |page: usize, index: usize, rect: [f64; 4]| Mark {
            page,
            index,
            id: String::new(),
            status: Status::Included,
            label: String::new(),
            comment: String::new(),
            author: String::new(),
            rect,
        };
        let marks = [mark(0, 3, [0.0, 0.0, 100.0, 100.0]), mark(0, 7, [40.0, 40.0, 60.0, 60.0]), mark(1, 2, [0.0, 0.0, 500.0, 500.0])];
        assert_eq!(mark_at(&marks, 0, 50.0, 50.0), Some(7), "the inner box");
        assert_eq!(mark_at(&marks, 0, 10.0, 10.0), Some(3), "the outer box");
        assert_eq!(mark_at(&marks, 0, 100.0, 100.0), Some(3), "edges count");
        assert_eq!(mark_at(&marks, 0, 150.0, 10.0), None, "outside every box");
        assert_eq!(mark_at(&marks, 1, 50.0, 50.0), Some(2), "only this page's boxes");
        assert_eq!(mark_at(&[], 0, 1.0, 1.0), None);
    }

    #[test]
    fn status_colours_are_opaque_and_distinct() {
        let colours: Vec<Color32> = Status::ALL.into_iter().map(color).collect();
        assert!(colours.iter().all(|c| c.a() == 255));
        assert_ne!(colours[0], colours[1]);
        assert_ne!(colours[1], colours[2]);
    }
}
