//! Atlas fork: the Print dialog's paper stays within what the selected printer takes. Moving a
//! job to a printer without its paper switches to the paper that printer uses by default, scaled
//! to fit, and says so; moving it back gives the paper back. A driver that can't say what it
//! takes changes nothing, and neither does label stock, whose drivers often list it a little short.

use std::collections::BTreeMap;

use pdfcraft_atlas::printer_papers::DriverPapers;
use pdfcraft_engine::print::{LABEL_PAPER, PAPERS, SizeMode};

use super::print_routing::PrintRouting;
use crate::PdfCraftApp;
use crate::print_ui::{Handling, PrintDraft};

/// What is known of one printer's papers.
#[derive(Clone, Debug, PartialEq)]
pub enum PaperRead {
    /// Its driver is being asked.
    Reading,
    Known(DriverPapers),
    /// Its driver couldn't say.
    Unknown,
}

/// The dialog's paper was switched because the printer doesn't take the one wanted.
#[derive(Clone, Debug, PartialEq)]
pub struct PaperSwap {
    printer: String,
    from: usize,
    to: usize,
    /// Actual size became Fit, as the page wouldn't fit the new paper.
    scaled: bool,
}

impl PaperSwap {
    /// The paper wanted, which the printer doesn't take.
    pub(super) fn from(&self) -> usize {
        self.from
    }

    /// What to tell the person.
    pub(super) fn note(&self) -> String {
        note(self.from, self.to, self.scaled)
    }
}

/// `ARCH C isn't available on this printer, so this uses US Letter, scaled to fit.`
pub(super) fn note(from: usize, to: usize, scaled: bool) -> String {
    let scaled = if scaled { ", scaled to fit" } else { "" };
    format!("{} isn't available on this printer, so this uses {}{scaled}.", short_name(from), short_name(to))
}

/// A paper's name without its dimensions.
fn short_name(paper: usize) -> &'static str {
    PAPERS.get(paper).map_or("", |(name, _)| name.split(" (").next().unwrap_or(name))
}

fn known<'a>(reads: &'a BTreeMap<String, PaperRead>, printer: Option<&str>) -> Option<&'a DriverPapers> {
    match reads.get(printer?) {
        Some(PaperRead::Known(papers)) => Some(papers),
        _ => None,
    }
}

/// The papers to offer for `printer`: those it takes, and `current` whatever it is. Every paper
/// when its driver can't say.
pub(super) fn offered_papers(reads: &BTreeMap<String, PaperRead>, printer: Option<&str>, current: usize) -> Vec<usize> {
    let papers = known(reads, printer);
    (0..PAPERS.len()).filter(|paper| *paper == current || papers.is_none_or(|p| p.takes(*paper))).collect()
}

/// The papers the Paper list offers for the dialog's printer.
pub(crate) fn offered(d: &PrintDraft) -> Vec<usize> {
    if !d.routing.active {
        return (0..PAPERS.len()).collect();
    }
    offered_papers(&d.routing.printer_papers, d.printer.as_deref(), d.paper)
}

/// Each frame of the dialog: ask about the printers selected, and keep their papers to what they take.
pub(super) fn fit(d: &mut PrintDraft) {
    ask(d);
    if d.routing.split {
        fit_rows(&mut d.routing);
    } else {
        fit_single(d);
    }
}

/// Have the selected printers' drivers read, once each.
fn ask(d: &mut PrintDraft) {
    let r = &mut d.routing;
    let selected = d.printer.iter().chain(r.rows.iter().filter_map(|row| row.printer.as_ref()));
    for printer in selected {
        if !r.printer_papers.contains_key(printer) {
            r.printer_papers.insert(printer.clone(), PaperRead::Reading);
            r.papers_to_read.push(printer.clone());
        }
    }
}

fn set_paper(d: &mut PrintDraft, paper: usize) {
    d.paper = paper;
    d.paper_before_label = None;
    d.sheet = 0;
    if !d.routing.paper_changed {
        d.routing.applied_paper = Some(paper);
    }
}

