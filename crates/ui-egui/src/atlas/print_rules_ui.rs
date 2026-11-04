//! Atlas fork: Preferences ▸ Printer rules, which paper goes to which printer. Each change is saved
//! at once to the file the Print dialog and the command line read.

use pdfcraft_atlas::print_rules::{PrinterRule, PrinterRules};
use pdfcraft_engine::print::PAPERS;

use super::print_routing::RulesSource;
use crate::PdfCraftApp;
use crate::theme::{self, Tokens};

/// The rules being edited.
#[derive(Default)]
pub(crate) struct RulesEditor {
    /// A row with no printer yet stays here, and out of the file, until it has one.
    rows: Vec<PrinterRule>,
    /// Why the rules couldn't be read or saved.
    problem: Option<String>,
}

/// Save the complete rows.
fn save(source: &mut RulesSource, rows: &[PrinterRule]) -> Result<(), String> {
    let rules = PrinterRules { rules: rows.iter().filter(|rule| !rule.printer.is_empty()).cloned().collect() };
    match source {
        RulesSource::Fixed(fixed) => {
            *fixed = rules;
            Ok(())
        }
        #[cfg(not(target_arch = "wasm32"))]
        RulesSource::File(path) => {
            if let Some(folder) = path.parent() {
                std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
            }
            crate::editing::write_atomically(&path.to_string_lossy(), rules.to_json().as_bytes()).map_err(|e| e.to_string())
        }
        #[cfg(target_arch = "wasm32")]
        RulesSource::File(_) => Err("a browser has no settings folder".to_string()),
    }
}

/// The Printer rules section of Preferences. Nothing without a rules source.
pub(crate) fn section(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    let Some(mut source) = app.atlas.print.source.clone() else { return };
    list_printers_once(app);
    let installed: Vec<String> = app.print_draft.printers.iter().map(|p| p.name.clone()).collect();
    let loading = app.print_draft.printers_loading;
    let editor = app.atlas.print.editor.get_or_insert_with(|| {
        let loaded = source.load();
        RulesEditor { rows: loaded.rules.rules, problem: loaded.problem }
    });
    ui.label(egui::RichText::new("Printer rules").font(theme::semibold(13.0)));
    ui.label(
        egui::RichText::new("Which printer each paper size goes to. Print reads the size from the PDF and preselects these.")
            .small()
            .color(t.text_muted),
    );
    let mut changed = false;
    let mut remove = None;
    egui::Grid::new("printer-rules").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
        for (i, rule) in editor.rows.iter_mut().enumerate() {
            craft_ui::ComboBox::from_id_salt(("printer-rule-paper", i))
                .selected_text(rule.paper.as_str())
                .width(190.0)
                .show_ui(ui, |ui| {
                    for (name, _) in &PAPERS {
                        if ui.selectable_label(rule.paper == *name, *name).clicked() {
                            rule.paper = (*name).to_string();
                            changed = true;
                        }
                    }
                })
                .response
                .accessible_name(format!("Paper of rule {}", i + 1));
            let shown = match rule.printer.as_str() {
                "" => "Choose a printer…".to_string(),
                name if !loading && !installed.iter().any(|p| p == name) => format!("{name} (not installed)"),
                name => name.to_string(),
            };
            craft_ui::ComboBox::from_id_salt(("printer-rule-printer", i))
                .selected_text(shown)
                .width(240.0)
                .show_ui(ui, |ui| {
                    for name in &installed {
                        if ui.selectable_label(rule.printer == *name, name.as_str()).clicked() {
                            rule.printer = name.clone();
                            changed = true;
                        }
                    }
                })
                .response
                .accessible_name(format!("Printer of rule {}", i + 1));
            if ui.button("Remove").on_hover_text("Remove this rule").clicked() {
                remove = Some(i);
            }
            ui.end_row();
        }
    });
    if let Some(i) = remove.filter(|i| *i < editor.rows.len()) {
        editor.rows.remove(i);
        changed = true;
    }
    ui.horizontal(|ui| {
        if ui.button("Add rule").clicked() {
            let paper = PAPERS.iter().map(|(name, _)| *name).find(|name| !editor.rows.iter().any(|rule| rule.paper == *name)).unwrap_or("US Letter");
            editor.rows.push(PrinterRule { paper: paper.to_string(), printer: String::new() });
        }
        if ui.button("Restore starting rules").clicked() {
            editor.rows = PrinterRules::starter().rules;
            changed = true;
        }
    });
    if changed {
        editor.problem = save(&mut source, &editor.rows).err().map(|e| format!("The printer rules couldn't be saved: {e}"));
        app.atlas.print.source = Some(source);
    }
    if let Some(duplicate) = first_duplicate(&editor.rows) {
        ui.label(egui::RichText::new(format!("{duplicate} has more than one rule. The first one is used.")).small().color(t.text_muted));
    }
    if let Some(problem) = &editor.problem {
        ui.label(egui::RichText::new(problem.as_str()).small().color(t.text_muted));
    }
}

/// The first paper with two rules.
fn first_duplicate(rows: &[PrinterRule]) -> Option<&str> {
    rows.iter().enumerate().find(|(i, rule)| rows.iter().take(*i).any(|earlier| earlier.paper == rule.paper)).map(|(_, rule)| rule.paper.as_str())
}

/// The printers to offer: Print may not have opened yet in this session.
fn list_printers_once(app: &mut PdfCraftApp) {
    if std::mem::replace(&mut app.atlas.print.asked_printers, true) || !app.print_draft.printers.is_empty() {
        return;
    }
    app.refresh_printers_for_rules();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(paper: &str, printer: &str) -> PrinterRule {
        PrinterRule { paper: paper.into(), printer: printer.into() }
    }

    #[test]
    fn a_row_without_a_printer_stays_out_of_the_rules() {
        let mut source = RulesSource::Fixed(PrinterRules::default());
        save(&mut source, &[rule("A4", "P"), rule("A3", "")]).unwrap();
        let RulesSource::Fixed(saved) = source else { panic!("the same source") };
        assert_eq!(saved.rules, [rule("A4", "P")]);
    }

    #[test]
    fn a_second_rule_for_a_paper_is_pointed_out() {
        assert_eq!(first_duplicate(&[rule("A4", "P"), rule("A3", "Q"), rule("A4", "R")]), Some("A4"));
        assert_eq!(first_duplicate(&[rule("A4", "P"), rule("A3", "P")]), None);
        assert_eq!(first_duplicate(&[]), None);
    }
}
