//! Atlas fork: a document tab dragged out of the tab strip opens in a new window, as in
//! Acrobat. The new window is another Atlas PDF process (`--new-window --tab-window …`) that
//! opens the file at the same page, and the tab closes here. A process can't take over another
//! one's unsaved edits, so a document with unsaved changes is saved first (asking where when it
//! has no file yet); a cancelled or failed save leaves the tab where it was.
//!
//! Upstream PdfCraft has open pull requests that move tabs between windows inside one process
//! (storytold/pdfcraft#3483, #3753). If one merges, it replaces this.

use std::ffi::OsString;

use egui::{Pos2, Rect, Vec2};
use pdfcraft_engine::DocId;

use crate::PdfCraftApp;
use crate::canvas::{self, Fit};

/// How far above or below the tab strip, in points, a dragged tab has to be to tear off.
/// Closer than that it reorders; anywhere past the window's sides tears off too.
pub(crate) const TEAR_DISTANCE: f32 = 40.0;
/// Where the pointer lands in the new window, from its content's top-left corner: about a third
/// of the way into its one tab.
const GRAB: Vec2 = egui::vec2(70.0, 18.0);
/// The smallest content size the new window gets (apps/pdfcraft's minimum window size).
const MIN_SIZE: Vec2 = egui::vec2(820.0, 520.0);
/// No monitor arrangement is wider or taller than this, in points.
const MAX_EXTENT: f32 = 32_768.0;

/// Starts another Atlas PDF with these arguments.
pub(crate) type Launcher = std::sync::Arc<dyn Fn(&[OsString]) -> Result<(), String> + Send + Sync>;

/// Whether a tab dragged to `pos` has left the strip: well above or below it, or past either side
/// of the window. All in the window's points.
pub(crate) fn torn_off(strip: Rect, window: Rect, pos: Pos2) -> bool {
    pos.y < strip.top() - TEAR_DISTANCE || pos.y > strip.bottom() + TEAR_DISTANCE || pos.x < window.left() || pos.x > window.right()
}

/// The new window's place and size, in points: the content area's size, and the outer
/// top-left corner when the desktop reports where windows are (Wayland doesn't).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub size: Vec2,
    pub position: Option<Pos2>,
}

impl Placement {
    /// A window the size of this one, placed so the pointer at `screen_pointer` sits on its tab.
    /// `frame` is how far the content is from the window's outer corner (the title bar).
    pub(crate) fn under_pointer(size: Vec2, screen_pointer: Option<Pos2>, frame: Vec2) -> Self {
        Self { size, position: screen_pointer.map(|p| p - frame - GRAB) }.sane()
    }

    /// Read from `--tab-window`: `W,H` or `W,H,X,Y`. Anything else, or numbers that aren't
    /// finite, is `None`; sizes and places out of range are pulled into it.
    pub fn parse(arg: &str) -> Option<Self> {
        let numbers: Vec<f32> = arg.split(',').map(|n| n.trim().parse::<f32>().ok().filter(|n| n.is_finite())).collect::<Option<_>>()?;
        let placement = match numbers.as_slice() {
            [w, h] => Self { size: egui::vec2(*w, *h), position: None },
            [w, h, x, y] => Self { size: egui::vec2(*w, *h), position: Some(egui::pos2(*x, *y)) },
            _ => return None,
        };
        Some(placement.sane())
    }

    /// The `--tab-window` value [`Self::parse`] reads.
    pub(crate) fn arg(&self) -> String {
        match self.position {
            Some(p) => format!("{},{},{},{}", self.size.x.round(), self.size.y.round(), p.x.round(), p.y.round()),
            None => format!("{},{}", self.size.x.round(), self.size.y.round()),
        }
    }

    /// What the new window does with its first frame. eframe opens it where the last window
    /// closed (`app.ron`), so it is moved here afterwards, out of a maximized state first.
    pub fn commands(&self) -> Vec<egui::ViewportCommand> {
        let mut commands = vec![egui::ViewportCommand::Maximized(false), egui::ViewportCommand::InnerSize(self.size)];
        if let Some(p) = self.position {
            commands.push(egui::ViewportCommand::OuterPosition(p));
        }
        commands
    }

    fn sane(self) -> Self {
        let size = self.size.max(MIN_SIZE).min(egui::Vec2::splat(MAX_EXTENT));
        let position = self.position.map(|p| p.clamp(egui::pos2(-MAX_EXTENT, -MAX_EXTENT), egui::pos2(MAX_EXTENT, MAX_EXTENT)));
        Self { size, position }
    }
}

