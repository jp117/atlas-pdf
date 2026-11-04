//! Atlas fork: the Print dialog follows the printer rules (`pdfcraft_atlas::print_rules`). When it
//! opens it reads the size of the pages to print, preselects the paper and printer the rules give
//! that size, and says why. What the person changes stands for that job. With no rules source
//! (tests of the upstream dialog, the browser) the dialog behaves as it always did.

use std::path::PathBuf;

use pdfcraft_atlas::print_rules::{self, Conflict, LoadedRules, Plan, PrinterRules, Selected, SizeGroup};
use pdfcraft_engine::print::{self, PAPERS, SizeMode};

use crate::PdfCraftApp;
use crate::print_ui::{Handling, PrintDraft};
use crate::theme::Tokens;

/// The colour of a warning that a job goes against the rules: legible on the light and dark themes.
const WARNING: egui::Color32 = egui::Color32::from_rgb(196, 110, 0);

/// Where the printer rules come from.
#[derive(Clone, Debug)]
pub(crate) enum RulesSource {
    /// The rules file the app and the command line share, read each time Print opens.
    File(PathBuf),
    /// Rules held in memory, for tests.
    Fixed(PrinterRules),
}

impl RulesSource {
    pub(super) fn load(&self) -> LoadedRules {
        match self {
            RulesSource::File(path) => PrinterRules::load(path),
            RulesSource::Fixed(rules) => LoadedRules { rules: rules.clone(), problem: None },
        }
    }
}

/// The app's printer-rules state.
#[derive(Default)]
pub(crate) struct PrintRulesState {
    pub(crate) source: Option<RulesSource>,
    /// Preferences ▸ Printer rules, while it is open.
    pub(crate) editor: Option<super::print_rules_ui::RulesEditor>,
    /// Preferences has asked for the printer list.
    pub(crate) asked_printers: bool,
    /// Stands in for the Windows setting, in tests.
    pub(crate) windows_manages_default: Option<bool>,
}

impl PdfCraftApp {
    /// Follow the printer rules in this file (the desktop app's: `print_rules::app_file`).
    pub fn use_print_rules_file(&mut self, path: PathBuf) {
        self.atlas.print.source = Some(RulesSource::File(path));
    }

    /// Follow these printer rules.
    pub fn use_print_rules(&mut self, rules: PrinterRules) {
        self.atlas.print.source = Some(RulesSource::Fixed(rules));
    }

    /// Take this as Windows' "Let Windows manage my default printer" setting, instead of reading it.
    pub fn assume_windows_manages_default_printer(&mut self, manages: bool) {
        self.atlas.print.windows_manages_default = Some(manages);
    }

    /// The printer list, for Preferences ▸ Printer rules, the way Print reads it.
    pub(crate) fn refresh_printers_for_rules(&mut self) {
        self.print_draft.printers_loading = true;
        self.refresh_print_list();
    }
}

/// One size's job, when mixed page sizes go out separately.
#[derive(Clone, Debug, PartialEq)]
pub struct JobRow {
    /// The size this row is for: its paper, else its size to the point.
    key: (Option<usize>, (i64, i64)),
    /// `None`: the person must choose.
    pub printer: Option<String>,
    pub paper: usize,
    /// The person chose these, so a new plan keeps them.
    printer_picked: bool,
    paper_picked: bool,
    /// Why this printer and paper.
    pub reason: String,
    /// How this choice goes against the rules.
    pub conflict: Option<Conflict>,
}

impl JobRow {
    /// A row for `group`, keeping what the person chose in `old`.
    fn new(group: &SizeGroup, last_paper: usize, old: &[JobRow]) -> Self {
        // `as` saturates, and sizes are finite and at most what the plan accepted.
        let key = (group.paper, (group.size.0.round() as i64, group.size.1.round() as i64));
        let choice = print_rules::choose(group, last_paper);
        let kept = old.iter().find(|row| row.key == key);
        let printer_picked = kept.is_some_and(|row| row.printer_picked);
        let paper_picked = kept.is_some_and(|row| row.paper_picked);
        JobRow {
            key,
            printer: if printer_picked { kept.and_then(|row| row.printer.clone()) } else { choice.printer },
            paper: if paper_picked { kept.map_or(choice.paper, |row| row.paper) } else { choice.paper },
            printer_picked,
            paper_picked,
            reason: choice.reason,
            conflict: None,
        }
    }
}

