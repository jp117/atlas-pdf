//! Atlas fork: `doc_print`'s automatic paper and printer, the same rules the Print dialog follows
//! (`pdfcraft_atlas::print_rules`). `print_plan` says what would happen without sending anything;
//! `print_rules` and `print_rules_set` read and change the rules the app uses.

use pdfcraft_atlas::print_rules::{self, Conflict, Plan, PrinterRules, Route, Selected, SizeGroup};
use pdfcraft_engine::print::{self, Layout, PAPERS, spool};
use serde_json::{Value, json};

use crate::printing::{JobOptions, bad, paper_named, spool_job};
use crate::tools::ToolDef;
use crate::{Args, Automation, Result, failed, write_atomic};

/// The value of `paper` and `printer` that asks for the rules to choose.
const AUTO: &str = "auto";

/// One job `doc_print` has settled on.
struct PlannedJob {
    pages: Vec<usize>,
    paper: usize,
    printer: Option<String>,
    reason: String,
    options: JobOptions,
    title: String,
}

/// The name a layout goes by in `doc_print`'s arguments.
fn layout_name(layout: &Layout) -> &'static str {
    match layout {
        Layout::Size(print::SizeMode::Fit) => "fit",
        Layout::Size(print::SizeMode::Actual) => "actual",
        Layout::Size(print::SizeMode::Shrink) => "shrink",
        Layout::Size(print::SizeMode::Custom(_)) => "custom",
        Layout::Multiple { .. } => "multiple",
        Layout::Booklet { .. } => "booklet",
        Layout::Poster { .. } => "poster",
        Layout::Label { .. } => "label",
    }
}

/// A paper's name for reports.
fn paper_name(paper: usize) -> &'static str {
    PAPERS.get(paper).map_or("unknown", |(name, _)| name)
}

/// `pages` (0-based) as the 1-based numbers the tools report.
fn page_numbers(pages: &[usize]) -> Vec<usize> {
    pages.iter().map(|p| p.saturating_add(1)).collect()
}

/// Whether `doc_print`'s arguments ask the rules to choose.
pub(crate) fn is_auto(a: &Args) -> Result<bool> {
    let auto = |value: Option<&str>| value.is_some_and(|v| v.eq_ignore_ascii_case(AUTO));
    Ok(auto(a.opt_str("paper")?) || auto(a.opt_str("printer")?))
}

/// The system default printer's name.
fn system_default() -> Option<String> {
    spool::printers().into_iter().find(|p| p.default).map(|p| p.name)
}

/// The error for a job that goes against the rules: what is wrong and what to do.
fn conflict_error(found: &Conflict) -> crate::ToolError {
    let fix = found
        .fix
        .as_ref()
        .map_or(String::new(), |(printer, paper)| format!(" To follow the rules, pass printer {printer:?} and paper {:?}.", paper_name(*paper)));
    bad(format!("{}{fix} To send it as asked, pass allow_conflict: true.", found.message))
}

impl Automation {
    /// Use the printer rules in this file when a call passes none.
    pub fn with_print_rules_file(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.print_rules_file = Some(path.into());
        self
    }

    /// Use the printer rules the app uses.
    pub fn with_app_print_rules(mut self) -> Self {
        self.print_rules_file = print_rules::app_file();
        self
    }

    /// Take these as the installed printers, instead of asking the spooler.
    pub fn with_installed_printers(mut self, printers: Vec<String>) -> Self {
        self.installed_printers = Some(printers);
        self
    }

    /// The installed printers' names.
    fn installed(&self) -> Vec<String> {
        self.installed_printers.clone().unwrap_or_else(|| spool::printers().into_iter().map(|p| p.name).collect())
    }

    /// The rules for this call (its `rules`, else the rules file) and any problem reading them.
    /// `None` when neither exists: then the call has no rules to answer to.
    fn call_rules(&self, a: &Args) -> Result<Option<(PrinterRules, Option<String>)>> {
        if let Some(rules) = a.get("rules") {
            // Given by the caller, so anything left out of them is the caller's to fix.
            let (rules, problem) = PrinterRules::parse(&json!({ "rules": rules }).to_string()).map_err(bad)?;
            if let Some(problem) = problem {
                return Err(bad(format!("rules: {problem}")));
            }
            return Ok(Some((rules, None)));
        }
        Ok(self.print_rules_file.as_deref().map(|path| {
            let loaded = PrinterRules::load(path);
            (loaded.rules, loaded.problem)
        }))
    }