/// The arguments that open `path` in a window of its own, at the page `view` is on and in its
/// page display.
fn launch_args(path: &str, view: &canvas::DocView, placement: Placement) -> Vec<OsString> {
    let shown = view.normal_display();
    let mut args: Vec<OsString> = vec![
        "--new-window".into(),
        "--tab-window".into(),
        placement.arg().into(),
        "--page".into(),
        view.current.saturating_add(1).to_string().into(),
    ];
    args.extend(["--layout".into(), canvas::display_token(shown.layout, shown.continuous).into()]);
    // A fitted zoom refits to the new window; a fixed one carries over.
    if shown.fit == Fit::None && shown.zoom.is_finite() && shown.zoom > 0.0 {
        args.extend(["--zoom".into(), format!("{}%", shown.zoom * 100.0).into()]);
    }
    // Absolute, so a file name starting with `-` can't read as an option.
    args.push(std::path::absolute(path).map_or_else(|_| path.into(), OsString::from));
    args
}

/// Starts this program again with `args`. Its exit status is collected on a thread of its own,
/// so it never lingers as a zombie on Unix.
#[cfg(not(target_arch = "wasm32"))]
fn launch(args: &[OsString]) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let exe = std::env::current_exe().map_err(|e| format!("Couldn't find Atlas PDF to open a new window: {e}"))?;
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Couldn't open a new window: {e}"))?;
    let waited = std::thread::Builder::new().name("atlas-tab-window".into()).spawn(move || {
        let _ = child.wait();
    });
    if let Err(e) = waited {
        log::warn!("new window: not waiting for it: {e}");
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn launch(_: &[OsString]) -> Result<(), String> {
    Err("A browser tab can't open another window".into())
}

/// The tab strip's hook, each frame a document tab is dragged: whether it has torn off (and then
/// the hint at the pointer), and the pointer's last place in the window, kept when it leaves the
/// window (egui forgets it there, though the drag goes on).
pub(crate) fn dragging(ctx: &egui::Context, strip: Rect, name: Option<&str>) -> Option<(Pos2, bool)> {
    let id = egui::Id::unique("atlas_tab_drag_pos");
    let pos = match ctx.input(|i| i.pointer.latest_pos()) {
        Some(p) => {
            ctx.data_mut(|d| d.insert_temp(id, p));
            p
        }
        None => ctx.data(|d| d.get_temp::<Pos2>(id))?,
    };
    let torn = cfg!(not(target_arch = "wasm32")) && torn_off(strip, ctx.content_rect(), pos);
    if torn && let Some(name) = name {
        egui::Area::new(egui::Id::unique("atlas_tab_tear_hint"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos + egui::vec2(14.0, 18.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| ui.label(format!("Open {name} in a new window")));
            });
    }
    Some((pos, torn))
}

/// Where the new window goes when the tab is released at `pos` (window points).
pub(crate) fn placement_at(ctx: &egui::Context, pos: Pos2) -> Placement {
    let (inner, outer) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().outer_rect));
    let size = inner.map_or_else(|| ctx.content_rect().size(), |r| r.size());
    let frame = match (inner, outer) {
        (Some(inner), Some(outer)) => inner.min - outer.min,
        _ => Vec2::ZERO,
    };
    Placement::under_pointer(size, inner.map(|r| r.min + pos.to_vec2()), frame)
}

impl PdfCraftApp {
    /// Open document `id` in a new window and close its tab here, saving it first if it has
    /// unsaved changes or no file.
    pub(crate) fn move_tab_to_new_window(&mut self, id: DocId, placement: Placement) {
        let launcher = self.atlas.launcher.clone().unwrap_or_else(|| std::sync::Arc::new(launch));
        let Some(index) = self.views.iter().position(|v| v.id == id) else { return };
        let has_file = self.session.get(id).is_some_and(|d| d.path.is_some());
        if has_file && !self.has_unsaved_work(index) {
            self.launch_tab_window(id, placement, &launcher);
        } else {
            // The window opens once the save is written; a cancelled or failed save keeps the tab.
            self.save_then(index, crate::SaveTarget::InPlace, move |app| app.launch_tab_window(id, placement, &launcher));
        }
    }

