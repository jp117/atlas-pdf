//! Atlas fork: a tab dragged out of its strip leaves at once, in a window that follows the
//! pointer, onto any monitor.
//!
//! PdfCraft opens the tab's window only when the button comes up, and past the window's edge
//! nothing shows where it will go. Here, where the desktop lets the app place its windows
//! (Windows and X11; Wayland keeps PdfCraft's way), the tab leaves its strip as soon as it is
//! dragged out of its window, and its new window stays under the pointer:
//! - over a window's tab strip (the one it came from included) the carried window hides, that
//!   strip marks where the tab lands, and releasing puts it there;
//! - released anywhere else, the window stays where it is. A window taken from a maximized one
//!   and dropped off it (on another monitor) is maximized there; any other is kept smaller than
//!   its monitor;
//! - Escape puts the tab back where it was.
//!
//! While the pointer is still inside the window, a dragged tab does what it does in PdfCraft:
//! split view's right half splits, and anywhere else opens the window where it is let go.
//!
//! The window the drag began in keeps the pointer until the button comes up (Windows captures
//! it, X11's grab holds it), so the drag is read from that window's input however far the
//! pointer goes. Screen positions are in physical pixels, the one unit every monitor shares.

use crate::PdfCraftApp;
use crate::windows::{MAX_WINDOWS, TAB_GRIP, TabDrag, WindowGeometry, WindowId, WindowOp};
use egui::{Pos2, Rect, Vec2, ViewportCommand, vec2};
use pdfcraft_engine::DocId;

/// How much of a maximized window's size a window taken from it gets.
const FROM_MAXIMIZED: f32 = 0.75;
/// The most of its monitor a window dropped there takes.
const MONITOR_SHARE: f32 = 0.9;
/// How far above and below a tab strip a carried tab still drops into it, in points.
const STRIP_REACH: f32 = 12.0;
/// Where the pointer holds the tab when the press wasn't on it, in points from its top-left.
const DEFAULT_HOLD: Vec2 = vec2(40.0, 15.0);

/// A tab carried in a window of its own while the button is held.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TornTab {
    pub doc: DocId,
    /// The window the drag began in, which has the pointer until the button comes up.
    pub source: WindowId,
    /// Where the tab was in `source`, for Escape.
    pub index: usize,
    /// The tab's window, once it has opened; `asked` once it has been asked for.
    pub window: Option<WindowId>,
    pub asked: bool,
    /// Where in the tab the pointer holds it, in points.
    pub hold: Vec2,
    /// Where the pointer was last seen, in physical screen pixels.
    pub pointer: Pos2,
    /// The source window's place when it was maximized: a tab dropped outside it went to
    /// another monitor.
    pub from_maximized: Option<Rect>,
    /// The window is hidden over a tab strip.
    pub hidden: bool,
    /// Where the window goes while it is drawn inside the main one (tests).
    pub embedded_pos: Option<Pos2>,
}

/// A tab is being carried (or is about to be).
pub(crate) fn carrying(app: &PdfCraftApp) -> bool {
    app.atlas.tear_off.is_some()
}

/// The size of a window taken from the window with geometry `source`, in points: the same,
/// or smaller than a maximized one, so it doesn't open as large as the monitor.
pub(crate) fn new_window_size(source: &WindowGeometry) -> Vec2 {
    if source.maximized { source.rect.size() * FROM_MAXIMIZED } else { source.rect.size() }
}

/// Called from the strip of the current window while its tab `from` (showing `doc`, at `tab`)
/// is dragged past the detach zone with the pointer at `local`. True when the tab is torn off
/// (or already is); false while the pointer is inside the window or where windows can't be
/// placed, and the strip goes on as before.
pub(crate) fn begin(app: &mut PdfCraftApp, ctx: &egui::Context, doc: DocId, from: usize, tab: Option<Rect>, local: Pos2) -> bool {
    if app.atlas.tear_off.is_some() {
        return true;
    }
    let source = app.current_window();
    let Some(g) = app.window_geometry.get(&source).filter(|g| g.known && g.placed) else { return false };
    // Inside its window the tab's drop still means what it did (split view's right half, a new
    // window where it is let go): the window comes along only once the pointer leaves.
    if g.rect.contains(g.origin + local.to_vec2()) || app.window_count() >= MAX_WINDOWS || !app.can_detach(from) {
        return false;
    }
    let pointer = (g.origin + local.to_vec2()) * g.ppp;
    let hold = match (tab, ctx.input(|i| i.pointer.press_origin())) {
        (Some(r), Some(p)) if r.contains(p) => p - r.min,
        _ => DEFAULT_HOLD,
    };
    let from_maximized = g.maximized.then(|| g.screen_rect());
    app.atlas.tear_off =
        Some(TornTab { doc, source, index: from, window: None, asked: false, hold, pointer, from_maximized, hidden: false, embedded_pos: None });
    ctx.request_repaint();
    true
}