fn set_size(d: &mut PrintDraft, size: SizeMode) {
    d.size = size;
    if !d.routing.layout_changed && d.routing.applied_layout.is_some() {
        d.routing.applied_layout = Some((d.handling, size));
    }
}

fn fit_single(d: &mut PrintDraft) {
    if let Some(swap) = d.routing.paper_swap.clone() {
        let reading = d.printer.as_ref().is_some_and(|printer| d.routing.printer_papers.get(printer) == Some(&PaperRead::Reading));
        if d.paper != swap.to {
            // The person or the rules have moved on to another paper.
            d.routing.paper_swap = None;
        } else if d.printer.as_ref() == Some(&swap.printer) || reading {
            return;
        } else {
            // Another printer, or Save as PDF: the paper wanted comes back, if it can.
            set_paper(d, swap.from);
            if swap.scaled && d.handling == Handling::Size && d.size == SizeMode::Fit {
                set_size(d, SizeMode::Actual);
            }
            d.routing.paper_swap = None;
        }
    }
    let Some(papers) = known(&d.routing.printer_papers, d.printer.as_deref()) else { return };
    if d.paper == LABEL_PAPER || d.handling == Handling::Label || papers.takes(d.paper) {
        return;
    }
    let (Some(to), Some(printer)) = (papers.fallback(), d.printer.clone()) else { return };
    let scaled = d.handling == Handling::Size && d.size == SizeMode::Actual;
    d.routing.paper_swap = Some(PaperSwap { printer, from: d.paper, to, scaled });
    set_paper(d, to);
    if scaled {
        set_size(d, SizeMode::Fit);
    }
}

/// One size's job at a time: each row's printer and the paper wanted decide its paper.
fn fit_rows(r: &mut PrintRouting) {
    let reads = &r.printer_papers;
    for row in &mut r.rows {
        if !row.swapped {
            row.wanted = row.paper;
        }
        let papers = known(reads, row.printer.as_deref());
        let to = papers.filter(|p| row.wanted != LABEL_PAPER && !p.takes(row.wanted)).and_then(DriverPapers::fallback);
        row.paper = to.unwrap_or(row.wanted);
        row.swapped = to.is_some();
    }
}

impl PdfCraftApp {
    /// Take these as what the printers' drivers list, instead of asking them.
    pub fn assume_printer_papers(&mut self, papers: BTreeMap<String, DriverPapers>) {
        self.atlas.print.assumed_papers = Some(papers);
    }
}

/// What the dialog starts from when Print opens: what is known, and what it was told to assume. A
/// driver that couldn't say is asked again, in case it can now.
pub(super) fn reads_on_open(app: &mut PdfCraftApp) -> BTreeMap<String, PaperRead> {
    let mut reads = std::mem::take(&mut app.print_draft.routing.printer_papers);
    reads.retain(|_, read| *read != PaperRead::Unknown);
    if let Some(assumed) = &app.atlas.print.assumed_papers {
        reads.extend(assumed.iter().map(|(printer, papers)| (printer.clone(), PaperRead::Known(papers.clone()))));
    }
    reads
}

/// Start the reads the dialog asked for and pick up those that finished (each frame). Only the
/// desktop app's rules file means real printers: otherwise nothing can say.
pub(crate) fn poll_reads(app: &mut PdfCraftApp) {
    let wanted = std::mem::take(&mut app.print_draft.routing.papers_to_read);
    for printer in wanted {
        start_read(app, printer);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut arrived = Vec::new();
        app.atlas.print.paper_reads.retain(|(printer, rx)| match rx.try_recv() {
            Ok(papers) => {
                arrived.push((printer.clone(), papers));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            // A read that died says nothing.
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                arrived.push((printer.clone(), None));
                false
            }
        });
        for (printer, papers) in arrived {
            app.print_draft.routing.printer_papers.insert(printer, papers.map_or(PaperRead::Unknown, PaperRead::Known));
        }
    }
}

