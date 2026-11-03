//! Atlas fork: printing by the printer rules through the tools, with synthetic drawings. No job
//! reaches a real printer: every printer name here doesn't exist, so the furthest a job gets is
//! the spooler saying so. `packaging/windows/test-print-hold.ps1` covers real drivers.

use pdfcraft_automation::{Automation, ToolError};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const C_PRINTER: &str = "No_Such_Queue_Atlas_C";
const D_PRINTER: &str = "No_Such_Queue_Atlas_D";
const OFFICE: &str = "No_Such_Queue_Atlas_Office";

/// `(width, height, extra page keys)` per page.
fn drawing(pages: &[(u32, u32, &str)]) -> Vec<u8> {
    let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", i + 3)).collect();
    let mut objects =
        vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(), format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), pages.len())];
    for (w, h, extra) in pages {
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] {extra} >>"));
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

fn workdir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-atlas-print-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn rules() -> Value {
    json!([
        { "paper": "ARCH C", "printer": C_PRINTER },
        { "paper": "ARCH D", "printer": D_PRINTER },
        { "paper": "Letter", "printer": OFFICE },
    ])
}

fn automation(dir: &Path) -> Automation {
    Automation::new().with_root(dir).unwrap().with_installed_printers([C_PRINTER, D_PRINTER, OFFICE].map(String::from).to_vec())
}

fn call(a: &mut Automation, tool: &str, args: Value) -> Result<Value, ToolError> {
    a.call(tool, &args).map(|content| match content.into_iter().next() {
        Some(pdfcraft_automation::Content::Json(v)) => v,
        other => panic!("a JSON result, not {other:?}"),
    })
}

fn open(a: &mut Automation, dir: &Path, name: &str, pages: &[(u32, u32, &str)]) -> u64 {
    std::fs::write(dir.join(name), drawing(pages)).unwrap();
    call(a, "doc_open", json!({ "path": name })).unwrap()["doc"].as_u64().unwrap()
}

fn invalid(result: Result<Value, ToolError>) -> String {
    match result {
        Err(ToolError::InvalidArgs(message)) => message,
        other => panic!("an invalid-arguments error, not {other:?}"),
    }
}

/// The size comes from the page as displayed: the CropBox, /Rotate, either way round.
#[test]
fn the_plan_reads_each_page_as_displayed() {
    let dir = workdir("displayed");
    let mut a = automation(&dir);
    for (name, page, want) in [
        ("portrait.pdf", (1296, 1728, ""), "18 x 24"),
        ("landscape.pdf", (1728, 1296, ""), "18 x 24"),
        ("rotated.pdf", (1296, 1728, "/Rotate 90"), "18 x 24"),
        // A larger media box cropped down to 18 x 24: the crop is the page.
        ("cropped.pdf", (3000, 3000, "/CropBox [100 100 1396 1828]"), "18 x 24"),
        ("cropped-rotated.pdf", (3000, 3000, "/CropBox [100 100 1828 1396] /Rotate 270"), "18 x 24"),
        ("media.pdf", (1728, 2592, ""), "24 x 36"),
        ("letter.pdf", (612, 792, ""), "Letter"),
    ] {
        let doc = open(&mut a, &dir, name, &[page]);
        let plan = call(&mut a, "print_plan", json!({ "doc": doc, "rules": rules() })).unwrap();
        assert_eq!(plan["groups"][0]["size"], want, "{name}: {plan}");
        assert_eq!(plan["mixed"], false, "{name}");
    }
}

#[test]
fn the_plan_gives_the_printer_the_layout_and_the_reason() {
    let dir = workdir("plan");
    let mut a = automation(&dir);
    let doc = open(&mut a, &dir, "a.pdf", &[(1296, 1728, "")]);
    let plan = call(&mut a, "print_plan", json!({ "doc": doc, "rules": rules() })).unwrap();
    let group = &plan["groups"][0];
    assert_eq!(group["printer"], C_PRINTER);
    assert_eq!(group["route"], "ready");
    assert_eq!(group["paper"], "ARCH C (18 x 24 in)");
    assert_eq!(group["layout"], "actual");
    assert_eq!(group["reason"], format!("18 x 24 detected: {C_PRINTER}"));
    assert_eq!(group["page_numbers"], json!([1]));

    let doc = open(&mut a, &dir, "b.pdf", &[(612, 792, "")]);
    let plan = call(&mut a, "print_plan", json!({ "doc": doc, "rules": rules() })).unwrap();
    assert_eq!(plan["groups"][0]["layout"], "fit", "only wide-format drawings default to actual size");
}

