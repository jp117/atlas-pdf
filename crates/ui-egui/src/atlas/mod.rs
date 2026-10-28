//! Atlas fork: the interface for Atlas Switch's takeoff workflow.
//!
//! Everything Atlas adds to the window lives here, reached from a handful of one-line hooks in
//! the upstream files (`state.rs`, `canvas.rs`, `chrome.rs`, `menus.rs`, `editing.rs`,
//! `panels/right.rs`, `app/control.rs`, `app/keys.rs`), so the fork merges cleanly. Strings are plain English on purpose: `tl!` literals would have to be
//! added to every upstream translation catalogue.

mod layers_panel;
mod takeoff_panel;
mod takeoff_tool;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod updates;

use crate::{PdfCraftApp, QuickTool};
use pdfcraft_atlas::takeoff::{Listing, MarkText, Status};
use pdfcraft_engine::{DocId, Document, Edit};

pub(crate) use layers_panel::{apply as apply_layers, show as layers_panel};
pub(crate) use takeoff_panel::side_panel as takeoff_side_panel;
pub(crate) use takeoff_tool::page;

/// Atlas's per-window state.
#[derive(Default)]
pub(crate) struct AtlasState {
    /// The takeoff box being dragged.
    drag: Option<takeoff_tool::Drag>,
    /// The active document's marks, for its edit generation.
    listing: Option<(DocId, u64, Result<Listing, String>)>,
    /// The Takeoff panel's filters and editor.
    panel: takeoff_panel::PanelState,
    /// Updating an installed copy.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) updates: updates::UpdateState,
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

/// 1, 2 and 3: with a takeoff mark selected (by any tool), re-mark it with that status;
/// otherwise pick that takeoff tool. Plain digits, so only when nothing else has the keyboard
/// (the same rule as the V and H tool keys).
pub(crate) fn keys(app: &mut PdfCraftApp, i: usize, ctx: &egui::Context) {
    let Some(view) = app.views.get(i) else { return };
    let free = app.dialog.is_none() && !app.palette_open && !ctx.egui_wants_keyboard_input() && view.forms.focus.is_none();
    if !free || !ctx.input(|input| input.modifiers.is_none()) {
        return;
    }
    let Some(status) = Status::ALL.into_iter().find(|s| ctx.input(|input| input.key_pressed(digit(*s)))) else { return };
    if let Some(edit) = restatus_selected(app, i, status) {
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

/// Show the Takeoff panel in the side panel's place, as the rail's Takeoff button does.
fn open_panel(app: &mut PdfCraftApp) {
    if app.right != Some(crate::RightPanel::Takeoff) {
        app.choose_right_panel(Some(crate::RightPanel::Takeoff));
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

/// The Atlas menu's commands: a takeoff tool, the Takeoff panel and the CSV export.
const PANEL_COMMAND: &str = "atlas.takeoff_panel";
const EXPORT_COMMAND: &str = "atlas.export_csv";

fn menu_command(status: Status) -> &'static str {
    match status {
        Status::Included => "atlas.takeoff.included",
        Status::IncludedWithComments => "atlas.takeoff.included_with_comments",
        Status::Excluded => "atlas.takeoff.excluded",
    }
}

/// Whether `id` is one of the Atlas menu's commands.
pub(crate) fn is_menu_command(id: &str) -> bool {
    id == PANEL_COMMAND || id == EXPORT_COMMAND || Status::ALL.into_iter().any(|s| menu_command(s) == id)
}

/// The Atlas menu: the takeoff tools, the Takeoff panel and the CSV export. The keys go in the
/// labels, not as shortcuts: a plain digit as a native menu's key equivalent would take it from
/// text fields.
pub(crate) fn menu() -> craft_menu::Menu {
    use craft_menu::{Item, Node};
    let mut tools: Vec<Node> = Status::ALL
        .into_iter()
        .map(|status| Item::new(menu_command(status), format!("{} (key {})", status.label(), status.hotkey())).radio(false).into())
        .collect();
    tools.push(Node::Separator);
    tools.push(Item::new(PANEL_COMMAND, "Takeoff panel").into());
    tools.push(Item::new(EXPORT_COMMAND, "Export takeoff as CSV…").into());
    craft_menu::Menu::new("Atlas", vec![Node::submenu("Takeoff", tools)])
}

/// An Atlas menu item's state, or `None` for another menu's command.
pub(crate) fn menu_state(app: &PdfCraftApp, command: &craft_menu::CommandRef) -> Option<craft_menu::ItemState> {
    let id = command.id.as_str();
    if !is_menu_command(id) {
        return None;
    }
    let tool = Status::ALL.into_iter().find(|s| menu_command(*s) == id);
    Some(craft_menu::ItemState {
        enabled: id == PANEL_COMMAND || app.active.is_some(),
        checked: tool.map(|s| app.quick_tool == QuickTool::Takeoff(s)),
    })
}

/// Run an Atlas menu command; `false` for another menu's.
pub(crate) fn run_menu_command(app: &mut PdfCraftApp, command: &craft_menu::CommandRef) -> bool {
    let id = command.id.as_str();
    if let Some(status) = Status::ALL.into_iter().find(|s| menu_command(*s) == id) {
        pick_tool(app, status);
    } else if id == PANEL_COMMAND {
        open_panel(app);
    } else if id == EXPORT_COMMAND {
        takeoff_panel::export_csv(app);
    } else {
        return false;
    }
    true
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
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, label));
        if crate::theme::hand(resp).on_hover_text(format!("{label} (key {})", status.hotkey())).clicked() {
            pick_tool(app, status);
        }
    }
}
