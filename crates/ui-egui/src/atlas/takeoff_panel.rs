//! The Takeoff panel: the three tools with their counts, every mark sheet by sheet, the selected
//! mark's status and label, and CSV export. Comments go on the quote, not on boxes, so the panel
//! has none (a comment another program left on a box is kept when the box is edited).

use super::takeoff_tool::color;
use crate::{PdfCraftApp, QuickTool};
use egui::RichText;
use pdfcraft_atlas::takeoff::{self, Listing, MAX_TEXT, Mark, MarkText, Status};
use pdfcraft_engine::{DocId, Edit};

/// What the panel shows, and the selected mark's editable copy.
#[derive(Default)]
pub(crate) struct PanelState {
    /// Statuses filtered out of the list, in [`Status::ALL`] order.
    hidden: [bool; 3],
    /// Only the current page's marks.
    this_page: bool,
    editor: Option<Editor>,
}

struct Editor {
    doc: DocId,
    page: usize,
    index: usize,
    status: Status,
    label: String,
    comment: String,
    /// The mark as it was when the editor was filled: a change made elsewhere (a 1/2/3 key, an
    /// undo) refills it.
    original: MarkText,
}

fn slot(status: Status) -> usize {
    match status {
        Status::Included => 0,
        Status::IncludedWithComments => 1,
        Status::Excluded => 2,
    }
}

/// What a click in the panel asks for, applied once drawing is done.
enum Action {
    Tool(Status),
    Select(usize, usize),
    Edit(Box<Edit>),
    Export,
}

pub(crate) fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().id_salt("atlas-takeoff-panel").auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(6.0);
        let action = body(app, ui);
        apply(app, action);
    });
}

