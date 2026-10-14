//! Atlas fork: the interface for Atlas Switch's takeoff workflow.
//!
//! Everything Atlas adds to the window lives here, reached from a handful of one-line hooks in
//! the upstream files (`lib.rs`, `canvas.rs`, `chrome.rs`, `editing.rs`, `control.rs`), so the
//! fork rebases cleanly. Strings are plain English on purpose: `tl!` literals would have to be
//! added to every upstream translation catalogue.

mod layers_panel;
mod takeoff_panel;
mod takeoff_tool;

use crate::{PdfCraftApp, QuickTool};
use pdfcraft_atlas::takeoff::{Listing, MarkText, Status};
use pdfcraft_engine::{DocId, Document, Edit};

pub(crate) use layers_panel::{apply as apply_layers, show as layers_panel};
pub(crate) use takeoff_panel::show as takeoff_panel;
pub(crate) use takeoff_tool::page;

/// Atlas's per-window state.
#[derive(Default)]
pub(crate) struct AtlasState {
    /// The takeoff box being dragged.
    drag: Option<takeoff_tool::Drag>,
    /// A yellow box waiting for its comment.
    prompt: Option<takeoff_tool::Prompt>,
    /// The active document's marks, for its edit generation.
    listing: Option<(DocId, u64, Result<Listing, String>)>,
    /// The Takeoff panel's filters and editor.
    panel: takeoff_panel::PanelState,
}

impl AtlasState {
    /// The document's takeoff marks, read once per edit generation.
    fn listing(&mut self, id: DocId, doc: &Document) -> Option<&Listing> {
        let generation = doc.edit_generation();
        if self.listing.as_ref().is_none_or(|(d, g, _)| *d != id || *g != generation) {
            self.listing = Some((id, generation, doc.takeoff_marks()));
        }
        self.listing.as_ref().and_then(|(_, _, l)| l.as_ref().ok())
    }
}

/// The control channel's name for a takeoff tool.
pub(crate) fn tool_name(status: Status) -> String {
    format!("takeoff-{}", status.pdf_name().to_ascii_lowercase())
}

fn digit(status: Status) -> egui::Key {
    match status {
        Status::Included => egui::Key::Num1,
        Status::IncludedWithComments => egui::Key::Num2,
        Status::Excluded => egui::Key::Num3,
    }
}

/// 1, 2 and 3: with the Select tool and a takeoff mark selected, re-mark it with that status;
/// otherwise pick that takeoff tool. Plain digits, so only when nothing else has the keyboard
/// (the same rule as the V and H tool keys).
pub(crate) fn keys(app: &mut PdfCraftApp, i: usize, ctx: &egui::Context) {
    let Some(view) = app.views.get(i) else { return };
    let free = app.dialog.is_none() && !app.palette_open && !ctx.egui_wants_keyboard_input() && view.forms.focus.is_none();
    if !free || !ctx.input(|input| input.modifiers.is_none()) {
        return;
    }
    let Some(status) = Status::ALL.into_iter().find(|s| ctx.input(|input| input.key_pressed(digit(*s)))) else { return };
    if app.quick_tool == QuickTool::Select
        && let Some(edit) = restatus_selected(app, i, status)
    {
        if let Some(view) = app.views.get_mut(i) {
            view.pending_edit = Some(edit);
        }
        return;
    }
    pick_tool(app, status);
}

/// Pick a takeoff tool and show the Takeoff panel beside the drawing.
fn pick_tool(app: &mut PdfCraftApp, status: Status) {
    app.quick_tool = QuickTool::Takeoff(status);
    open_panel(app);
}