/// The builder of window `id`: a window opened to carry a tab doesn't take the focus on
/// Windows, where the window that has the pointer could lose it. It gets the focus when the
/// tab is dropped.
pub(crate) fn builder(app: &PdfCraftApp, id: WindowId, builder: egui::ViewportBuilder) -> egui::ViewportBuilder {
    match &app.atlas.tear_off {
        Some(torn) if cfg!(windows) && app.window_of_doc(torn.doc) == Some(id) && id != torn.source => builder.with_active(false),
        _ => builder,
    }
}

/// Where window `id` goes while it carries a tab and is drawn inside the main window.
pub(crate) fn embedded_pos(app: &PdfCraftApp, id: WindowId) -> Option<Pos2> {
    app.atlas.tear_off.as_ref().filter(|t| t.window == Some(id)).and_then(|t| t.embedded_pos)
}

/// The window and tab index a tab dropped at the physical screen position `screen` lands at:
/// a window other than `except` whose tab strip is under it. The focused window first, then
/// the newest (windows may overlap).
pub(crate) fn strip_under(app: &PdfCraftApp, screen: Pos2, except: WindowId) -> Option<(WindowId, usize)> {
    let mut found: Vec<(bool, WindowId, usize)> = app
        .window_geometry
        .iter()
        .filter(|(id, g)| **id != except && g.known && app.has_window(**id) && g.screen_rect().contains(screen))
        .filter_map(|(id, g)| {
            let local = g.local(screen);
            let strip = g.strip?.expand2(vec2(0.0, STRIP_REACH));
            strip.contains(local).then(|| (g.focused, *id, g.tabs.iter().filter(|(_, r)| r.center().x < local.x).count()))
        })
        .collect();
    found.sort_by_key(|(focused, id, _)| (std::cmp::Reverse(*focused), std::cmp::Reverse(*id)));
    found.first().map(|(_, id, index)| (*id, *index))
}

/// Once per frame, after every window's pass: carry the torn-off tab's window with the
/// pointer, and drop it when the button comes up.
pub(crate) fn follow(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(mut torn) = app.atlas.tear_off.take() else { return };
    if !app.has_window(torn.source) {
        app.tab_drag = None;
        return;
    }
    let (pos, down, escape) =
        ctx.input_for(torn.source.viewport(), |i| (i.pointer.latest_pos(), i.pointer.primary_down(), i.key_pressed(egui::Key::Escape)));
    if let Some(p) = pos
        && let Some(g) = app.window_geometry.get(&torn.source).filter(|g| g.known)
    {
        torn.pointer = (g.origin + p.to_vec2()) * g.ppp;
    }
    let embedded = ctx.embed_viewports();
    let Some(window) = torn.window.or_else(|| app.window_of_doc(torn.doc).filter(|w| *w != torn.source)) else {
        if torn.asked || escape {
            // The window wasn't opened (too many, say), or the drag was cancelled before it was.
            app.tab_drag = None;
            return;
        }
        app.queue_window_op(WindowOp::MoveTab { doc: torn.doc, from: torn.source, to: None, at: Some(torn.pointer) });
        torn.asked = true;
        if down {
            app.atlas.tear_off = Some(torn);
        } else {
            // Released in the frame it left: it stays where it opens.
            app.tab_drag = None;
        }
        return;
    };
    torn.window = Some(window);
    if app.window_of_doc(torn.doc) != Some(window) {
        // The tab closed, or went elsewhere, under the drag.
        app.tab_drag = None;
        return;
    }
    // The carried window has the pointer itself: the source lost it (another app took it, say),
    // so the button is as good as up.
    let lost = ctx.input_for(window.viewport(), |i| i.pointer.any_pressed() || i.pointer.any_released());
    if escape {
        app.tab_drag = None;
        app.queue_window_op(WindowOp::MoveTab { doc: torn.doc, from: window, to: Some((torn.source, Some(torn.index))), at: None });
        return;
    }
    let target = strip_under(app, torn.pointer, window);
    if !down || lost {
        app.tab_drag = None;
        drop_carried(app, ctx, &torn, window, target, embedded);
        return;
    }
    // Over a strip: the strip marks where the tab lands, and the window keeps out of the way.
    app.tab_drag = target.map(|(to, index)| TabDrag {
        doc: torn.doc,
        from: window,
        pointer: torn.pointer,
        over: Some((to, Some(index))),
        outside: true,
        cancelled: false,
    });
    if !embedded && target.is_some() != torn.hidden {
        torn.hidden = target.is_some();
        ctx.send_viewport_cmd_to(window.viewport(), ViewportCommand::Visible(!torn.hidden));
    }
    move_carried(app, ctx, &mut torn, window, embedded);
    ctx.request_repaint();
    app.atlas.tear_off = Some(torn);
}