/// What the plan was last made from: it is made again when any of this changes.
#[derive(Clone, Debug, PartialEq)]
struct Seen {
    pages: Vec<usize>,
    printers: Vec<String>,
    loading: bool,
}

/// The Print dialog's state for following the rules, for one opening of the dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrintRouting {
    /// The rules apply to this print.
    pub active: bool,
    pub rules: PrinterRules,
    /// Why the rules (or some of them) couldn't be read.
    pub rules_problem: Option<String>,
    /// The pages to print, by size.
    pub plan: Plan,
    seen: Option<Seen>,
    /// The default printer when a job was last sent, kept across sessions.
    pub default_seen: Option<String>,
    /// The note about Windows moving the default printer has been shown, kept across sessions.
    pub note_shown: bool,
    /// "Let Windows manage my default printer" is on.
    pub windows_manages_default: bool,
    /// Shown once, in the Print dialog this was found in.
    pub default_note: Option<String>,
    /// The person asked to edit the rules: Preferences opens instead.
    pub open_rules: bool,
    /// Why this printer and paper, for the dialog to say.
    pub reason: Option<String>,
    /// No printer may be assumed: Print waits until one is chosen.
    pub must_choose: bool,
    /// The paper before the rules chose one, which is what "last used" still means until a job
    /// is sent on the rules' paper.
    pub last_used_paper: usize,
    /// The paper the rules last put in the dialog.
    applied_paper: Option<usize>,
    /// The person changed the paper for this job.
    pub paper_changed: bool,
    /// The layout the rules last put in the dialog.
    applied_layout: Option<(Handling, SizeMode)>,
    /// The layout before the rules chose Actual size, given back to a job they don't choose it for.
    layout_before: Option<(Handling, SizeMode)>,
    /// The person changed the layout for this job.
    layout_changed: bool,
    /// A job went out on the dialog's paper.
    sent: bool,
    /// How the selected printer and paper go against the rules.
    pub conflict: Option<Conflict>,
    /// Mixed page sizes go out as one job per size, on these rows' printers and papers.
    pub split: bool,
    /// The person chose to put every page on one paper instead.
    pub one_paper: bool,
    /// Per size, in the plan's order, while the pages are of mixed sizes.
    pub rows: Vec<JobRow>,
}

impl PrintRouting {
    /// The paper to remember as the last one used: what the rules put in the dialog doesn't count
    /// until a job is sent on it. So the setting keeps meaning what the person last used.
    pub(crate) fn paper_to_remember(&self, paper: usize) -> usize {
        if self.active && self.applied_paper.is_some() && !self.paper_changed && !self.sent { self.last_used_paper } else { paper }
    }

    /// The layout to start the next print from, likewise.
    fn layout_to_remember(&self) -> Option<(Handling, SizeMode)> {
        if self.active && !self.layout_changed { self.layout_before } else { None }
    }
}

/// Print has opened: start following the rules, when there are any to follow. The printer picked
/// last time and the paper used last time don't carry over: the pages decide.
pub(crate) fn on_open(app: &mut PdfCraftApp) {
    // Preferences reads the file again next time, in case it changed.
    app.atlas.print.editor = None;
    let manages = app.atlas.print.windows_manages_default.unwrap_or_else(windows_manages_default);
    let Some(source) = &app.atlas.print.source else { return };
    let loaded = source.load();
    let d = &mut app.print_draft;
    d.paper = d.routing.paper_to_remember(d.paper);
    if let Some((handling, size)) = d.routing.layout_to_remember() {
        d.handling = handling;
        d.size = size;
    }
    d.printer_picked = false;
    d.routing = PrintRouting {
        active: true,
        rules: loaded.rules,
        rules_problem: loaded.problem,
        last_used_paper: d.paper,
        default_seen: d.routing.default_seen.take(),
        note_shown: d.routing.note_shown,
        windows_manages_default: manages,
        ..PrintRouting::default()
    };
}

