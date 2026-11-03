//! Atlas fork: the Print dialog following the printer rules, in the real shell. Printers here are
//! names in a list: no job reaches a spooler, and `packaging/windows/test-print-hold.ps1` covers
//! the real drivers through the real window.

use std::sync::Arc;

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_atlas::print_rules::{PrinterRule, PrinterRules, paper_named};
use pdfcraft_engine::print::{SizeMode, spool};
use pdfcraft_ui_egui::{Dialog, PdfCraftApp, PrintHandling};

const CANON_C: &str = "Canon TX-3200 18x24";
const CANON_D: &str = "Canon TX-3200";
const OFFICE: &str = "HP LaserJet M607 [F35101]";

const ARCH_C: (u32, u32) = (1296, 1728);
const ARCH_D: (u32, u32) = (1728, 2592);
const LETTER: (u32, u32) = (612, 792);

/// A document of these page sizes.
fn drawing(pages: &[(u32, u32)]) -> Vec<u8> {
    let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", i + 3)).collect();
    let mut objects =
        vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(), format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), pages.len())];
    for (w, h) in pages {
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] >>"));
    }
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

fn printer(name: &str, default: bool) -> spool::Printer {
    spool::Printer { name: name.to_string(), default, two_sided: None, description: String::new() }
}

/// The app with the starter rules, these printers installed and this document open.
fn harness_with(pages: &[(u32, u32)], installed: &'static [&'static str], settings: Option<&str>) -> Harness<'static, PdfCraftApp> {
    let bytes = drawing(pages);
    let settings = settings.map(str::to_string);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        if let Some(json) = &settings {
            app.restore(json);
        }
        app.use_print_rules(PrinterRules::starter());
        app.printer_source = Some(Arc::new(move || installed.iter().map(|name| printer(name, *name == OFFICE)).collect()));
        app.open_bytes("drawing.pdf", None, bytes.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h
}

const ALL: &[&str] = &[OFFICE, CANON_D, CANON_C];

fn harness(pages: &[(u32, u32)]) -> Harness<'static, PdfCraftApp> {
    harness_with(pages, ALL, None)
}