/// Put the carried window where the pointer holds its tab.
fn move_carried(app: &PdfCraftApp, ctx: &egui::Context, torn: &mut TornTab, window: WindowId, embedded: bool) {
    // Until the window has been drawn it is where it was opened, under the pointer.
    let Some(g) = app.window_geometry.get(&window).filter(|g| g.known && g.placed) else { return };
    let ppp = if g.ppp > 0.0 { g.ppp } else { 1.0 };
    // Where the tab's hold point is now, on screen, in the window's points.
    let held = match g.tabs.iter().find(|(d, _)| *d == torn.doc) {
        Some((_, r)) => g.origin + (r.min + torn.hold).to_vec2(),
        None => g.rect.min + TAB_GRIP,
    };
    let shift = Pos2::new(torn.pointer.x / ppp, torn.pointer.y / ppp) - held;
    if !shift.is_finite() || shift.length() * ppp < 1.0 {
        return;
    }
    if embedded {
        let area = ctx.memory(|m| m.area_rect(egui::Id::unique(window.viewport())));
        torn.embedded_pos = area.map(|a| a.min + shift);
    } else {
        ctx.send_viewport_cmd_to(window.viewport(), ViewportCommand::OuterPosition(g.origin - g.frame + shift));
    }
}

/// The button came up with the tab carried in `window`: into the strip under the pointer, or
/// the window stays where it is.
fn drop_carried(app: &mut PdfCraftApp, ctx: &egui::Context, torn: &TornTab, window: WindowId, target: Option<(WindowId, usize)>, embedded: bool) {
    if let Some((to, index)) = target {
        // The window, left without tabs, closes.
        app.queue_window_op(WindowOp::MoveTab { doc: torn.doc, from: window, to: Some((to, Some(index))), at: None });
        return;
    }
    app.focused_window = window;
    if embedded {
        return;
    }
    let viewport = window.viewport();
    if torn.hidden {
        ctx.send_viewport_cmd_to(viewport, ViewportCommand::Visible(true));
    }
    ctx.send_viewport_cmd_to(viewport, ViewportCommand::Focus);
    if torn.from_maximized.is_some_and(|source| !source.contains(torn.pointer)) {
        ctx.send_viewport_cmd_to(viewport, ViewportCommand::Maximized(true));
    } else if let Some(g) = app.window_geometry.get(&window)
        && let Some(size) = fit_to_monitor(g.rect.size(), g.monitor)
    {
        ctx.send_viewport_cmd_to(viewport, ViewportCommand::InnerSize(size));
    }
}

/// A window of `size` on a monitor of `monitor` (points): the size to shrink it to when it is
/// larger than [`MONITOR_SHARE`] of the monitor, `None` when it fits.
fn fit_to_monitor(size: Vec2, monitor: Option<Vec2>) -> Option<Vec2> {
    let most = monitor.filter(|m| m.x > 0.0 && m.y > 0.0)? * MONITOR_SHARE;
    (size.x > most.x || size.y > most.y).then(|| size.min(most))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_dropped_on_a_smaller_monitor_shrinks_to_fit() {
        let monitor = Some(vec2(1920.0, 1080.0));
        assert_eq!(fit_to_monitor(vec2(1200.0, 800.0), monitor), None, "it fits");
        assert_eq!(fit_to_monitor(vec2(2560.0, 900.0), monitor), Some(vec2(1728.0, 900.0)));
        assert_eq!(fit_to_monitor(vec2(2560.0, 1400.0), monitor), Some(vec2(1728.0, 972.0)));
        assert_eq!(fit_to_monitor(vec2(2560.0, 1400.0), None), None, "monitor unknown");
        assert_eq!(fit_to_monitor(vec2(2560.0, 1400.0), Some(Vec2::ZERO)), None);
    }

    #[test]
    fn a_window_taken_from_a_maximized_one_is_smaller() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(2000.0, 1200.0));
        let normal = WindowGeometry { known: true, rect, ..Default::default() };
        assert_eq!(new_window_size(&normal), vec2(2000.0, 1200.0));
        let maximized = WindowGeometry { maximized: true, ..normal };
        assert_eq!(new_window_size(&maximized), vec2(1500.0, 900.0));
    }

    #[test]
    fn screen_positions_are_physical_pixels_so_monitors_with_different_scales_line_up() {
        // A laptop at 150 % with an external monitor at 100 % to its right, 1920 px along. In
        // points the two windows' places can't be compared; in pixels they can.
        let laptop = WindowGeometry {
            known: true,
            origin: Pos2::new(0.0, 0.0),
            rect: Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 720.0)),
            ppp: 1.5,
            ..Default::default()
        };
        let external = WindowGeometry {
            known: true,
            origin: Pos2::new(2000.0, 100.0),
            rect: Rect::from_min_size(Pos2::new(2000.0, 100.0), vec2(1200.0, 800.0)),
            ppp: 1.0,
            strip: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 38.0))),
            ..Default::default()
        };
        assert_eq!(laptop.screen_rect(), Rect::from_min_size(Pos2::ZERO, vec2(1920.0, 1080.0)));
        // A point 1400 pt into the laptop window is past its right edge (1280 pt = 1920 px),
        // 2100 px along: on the external monitor, 100 px into that window.
        let screen = Pos2::new(1400.0, 80.0) * 1.5;
        assert!(!laptop.screen_rect().contains(screen));
        assert!(external.screen_rect().contains(screen));
        assert_eq!(external.local(screen), Pos2::new(100.0, 20.0));
    }
}