    fn page_sizes(&self, a: &Args) -> Result<Vec<(f64, f64)>> {
        Ok(self.doc(a)?.info.pages.iter().map(|p| (f64::from(p.width), f64::from(p.height))).collect())
    }

    /// `print_plan`: the pages by size, where the rules send each size, and why. Sends nothing.
    pub(crate) fn print_plan(&self, a: &Args) -> Result<Value> {
        let pages = self.print_pages(a)?;
        let sizes = self.page_sizes(a)?;
        let (rules, problem) = self.call_rules(a)?.unwrap_or_default();
        let printers = self.installed();
        let names: Vec<&str> = printers.iter().map(String::as_str).collect();
        let plan = print_rules::plan(&sizes, &pages, &rules, Some(&names));
        let groups: Vec<Value> = plan.groups.iter().map(group_json).collect();
        Ok(json!({
            "mixed": plan.is_mixed(),
            "can_split": plan.can_split(),
            "breakdown": plan.breakdown(),
            "groups": groups,
            "rules_problem": problem,
        }))
    }

    /// `print_rules`: the rules the app and `doc_print` use, and the file they live in.
    pub(crate) fn print_rules(&self) -> Result<Value> {
        let Some(path) = self.print_rules_file.as_deref() else {
            return Ok(json!({ "file": Value::Null, "rules": [], "problem": "this session has no printer rules file" }));
        };
        let loaded = PrinterRules::load(path);
        Ok(json!({ "file": path.to_string_lossy(), "rules": loaded.rules.rules, "problem": loaded.problem }))
    }

    /// `print_rules_set`: replace the rules. A session confined to a folder may not change what
    /// happens outside it.
    pub(crate) fn print_rules_set(&self, a: &Args) -> Result<Value> {
        if self.root.is_some() || self.roots.is_some() {
            return Err(bad(
                "this session is confined to a folder, so it can't change the printer rules; pass rules to doc_print or print_plan instead",
            ));
        }
        let Some(path) = self.print_rules_file.as_deref() else {
            return Err(bad("this session has no printer rules file; pass rules to doc_print or print_plan instead"));
        };
        let rules = a.get("rules").ok_or_else(|| bad("rules is required: a list of { paper, printer }"))?;
        let (rules, problem) = PrinterRules::parse(&json!({ "rules": rules }).to_string()).map_err(bad)?;
        if let Some(problem) = problem {
            return Err(bad(format!("rules: {problem}")));
        }
        write_atomic(path, rules.to_json().as_bytes())?;
        Ok(json!({ "file": path.to_string_lossy(), "rules": rules.rules }))
    }

    /// Before an explicit printer and paper are sent: refuse what goes against the rules, unless
    /// the caller says to send it anyway. A session with no rules has nothing to answer to.
    pub(crate) fn check_print_rules(&self, a: &Args, pages: &[usize], paper: usize, printer: &str, layout: &Layout) -> Result<()> {
        let Some((rules, _)) = self.call_rules(a)? else { return Ok(()) };
        if a.opt_bool("allow_conflict")?.unwrap_or(false) {
            return Ok(());
        }
        let printer = if printer == "default" { system_default().unwrap_or_default() } else { printer.to_string() };
        let printers = self.installed();
        let names: Vec<&str> = printers.iter().map(String::as_str).collect();
        let plan = print_rules::plan(&self.page_sizes(a)?, pages, &rules, Some(&names));
        let selected = Selected { printer: &printer, paper, one_page_per_sheet: matches!(layout, Layout::Size(_)) };
        match print_rules::conflict(plan.single(), selected, &rules, Some(&names)) {
            Some(found) => Err(conflict_error(&found)),
            None => Ok(()),
        }
    }