    fn launch_tab_window(&mut self, id: DocId, placement: Placement, launcher: &Launcher) {
        let Some(index) = self.views.iter().position(|v| v.id == id) else { return };
        let (Some(view), Some(path)) = (self.views.get(index), self.session.get(id).and_then(|d| d.path.clone())) else { return };
        match launcher(&launch_args(&path, view, placement)) {
            // Asks first if something was typed into it while it was saved.
            Ok(()) => self.request_close_tab(index),
            Err(e) => self.notify_error(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    const PDF: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] >> endobj\n4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF";

    type Calls = Arc<Mutex<Vec<Vec<String>>>>;

    /// A launcher that records its arguments and answers `result`.
    fn recorder(app: &mut PdfCraftApp, result: Result<(), String>) -> Calls {
        let calls: Calls = Arc::default();
        let seen = Arc::clone(&calls);
        app.atlas.launcher = Some(Arc::new(move |args: &[OsString]| {
            seen.lock().map_err(|e| e.to_string())?.push(args.iter().map(|a| a.to_string_lossy().into_owned()).collect());
            result.clone()
        }));
        calls
    }

    fn calls(calls: &Calls) -> Vec<Vec<String>> {
        calls.lock().unwrap().clone()
    }

    /// A fresh folder for one test, removed when it ends.
    struct Folder(std::path::PathBuf);

    impl Folder {
        fn new(test: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("atlas-tab-window-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn file(&self, name: &str) -> String {
            self.0.join(name).to_string_lossy().into_owned()
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// An app with `drawing.pdf` saved in a fresh folder and open in a tab.
    fn app_with_file(test: &str) -> (PdfCraftApp, Folder, String) {
        let dir = Folder::new(test);
        let path = dir.file("drawing.pdf");
        std::fs::write(&path, PDF).unwrap();
        let mut app = PdfCraftApp::new();
        app.open_bytes("drawing.pdf", Some(path.clone()), PDF.to_vec()).unwrap();
        (app, dir, path)
    }

    const PLACE: Placement = Placement { size: egui::vec2(1200.0, 800.0), position: Some(egui::pos2(300.0, 200.0)) };

    /// Presses the tab, drags it to `to` in a few steps and lets go.
    fn drag_tab(h: &mut egui_kittest::Harness<'static, PdfCraftApp>, name: &str, to: Pos2) {
        use egui_kittest::kittest::Queryable as _;
        let from = h.get_by_role_and_label(egui::accesskit::Role::Tab, name).rect().center();
        h.event(egui::Event::PointerMoved(from));
        h.event(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        h.run_steps(1);
        for k in 1..=6 {
            h.event(egui::Event::PointerMoved(from.lerp(to, k as f32 / 6.0)));
            h.run_steps(1);
        }
        h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        h.run_steps(3);
    }

    /// The tab strip itself: a tab let go of a little below the strip stays (it reorders), and
    /// one let go of well below it moves to a new window.
    #[test]
    fn dragging_a_tab_out_of_the_strip_opens_it_in_a_new_window() {
        let dir = Folder::new("drag");
        let mut paths = Vec::new();
        for name in ["first.pdf", "second.pdf"] {
            let path = dir.file(name);
            std::fs::write(&path, PDF).unwrap();
            paths.push(path);
        }
        let launched: Calls = Arc::default();
        let seen = Arc::clone(&launched);
        let opened = paths.clone();
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            for path in &opened {
                app.open_bytes(path.rsplit(['/', '\\']).next().unwrap(), Some(path.clone()), PDF.to_vec()).unwrap();
            }
            let seen = Arc::clone(&seen);
            app.atlas.launcher = Some(Arc::new(move |args: &[OsString]| {
                seen.lock().unwrap().push(args.iter().map(|a| a.to_string_lossy().into_owned()).collect());
                Ok(())
            }));
            app
        });
        h.run_steps(4);
        let tab = {
            use egui_kittest::kittest::Queryable as _;
            h.get_by_role_and_label(egui::accesskit::Role::Tab, "first.pdf").rect()
        };
        drag_tab(&mut h, "first.pdf", tab.center() + egui::vec2(0.0, 30.0));
        assert!(calls(&launched).is_empty(), "near the strip it stays");
        assert_eq!(h.state().views.len(), 2);
        drag_tab(&mut h, "first.pdf", tab.center() + egui::vec2(0.0, 300.0));
        let calls = calls(&launched);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].last(), Some(&paths[0]));
        assert_eq!(h.state().views.len(), 1);
        assert_eq!(h.state().session.get(h.state().views[0].id).and_then(|d| d.path.clone()), Some(paths[1].clone()));
    }

    #[test]
    fn a_tab_tears_off_well_above_or_below_the_strip_or_past_the_window() {
        let window = Rect::from_min_size(Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let strip = Rect::from_min_max(Pos2::ZERO, egui::pos2(1000.0, 38.0));
        assert!(!torn_off(strip, window, egui::pos2(500.0, 20.0)), "on the strip");
        assert!(!torn_off(strip, window, egui::pos2(500.0, 38.0 + TEAR_DISTANCE)), "just below it");
        assert!(torn_off(strip, window, egui::pos2(500.0, 39.0 + TEAR_DISTANCE)), "past the distance");
        assert!(torn_off(strip, window, egui::pos2(500.0, -41.0)), "above the window");
        assert!(!torn_off(strip, window, egui::pos2(500.0, -30.0)), "in the title bar above it");
        assert!(torn_off(strip, window, egui::pos2(-1.0, 20.0)), "past the left side");
        assert!(torn_off(strip, window, egui::pos2(1001.0, 20.0)), "past the right side");
        assert!(!torn_off(strip, window, egui::pos2(f32::NAN, f32::NAN)), "no position, no tear");
    }

    #[test]
    fn placements_round_trip_and_bad_ones_are_refused_or_pulled_into_range() {
        assert_eq!(Placement::parse(&PLACE.arg()), Some(PLACE));
        let unplaced = Placement { size: PLACE.size, position: None };
        assert_eq!(Placement::parse(&unplaced.arg()), Some(unplaced));
        for bad in ["", "1200", "1200,800,3", "a,b", "NaN,800", "inf,800,0,0", "1,2,3,4,5", "1200,,800"] {
            assert_eq!(Placement::parse(bad), None, "{bad:?}");
        }
        let tiny = Placement::parse("10,10,-1e30,1e30").unwrap();
        assert_eq!(tiny.size, MIN_SIZE);
        assert_eq!(tiny.position, Some(egui::pos2(-MAX_EXTENT, MAX_EXTENT)));
        assert_eq!(Placement::parse("1e9,1e9").unwrap().size, Vec2::splat(MAX_EXTENT));
    }

    #[test]
    fn the_new_window_is_unmaximized_sized_then_placed() {
        assert_eq!(
            PLACE.commands(),
            [
                egui::ViewportCommand::Maximized(false),
                egui::ViewportCommand::InnerSize(PLACE.size),
                egui::ViewportCommand::OuterPosition(egui::pos2(300.0, 200.0)),
            ]
        );
        assert_eq!(Placement { position: None, ..PLACE }.commands().len(), 2, "nowhere to place it on Wayland");
        let under = Placement::under_pointer(PLACE.size, Some(egui::pos2(500.0, 400.0)), egui::vec2(8.0, 31.0));
        assert_eq!(under.position, Some(egui::pos2(500.0 - 8.0 - GRAB.x, 400.0 - 31.0 - GRAB.y)));
    }

    #[test]
    fn a_saved_tab_opens_in_a_new_window_at_its_page_and_closes_here() {
        let (mut app, _dir, path) = app_with_file("moves");
        let launched = recorder(&mut app, Ok(()));
        let id = app.views[0].id;
        app.views[0].go_to_page(1);
        app.move_tab_to_new_window(id, PLACE);
        let calls = calls(&launched);
        assert_eq!(calls.len(), 1);
        let args = &calls[0];
        assert_eq!(args[..5], ["--new-window", "--tab-window", "1200,800,300,200", "--page", "2"]);
        assert_eq!(args.last().map(String::as_str), Some(path.as_str()));
        assert!(app.views.is_empty(), "the tab moved");
        assert!(app.session.get(id).is_none());
    }

    #[test]
    fn a_tab_stays_when_the_new_window_cant_start() {
        let (mut app, _dir, _) = app_with_file("fails");
        let _ = recorder(&mut app, Err("Couldn't open a new window: denied".into()));
        let id = app.views[0].id;
        app.move_tab_to_new_window(id, PLACE);
        assert_eq!(app.views.len(), 1);
        assert_eq!(app.toast.as_ref().map(|(m, _)| m.as_str()), Some("Couldn't open a new window: denied"));
    }

    #[test]
    fn unsaved_changes_are_saved_before_the_tab_moves() {
        let (mut app, _dir, path) = app_with_file("dirty");
        let launched = recorder(&mut app, Ok(()));
        let id = app.views[0].id;
        app.session.apply(id, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
        assert!(app.session.get(id).unwrap().dirty);
        app.move_tab_to_new_window(id, PLACE);
        assert_eq!(calls(&launched).len(), 1, "launched once saved");
        assert!(app.views.is_empty());
        let saved = std::fs::read(&path).unwrap();
        assert_ne!(saved, PDF, "the edit is in the file the new window opens");
    }

    /// `save_override` stands in for the place picked in the Save dialog.
    #[test]
    fn a_document_without_a_file_is_saved_where_the_user_picks_then_moves() {
        let dir = Folder::new("new");
        let picked = dir.file("scan.pdf");
        let mut app = PdfCraftApp::new();
        let launched = recorder(&mut app, Ok(()));
        app.open_bytes("scan.pdf", None, PDF.to_vec()).unwrap();
        app.save_override = Some(picked.clone());
        let id = app.views[0].id;
        app.move_tab_to_new_window(id, PLACE);
        assert!(std::path::Path::new(&picked).is_file());
        assert_eq!(calls(&launched)[0].last(), Some(&picked));
        assert!(app.views.is_empty());
    }
}