/// `paper: auto` lays the sheets out on the size read from the file, whatever the default paper.
#[test]
fn paper_auto_lays_out_on_the_detected_size_at_actual_size() {
    let dir = workdir("paper");
    let mut a = automation(&dir);
    let doc = open(&mut a, &dir, "a.pdf", &[(1728, 1296, "")]);
    let out = call(&mut a, "doc_print", json!({ "doc": doc, "paper": "auto", "path": "sheets.pdf", "rules": rules() })).unwrap();
    assert_eq!(out["paper"], "ARCH C (18 x 24 in)");
    assert_eq!(out["layout"], "actual");
    assert_eq!(out["reason"], "18 x 24 detected");
    assert_eq!(out["printer"], Value::Null);
    assert_eq!(out["jobs"].as_array().map(Vec::len), Some(1));

    let sheets = call(&mut a, "doc_open", json!({ "path": "sheets.pdf" })).unwrap()["doc"].as_u64().unwrap();
    let plan = call(&mut a, "print_plan", json!({ "doc": sheets, "rules": rules() })).unwrap();
    assert_eq!(plan["groups"][0]["size"], "18 x 24", "the print-ready sheet is the drawing's own size: {plan}");
}

/// A job chosen by the rules reaches the spooler for that printer. The printer doesn't exist, so
/// the spooler's refusal naming it is the proof, and no paper is at risk.
#[test]
fn printer_auto_sends_to_the_printer_the_rules_give() {
    let dir = workdir("printer");
    let mut a = automation(&dir);
    let doc = open(&mut a, &dir, "a.pdf", &[(1728, 2592, "")]);
    let error = call(&mut a, "doc_print", json!({ "doc": doc, "paper": "auto", "printer": "auto", "rules": rules() })).unwrap_err();
    assert!(error.to_string().contains(D_PRINTER), "{error}");
}

#[test]
fn no_rule_and_a_missing_printer_ask_instead_of_falling_back() {
    let dir = workdir("ask");
    let mut a = automation(&dir);
    // ANSI D: a standard size no rule covers.
    let doc = open(&mut a, &dir, "a.pdf", &[(1584, 2448, "")]);
    let message = invalid(call(&mut a, "doc_print", json!({ "doc": doc, "paper": "auto", "printer": "auto", "rules": rules() })));
    assert!(message.contains("22 x 34 detected, and no printer rule covers it") && message.contains("Pass printer"), "{message}");

    // No standard size at all: nothing to lay it out on either.
    let doc = open(&mut a, &dir, "b.pdf", &[(1440, 2160, "")]);
    let message = invalid(call(&mut a, "doc_print", json!({ "doc": doc, "paper": "auto", "printer": "auto", "rules": rules() })));
    assert!(message.contains("20 x 30 in detected") && message.contains("Pass paper"), "{message}");

    // A rule whose printer isn't installed.
    let doc = open(&mut a, &dir, "c.pdf", &[(1296, 1728, "")]);
    let gone = json!([{ "paper": "ARCH C", "printer": "Gone Printer" }]);
    let message = invalid(call(&mut a, "doc_print", json!({ "doc": doc, "paper": "auto", "printer": "auto", "rules": gone })));
    assert!(message.contains("Gone Printer") && message.contains("isn't installed"), "{message}");
    let plan = call(&mut a, "print_plan", json!({ "doc": doc, "rules": gone })).unwrap();
    assert_eq!(plan["groups"][0]["route"], "printer-missing");
}