    /// Settle what each job is, or say what the caller must decide. Nothing is sent here.
    fn plan_jobs(&self, a: &Args, pages: Vec<usize>) -> Result<Vec<PlannedJob>> {
        let (rules, problem) = self.call_rules(a)?.unwrap_or_default();
        let printers = self.installed();
        let names: Vec<&str> = printers.iter().map(String::as_str).collect();
        let plan = print_rules::plan(&self.page_sizes(a)?, &pages, &rules, Some(&names));
        let printer_asked = a.opt_str("printer")?;
        let allow = a.opt_bool("allow_conflict")?.unwrap_or(false);
        let split = match a.opt_str("mixed")?.unwrap_or("refuse") {
            "refuse" => false,
            "split" => true,
            m => return Err(bad(format!("unknown mixed {m:?}; choose refuse or split"))),
        };
        // Where the rules can't choose, the message says why they can't.
        let hint = problem.map_or(String::new(), |p| format!(" ({p})"));
        let title = self.doc(a)?.name.clone();
        match a.opt_str("paper")? {
            Some(paper) if !paper.eq_ignore_ascii_case(AUTO) => {
                // The caller chose one paper for every page, so the rules only choose the printer.
                let (paper, _) = paper_named(Some(paper))?;
                let printer = printer_for_paper(paper, &rules, &names, &hint)?;
                let options = JobOptions::read(a, default_layout(plan.single().filter(|g| g.paper == Some(paper))))?;
                let selected = Selected { printer: &printer, paper, one_page_per_sheet: matches!(options.layout, Layout::Size(_)) };
                if !allow && let Some(found) = print_rules::conflict(plan.single(), selected, &rules, Some(&names)) {
                    return Err(conflict_error(&found));
                }
                let reason = format!("{} asked for: {printer}", print_rules::size_name(paper));
                Ok(vec![PlannedJob { pages, paper, printer: Some(printer), reason, options, title }])
            }
            _ => {
                if plan.is_mixed() {
                    check_can_split(&plan, split, a.opt_str("path")?.is_some())?;
                }
                let mixed = plan.is_mixed();
                let mut jobs = Vec::with_capacity(plan.groups.len());
                for group in &plan.groups {
                    let Some(paper) = group.paper else {
                        return Err(bad(format!("{} Pass paper: this size isn't a standard paper.{hint}", group.reason())));
                    };
                    let options = JobOptions::read(a, default_layout(Some(group)))?;
                    let (printer, reason) = match printer_asked {
                        None => (None, format!("{} detected", group.size_name())),
                        Some(asked) if asked.eq_ignore_ascii_case(AUTO) => match &group.route {
                            Route::Ready(printer) => (Some(printer.clone()), group.reason()),
                            Route::NoRule | Route::Missing(_) => {
                                return Err(bad(format!("{} Pass printer, or add a rule with print_rules_set.{hint}", group.reason())));
                            }
                        },
                        Some(asked) => {
                            let name = if asked == "default" { system_default().unwrap_or_default() } else { asked.to_string() };
                            let selected = Selected { printer: &name, paper, one_page_per_sheet: matches!(options.layout, Layout::Size(_)) };
                            if !allow && let Some(found) = print_rules::conflict(Some(group), selected, &rules, Some(&names)) {
                                return Err(conflict_error(&found));
                            }
                            (Some(asked.to_string()), format!("{} detected; printer as asked: {asked}", group.size_name()))
                        }
                    };
                    let title = if mixed { job_title(&title, group) } else { title.clone() };
                    jobs.push(PlannedJob { pages: group.pages.clone(), paper, printer, reason, options, title });
                }
                Ok(jobs)
            }
        }
    }