/// Each frame of the dialog: notice what the person changed, and choose again when the pages to
/// print or the printers changed.
pub(crate) fn update(d: &mut PrintDraft, sizes: &[(f64, f64)], labels: &[String]) {
    if !d.routing.active {
        return;
    }
    if d.routing.applied_paper.is_some_and(|paper| paper != d.paper) {
        d.routing.paper_changed = true;
    }
    if d.routing.applied_layout.is_some_and(|layout| layout != (d.handling, d.size)) {
        d.routing.layout_changed = true;
    }
    // A selection that isn't valid yet (a range being typed) keeps the last plan.
    if let Ok(settings) = d.settings(sizes.len(), labels) {
        let seen = Seen { pages: settings.pages, printers: d.printers.iter().map(|p| p.name.clone()).collect(), loading: d.printers_loading };
        if d.routing.seen.as_ref() != Some(&seen) {
            let names: Vec<&str> = seen.printers.iter().map(String::as_str).collect();
            let known = printers_known(d);
            d.routing.plan = print_rules::plan(sizes, &seen.pages, &d.routing.rules, known.then_some(names.as_slice()));
            d.routing.seen = Some(seen);
            apply(d);
        }
    }
    // Booklets and the like put pages on other papers on purpose: one job, on one paper.
    if d.routing.split && d.handling != Handling::Size {
        set_split(d, false);
    }
    d.routing.conflict = if splitting(d) { None } else { selected_conflict(d) };
    note_default_printer(d);
    refresh_row_conflicts(d);
}

/// Say once if Windows has moved the default printer since a job was last sent.
fn note_default_printer(d: &mut PrintDraft) {
    if d.printers_loading || d.routing.default_note.is_some() {
        return;
    }
    let now = d.printers.iter().find(|p| p.default).map(|p| p.name.as_str());
    let r = &mut d.routing;
    r.default_note = print_rules::default_printer_note(r.windows_manages_default, r.default_seen.as_deref(), now, r.note_shown);
    r.note_shown |= r.default_note.is_some();
}

/// Whether "Let Windows manage my default printer" is on: it is unless `LegacyDefaultPrinterMode`
/// says otherwise. Read only: Atlas PDF doesn't change it.
#[cfg(target_os = "windows")]
fn windows_manages_default() -> bool {
    use winsafe::{HKEY, RegistryValue, co::RRF};
    let key = "Software\\Microsoft\\Windows NT\\CurrentVersion\\Windows";
    !matches!(HKEY::CURRENT_USER.RegGetValue(Some(key), Some("LegacyDefaultPrinterMode"), RRF::RT_REG_DWORD), Ok(RegistryValue::Dword(1)))
}

#[cfg(not(target_os = "windows"))]
fn windows_manages_default() -> bool {
    false
}

/// Send each size to its printer, or everything on one paper.
fn set_split(d: &mut PrintDraft, split: bool) {
    d.routing.one_paper = !split;
    d.printer_picked = false;
    d.routing.paper_changed = false;
    // Planned again on the next frame.
    d.routing.seen = None;
    if split && d.handling != Handling::Size {
        d.handling = Handling::Size;
    }
}

fn refresh_row_conflicts(d: &mut PrintDraft) {
    let names: Vec<&str> = d.printers.iter().map(|p| p.name.as_str()).collect();
    let known = printers_known(d);
    let r = &mut d.routing;
    for (row, group) in r.rows.iter_mut().zip(&r.plan.groups) {
        row.conflict = row.printer.as_deref().and_then(|printer| {
            let selected = Selected { printer, paper: row.paper, one_page_per_sheet: true };
            print_rules::conflict(Some(group), selected, &r.rules, known.then_some(names.as_slice()))
        });
    }
}