fn body(app: &mut PdfCraftApp, ui: &mut egui::Ui) -> Option<Action> {
    let Some((index, id)) = app.active_ids() else {
        ui.label("Open a drawing to start a takeoff.");
        return None;
    };
    let doc = app.session.get(id)?;
    let listing: Listing = app.atlas.listing(id, doc).cloned().unwrap_or_default();
    let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
    let can_edit = doc.allows_annotation();
    let view = app.views.get(index)?;
    let (current, selected) = (view.current, view.comments.selected);
    let state = &mut app.atlas.panel;
    let mut action = None;

    // The tools, with how many of each this drawing has.
    let counts = listing.counts();
    ui.horizontal_wrapped(|ui| {
        for status in Status::ALL {
            let n = counts.get(slot(status)).copied().unwrap_or(0);
            let text = RichText::new(format!("■ {}  {n}", status.label())).color(color(status)).strong();
            let tool = egui::Button::new(text).selected(app.quick_tool == QuickTool::Takeoff(status));
            if ui.add_enabled(can_edit, tool).on_hover_text(format!("Draw {} boxes (key {})", status.label(), status.hotkey())).clicked() {
                action = Some(Action::Tool(status));
            }
        }
    });
    if !can_edit {
        ui.label(RichText::new("This document's security settings don't allow comments.").weak());
    }
    ui.label(RichText::new("Drag a box over equipment; click a box to select it. 1, 2, 3 re-mark the selected box (or pick the tool when none is selected); Delete removes it.").small().weak());
    if listing.truncated {
        ui.label(RichText::new("This document has too many annotations; only the first are listed.").weak());
    }
    ui.separator();

    // Filters.
    ui.horizontal_wrapped(|ui| {
        for status in Status::ALL {
            if let Some(hidden) = state.hidden.get_mut(slot(status)) {
                let mut shown = !*hidden;
                if ui.add(crate::widgets::checkbox(&mut shown, RichText::new(status.label()).color(color(status)))).changed() {
                    *hidden = !shown;
                }
            }
        }
    });
    ui.add(crate::widgets::checkbox(&mut state.this_page, "This page only"));

    // The marks, sheet by sheet.
    let shown: Vec<&Mark> = listing
        .marks
        .iter()
        .filter(|m| !state.hidden.get(slot(m.status)).copied().unwrap_or(false))
        .filter(|m| !state.this_page || m.page == current)
        .collect();
    if listing.marks.is_empty() {
        ui.label(RichText::new("No takeoff marks yet.").weak());
    } else if shown.is_empty() {
        ui.label(RichText::new("No marks match the filters.").weak());
    }
    let mut last_page = None;
    for m in &shown {
        if last_page != Some(m.page) {
            last_page = Some(m.page);
            ui.add_space(4.0);
            ui.label(RichText::new(sheet_title(&labels, m.page)).strong());
        }
        let is_selected = selected == Some((m.page, m.index));
        let label = if m.label.is_empty() { "(no label)" } else { m.label.as_str() };
        let row = ui.horizontal(|ui| {
            let (swatch, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().rect_filled(swatch, 2.0, color(m.status));
            ui.selectable_label(is_selected, label)
        });
        if row.inner.clicked() {
            action = Some(Action::Select(m.page, m.index));
        }
    }

    // The selected mark.
    let mark = selected.and_then(|(page, index)| listing.marks.iter().find(|m| m.page == page && m.index == index));
    match mark {
        None => state.editor = None,
        Some(m) => {
            let now = MarkText { status: m.status, label: m.label.clone(), comment: m.comment.clone() };
            let fresh = state.editor.as_ref().is_none_or(|e| e.doc != id || e.page != m.page || e.index != m.index || e.original != now);
            if fresh {
                state.editor = Some(Editor {
                    doc: id,
                    page: m.page,
                    index: m.index,
                    status: m.status,
                    label: m.label.clone(),
                    comment: m.comment.clone(),
                    original: now,
                });
            }
        }
    }
    if let Some(e) = state.editor.as_mut() {
        ui.separator();
        ui.label(RichText::new("Selected mark").strong());
        ui.horizontal_wrapped(|ui| {
            for status in Status::ALL {
                crate::widgets::radio_value(ui, &mut e.status, status, RichText::new(status.label()).color(color(status)));
            }
        });
        ui.label("Label");
        ui.add(egui::TextEdit::singleline(&mut e.label).desired_width(f32::INFINITY).char_limit(MAX_TEXT));
        ui.horizontal(|ui| {
            let text = MarkText { status: e.status, label: e.label.clone(), comment: e.comment.clone() };
            if ui.add_enabled(can_edit, egui::Button::new("Save")).clicked() {
                action = Some(Action::Edit(Box::new(Edit::UpdateTakeoffMark { page: e.page, index: e.index, text })));
            }
            if ui.add_enabled(can_edit, egui::Button::new("Delete")).clicked() {
                action = Some(Action::Edit(Box::new(Edit::DeleteAnnotation { page: e.page, index: e.index })));
            }
        });
    }

    ui.separator();
    if ui.add_enabled(!listing.marks.is_empty(), egui::Button::new("Export takeoff as CSV…")).clicked() {
        action = Some(Action::Export);
    }
    action
}

/// "E-600 · page 3", or "Page 3" for a page without a label.
fn sheet_title(labels: &[String], page: usize) -> String {
    let number = page.saturating_add(1);
    match labels.get(page).map(|l| l.trim()).filter(|l| !l.is_empty() && *l != number.to_string()) {
        Some(label) => format!("{label} · page {number}"),
        None => format!("Page {number}"),
    }
}

fn apply(app: &mut PdfCraftApp, action: Option<Action>) {
    let Some(action) = action else { return };
    let Some((index, _)) = app.active_ids() else { return };
    match action {
        Action::Tool(status) => app.quick_tool = QuickTool::Takeoff(status),
        Action::Select(page, annotation) => {
            app.quick_tool = QuickTool::Select;
            if let Some(view) = app.views.get_mut(index) {
                view.go_to_page(page);
                view.comments.selected = Some((page, annotation));
            }
        }
        Action::Edit(edit) => {
            if let Some(view) = app.views.get_mut(index) {
                view.pending_edit = Some(*edit);
            }
        }
        Action::Export => export_csv(app),
    }
}

/// Save the active document's marks as CSV beside it (asking where).
pub(crate) fn export_csv(app: &mut PdfCraftApp) {
    let Some((_, id)) = app.active_ids() else { return };
    let Some(doc) = app.session.get(id) else { return };
    let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
    let name = format!("{} takeoff.csv", doc.name.trim_end_matches(".pdf").trim_end_matches(".PDF"));
    let csv = match doc.takeoff_marks() {
        Ok(listing) => takeoff::to_csv(&listing.marks, &|page| labels.get(page).cloned().unwrap_or_default()),
        Err(e) => {
            app.notify(e);
            return;
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let write =
            move |app: &mut PdfCraftApp, path: std::path::PathBuf| match crate::editing::write_atomically(&path.to_string_lossy(), csv.as_bytes()) {
                Ok(()) => app.notify("Saved the takeoff."),
                Err(e) => app.notify(e.to_string()),
            };
        match app.export_dir_override.as_ref().map(|d| std::path::PathBuf::from(d).join(&name)) {
            Some(path) => write(app, path),
            None => {
                let dialog = rfd::AsyncFileDialog::new().set_file_name(&name).add_filter("CSV", &["csv"]);
                app.ask_one(crate::pickers::Ask::Save(dialog), None, write);
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    if let Err(e) = crate::editing::download(&name, csv.as_bytes()) {
        app.notify(&e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_titles_use_real_labels_only() {
        let labels = vec!["E-600".to_owned(), "2".to_owned(), "  ".to_owned()];
        assert_eq!(sheet_title(&labels, 0), "E-600 · page 1");
        // A label that is just the page number says nothing new.
        assert_eq!(sheet_title(&labels, 1), "Page 2");
        assert_eq!(sheet_title(&labels, 2), "Page 3");
        assert_eq!(sheet_title(&labels, 9), "Page 10");
    }
}
