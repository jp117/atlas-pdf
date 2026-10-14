//! Atlas fork: the interface for Atlas Switch's takeoff workflow.
//!
//! Everything Atlas adds to the window lives here, reached from a handful of one-line hooks in
//! the upstream files (`lib.rs`, `canvas.rs`, `chrome.rs`, `editing.rs`, `control.rs`), so the
//! fork rebases cleanly. Strings are plain English on purpose: `tl!` literals would have to be
//! added to every upstream translation catalogue.

mod takeoff_tool;

use crate::{PdfCraftApp, QuickTool};
use pdfcraft_atlas::takeoff::{Listing, MarkText, Status};
use pdfcraft_engine::{DocId, Document, Edit};

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
    app.quick_tool = QuickTool::Takeoff(status);
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
            app.quick_tool = QuickTool::Takeoff(status);
            ui.close();
        }
    }
}

/// The takeoff tool a control-channel name picks (`takeoff-included`, …).
pub(crate) fn tool_from_name(name: &str) -> Option<QuickTool> {
    Status::ALL.into_iter().find(|s| tool_name(*s) == name).map(QuickTool::Takeoff)
}