#[test]
fn mixed_sizes_are_never_put_on_one_paper_unasked() {
    let dir = workdir("mixed");
    let mut a = automation(&dir);
    let pages = [(1296, 1728, ""), (612, 792, ""), (1728, 1296, ""), (1728, 2592, "")];
    let doc = open(&mut a, &dir, "a.pdf", &pages);
    let plan = call(&mut a, "print_plan", json!({ "doc": doc, "rules": rules() })).unwrap();
    assert_eq!(plan["mixed"], true);
    assert_eq!(plan["breakdown"], "2 pages 18 x 24, 1 page Letter, 1 page 24 x 36");
    assert_eq!(plan["groups"][0]["page_numbers"], json!([1, 3]));

    let message = invalid(call(&mut a, "doc_print", json!({ "doc": doc, "paper": "auto", "printer": "auto", "rules": rules() })));
    assert!(message.contains("2 pages 18 x 24, 1 page Letter, 1 page 24 x 36") && message.contains("split"), "{message}");
    let message = invalid(call(&mut a, "doc_print", json!({ "doc": doc, "paper": "auto", "path": "x.pdf", "mixed": "split", "rules": rules() })));
    assert!(message.contains("can't be saved"), "{message}");
    assert!(!dir.join("x.pdf").exists());
    assert!(
        invalid(call(&mut a, "doc_print", json!({ "doc": doc, "paper": "auto", "printer": "auto", "mixed": "sideways", "rules": rules() })))
            .contains("sideways")
    );

    // Asked to split: the first job goes to the first size's printer. Nothing was sent, so the
    // error doesn't claim otherwise.
    let error = call(&mut a, "doc_print", json!({ "doc": doc, "paper": "auto", "printer": "auto", "mixed": "split", "rules": rules() }))
        .unwrap_err()
        .to_string();
    assert!(error.contains(C_PRINTER) && !error.contains("were sent"), "{error}");

    // One range of one size isn't mixed.
    let plan = call(&mut a, "print_plan", json!({ "doc": doc, "pages": "1,3", "rules": rules() })).unwrap();
    assert_eq!(plan["mixed"], false);
}

#[test]
fn what_goes_against_the_rules_is_refused_with_the_fix_until_allowed() {
    let dir = workdir("conflict");
    let mut a = automation(&dir);
    let doc = open(&mut a, &dir, "a.pdf", &[(1296, 1728, "")]);
    // The reported waste: ARCH C on the printer set up for the 36 inch roll.
    let args = json!({ "doc": doc, "paper": "ARCH C", "printer": D_PRINTER, "layout": "actual", "rules": rules() });
    let message = invalid(call(&mut a, "doc_print", args));
    assert!(message.contains(&format!("pass printer \"{C_PRINTER}\"")) && message.contains("allow_conflict"), "{message}");
    // The reported bug: no paper given, so US Letter, on the 24 inch roll.
    let message = invalid(call(&mut a, "doc_print", json!({ "doc": doc, "printer": C_PRINTER, "rules": rules() })));
    assert!(message.contains("These pages are 18 x 24"), "{message}");
    // Allowed: past the rules, to the spooler, which doesn't know this printer.
    let args = json!({ "doc": doc, "paper": "ARCH C", "printer": D_PRINTER, "layout": "actual", "allow_conflict": true, "rules": rules() });
    let error = call(&mut a, "doc_print", args).unwrap_err().to_string();
    assert!(error.contains(D_PRINTER) && !error.contains("allow_conflict"), "{error}");
    // paper auto with a printer the rules don't give this size.
    let message = invalid(call(&mut a, "doc_print", json!({ "doc": doc, "paper": "auto", "printer": D_PRINTER, "rules": rules() })));
    assert!(message.contains("These pages are 18 x 24"), "{message}");
    // Without any rules there is nothing to go against: doc_print as it always was.
    let error = call(&mut a, "doc_print", json!({ "doc": doc, "paper": "ARCH C", "printer": D_PRINTER })).unwrap_err().to_string();
    assert!(!error.contains("allow_conflict"), "{error}");
}

#[test]
fn an_explicit_paper_with_printer_auto_uses_that_papers_rule() {
    let dir = workdir("explicit");
    let mut a = automation(&dir);
    let doc = open(&mut a, &dir, "a.pdf", &[(612, 792, ""), (1296, 1728, "")]);
    // Every page on one paper, as asked: mixed sizes are no obstacle then.
    let error = call(&mut a, "doc_print", json!({ "doc": doc, "paper": "ARCH D", "printer": "auto", "layout": "fit", "rules": rules() }))
        .unwrap_err()
        .to_string();
    assert!(error.contains(D_PRINTER), "{error}");
    let message = invalid(call(&mut a, "doc_print", json!({ "doc": doc, "paper": "A4", "printer": "auto", "rules": rules() })));
    assert!(message.contains("no printer rule covers A4"), "{message}");
    assert!(
        invalid(call(&mut a, "doc_print", json!({ "doc": doc, "paper": "Quarto", "printer": "auto", "rules": rules() }))).contains("unknown paper")
    );
    let doc = open(&mut a, &dir, "b.pdf", &[(1296, 1728, "")]);
    assert!(invalid(call(&mut a, "doc_print", json!({ "doc": doc, "paper": "auto" }))).contains("pass either"));
    assert!(invalid(call(&mut a, "doc_print", json!({ "doc": doc, "paper": "auto", "printer": "auto", "path": "x.pdf" }))).contains("pass either"));
}