/// The settings for one size's job: Actual size for wide-format drawings, otherwise as the dialog
/// has it.
fn row_settings(base: &print::Settings, group: &SizeGroup, row: &JobRow) -> Option<print::Settings> {
    let paper = PAPERS.get(row.paper)?.1;
    let layout = if group.wide_format() { print::Layout::Size(SizeMode::Actual) } else { base.layout };
    Some(print::Settings { pages: group.pages.clone(), paper, layout, ..base.clone() })
}

/// The sheets the dialog's settings make: per size when the sizes go out separately, so the
/// preview shows each on its own paper.
pub(crate) fn sheets_to_print(d: &PrintDraft, sizes: &[(f64, f64)], settings: &print::Settings) -> Result<Vec<print::Sheet>, print::PrintError> {
    if !splitting(d) {
        return print::layout(sizes, settings);
    }
    let mut sheets = Vec::new();
    for (group, row) in d.routing.plan.groups.iter().zip(&d.routing.rows) {
        let Some(settings) = row_settings(settings, group, row) else { continue };
        sheets.extend(print::layout(sizes, &settings)?);
    }
    Ok(sheets)
}

/// Send one job per size, when that is what the dialog holds. `None` when it isn't. Everything is
/// laid out before anything is sent, so a size that can't be leaves no jobs behind. A job is only
/// sent to the printer its row names.
pub(crate) fn print_split(app: &mut PdfCraftApp) -> Option<bool> {
    if !splitting(&app.print_draft) {
        return None;
    }
    let (_, id) = app.active_ids()?;
    let doc = app.session.get(id)?;
    let name = doc.name.clone();
    let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
    let base = match app.print_draft.settings(doc.info.pages.len(), &labels) {
        Ok(settings) => settings,
        Err(e) => {
            app.notify_tr(&e);
            return Some(false);
        }
    };
    note_default_before_send(&mut app.print_draft);
    let mut jobs = Vec::new();
    for (group, row) in app.print_draft.routing.plan.groups.clone().iter().zip(app.print_draft.routing.rows.clone()) {
        let Some(printer) = row.printer.clone() else { return Some(false) };
        let Some(settings) = row_settings(&base, group, &row) else { return Some(false) };
        let bytes = match app.session.print_pdf(id, &settings) {
            Ok(bytes) => bytes,
            Err(e) => {
                app.notify_error(e);
                return Some(false);
            }
        };
        let mut job = app.print_draft.job(&group.job_title(&name));
        job.printer = Some(printer.clone());
        job.options.retain(|(key, _)| key != "media");
        job.options.extend(print::media_option(row.paper));
        jobs.push((printer, bytes, job));
    }
    let mut sent = true;
    for (printer, bytes, job) in jobs {
        sent &= app.send_to_printer(printer, bytes, job);
    }
    Some(sent)
}

