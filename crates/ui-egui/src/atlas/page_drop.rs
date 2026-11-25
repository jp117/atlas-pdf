//! Atlas fork: page thumbnails dragged from one window's Pages panel into another window's.
//!
//! The pages are copied, as Copy then Paste would: the source document keeps them, and the
//! target gets them at the gap under the pointer, with colliding form fields renamed.
//!
//! Each window is its own egui viewport, and the button stays with the window the drag started
//! in (X11's implicit grab, Windows' mouse capture), so no other window sees the pointer. Every
//! Pages panel therefore writes down where it is on screen, in physical pixels (the one unit
//! every monitor shares), and the dragging panel compares the pointer with them. The notes live
//! in egui's memory, which every viewport shares, so `panels/pages.rs` needs no access to the
//! app. The drop itself waits in that memory until [`apply`] runs after the frame's passes.
//!
//! Where the desktop keeps window positions to itself (Wayland), no panel can say where it is,
//! so the drag stays inside its window, as it did before.

use egui::{Align2, Pos2, Rect, Stroke, Ui, ViewportId, pos2, vec2};
use pdfcraft_engine::{DocId, Edit};

use crate::PdfCraftApp;
use crate::theme::{self, Tokens};

/// Where the notes are kept in egui's memory.
fn key() -> egui::Id {
    egui::Id::unique("atlas-page-drop")
}

/// A Pages panel as it was last drawn.
#[derive(Clone, Debug, PartialEq)]
struct Panel {
    viewport: ViewportId,
    doc: DocId,
    /// The viewport's pass count when this was written: a panel not drawn since is gone.
    pass: u64,
    /// The panel may take pages (the document may be assembled, and nothing is over it).
    accepts: bool,
    /// The panel's visible part, on screen in physical pixels.
    clip: Rect,
    /// Each thumbnail, top to bottom, on screen in physical pixels.
    rows: Vec<Rect>,
}

/// Thumbnails being dragged out of a Pages panel.
#[derive(Clone, Debug, PartialEq)]
struct Drag {
    viewport: ViewportId,
    doc: DocId,
    pages: Vec<usize>,
    /// Where the pointer was last seen, on screen in physical pixels (`None` before the desktop
    /// said where the window is).
    pointer: Option<Pos2>,
    /// The pointer has left the window: releasing there moves nothing.
    outside: bool,
    /// The other panel under the pointer, and the gap the pages would go to.
    over: Option<(ViewportId, DocId, usize)>,
}

/// Pages released over another document's Pages panel, for [`apply`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PageDrop {
    pub from: DocId,
    pub pages: Vec<usize>,
    pub to: DocId,
    pub at: usize,
}

#[derive(Clone, Debug, Default)]
struct Notes {
    panels: Vec<Panel>,
    drag: Option<Drag>,
    drop: Option<PageDrop>,
}

fn notes<R>(ctx: &egui::Context, f: impl FnOnce(&mut Notes) -> R) -> R {
    ctx.data_mut(|d| f(d.get_temp_mut_or_default::<Notes>(key())))
}

/// The current window's content on screen in physical pixels, and its pixels per point.
/// `None` where the desktop doesn't say where the window is. The main window counts as being
/// at (0, 0) then, as `PdfCraftApp::track_window` has it: windows embedded in it share its
/// viewport, and on Wayland no other window can say where it is, so no drop lands wrongly.
fn window_on_screen(ctx: &egui::Context) -> Option<(Rect, f32)> {
    let inner = match ctx.input(|i| i.viewport().inner_rect).filter(|r| r.is_finite()) {
        Some(inner) => inner,
        None if ctx.viewport_id() == ViewportId::ROOT => Rect::from_min_size(Pos2::ZERO, ctx.content_rect().size()),
        None => return None,
    };
    let ppp = ctx.pixels_per_point();
    Some((Rect::from_min_max(inner.min * ppp, inner.max * ppp), ppp))
}

/// `local`, a position in the current window, on screen in physical pixels.
fn to_screen(window: Rect, ppp: f32, local: Pos2) -> Pos2 {
    window.min + local.to_vec2() * ppp
}