/// Ask a driver off the GUI thread: a network printer can take seconds to answer.
#[cfg(not(target_arch = "wasm32"))]
fn start_read(app: &mut PdfCraftApp, printer: String) {
    let real = matches!(app.atlas.print.source, Some(super::print_routing::RulesSource::File(_)));
    let (tx, rx) = std::sync::mpsc::channel();
    let wake = crate::print_ui::WakeOnExit(app.ctx.clone());
    let name = printer.clone();
    let spawned = real.then(|| {
        std::thread::Builder::new().name("printer papers".into()).spawn(move || {
            let _wake = wake;
            let tx = tx;
            // The receiver may be gone (the app quit): nothing to report to then.
            let _ = tx.send(pdfcraft_atlas::printer_papers::read(&name));
        })
    });
    match spawned {
        Some(Ok(_)) => app.atlas.print.paper_reads.push((printer, rx)),
        // Nothing to ask, or no thread to ask on.
        _ => {
            app.print_draft.routing.printer_papers.insert(printer, PaperRead::Unknown);
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn start_read(app: &mut PdfCraftApp, printer: String) {
    app.print_draft.routing.printer_papers.insert(printer, PaperRead::Unknown);
}

#[cfg(test)]
mod tests {
    use pdfcraft_atlas::print_rules::{PrinterRules, paper_named};
    use pdfcraft_engine::print::spool;

    use super::super::print_routing::{self, update};
    use super::*;

    const HP: &str = "HP LaserJet M607 [F35101]";
    const CANON_C: &str = "Canon TX-3200 18x24";
    const CANON_D: &str = "Canon TX-3200";
    const OTHER: &str = "Printer whose driver can't say";

    const LETTER: (u32, u32) = (215_900, 279_400);
    const LEGAL: (u32, u32) = (215_900, 355_600);
    const ARCH_C: (u32, u32) = (457_200, 609_600);
    const ARCH_D: (u32, u32) = (609_600, 914_400);
    const TABLOID: (u32, u32) = (279_400, 431_800);

    const PAGE_C: (f64, f64) = (1296.0, 1728.0);
    const PAGE_D: (f64, f64) = (1728.0, 2592.0);
    const PAGE_LETTER: (f64, f64) = (612.0, 792.0);
    const PAGE_TABLOID: (f64, f64) = (792.0, 1224.0);

    fn paper(name: &str) -> usize {
        paper_named(name).unwrap()
    }

    fn known_papers(sizes: &[(u32, u32)], default: (u32, u32)) -> PaperRead {
        PaperRead::Known(DriverPapers::new(sizes.iter().copied(), Some(default)).unwrap())
    }

    /// The dialog with the starter rules and three printers whose drivers have answered.
    fn draft() -> PrintDraft {
        let mut d = PrintDraft::default();
        d.routing.active = true;
        d.routing.rules = PrinterRules::starter();
        d.printers_loading = false;
        d.printers = [HP, CANON_C, CANON_D, OTHER]
            .map(|name| spool::Printer { name: name.to_string(), description: String::new(), default: name == HP, two_sided: None })
            .to_vec();
        let reads = &mut d.routing.printer_papers;
        reads.insert(HP.to_string(), known_papers(&[LETTER, LEGAL], LETTER));
        let drawing_sizes = [LETTER, LEGAL, TABLOID, ARCH_C, ARCH_D];
        reads.insert(CANON_C.to_string(), known_papers(&drawing_sizes, ARCH_C));
        reads.insert(CANON_D.to_string(), known_papers(&drawing_sizes, ARCH_D));
        reads.insert(OTHER.to_string(), PaperRead::Unknown);
        d
    }

    fn labels(n: usize) -> Vec<String> {
        (1..=n).map(|page| page.to_string()).collect()
    }

    fn frame(d: &mut PrintDraft, sizes: &[(f64, f64)]) {
        update(d, sizes, &labels(sizes.len()));
    }

    fn pick(d: &mut PrintDraft, sizes: &[(f64, f64)], printer: Option<&str>) {
        d.printer = printer.map(str::to_string);
        d.printer_picked = true;
        frame(d, sizes);
    }

    /// The case this exists for: an 18 x 24 drawing, sent to the office printer for a small copy.
    #[test]
    fn a_printer_without_the_paper_gets_its_default_paper_scaled_to_fit() {
        let sizes = [PAGE_C];
        let mut d = draft();
        frame(&mut d, &sizes);
        assert_eq!((d.printer.as_deref(), d.paper, d.size), (Some(CANON_C), paper("ARCH C"), SizeMode::Actual));
        pick(&mut d, &sizes, Some(HP));
        assert_eq!((d.paper, d.size), (paper("US Letter"), SizeMode::Fit));
        assert_eq!(d.routing.paper_swap.as_ref().unwrap().note(), "ARCH C isn't available on this printer, so this uses US Letter, scaled to fit.");
        let settings = d.settings(1, &labels(1)).unwrap();
        assert_eq!(settings.paper, PAPERS[paper("US Letter")].1, "the job is laid out on Letter");
        let sheets = print_routing::sheets_to_print(&d, &sizes, &settings).unwrap();
        assert!(sheets[0].placed[0].matrix.0[..4].iter().map(|v| v.abs()).fold(0.0, f64::max) < 1.0, "the page is scaled down, not cut");
    }

    #[test]
    fn the_paper_wanted_comes_back_with_a_printer_that_takes_it() {
        let sizes = [PAGE_C];
        let mut d = draft();
        frame(&mut d, &sizes);
        pick(&mut d, &sizes, Some(HP));
        pick(&mut d, &sizes, Some(CANON_C));
        assert_eq!((d.paper, d.size), (paper("ARCH C"), SizeMode::Actual));
        assert_eq!(d.routing.paper_swap, None);
        assert_eq!(d.routing.conflict, None);
    }

    #[test]
    fn save_as_pdf_gets_the_paper_wanted_back() {
        let sizes = [PAGE_C];
        let mut d = draft();
        frame(&mut d, &sizes);
        pick(&mut d, &sizes, Some(HP));
        pick(&mut d, &sizes, None);
        assert_eq!((d.paper, d.size), (paper("ARCH C"), SizeMode::Actual));
    }

    #[test]
    fn a_driver_that_cant_say_changes_nothing() {
        let sizes = [PAGE_C];
        let mut d = draft();
        frame(&mut d, &sizes);
        pick(&mut d, &sizes, Some(OTHER));
        assert_eq!((d.paper, d.size), (paper("ARCH C"), SizeMode::Actual));
        assert_eq!(offered(&d).len(), PAPERS.len());
    }

    #[test]
    fn nothing_changes_until_the_driver_answers() {
        let sizes = [PAGE_C];
        let mut d = draft();
        d.routing.printer_papers.remove(HP);
        frame(&mut d, &sizes);
        pick(&mut d, &sizes, Some(HP));
        assert_eq!(d.routing.papers_to_read, [HP.to_string()], "asked once");
        assert_eq!(d.paper, paper("ARCH C"));
        frame(&mut d, &sizes);
        assert_eq!(d.routing.papers_to_read, [HP.to_string()], "not asked again");
        d.routing.printer_papers.insert(HP.to_string(), known_papers(&[LETTER], LETTER));
        frame(&mut d, &sizes);
        assert_eq!(d.paper, paper("US Letter"));
    }

    /// Going back through a printer still being read waits for its answer, then a driver that
    /// can't say gets the paper wanted back rather than keeping the other printer's.
    #[test]
    fn leaving_for_a_printer_being_read_waits_for_its_answer() {
        let sizes = [PAGE_C];
        let mut d = draft();
        frame(&mut d, &sizes);
        pick(&mut d, &sizes, Some(HP));
        d.routing.printer_papers.insert(OTHER.to_string(), PaperRead::Reading);
        pick(&mut d, &sizes, Some(OTHER));
        assert_eq!(d.paper, paper("US Letter"));
        d.routing.printer_papers.insert(OTHER.to_string(), PaperRead::Unknown);
        frame(&mut d, &sizes);
        assert_eq!((d.paper, d.size), (paper("ARCH C"), SizeMode::Actual));
    }

    #[test]
    fn one_printer_without_the_paper_to_another_keeps_the_paper_wanted() {
        let sizes = [PAGE_D];
        let mut d = draft();
        d.routing.printer_papers.insert(OTHER.to_string(), known_papers(&[LEGAL], LEGAL));
        frame(&mut d, &sizes);
        pick(&mut d, &sizes, Some(HP));
        pick(&mut d, &sizes, Some(OTHER));
        assert_eq!(d.paper, paper("US Legal"), "this printer's default");
        pick(&mut d, &sizes, Some(CANON_D));
        assert_eq!((d.paper, d.size), (paper("ARCH D"), SizeMode::Actual));
    }

    #[test]
    fn the_paper_list_offers_what_the_printer_takes_and_what_is_selected() {
        let sizes = [PAGE_C];
        let mut d = draft();
        frame(&mut d, &sizes);
        pick(&mut d, &sizes, Some(HP));
        assert_eq!(offered(&d), [paper("US Letter"), paper("US Legal")]);
        // Label stock is left alone, so it stays selectable.
        d.routing.paper_swap = None;
        d.paper = LABEL_PAPER;
        assert!(offered(&d).contains(&LABEL_PAPER));
    }

    /// Thermal drivers list 100 x 150 mm, a little short of the 4 x 6 sheet, and print it anyway.
    #[test]
    fn label_stock_is_left_alone() {
        let sizes = [PAGE_C];
        let mut d = draft();
        frame(&mut d, &sizes);
        pick(&mut d, &sizes, Some(HP));
        d.set_handling(Handling::Label);
        frame(&mut d, &sizes);
        assert_eq!((d.paper, d.handling), (LABEL_PAPER, Handling::Label));
    }

    #[test]
    fn a_size_chosen_after_the_switch_stands() {
        let sizes = [PAGE_C];
        let mut d = draft();
        frame(&mut d, &sizes);
        pick(&mut d, &sizes, Some(HP));
        d.size = SizeMode::Actual;
        frame(&mut d, &sizes);
        frame(&mut d, &sizes);
        assert_eq!((d.paper, d.size), (paper("US Letter"), SizeMode::Actual));
    }

    /// A paper shown only because the driver lacks the one wanted isn't "the last paper used".
    #[test]
    fn a_switched_paper_is_not_remembered_until_a_job_goes_out_on_it() {
        let sizes = [PAGE_C];
        let mut d = draft();
        d.routing.last_used_paper = paper("ARCH D");
        frame(&mut d, &sizes);
        pick(&mut d, &sizes, Some(HP));
        assert_eq!(d.routing.paper_to_remember(d.paper), paper("ARCH D"));
        assert!(!d.routing.paper_changed);
        print_routing::before_send(&mut d);
        assert_eq!(d.routing.paper_to_remember(d.paper), paper("US Letter"));
    }

    #[test]
    fn choosing_other_pages_chooses_the_paper_again() {
        let sizes = [PAGE_C, PAGE_LETTER];
        let mut d = draft();
        d.which = crate::print_ui::Which::Range;
        d.range = "1".to_string();
        frame(&mut d, &sizes);
        pick(&mut d, &sizes, Some(HP));
        assert!(d.routing.paper_swap.is_some());
        d.range = "2".to_string();
        frame(&mut d, &sizes);
        assert_eq!(d.paper, paper("US Letter"));
        assert_eq!(d.routing.paper_swap, None, "Letter pages on Letter: nothing to explain");
    }

    /// The starter rules send Tabloid to the office printer, which here doesn't take it: the fix
    /// button would only put back a paper that is switched away again.
    #[test]
    fn a_rule_for_a_paper_its_printer_lacks_offers_no_fix_that_cant_work() {
        let sizes = [PAGE_TABLOID];
        let mut d = draft();
        frame(&mut d, &sizes);
        assert_eq!((d.printer.as_deref(), d.paper), (Some(HP), paper("US Letter")));
        assert_eq!(d.routing.conflict, None);
    }

    /// The warning about the rules still stands when the person sends a drawing somewhere else.
    #[test]
    fn going_against_the_rules_still_warns_with_the_rules_paper() {
        let sizes = [PAGE_C];
        let mut d = draft();
        frame(&mut d, &sizes);
        pick(&mut d, &sizes, Some(HP));
        let conflict = d.routing.conflict.clone().unwrap();
        assert_eq!(conflict.fix, Some((CANON_C.to_string(), paper("ARCH C"))));
    }

    #[test]
    fn the_fix_for_going_against_the_rules_ends_the_switch() {
        let sizes = [PAGE_C];
        let mut d = draft();
        frame(&mut d, &sizes);
        pick(&mut d, &sizes, Some(HP));
        print_routing::apply_fix(&mut d, CANON_C.to_string(), paper("ARCH C"));
        frame(&mut d, &sizes);
        assert_eq!((d.printer.as_deref(), d.paper, d.size), (Some(CANON_C), paper("ARCH C"), SizeMode::Actual));
        assert_eq!((d.routing.paper_swap.clone(), d.routing.conflict.clone()), (None, None));
    }

    #[test]
    fn a_size_going_out_alone_keeps_to_its_printers_papers_too() {
        let sizes = [PAGE_C, PAGE_LETTER];
        let mut d = draft();
        frame(&mut d, &sizes);
        assert!(print_routing::splitting(&d));
        d.routing.rows[0].printer = Some(HP.to_string());
        frame(&mut d, &sizes);
        let row = &d.routing.rows[0];
        assert_eq!((row.paper, row.swapped), (paper("US Letter"), true));
        assert_eq!(row.conflict.as_ref().unwrap().fix, Some((CANON_C.to_string(), paper("ARCH C"))));
        let settings = d.settings(2, &labels(2)).unwrap();
        let sheets = print_routing::sheets_to_print(&d, &sizes, &settings).unwrap();
        assert_eq!(sheets[0].size, (612.0, 792.0));
        assert!(sheets[0].placed[0].matrix.0[..4].iter().map(|v| v.abs()).fold(0.0, f64::max) < 1.0, "scaled, not cut");
        d.routing.rows[0].printer = Some(CANON_C.to_string());
        frame(&mut d, &sizes);
        let row = &d.routing.rows[0];
        assert_eq!((row.paper, row.swapped), (paper("ARCH C"), false));
    }

    #[test]
    fn a_row_with_a_rule_for_a_paper_its_printer_lacks_offers_no_fix_that_cant_work() {
        let sizes = [PAGE_TABLOID, PAGE_C];
        let mut d = draft();
        frame(&mut d, &sizes);
        let row = d.routing.rows.iter().find(|row| row.printer.as_deref() == Some(HP)).unwrap();
        assert_eq!((row.paper, row.swapped), (paper("US Letter"), true));
        assert_eq!(row.conflict, None);
    }

    #[test]
    fn each_selected_printer_is_asked_about_once() {
        let sizes = [PAGE_C, PAGE_LETTER];
        let mut d = draft();
        d.routing.printer_papers.clear();
        frame(&mut d, &sizes);
        frame(&mut d, &sizes);
        assert_eq!(d.routing.papers_to_read, [CANON_C.to_string(), HP.to_string()]);
    }
}