#[test]
fn rules_passed_to_a_call_are_checked() {
    let dir = workdir("bad-rules");
    let mut a = automation(&dir);
    let doc = open(&mut a, &dir, "a.pdf", &[(1296, 1728, "")]);
    for bad in [json!("rules"), json!([{ "paper": "Quarto", "printer": "X" }]), json!([{ "paper": "A4" }]), json!([1])] {
        assert!(matches!(call(&mut a, "print_plan", json!({ "doc": doc, "rules": bad })), Err(ToolError::InvalidArgs(_))), "{bad}");
    }
}

#[test]
fn the_rules_file_is_read_and_replaced_outside_a_confined_session_only() {
    let dir = workdir("rules-file");
    let file = dir.join("print-rules.json");
    let mut free = Automation::new().with_print_rules_file(&file).with_installed_printers(vec![C_PRINTER.to_string()]);

    // No file yet: the starter rules, named as Atlas's printers.
    let read = call(&mut free, "print_rules", json!({})).unwrap();
    assert_eq!(read["rules"].as_array().map(Vec::len), Some(5));
    assert_eq!(read["problem"], Value::Null);

    call(&mut free, "print_rules_set", json!({ "rules": [{ "paper": "ARCH C", "printer": C_PRINTER }] })).unwrap();
    let read = call(&mut free, "print_rules", json!({})).unwrap();
    assert_eq!(read["rules"], json!([{ "paper": "ARCH C (18 x 24 in)", "printer": C_PRINTER }]));
    assert!(call(&mut free, "print_rules_set", json!({ "rules": [{ "paper": "Quarto", "printer": "X" }] })).is_err());
    assert!(call(&mut free, "print_rules_set", json!({})).is_err());

    // doc_print follows the file.
    std::fs::write(dir.join("a.pdf"), drawing(&[(1296, 1728, "")])).unwrap();
    let doc = call(&mut free, "doc_open", json!({ "path": dir.join("a.pdf") })).unwrap()["doc"].as_u64().unwrap();
    let plan = call(&mut free, "print_plan", json!({ "doc": doc })).unwrap();
    assert_eq!(plan["groups"][0]["printer"], C_PRINTER);

    // A broken file: no rules, and the plan says why, so nothing prints somewhere wrong.
    std::fs::write(&file, "{ broken").unwrap();
    let plan = call(&mut free, "print_plan", json!({ "doc": doc })).unwrap();
    assert_eq!(plan["groups"][0]["route"], "no-rule");
    assert!(plan["rules_problem"].as_str().unwrap().contains("can't be used"));
    let message = invalid(call(&mut free, "doc_print", json!({ "doc": doc, "paper": "auto", "printer": "auto" })));
    assert!(message.contains("can't be used"), "{message}");

    // A session confined to a folder can't reach the file outside it.
    let mut confined = Automation::new().with_root(&dir).unwrap().with_print_rules_file(&file);
    let message = invalid(call(&mut confined, "print_rules_set", json!({ "rules": [{ "paper": "A4", "printer": "P" }] })));
    assert!(message.contains("confined"), "{message}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ broken", "unchanged");
}

/// `doc_print` to a real printer still reports nothing new when it has no rules, and a path job
/// is unchanged: the old arguments behave as they did.
#[test]
fn explicit_jobs_without_rules_are_unchanged() {
    let dir = workdir("unchanged");
    let mut a = automation(&dir);
    let doc = open(&mut a, &dir, "a.pdf", &[(1296, 1728, "")]);
    let out = call(&mut a, "doc_print", json!({ "doc": doc, "paper": "ARCH C", "layout": "actual", "path": "x.pdf" })).unwrap();
    assert_eq!((out["sheets"].as_u64(), out["pages"].as_u64()), (Some(1), Some(1)));
    assert_eq!(out.get("reason"), None);
}