/// A rect in the current window, on screen in physical pixels.
fn rect_to_screen(window: Rect, ppp: f32, local: Rect) -> Rect {
    Rect::from_min_max(to_screen(window, ppp, local.min), to_screen(window, ppp, local.max))
}

/// The gap (0 = before the first thumbnail, n = after the last) nearest the height `y`.
fn gap(rows: &[Rect], y: f32) -> Option<usize> {
    let (i, r) = rows.iter().enumerate().min_by(|(_, a), (_, b)| (a.center().y - y).abs().total_cmp(&(b.center().y - y).abs()))?;
    Some(if y < r.center().y { i } else { i + 1 })
}

/// Whether a panel noted at its viewport's pass `pass` is still drawn,
/// the viewport having completed `now` passes. Every window is drawn every frame, so a panel
/// drawn last frame or this one is current.
fn current(pass: u64, now: u64) -> bool {
    now <= pass.saturating_add(1)
}

/// The panel under `screen` that would take pages from `from`: another document's, taking
/// pages, and drawn recently (`now` says how many passes a viewport has completed). A panel in
/// another window counts only once the pointer has left the dragging window (`outside`); one in
/// the same viewport (a window embedded in the main one) counts whenever the pointer is on it.
fn target(
    panels: &[Panel],
    from: (ViewportId, DocId),
    screen: Pos2,
    outside: bool,
    now: impl Fn(ViewportId) -> u64,
) -> Option<(ViewportId, DocId, usize)> {
    panels
        .iter()
        .filter(|p| p.doc != from.1 && p.accepts && current(p.pass, now(p.viewport)))
        .filter(|p| outside || p.viewport == from.0)
        .find(|p| p.clip.contains(screen))
        .and_then(|p| Some((p.viewport, p.doc, gap(&p.rows, screen.y)?)))
}

/// The Pages panel of `doc` was drawn with these thumbnails (`rows`, in the window's
/// coordinates). Write down where it is, and mark where pages dragged from another window
/// would land in it. `accepts`: the panel may take pages now.
pub(crate) fn panel(ui: &Ui, t: &Tokens, doc: DocId, rows: &[Rect], accepts: bool) {
    let ctx = ui.ctx();
    let viewport = ctx.viewport_id();
    let Some((window, ppp)) = window_on_screen(ctx) else {
        // Nowhere on screen to be found: forget this panel.
        notes(ctx, |n| n.panels.retain(|p| !(p.viewport == viewport && p.doc == doc)));
        return;
    };
    let note = Panel {
        viewport,
        doc,
        pass: ctx.cumulative_pass_nr(),
        accepts,
        clip: rect_to_screen(window, ppp, ui.clip_rect()),
        rows: rows.iter().map(|r| rect_to_screen(window, ppp, *r)).collect(),
    };
    let incoming = notes(ctx, |n| {
        n.panels.retain(|p| !(p.viewport == viewport && p.doc == doc));
        n.panels.push(note);
        n.drag
            .as_ref()
            .filter(|d| d.over.is_some_and(|(v, to, _)| v == viewport && to == doc))
            .map(|d| (d.pages.len(), d.pointer, d.over.map_or(0, |(_, _, gap)| gap)))
    });
    let Some((count, pointer, gap)) = incoming else { return };
    // The pages from the other window would land here: a bar on the gap, and the count by the pointer.
    let painter = ui.painter().with_clip_rect(ui.clip_rect());
    if let (Some(first), Some(last)) = (rows.first(), rows.last()) {
        let y = rows.get(gap).map_or(last.bottom() + 20.0, |r| r.top() - 3.0);
        painter.line_segment([pos2(first.left(), y), pos2(first.right(), y)], Stroke::new(3.0, t.accent));
    }
    if let Some(p) = pointer {
        let local = Pos2::ZERO + (p - window.min) / ppp;
        let what = if count == 1 { "Copy 1 page here".to_owned() } else { format!("Copy {count} pages here") };
        painter.text(local + vec2(14.0, 14.0), Align2::LEFT_TOP, what, theme::medium(12.0), t.accent_text);
    }
    ctx.request_repaint();
}