/// The mixed-size breakdown: one row per size with its printer and paper, or the choice to put
/// every page on one paper.
fn mixed_panel(ui: &mut egui::Ui, d: &mut PrintDraft, t: &Tokens) {
    if !d.routing.plan.is_mixed() {
        return;
    }
    let can_split = d.routing.plan.can_split();
    let jobs = d.routing.plan.groups.len();
    let mut split = d.routing.split;
    if can_split {
        ui.add_enabled_ui(d.handling == Handling::Size, |ui| {
            crate::widgets::radio_value(ui, &mut split, true, format!("Send each size to its printer ({jobs} separate jobs)"));
        });
    } else {
        ui.label(
            egui::RichText::new(format!(
                "These pages have too many sizes to send apart ({jobs}). Choose one paper for them all, or print fewer pages."
            ))
            .small()
            .color(t.text_muted),
        );
    }
    crate::widgets::radio_value(ui, &mut split, false, "Print every page on one paper");
    if split != d.routing.split {
        set_split(d, split);
    }
    if !d.routing.split {
        ui.label(
            egui::RichText::new(
                "Pages of other sizes are scaled or cut to fit that paper, as Page Sizing says. The paper shown is the last one used.",
            )
            .small()
            .color(t.text_muted),
        );
        return;
    }
    let printers: Vec<String> = d.printers.iter().map(|p| p.name.clone()).collect();
    let groups = d.routing.plan.groups.clone();
    let mut fixes = Vec::new();
    egui::Grid::new("print-size-jobs").num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
        for (i, (group, row)) in groups.iter().zip(d.routing.rows.iter_mut()).enumerate() {
            let size = group.size_name();
            let caption = ui.label(format!("{} {size}", print_rules::page_count(group.pages.len())));
            let shown = row.printer.clone().unwrap_or_else(|| "Choose a printer…".to_string());
            let before = row.printer.clone();
            craft_ui::ComboBox::from_id_salt(("print-size-printer", i))
                .selected_text(shown)
                .width(190.0)
                .show_ui(ui, |ui| {
                    for name in &printers {
                        ui.selectable_value(&mut row.printer, Some(name.clone()), name.as_str());
                    }
                })
                .response
                .labelled_by(caption.id);
            if row.printer != before {
                row.printer_picked = true;
            }
            let before = row.paper;
            craft_ui::ComboBox::from_id_salt(("print-size-paper", i))
                .selected_text(PAPERS.get(row.paper).map_or("", |p| p.0))
                .width(150.0)
                .show_ui(ui, |ui| {
                    for (index, (name, _)) in PAPERS.iter().enumerate() {
                        ui.selectable_value(&mut row.paper, index, *name);
                    }
                })
                .response
                .accessible_name(format!("Paper for {size}"));
            if row.paper != before {
                row.paper_picked = true;
            }
            ui.end_row();
            if row.printer.is_none() || group.paper.is_none() {
                ui.label("");
                ui.label(egui::RichText::new(row.reason.as_str()).small().color(t.text_muted));
                ui.end_row();
            }
            if let Some(conflict) = &row.conflict {
                ui.label("");
                ui.label(egui::RichText::new(conflict.message.as_str()).small().color(WARNING));
                if let Some(label) = conflict.fix_label()
                    && ui.button(label).clicked()
                    && let Some(fix) = conflict.fix.clone()
                {
                    fixes.push((i, fix));
                }
                ui.end_row();
            }
        }
    });
    for (i, (printer, paper)) in fixes {
        if let Some(row) = d.routing.rows.get_mut(i) {
            row.printer = Some(printer);
            row.paper = paper;
            row.printer_picked = false;
            row.paper_picked = false;
        }
    }
}

/// Whether the printer list can be believed: until the first one is in, no printer can be called
/// missing.
fn printers_known(d: &PrintDraft) -> bool {
    !d.printers_loading || !d.printers.is_empty()
}

/// How the dialog's printer and paper go against the rules. Save as PDF goes against nothing.
fn selected_conflict(d: &PrintDraft) -> Option<Conflict> {
    let printer = d.printer.as_deref()?;
    let names: Vec<&str> = d.printers.iter().map(|p| p.name.as_str()).collect();
    let selected = Selected { printer, paper: d.paper, one_page_per_sheet: d.handling == Handling::Size };
    print_rules::conflict(d.routing.plan.single(), selected, &d.routing.rules, printers_known(d).then_some(names.as_slice()))
}

/// The one-click fix: the printer and paper the rules want, as if they had chosen them.
fn apply_fix(d: &mut PrintDraft, printer: String, paper: usize) {
    d.printer = Some(printer);
    d.printer_picked = false;
    d.paper = paper;
    d.paper_before_label = None;
    d.routing.paper_changed = false;
    d.routing.applied_paper = Some(paper);
    if d.routing.plan.single().is_some_and(print_rules::SizeGroup::wide_format) && d.handling == Handling::Size {
        d.size = SizeMode::Actual;
    }
    d.routing.layout_changed = false;
    d.routing.applied_layout = Some((d.handling, d.size));
    d.routing.conflict = None;
}