/// Open Print and wait for the printer list, which arrives on a worker.
fn open_print(h: &mut Harness<'static, PdfCraftApp>) {
    assert!(h.state_mut().execute("print.dialog"));
    for _ in 0..200 {
        h.run_steps(1);
        if !h.state().print_draft.printers_loading {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(3);
    assert!(!h.state().print_draft.printers_loading, "the printer list arrived");
}

fn paper(name: &str) -> usize {
    paper_named(name).unwrap()
}

fn is_print_enabled(h: &Harness<'static, PdfCraftApp>) -> bool {
    !h.get_by_role_and_label(Role::Button, "Print").accesskit_node().is_disabled()
}

#[test]
fn the_dialog_preselects_the_printer_paper_and_actual_size_and_says_why() {
    for (page, want_paper, want_printer, reason) in [
        (ARCH_C, "ARCH C", CANON_C, "18 x 24 detected: Canon TX-3200 18x24"),
        ((1728, 1296), "ARCH C", CANON_C, "18 x 24 detected: Canon TX-3200 18x24"),
        (ARCH_D, "ARCH D", CANON_D, "24 x 36 detected: Canon TX-3200"),
    ] {
        let mut h = harness(&[page]);
        open_print(&mut h);
        let d = &h.state().print_draft;
        assert_eq!(d.printer.as_deref(), Some(want_printer), "{page:?}");
        assert_eq!(d.paper, paper(want_paper), "{page:?}");
        assert_eq!((d.handling, d.size), (PrintHandling::Size, SizeMode::Actual), "{page:?}");
        h.get_by_label(reason);
        assert!(is_print_enabled(&h));
    }
}

#[test]
fn office_sizes_go_to_the_office_printer_and_keep_their_layout() {
    let mut h = harness(&[LETTER]);
    open_print(&mut h);
    let d = &h.state().print_draft;
    assert_eq!(d.printer.as_deref(), Some(OFFICE));
    assert_eq!(d.paper, paper("Letter"));
    assert_eq!(d.size, SizeMode::Fit, "only wide-format drawings default to actual size");
    h.get_by_label("Letter detected: HP LaserJet M607 [F35101]");
}

/// The reported bug: the paper saved from the last print stood in for the size in the file.
#[test]
fn the_last_used_paper_never_overrides_the_detected_size() {
    let mut h = harness_with(&[ARCH_C], ALL, Some(r#"{"print_paper":"US Letter"}"#));
    open_print(&mut h);
    assert_eq!(h.state().print_draft.paper, paper("ARCH C"));
    let mut h = harness_with(&[ARCH_D], ALL, Some(r#"{"print_paper":"ARCH C (18 x 24 in)"}"#));
    open_print(&mut h);
    assert_eq!(h.state().print_draft.paper, paper("ARCH D"));
}

#[test]
fn what_the_rules_preselected_is_not_remembered_as_the_last_paper() {
    let mut h = harness_with(&[ARCH_C], ALL, Some(r#"{"print_paper":"A4"}"#));
    open_print(&mut h);
    assert_eq!(h.state().print_draft.paper, paper("ARCH C"));
    let saved: serde_json::Value = serde_json::from_str(&h.state().persist()).unwrap();
    assert_eq!(saved["print_paper"], "A4", "nothing was printed on ARCH C");
}

/// A size no rule covers: its own paper when it has one, the printer asked for.
#[test]
fn a_standard_size_with_no_rule_asks_for_a_printer_and_keeps_its_paper() {
    let mut h = harness_with(&[(1584, 2448)], ALL, Some(r#"{"print_paper":"US Letter"}"#));
    open_print(&mut h);
    let d = &h.state().print_draft;
    assert_eq!(d.printer, None, "not the default printer, not the last one");
    assert_eq!(d.paper, paper("ANSI D"));
    h.get_by_label("22 x 34 detected, and no printer rule covers it. Choose a printer.");
    h.get_by_value("Choose a printer…");
    assert!(!is_print_enabled(&h));
    assert!(!h.state_mut().print_now(), "print_now refuses too");
}

/// A size that is no paper at all takes the last paper, and says so.
#[test]
fn a_size_no_paper_matches_uses_the_last_paper_and_says_so() {
    let mut h = harness_with(&[(1440, 2160)], ALL, Some(r#"{"print_paper":"ARCH D (24 x 36 in)"}"#));
    open_print(&mut h);
    let d = &h.state().print_draft;
    assert_eq!((d.printer.as_deref(), d.paper), (None, paper("ARCH D")));
    h.get_by_label("20 x 30 in detected. This isn't a standard paper, and no printer rule covers it. Choose a printer. The paper shown is the last one used: ARCH D (24 x 36 in).");
}

#[test]
fn a_rule_for_a_missing_printer_asks_instead_of_falling_back() {
    let mut h = harness_with(&[ARCH_C], &[OFFICE, CANON_D], None);
    open_print(&mut h);
    let d = &h.state().print_draft;
    assert_eq!(d.printer, None, "neither the default printer nor the other roll's");
    assert_eq!(d.paper, paper("ARCH C"));
    h.get_by_label("18 x 24 detected, but the printer rule names Canon TX-3200 18x24, which isn't installed. Choose a printer.");
    assert!(!is_print_enabled(&h));

    // Choosing one lets the job go: the choice stands for this job, and no warning is due.
    h.get_by_value("Choose a printer…").click();
    h.run_steps(2);
    h.get_by_label(CANON_D).click();
    h.run_steps(3);
    assert_eq!(h.state().print_draft.printer.as_deref(), Some(CANON_D));
    assert!(is_print_enabled(&h));
}

#[test]
fn a_changed_selection_of_pages_chooses_again() {
    let mut h = harness(&[ARCH_C, LETTER]);
    open_print(&mut h);
    let draft = &mut h.state_mut().print_draft;
    draft.which = pdfcraft_ui_egui::PrintWhich::Range;
    draft.range = "2".into();
    h.run_steps(3);
    let d = &h.state().print_draft;
    assert_eq!((d.printer.as_deref(), d.paper), (Some(OFFICE), paper("Letter")));
    let draft = &mut h.state_mut().print_draft;
    draft.range = "1".into();
    h.run_steps(3);
    let d = &h.state().print_draft;
    assert_eq!((d.printer.as_deref(), d.paper, d.size), (Some(CANON_C), paper("ARCH C"), SizeMode::Actual));
    // A range being typed that isn't valid keeps what was chosen.
    h.state_mut().print_draft.range = "1-".into();
    h.state_mut().print_draft.range = "99".into();
    h.run_steps(3);
    assert_eq!(h.state().print_draft.printer.as_deref(), Some(CANON_C));
}

/// The person can override both for the job; the next print starts from the rules again.
#[test]
fn a_manual_choice_stands_for_this_job_only() {
    let mut h = harness(&[ARCH_C]);
    open_print(&mut h);
    h.get_by_value(CANON_C).click();
    h.run_steps(2);
    h.get_by_label("HP LaserJet M607 [F35101] (default)").click();
    h.run_steps(5);
    h.get_by_value("ARCH C (18 x 24 in)").click();
    h.run_steps(2);
    h.get_by_label("Tabloid").click();
    h.run_steps(3);
    let d = &h.state().print_draft;
    assert_eq!((d.printer.as_deref(), d.paper), (Some(OFFICE), paper("Tabloid")));
    h.run_steps(3);
    let d = &h.state().print_draft;
    assert_eq!((d.printer.as_deref(), d.paper), (Some(OFFICE), paper("Tabloid")), "stays through further frames");

    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, None);
    open_print(&mut h);
    let d = &h.state().print_draft;
    assert_eq!((d.printer.as_deref(), d.paper), (Some(CANON_C), paper("ARCH C")));
}

#[test]
fn the_size_chosen_for_a_drawing_doesnt_carry_over_to_an_office_page() {
    let mut h = harness(&[ARCH_C]);
    open_print(&mut h);
    assert_eq!(h.state().print_draft.size, SizeMode::Actual);
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    h.state_mut().open_bytes("memo.pdf", None, drawing(&[LETTER])).unwrap();
    h.run_steps(3);
    open_print(&mut h);
    assert_eq!(h.state().print_draft.size, SizeMode::Fit);
}

#[test]
fn without_rules_the_dialog_is_upstreams() {
    let bytes = drawing(&[ARCH_C]);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.printer_source = Some(Arc::new(|| vec![printer(OFFICE, true), printer(CANON_C, false)]));
        app.open_bytes("drawing.pdf", None, bytes.clone()).unwrap();
        app
    });
    h.run_steps(4);
    open_print(&mut h);
    let d = &h.state().print_draft;
    assert_eq!((d.printer.as_deref(), d.paper), (Some(OFFICE), 0));
    assert!(!d.routing.active);
    assert_eq!(h.state().dialog, Some(Dialog::Print));
}

/// Rules are data: another office, other printers.
#[test]
fn the_rules_decide_not_the_printer_names_in_the_code() {
    let bytes = drawing(&[ARCH_C]);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.use_print_rules(PrinterRules { rules: vec![PrinterRule { paper: "ARCH C".into(), printer: "Plotter 7".into() }] });
        app.printer_source = Some(Arc::new(|| vec![printer("Plotter 7", false), printer(OFFICE, true)]));
        app.open_bytes("drawing.pdf", None, bytes.clone()).unwrap();
        app
    });
    h.run_steps(4);
    open_print(&mut h);
    assert_eq!(h.state().print_draft.printer.as_deref(), Some("Plotter 7"));
}

/// Pick a printer and paper from the dialog's menus, as a person does.
fn pick(h: &mut Harness<'static, PdfCraftApp>, shown_printer: &str, printer: &str, shown_paper: &str, paper: &str) {
    h.get_by_value(shown_printer).click();
    h.run_steps(2);
    h.get_by_label(printer).click();
    h.run_steps(5);
    h.get_by_value(shown_paper).click();
    h.run_steps(2);
    h.get_by_label(paper).click();
    h.run_steps(3);
}

/// The reported waste: ARCH C sent to the printer set up for the 36 inch roll went without a word.
#[test]
fn the_wrong_printer_warns_before_sending_and_one_click_fixes_it() {
    let mut h = harness(&[ARCH_C]);
    open_print(&mut h);
    assert!(h.query_by_label("Print anyway").is_none(), "nothing to warn about yet");
    h.get_by_value(CANON_C).click();
    h.run_steps(2);
    h.get_by_label(CANON_D).click();
    h.run_steps(3);
    h.get_by_label(
        "These pages are 18 x 24, which the printer rules send to Canon TX-3200 18x24. You have selected Canon TX-3200, ARCH C (18 x 24 in).",
    );
    h.get_by_role_and_label(Role::Button, "Print anyway");

    h.get_by_role_and_label(Role::Button, "Use Canon TX-3200 18x24, ARCH C (18 x 24 in)").click();
    h.run_steps(3);
    let d = &h.state().print_draft;
    assert_eq!((d.printer.as_deref(), d.paper), (Some(CANON_C), paper("ARCH C")));
    assert!(h.query_by_label("Print anyway").is_none());
    h.get_by_role_and_label(Role::Button, "Print");
}

/// The reported bug, as an override: Letter on the 24 inch roll.
#[test]
fn the_wrong_paper_warns_and_the_fix_restores_the_size_and_actual_size() {
    let mut h = harness(&[ARCH_C]);
    open_print(&mut h);
    h.get_by_value("ARCH C (18 x 24 in)").click();
    h.run_steps(2);
    h.get_by_label("US Letter").click();
    h.run_steps(3);
    h.get_by_role_and_label(Role::Button, "Print anyway");
    h.get_by_role_and_label(Role::Button, "Use Canon TX-3200 18x24, ARCH C (18 x 24 in)").click();
    h.run_steps(3);
    assert_eq!(h.state().print_draft.paper, paper("ARCH C"));
    assert!(h.query_by_label("Print anyway").is_none());
}

/// Print anyway honours the override: the warning doesn't block the person's choice.
#[test]
fn printing_past_a_warning_sends_what_was_chosen() {
    let mut h = harness(&[ARCH_C]);
    open_print(&mut h);
    // Near the top of the Paper menu: a click can't reach an item scrolled out of the menu's view.
    pick(&mut h, CANON_C, CANON_D, "ARCH C (18 x 24 in)", "US Legal");
    h.get_by_role_and_label(Role::Button, "Print anyway");
    let d = &h.state().print_draft;
    assert_eq!((d.printer.as_deref(), d.paper), (Some(CANON_D), paper("Legal")));
}

#[test]
fn save_as_pdf_goes_against_no_rule() {
    let mut h = harness(&[ARCH_C]);
    open_print(&mut h);
    h.get_by_value(CANON_C).click();
    h.run_steps(2);
    h.get_by_label("Save as PDF").click();
    h.run_steps(3);
    assert!(h.state().print_draft.routing.conflict.is_none());
    assert!(h.query_by_label("Print anyway").is_none());
}

/// A choice the person was asked to make carries no warning.
#[test]
fn choosing_for_a_size_with_no_rule_is_not_a_conflict() {
    let mut h = harness(&[(1584, 2448)]);
    open_print(&mut h);
    h.get_by_value("Choose a printer…").click();
    h.run_steps(2);
    h.get_by_label(CANON_D).click();
    h.run_steps(3);
    assert!(h.state().print_draft.routing.conflict.is_none());
    h.get_by_role_and_label(Role::Button, "Print");
}