/// While thumbnails of `doc` are dragged in its Pages panel: whether the pointer has gone to
/// another window, so the panel's own move should stand aside (no bar, no scrolling, and no
/// move on release). On release over another document's Pages panel, the pages are queued for
/// [`apply`] and the drag ends.
pub(crate) fn elsewhere(ui: &Ui, view: &mut crate::DocView, moving: &[usize], dropped: bool) -> bool {
    let ctx = ui.ctx();
    let viewport = ctx.viewport_id();
    let doc = view.id;
    let Some((window, ppp)) = window_on_screen(ctx) else { return false };
    // Released over another window, the pointer goes with that frame (X11): the release counts
    // where the drag was last seen.
    let latest = ctx.pointer_latest_pos().map(|p| (to_screen(window, ppp, p), !ui.ctx().content_rect().contains(p)));
    // egui's memory is locked while `notes` runs: read what the other viewports did first.
    let (panels, before) = notes(ctx, |n| (n.panels.clone(), n.drag.take().filter(|d| d.viewport == viewport && d.doc == doc)));
    let passes: Vec<(ViewportId, u64)> = panels.iter().map(|p| (p.viewport, ctx.cumulative_pass_nr_for(p.viewport))).collect();
    let now = |v: ViewportId| passes.iter().find(|(id, _)| *id == v).map_or(0, |(_, n)| *n);
    let (pointer, outside, over) = match (latest, &before) {
        (Some((p, outside)), _) => (Some(p), outside, target(&panels, (viewport, doc), p, outside, now)),
        (None, Some(d)) => (d.pointer, d.outside, d.over),
        (None, None) => (None, false, None),
    };
    notes(ctx, |n| {
        if !dropped {
            n.drag = Some(Drag { viewport, doc, pages: moving.to_vec(), pointer, outside, over });
        } else if let Some((_, to, at)) = over {
            let mut pages = moving.to_vec();
            pages.sort_unstable();
            pages.dedup();
            n.drop = Some(PageDrop { from: doc, pages, to, at });
        }
    });
    if over.is_none() && !outside {
        return false;
    }
    if dropped {
        view.panel_drag = None;
    } else {
        ctx.request_repaint();
    }
    true
}

/// Insert pages released over another window's Pages panel into its document. Runs once per
/// frame, after every window's pass.
pub(crate) fn apply(app: &mut PdfCraftApp, ctx: &egui::Context) {
    // A drag the panel no longer updates (it ended, or its window closed) marks nothing.
    let dragging = notes(ctx, |n| n.drag.as_ref().map(|d| d.viewport));
    if let Some(viewport) = dragging
        && !ctx.input_for(viewport, |i| i.pointer.any_down())
    {
        notes(ctx, |n| n.drag = None);
    }
    let Some(drop) = notes(ctx, |n| n.drop.take()) else { return };
    insert(app, drop);
}