    /// `doc_print` with `paper` or `printer` set to `auto`.
    pub(crate) fn doc_print_auto(&mut self, a: &Args) -> Result<Value> {
        let path = a.opt_str("path")?;
        let printer = a.opt_str("printer")?;
        match (path, printer) {
            (Some(_), None) | (None, Some(_)) => {}
            _ => return Err(bad("pass either path (save the print-ready PDF) or printer (a name from printers, \"default\", or \"auto\")")),
        }
        let id = self.doc(a)?.id;
        let pages = self.print_pages(a)?;
        let jobs = self.plan_jobs(a, pages)?;
        let sizes = self.page_sizes(a)?;
        // Everything is laid out before anything is sent, so one bad size sends nothing.
        let mut ready = Vec::with_capacity(jobs.len());
        for job in jobs {
            let (_, paper) = PAPERS.get(job.paper).copied().ok_or_else(|| bad("unknown paper"))?;
            let settings = job.options.settings(job.pages.clone(), paper);
            let sheets = print::layout(&sizes, &settings).map_err(|e| bad(e.to_string()))?.len();
            let bytes = self.session.print_pdf(id, &settings).map_err(failed)?;
            ready.push((job, sheets, bytes));
        }
        let mut reports = Vec::with_capacity(ready.len());
        let total = ready.len();
        for (sent, (job, sheets, bytes)) in ready.into_iter().enumerate() {
            let mut report = json!({
                "paper": paper_name(job.paper),
                "printer": job.printer,
                "reason": job.reason,
                "layout": layout_name(&job.options.layout),
                "page_numbers": page_numbers(&job.pages),
                "pages": job.pages.len(),
                "sheets": sheets,
                "bytes": bytes.len(),
            });
            match (&job.printer, path) {
                (Some(printer), _) => {
                    let spooled = spool_job(a, printer, job.title.clone(), job.options.duplex, job.paper)
                        .and_then(|spec| spool::submit(&bytes, &spec).map_err(|e| failed(e.to_string())));
                    match spooled {
                        Ok(receipt) => report["job"] = json!(receipt),
                        Err(e) if sent == 0 => return Err(e),
                        // Don't let the caller resend jobs that went out.
                        Err(e) => return Err(failed(format!("{sent} of {total} jobs were sent before this one failed: {e}"))),
                    }
                }
                (None, Some(p)) => {
                    let path = self.resolve(p, true)?;
                    write_atomic(&path, &bytes)?;
                    report["path"] = json!(path.to_string_lossy());
                }
                (None, None) => {}
            }
            reports.push(report);
        }
        Ok(match reports.as_slice() {
            [one] => {
                let mut out = one.clone();
                out["jobs"] = json!(reports);
                out
            }
            _ => {
                let pages: u64 = reports.iter().filter_map(|r| r["pages"].as_u64()).sum();
                let sheets: u64 = reports.iter().filter_map(|r| r["sheets"].as_u64()).sum();
                json!({ "mixed": true, "pages": pages, "sheets": sheets, "reason": "mixed page sizes: one job per size", "jobs": reports })
            }
        })
    }
}

/// The name a size's job goes by when a document is split.
fn job_title(title: &str, group: &SizeGroup) -> String {
    let stem = title.strip_suffix(".pdf").or_else(|| title.strip_suffix(".PDF")).unwrap_or(title);
    format!("{stem} ({}).pdf", group.size_name())
}

/// Actual size for wide-format drawings of a size matched to their paper; otherwise `doc_print`'s
/// usual default.
fn default_layout(group: Option<&SizeGroup>) -> &'static str {
    if group.is_some_and(SizeGroup::wide_format) { "actual" } else { "fit" }
}

/// The printer the rules give an explicitly chosen paper.
fn printer_for_paper(paper: usize, rules: &PrinterRules, printers: &[&str], hint: &str) -> Result<String> {
    let size = print_rules::size_name(paper);
    let Some(printer) = rules.printer_for(paper) else {
        return Err(bad(format!("no printer rule covers {size}. Pass printer, or add a rule with print_rules_set.{hint}")));
    };
    if !printers.contains(&printer) {
        return Err(bad(format!("the printer rule for {size} names {printer}, which isn't installed. Pass printer.{hint}")));
    }
    Ok(printer.to_string())
}

/// Mixed sizes never go on one paper unasked: say what is in the document and what to pass.
fn check_can_split(plan: &Plan, split: bool, to_file: bool) -> Result<()> {
    let breakdown = plan.breakdown();
    if to_file {
        return Err(bad(format!(
            "mixed page sizes ({breakdown}) can't be saved as one print-ready PDF automatically; pass paper, or pages for one size"
        )));
    }
    if !split {
        return Err(bad(format!(
            "mixed page sizes ({breakdown}). Pass mixed: \"split\" to send each size to its own printer as a separate job, or pass paper to put every page on one paper"
        )));
    }
    if !plan.can_split() {
        return Err(bad(format!("too many page sizes to send as separate jobs ({breakdown}); pass paper, or pages for one size")));
    }
    Ok(())
}