/// What the Print button says, when it isn't plain Print: sending past a warning is a choice.
pub(crate) fn print_label(d: &PrintDraft) -> Option<String> {
    if !d.routing.active {
        return None;
    }
    if splitting(d) {
        let jobs = d.routing.rows.len();
        let warned = d.routing.rows.iter().any(|row| row.conflict.is_some());
        return Some(if warned { format!("Send {jobs} jobs anyway") } else { format!("Send {jobs} jobs") });
    }
    d.routing.conflict.is_some().then(|| "Print anyway".to_string())
}

/// The warning before sending, with its fix, above the Print button.
pub(crate) fn conflict_strip(ui: &mut egui::Ui, d: &mut PrintDraft) {
    if !d.routing.active {
        return;
    }
    let Some(conflict) = d.routing.conflict.clone() else { return };
    let mut fix = false;
    egui::Frame::new().stroke(egui::Stroke::new(1.0, WARNING)).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(conflict.message.as_str()).color(WARNING).strong());
            if let Some(label) = conflict.fix_label() {
                fix = ui.button(label).clicked();
            }
        },
    );
    if fix && let Some((printer, paper)) = conflict.fix {
        apply_fix(d, printer, paper);
    }
    ui.add_space(4.0);
}

/// Put the plan's choice in the dialog, except what the person has changed for this job.
fn apply(d: &mut PrintDraft) {
    let last = d.routing.last_used_paper;
    let Some(group) = d.routing.plan.single() else {
        apply_mixed(d);
        return;
    };
    d.routing.split = false;
    d.routing.rows.clear();
    let choice = print_rules::choose(group, last);
    d.routing.reason = Some(choice.reason);
    d.routing.must_choose = choice.printer.is_none();
    if !d.printer_picked {
        d.printer = choice.printer;
    }
    if !d.routing.paper_changed {
        d.paper = choice.paper;
        d.paper_before_label = None;
        d.routing.applied_paper = Some(choice.paper);
    }
    if d.routing.layout_changed {
        return;
    }
    if choice.actual {
        if d.routing.layout_before.is_none() {
            d.routing.layout_before = Some((d.handling, d.size));
        }
        (d.handling, d.size) = (Handling::Size, SizeMode::Actual);
        d.routing.applied_layout = Some((d.handling, d.size));
    } else if let Some((handling, size)) = d.routing.layout_before.take() {
        (d.handling, d.size) = (handling, size);
        d.routing.applied_layout = None;
    }
}

/// Pages of several sizes: one job per size, unless the person chose one paper or the layout puts
/// pages on other papers on purpose. Never scaled to one size without being asked.
fn apply_mixed(d: &mut PrintDraft) {
    let r = &mut d.routing;
    if !r.plan.is_mixed() {
        r.reason = None;
        r.must_choose = false;
        r.split = false;
        r.rows.clear();
        return;
    }
    r.split = !r.one_paper && r.plan.can_split() && d.handling == Handling::Size;
    let old = std::mem::take(&mut r.rows);
    if r.plan.can_split() {
        r.rows = r.plan.groups.iter().map(|group| JobRow::new(group, r.last_used_paper, &old)).collect();
    }
    r.reason = Some(format!("Mixed page sizes: {}.", r.plan.breakdown()));
    if r.split {
        r.must_choose = false;
        if !d.printer_picked {
            d.printer = None;
        }
    } else {
        r.must_choose = true;
        if !d.printer_picked {
            d.printer = None;
        }
        if !r.paper_changed {
            d.paper = r.last_used_paper;
            d.paper_before_label = None;
            r.applied_paper = Some(d.paper);
        }
    }
}

/// Mixed page sizes are going out as one job per size.
pub(crate) fn splitting(d: &PrintDraft) -> bool {
    d.routing.active && d.routing.split
}

/// The person hasn't chosen a printer and the rules can't: the dialog asks.
pub(crate) fn must_choose(d: &PrintDraft) -> bool {
    d.routing.active && d.routing.must_choose && d.printer.is_none() && !d.printer_picked
}