/// Copy `drop`'s pages into its target document, in the window that shows it.
pub(crate) fn insert(app: &mut PdfCraftApp, drop: PageDrop) {
    let Some(source) = app.session.get(drop.from) else { return };
    let (name, bytes) = (source.name.clone(), source.bytes.clone());
    let Some(window) = app.window_of_doc(drop.to) else { return };
    let count = drop.pages.len();
    let done = app.with_window(window, |app| {
        // The panel showed the window's active document, and must still.
        let Some((i, id)) = app.active_ids() else { return false };
        let allowed = app.session.get(id).is_some_and(|d| d.allows_assembly() && d.read_only_reason.is_none());
        if id != drop.to || !allowed || app.dialog.is_some() {
            app.notify("The document changed while you were dragging, so no pages were copied.");
            return false;
        }
        let n = app.session.get(id).map_or(0, |d| d.info.pages.len());
        let at = drop.at.min(n);
        let edit = Edit::InsertPagesFrom {
            name: name.clone(),
            bytes,
            pages: Some(drop.pages.clone()),
            at,
            field_conflicts: pdfcraft_engine::FieldConflicts::Rename,
        };
        if !app.apply_edit(edit) {
            return false;
        }
        if let Some(view) = app.views.get_mut(i) {
            view.select_pages(&(at..at + count).collect::<Vec<_>>());
        }
        let what = if count == 1 { "1 page".to_owned() } else { format!("{count} pages") };
        app.notify(format!("Copied {what} from {name}"));
        true
    });
    if done == Some(true) {
        app.queue_window_op(crate::WindowOp::Focus(window));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(viewport: ViewportId, doc: u64, clip: Rect) -> Panel {
        let rows = (0..3).map(|i| Rect::from_min_size(pos2(clip.left() + 8.0, clip.top() + i as f32 * 100.0), vec2(80.0, 90.0))).collect();
        Panel { viewport, doc: DocId(doc), pass: 10, accepts: true, clip, rows }
    }

    fn other() -> ViewportId {
        ViewportId::from_hash_of(("pdfcraft-window", 1))
    }

    /// A panel in another window takes the pages once the pointer has left the dragging
    /// window, at the gap nearest the pointer.
    #[test]
    fn a_panel_in_another_window_takes_pages_at_the_gap_under_the_pointer() {
        let panels = [panel(other(), 2, Rect::from_min_size(pos2(2000.0, 100.0), vec2(200.0, 400.0)))];
        let from = (ViewportId::ROOT, DocId(1));
        let now = |_| 10;
        assert_eq!(target(&panels, from, pos2(2050.0, 120.0), true, now), Some((other(), DocId(2), 0)));
        assert_eq!(target(&panels, from, pos2(2050.0, 180.0), true, now), Some((other(), DocId(2), 1)));
        assert_eq!(target(&panels, from, pos2(2050.0, 399.0), true, now), Some((other(), DocId(2), 3)));
        // Still inside the dragging window (it may cover the other one): no.
        assert_eq!(target(&panels, from, pos2(2050.0, 180.0), false, now), None);
        // Beside the panel: no.
        assert_eq!(target(&panels, from, pos2(1900.0, 180.0), true, now), None);
    }

    /// The dragged document's own panel, a panel that can't take pages, and a panel no longer
    /// drawn are never targets.
    #[test]
    fn only_another_documents_live_panel_that_takes_pages_is_a_target() {
        let clip = Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 400.0));
        let from = (ViewportId::ROOT, DocId(1));
        let p = pos2(50.0, 50.0);
        assert_eq!(target(&[panel(other(), 1, clip)], from, p, true, |_| 10), None, "the same document");
        let mut locked = panel(other(), 2, clip);
        locked.accepts = false;
        assert_eq!(target(&[locked], from, p, true, |_| 10), None, "a document that may not be assembled");
        assert_eq!(target(&[panel(other(), 2, clip)], from, p, true, |_| 12), None, "a panel not drawn for two passes");
        assert!(target(&[panel(other(), 2, clip)], from, p, true, |_| 11).is_some(), "a panel drawn last pass");
        // A window embedded in the main one shares its viewport: its panel counts without leaving.
        assert!(target(&[panel(ViewportId::ROOT, 2, clip)], from, p, false, |_| 10).is_some());
    }

    /// A PDF whose pages are `widths` points wide, so a test can tell them apart.
    fn pages_pdf(widths: &[u32]) -> Vec<u8> {
        let kids: Vec<String> = (0..widths.len()).map(|i| format!("{} 0 R", i + 3)).collect();
        let mut pdf = format!(
            "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [{}] /Count {} >> endobj\n",
            kids.join(" "),
            widths.len()
        );
        for (i, w) in widths.iter().enumerate() {
            pdf.push_str(&format!("{} 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} 300] >> endobj\n", i + 3));
        }
        pdf.push_str("trailer << /Root 1 0 R >>\n%%EOF");
        pdf.into_bytes()
    }

    /// Dragging thumbnails out of the Pages panel onto another window's Pages panel copies the
    /// pages there, at the gap under the pointer; the document they came from keeps them in
    /// order. Released outside the window and off any panel, the drag changes nothing.
    ///
    /// The tests' windows are embedded in the main one and draw only their tab strip, so the
    /// second window's panel is the note a real one would leave: off to the right of the main
    /// window, where a second window on another monitor would be.
    #[test]
    fn pages_dragged_onto_another_windows_pages_panel_are_copied_there() {
        use crate::{RightPanel, WindowId, WindowOp};
        use egui::{Event, PointerButton};
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;
        let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(|_cc| {
            let mut app = PdfCraftApp::new();
            let _ = app.set_option("language", "en");
            let _ = app.open_bytes("source.pdf", None, pages_pdf(&[301, 302, 303]));
            let _ = app.open_bytes("target.pdf", None, pages_pdf(&[401, 402]));
            app.right = Some(RightPanel::Pages);
            app
        });
        h.run_steps(4);
        let (source, target) = (h.state().views[0].id, h.state().views[1].id);
        let ctx = h.ctx.clone();
        h.state_mut().queue_window_op(WindowOp::MoveTab { doc: target, from: WindowId::ROOT, to: None, at: None });
        h.state_mut().apply_window_ops(&ctx);
        h.run_steps(3);
        let second = h.state().window_of_doc(target).unwrap();
        assert_ne!(second, WindowId::ROOT);
        assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(source));
        // target.pdf's Pages panel, two thumbnails, right of the 1400 pt wide main window.
        let rows = vec![Rect::from_min_size(pos2(2450.0, 100.0), vec2(100.0, 90.0)), Rect::from_min_size(pos2(2450.0, 200.0), vec2(100.0, 90.0))];
        let note = Panel {
            viewport: second.viewport(),
            doc: target,
            pass: 0,
            accepts: true,
            clip: Rect::from_min_size(pos2(2400.0, 50.0), vec2(200.0, 400.0)),
            rows,
        };
        notes(&ctx, |n| n.panels.push(note));

        let widths = |h: &Harness<'static, PdfCraftApp>, id: DocId| -> Vec<u32> {
            h.state().session.get(id).unwrap().info.pages.iter().map(|p| p.width.round() as u32).collect()
        };
        let drag = |h: &mut Harness<'static, PdfCraftApp>, to: Pos2| {
            let from = h.get_by_label("Page 3").rect().center();
            let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Default::default() };
            h.event(Event::PointerMoved(from));
            h.run_steps(1);
            h.event(button(from, true));
            h.run_steps(1);
            for k in 1..=10 {
                h.event(Event::PointerMoved(from + (to - from) * (k as f32 / 10.0)));
                h.run_steps(1);
            }
            h.event(button(to, false));
            h.run_steps(4);
        };

        // Just above the middle of target.pdf's page 2: between its pages 1 and 2.
        drag(&mut h, pos2(2500.0, 230.0));
        assert_eq!(widths(&h, target), [401, 303, 402], "page 3 of source.pdf is copied in between");
        assert_eq!(widths(&h, source), [301, 302, 303], "source.pdf keeps its pages where they were");
        let picked = h.state_mut().with_window(second, |a| a.views[0].selected.iter().copied().collect::<Vec<_>>());
        assert_eq!(picked, Some(vec![1]), "the copied page is selected");
        assert!(notes(&ctx, |n| n.drag.is_none() && n.drop.is_none()), "the drag is over");

        // Below that panel, outside the main window: nothing moves or is copied.
        drag(&mut h, pos2(2500.0, 800.0));
        assert_eq!(widths(&h, target), [401, 303, 402]);
        assert_eq!(widths(&h, source), [301, 302, 303]);
    }

    #[test]
    fn positions_go_to_physical_screen_pixels() {
        let window = Rect::from_min_size(pos2(3000.0, 200.0), vec2(1000.0, 800.0));
        assert_eq!(to_screen(window, 2.0, pos2(10.0, 20.0)), pos2(3020.0, 240.0));
    }
}