/// Show the Takeoff panel. The first time it shares the Inspector's place as a tab rather than
/// halving the drawing; after that it comes back wherever it was last put.
fn open_panel(app: &mut PdfCraftApp) {
    use crate::docking::Panel;
    use serde_json::json;
    if app.docking.layout.contains(&Panel::Takeoff) {
        return;
    }
    let first_time = !app.docking.hidden.iter().any(|(panel, _)| *panel == Panel::Takeoff);
    let mut steps = vec![json!({"operation": "open", "panel": "takeoff"})];
    if first_time && app.docking.layout.contains(&Panel::Inspector) {
        steps.push(json!({"operation": "move", "panel": "takeoff", "target": "inspector", "zone": "center", "before": "inspector"}));
        steps.push(json!({"operation": "activate", "panel": "takeoff"}));
    }
    for step in steps {
        if let Err(error) = crate::docking::command(app, false, &step) {
            app.notify(error);
            return;
        }
    }
}

/// The edit that gives the selected takeoff mark `status`, keeping its label and comment.
fn restatus_selected(app: &mut PdfCraftApp, i: usize, status: Status) -> Option<Edit> {
    let view = app.views.get(i)?;
    let (page, index) = view.comments.selected?;
    let id = view.id;
    let doc = app.session.get(id)?;
    let mark = app.atlas.listing(id, doc)?.marks.iter().find(|m| m.page == page && m.index == index)?;
    if mark.status == status {
        return None;
    }
    let text = MarkText { status, label: mark.label.clone(), comment: mark.comment.clone() };
    Some(Edit::UpdateTakeoffMark { page, index, text })
}

/// The Atlas menu: the takeoff tools.
pub(crate) fn menu(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("Takeoff").small().weak());
    for status in Status::ALL {
        let text = egui::RichText::new(format!("■  {}", status.label())).color(takeoff_tool::color(status));
        let button = egui::Button::new(text).shortcut_text(status.hotkey().to_string()).selected(app.quick_tool == QuickTool::Takeoff(status));
        if ui.add_enabled(app.active.is_some(), button).clicked() {
            pick_tool(app, status);
            ui.close();
        }
    }
    ui.separator();
    if ui.button("Takeoff panel").clicked() {
        open_panel(app);
        ui.close();
    }
    if ui.add_enabled(app.active.is_some(), egui::Button::new("Export takeoff as CSV…")).clicked() {
        takeoff_panel::export_csv(app);
        ui.close();
    }
}

/// The takeoff tool a control-channel name picks (`takeoff-included`, …).
pub(crate) fn tool_from_name(name: &str) -> Option<QuickTool> {
    Status::ALL.into_iter().find(|s| tool_name(*s) == name).map(QuickTool::Takeoff)
}

/// The takeoff tools in the floating tool strip beside the page: a divider, then a green, a
/// yellow and a red box, each with its key in the corner.
pub(crate) fn quick_bar_tools(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &crate::theme::Tokens) {
    use egui::{Rect, Sense, Stroke, vec2};
    let (line, _) = ui.allocate_exact_size(vec2(32.0, 9.0), Sense::hover());
    ui.painter().hline(line.x_range().shrink(6.0), line.center().y, Stroke::new(1.0, t.divider));
    for status in Status::ALL {
        let selected = app.quick_tool == QuickTool::Takeoff(status);
        let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
        let press = crate::theme::Press::track(ui, &resp);
        press.wash(ui, rect, t.radius, selected);
        let body = rect.translate(press.offset());
        let colour = takeoff_tool::color(status);
        let mark = Rect::from_center_size(body.center() - vec2(1.0, 1.0), vec2(16.0, 13.0));
        ui.painter().rect_filled(mark, 2.0, colour.gamma_multiply(0.18));
        ui.painter().rect_stroke(mark, 2.0, Stroke::new(2.5, colour), egui::StrokeKind::Middle);
        ui.painter().text(
            body.right_bottom() + vec2(-3.0, -1.0),
            egui::Align2::RIGHT_BOTTOM,
            status.hotkey(),
            crate::theme::medium(9.0),
            t.text_faint,
        );
        let label = status.label();
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, label));
        if crate::theme::hand(resp).on_hover_text(format!("{label} (key {})", status.hotkey())).clicked() {
            pick_tool(app, status);
        }
    }
}