/// Print can't go ahead yet.
pub(crate) fn blocks_print(d: &PrintDraft) -> bool {
    must_choose(d) || (splitting(d) && d.routing.rows.iter().any(|row| row.printer.is_none()))
}

/// A job is going out with what the dialog holds.
pub(crate) fn before_send(d: &mut PrintDraft) {
    d.routing.sent = true;
    note_default_before_send(d);
}

/// Remember the default printer as a job leaves, to see later whether Windows moved it. Atlas PDF
/// names the printer in every job and never sets the default.
fn note_default_before_send(d: &mut PrintDraft) {
    d.routing.default_seen = d.printers.iter().find(|p| p.default).map(|p| p.name.clone());
}

/// The choices kept in the app's settings.
pub(crate) fn persist(routing: &PrintRouting, settings: &mut serde_json::Value) {
    if let Some(printer) = &routing.default_seen {
        settings["print_default_seen"] = serde_json::json!(printer);
    }
    if routing.note_shown {
        settings["print_default_note_shown"] = serde_json::Value::Bool(true);
    }
}

/// Read back what [`persist`] kept. Anything malformed is ignored.
pub(crate) fn restore(routing: &mut PrintRouting, settings: &serde_json::Value) {
    if let Some(printer) = settings["print_default_seen"].as_str() {
        routing.default_seen = Some(printer.to_string());
    }
    if let Some(shown) = settings["print_default_note_shown"].as_bool() {
        routing.note_shown = shown;
    }
}

/// The reason line, under the printer and paper.
pub(crate) fn panel(ui: &mut egui::Ui, d: &mut PrintDraft, t: &Tokens) {
    if !d.routing.active {
        return;
    }
    ui.add_space(4.0);
    if let Some(reason) = &d.routing.reason {
        ui.label(egui::RichText::new(reason.as_str()).strong());
    }
    if d.routing.must_choose && d.printer_picked {
        ui.label(egui::RichText::new("You chose this printer for this job.").small().color(t.text_muted));
    }
    if let Some(problem) = &d.routing.rules_problem {
        ui.label(egui::RichText::new(problem.as_str()).small().color(t.text_muted));
    }
    if let Some(note) = &d.routing.default_note {
        ui.label(egui::RichText::new(note.as_str()).small().color(t.text_muted));
    }
    mixed_panel(ui, d, t);
    if ui.small_button("Printer rules…").on_hover_text("Choose which printer each paper size goes to").clicked() {
        d.routing.open_rules = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each size previews on its own paper, at its own size: nothing is scaled to one.
    #[test]
    fn separate_jobs_keep_every_page_at_its_own_size() {
        let sizes = [(1296.0, 1728.0), (612.0, 792.0), (1728.0, 2592.0)];
        let labels: Vec<String> = ["1", "2", "3"].map(String::from).to_vec();
        let mut d = PrintDraft::default();
        d.routing.active = true;
        d.routing.rules = PrinterRules::starter();
        update(&mut d, &sizes, &labels);
        assert!(splitting(&d));
        let settings = d.settings(3, &labels).unwrap();
        let sheets = sheets_to_print(&d, &sizes, &settings).unwrap();
        assert_eq!(sheets.iter().map(|s| s.size).collect::<Vec<_>>(), sizes);
        let scale = |i: usize| sheets[i].placed[0].matrix.0[0];
        assert_eq!((scale(0), scale(2)), (1.0, 1.0), "wide-format pages at actual size");
    }

    #[test]
    fn the_rules_choice_is_not_remembered_as_the_last_paper_until_it_is_used() {
        let mut routing = PrintRouting { active: true, last_used_paper: 3, applied_paper: Some(9), ..PrintRouting::default() };
        assert_eq!(routing.paper_to_remember(9), 3, "only preselected");
        routing.sent = true;
        assert_eq!(routing.paper_to_remember(9), 9, "printed on it");
        routing.sent = false;
        routing.paper_changed = true;
        assert_eq!(routing.paper_to_remember(7), 7, "picked by hand");
        assert_eq!(PrintRouting::default().paper_to_remember(7), 7, "no rules: the upstream setting");
    }
}