/// A size group as `print_plan` reports it.
fn group_json(group: &SizeGroup) -> Value {
    let (route, printer) = match &group.route {
        Route::Ready(printer) => ("ready", Some(printer.as_str())),
        Route::NoRule => ("no-rule", None),
        Route::Missing(printer) => ("printer-missing", Some(printer.as_str())),
    };
    json!({
        "size": group.size_name(),
        "inches": print_rules::inches(group.size),
        "paper": group.paper.map(paper_name),
        "route": route,
        "printer": printer,
        "reason": group.reason(),
        "wide_format": group.wide_format(),
        "layout": default_layout(Some(group)),
        "page_numbers": page_numbers(&group.pages),
        "pages": group.pages.len(),
    })
}

/// The tools of printing by the rules, appended to the tool table.
pub(crate) fn tools() -> Vec<ToolDef> {
    let schema =
        |props: Value, required: &[&str]| json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false });
    let rules = || {
        json!({ "type": "array", "items": { "type": "object", "properties": { "paper": { "type": "string" }, "printer": { "type": "string" } }, "required": ["paper", "printer"], "additionalProperties": false },
            "description": "Printer rules: which printer each paper goes to. Paper names as in doc_print; the first rule for a paper wins." })
    };
    let def = |name, title, description, read_only, input_schema| ToolDef {
        name,
        title,
        description,
        input_schema,
        read_only,
        destructive: false,
        command: None,
    };
    vec![
        def(
            "print_plan",
            "Plan a print by page size",
            "Report what doc_print with paper and printer \"auto\" would do, without sending anything: the pages to print grouped by size (read from each page as displayed, after its CropBox and /Rotate, portrait and landscape being one size), and for each size its standard paper, the printer the rules give it, the route (ready, no-rule or printer-missing), the reason, and the default layout (actual for wide-format drawings). mixed is true when there is more than one size. rules overrides the app's printer rules for this call.",
            true,
            schema(
                json!({
                    "doc": { "type": "integer", "minimum": 1, "description": "Document id from doc_open or doc_list." },
                    "pages": { "type": "string" },
                    "subset": { "type": "string", "enum": ["all", "odd", "even"] },
                    "reverse": { "type": "boolean" },
                    "rules": rules(),
                }),
                &["doc"],
            ),
        ),
        def(
            "print_rules",
            "Read the printer rules",
            "The printer rules the app and doc_print use (which printer each paper goes to), the file they live in, and any problem reading it.",
            true,
            schema(json!({}), &[]),
        ),
        def(
            "print_rules_set",
            "Change the printer rules",
            "Replace the printer rules the app and doc_print use. Refused in a session confined with --root or the automation roots, since it changes what happens outside the folder.",
            false,
            schema(json!({ "rules": rules() }), &["rules"]),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_of_a_split_document_are_named_for_their_size() {
        let group = SizeGroup { paper: print_rules::paper_named("ARCH C"), size: (1296.0, 1728.0), pages: vec![0], route: Route::NoRule };
        assert_eq!(job_title("Plans.pdf", &group), "Plans (18 x 24).pdf");
        assert_eq!(job_title("Plans", &group), "Plans (18 x 24).pdf");
    }

    #[test]
    fn a_conflict_error_says_how_to_follow_the_rules_and_how_not_to() {
        let found = Conflict { message: "Wrong.".into(), fix: Some(("Canon".into(), 0)) };
        let crate::ToolError::InvalidArgs(message) = conflict_error(&found) else { panic!("an invalid-arguments error") };
        assert_eq!(
            message,
            "Wrong. To follow the rules, pass printer \"Canon\" and paper \"US Letter\". To send it as asked, pass allow_conflict: true."
        );
    }

    #[test]
    fn rule_entries_serialise_as_the_tool_describes_them() {
        let rule = print_rules::PrinterRule { paper: "A4".into(), printer: "P".into() };
        assert_eq!(json!(rule), json!({ "paper": "A4", "printer": "P" }));
    }
}
