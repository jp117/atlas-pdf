//! Which printer and paper a document's pages belong on: Atlas Switch's wide-format printers each
//! hold one roll, set up as its own Windows printer, so the paper follows from the pages and the
//! printer from the paper. Pure logic: the Print dialog and `doc_print` share it, and neither the
//! spooler nor a printer is needed to test it.
//!
//! - [`detect_paper`] reads a page's displayed size as a standard paper, either way round.
//! - [`PrinterRules`] maps papers to printers. They live in a small JSON file ([`FILE_NAME`]) in the
//!   settings folder, so the app and the command line read the same rules.
//! - [`plan`] groups the pages to print by size and routes each group.
//! - [`Choice`] is what to preselect for one group, and why; [`conflict`] says when what is
//!   selected goes against the rules, and what to do about it.

use pdfcraft_print::PAPERS;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// How far a page may be from a standard paper, in points on each side, and still be that paper
/// (0.7 mm: CAD exports round their sizes). No two of [`PAPERS`] are this close.
pub const SIZE_TOLERANCE: f64 = 2.0;
/// The printer rules file, in the settings folder.
pub const FILE_NAME: &str = "print-rules.json";
/// The most rules kept from a file.
pub const MAX_RULES: usize = 256;
/// The largest rules file read.
pub const MAX_FILE_BYTES: u64 = 1 << 20;
/// The most separate jobs offered for one document: past this, a file isn't a set of drawings.
pub const MAX_JOBS: usize = 16;
/// The short side of Tabloid, the largest paper an office printer usually takes. A drawing with a
/// wider short side is wide-format.
const OFFICE_SHORT_SIDE: f64 = 792.0;

/// Whether `paper` (a [`PAPERS`] index) is the one a name means: its full name, without "US", or
/// without its dimensions ("ARCH C"), ignoring case.
fn named(paper: &str, asked: &str) -> bool {
    paper.eq_ignore_ascii_case(asked)
        || paper.strip_prefix("US ").is_some_and(|short| short.eq_ignore_ascii_case(asked))
        || paper.split(" (").next().is_some_and(|short| short.eq_ignore_ascii_case(asked))
}

/// The [`PAPERS`] index of a paper's name or short name.
pub fn paper_named(name: &str) -> Option<usize> {
    PAPERS.iter().position(|(paper, _)| named(paper, name.trim()))
}

/// A page's displayed size as a standard paper, either way round: the closest within
/// [`SIZE_TOLERANCE`]. Nothing for a size that isn't finite and positive.
pub fn detect_paper(size: (f64, f64)) -> Option<usize> {
    let (short, long) = normalised(size)?;
    let mut best: Option<(usize, f64)> = None;
    for (index, (_, (w, h))) in PAPERS.iter().enumerate() {
        let off = (w - short).abs().max((h - long).abs());
        if off <= SIZE_TOLERANCE && best.is_none_or(|(_, b)| off < b) {
            best = Some((index, off));
        }
    }
    best.map(|(index, _)| index)
}

/// `(short side, long side)`, for a size that is finite and positive.
fn normalised(size: (f64, f64)) -> Option<(f64, f64)> {
    (size.0.is_finite() && size.1.is_finite() && size.0 > 0.0 && size.1 > 0.0).then(|| (size.0.min(size.1), size.0.max(size.1)))
}

/// What people call a paper: "18 x 24" for ARCH C, "Letter", "A4".
pub fn size_name(paper: usize) -> String {
    let Some((name, _)) = PAPERS.get(paper) else { return "unknown size".to_string() };
    if let Some((_, dims)) = name.split_once(" (")
        && let Some(inches) = dims.strip_suffix(" in)")
    {
        return inches.to_string();
    }
    let name = name.split(" (").next().unwrap_or(name);
    name.strip_prefix("US ").unwrap_or(name).to_string()
}

/// A paper's name as the Paper menu shows it.
fn paper_label(paper: usize) -> &'static str {
    PAPERS.get(paper).map_or("unknown", |(name, _)| name)
}

/// A size in inches: "20 x 30 in", "20.5 x 30.25 in".
pub fn inches(size: (f64, f64)) -> String {
    let side = |points: f64| {
        let text = format!("{:.2}", points / 72.0);
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    format!("{} x {} in", side(size.0), side(size.1))
}

/// Whether a page of this size is a wide-format drawing, printed at actual size by default.
pub fn is_wide_format(size: (f64, f64)) -> bool {
    normalised(size).is_some_and(|(short, _)| short > OFFICE_SHORT_SIDE + SIZE_TOLERANCE)
}

/// One rule: this paper goes to this printer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrinterRule {
    /// A name from [`PAPERS`] (short names are understood in a file).
    pub paper: String,
    /// The printer's name, as Windows lists it.
    pub printer: String,
}

/// The rules in force. The first rule for a paper wins.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrinterRules {
    pub rules: Vec<PrinterRule>,
}

/// The rules file as written.
#[derive(Serialize, Deserialize)]
struct RulesFile {
    #[serde(default)]
    version: u32,
    rules: Vec<PrinterRule>,
}

/// What a rules file gave: the rules, and anything worth telling the person.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadedRules {
    pub rules: PrinterRules,
    /// Why the file (or part of it) couldn't be used.
    pub problem: Option<String>,
}

impl PrinterRules {
    /// The rules a new install starts with, until someone edits them.
    pub fn starter() -> Self {
        let rule = |paper: &str, printer: &str| PrinterRule { paper: paper.to_string(), printer: printer.to_string() };
        const OFFICE: &str = "HP LaserJet M607 [F35101]";
        PrinterRules {
            rules: vec![
                rule("ARCH C (18 x 24 in)", "Canon TX-3200 18x24"),
                rule("ARCH D (24 x 36 in)", "Canon TX-3200"),
                rule("US Letter", OFFICE),
                rule("US Legal", OFFICE),
                rule("Tabloid", OFFICE),
            ],
        }
    }

    /// The printer the rules give a paper.
    pub fn printer_for(&self, paper: usize) -> Option<&str> {
        let name = PAPERS.get(paper)?.0;
        self.rules.iter().find(|r| named(name, &r.paper) && !r.printer.is_empty()).map(|r| r.printer.as_str())
    }

    /// Read a rules file's text. A rule that names no known paper or no printer is left out and
    /// reported; text that isn't a rules file is an error.
    pub fn parse(text: &str) -> Result<(PrinterRules, Option<String>), String> {
        let file: RulesFile = serde_json::from_str(text).map_err(|e| format!("it isn't a printer rules file: {e}"))?;
        let mut rules = Vec::new();
        let mut skipped = Vec::new();
        for rule in file.rules {
            let paper = paper_named(&rule.paper);
            match paper {
                Some(index) if !rule.printer.trim().is_empty() && rules.len() < MAX_RULES => {
                    rules.push(PrinterRule { paper: paper_label(index).to_string(), printer: rule.printer.trim().to_string() });
                }
                _ => skipped.push(rule.paper),
            }
        }
        let problem = (!skipped.is_empty()).then(|| format!("{} rule(s) were left out: {}", skipped.len(), bounded_list(&skipped)));
        Ok((PrinterRules { rules }, problem))
    }

    /// The rules as a file's text.
    pub fn to_json(&self) -> String {
        // Plain strings and a number: this can't fail, but a settings write never panics anyway.
        serde_json::to_string_pretty(&RulesFile { version: 1, rules: self.rules.clone() }).unwrap_or_default()
    }

    /// The rules at `path`: the starter rules when there is no file yet. A file that can't be
    /// used gives no rules and says why, so every size asks instead of printing somewhere wrong.
    pub fn load(path: &Path) -> LoadedRules {
        use std::io::Read as _;
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LoadedRules { rules: Self::starter(), problem: None },
            Err(e) => return unusable(path, &e.to_string()),
        };
        let mut text = String::new();
        // One byte over the limit, to tell a file that fits from one that doesn't.
        if let Err(e) = file.take(MAX_FILE_BYTES.saturating_add(1)).read_to_string(&mut text) {
            return unusable(path, &e.to_string());
        }
        if text.len() as u64 > MAX_FILE_BYTES {
            return unusable(path, "it is too large");
        }
        match Self::parse(&text) {
            Ok((rules, problem)) => LoadedRules { rules, problem: problem.map(|p| format!("{}: {p}", path.display())) },
            Err(e) => unusable(path, &e),
        }
    }
}

fn unusable(path: &Path, why: &str) -> LoadedRules {
    LoadedRules { rules: PrinterRules::default(), problem: Some(format!("the printer rules in {} can't be used: {why}", path.display())) }
}

/// Names for a message, the first few only: they come from a file.
fn bounded_list(names: &[String]) -> String {
    let shown: Vec<String> = names.iter().take(5).map(|n| n.chars().take(40).collect()).collect();
    let more = names.len().saturating_sub(shown.len());
    if more == 0 { shown.join(", ") } else { format!("{} and {more} more", shown.join(", ")) }
}

/// Where a group of pages goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// A rule names this printer, and it is installed (or the printer list isn't known yet).
    Ready(String),
    /// No rule covers this size.
    NoRule,
    /// The rule names this printer, and it isn't installed.
    Missing(String),
}

/// Pages of one size.
#[derive(Clone, Debug, PartialEq)]
pub struct SizeGroup {
    /// The standard paper they match.
    pub paper: Option<usize>,
    /// `(short side, long side)` in points, of the group's first page.
    pub size: (f64, f64),
    /// The pages (0-based), in print order.
    pub pages: Vec<usize>,
    pub route: Route,
}

impl SizeGroup {
    /// The size as people say it: "18 x 24", or inches for a size that isn't a standard paper.
    pub fn size_name(&self) -> String {
        self.paper.map_or_else(|| inches(self.size), size_name)
    }

    /// Why this group goes where it does, or what the person must decide.
    pub fn reason(&self) -> String {
        let size = self.size_name();
        match &self.route {
            Route::Ready(printer) => format!("{size} detected: {printer}"),
            Route::Missing(printer) => format!("{size} detected, but the printer rule names {printer}, which isn't installed. Choose a printer."),
            Route::NoRule if self.paper.is_some() => format!("{size} detected, and no printer rule covers it. Choose a printer."),
            Route::NoRule => format!("{size} detected. This isn't a standard paper, and no printer rule covers it. Choose a printer."),
        }
    }

    /// Whether these pages are wide-format drawings.
    pub fn wide_format(&self) -> bool {
        is_wide_format(self.size)
    }
}

/// The pages to print, by size.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// In the order the sizes first appear.
    pub groups: Vec<SizeGroup>,
}

impl Plan {
    pub fn is_mixed(&self) -> bool {
        self.groups.len() > 1
    }

    /// The one size, when there is exactly one.
    pub fn single(&self) -> Option<&SizeGroup> {
        match self.groups.as_slice() {
            [group] => Some(group),
            _ => None,
        }
    }

    /// Whether each size can go out as its own job.
    pub fn can_split(&self) -> bool {
        self.is_mixed() && self.groups.len() <= MAX_JOBS
    }

    /// The breakdown as one line: "2 pages 18 x 24, 1 page Letter".
    pub fn breakdown(&self) -> String {
        let parts: Vec<String> = self.groups.iter().take(6).map(|g| format!("{} {}", page_count(g.pages.len()), g.size_name())).collect();
        let more = self.groups.len().saturating_sub(parts.len());
        if more == 0 { parts.join(", ") } else { format!("{} and {more} more size(s)", parts.join(", ")) }
    }
}

/// "1 page", "3 pages".
pub fn page_count(n: usize) -> String {
    if n == 1 { "1 page".to_string() } else { format!("{n} pages") }
}

/// Sizes that aren't a standard paper group to the nearest point.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum GroupKey {
    Paper(usize),
    Other(i64, i64),
    Unreadable,
}

fn group_key(size: (f64, f64)) -> GroupKey {
    if let Some(paper) = detect_paper(size) {
        return GroupKey::Paper(paper);
    }
    // `as` saturates, and the size is finite and positive here.
    normalised(size).map_or(GroupKey::Unreadable, |(short, long)| GroupKey::Other(short.round() as i64, long.round() as i64))
}

/// Group `pages` (positions in `sizes`, each page's displayed size) by size and route each group.
/// `printers` is the installed printers, `None` while they aren't known: no printer is called
/// missing until the list is in. Pages past the end of `sizes` are ignored.
pub fn plan(sizes: &[(f64, f64)], pages: &[usize], rules: &PrinterRules, printers: Option<&[&str]>) -> Plan {
    let mut groups: Vec<SizeGroup> = Vec::new();
    let mut found: BTreeMap<GroupKey, usize> = BTreeMap::new();
    for &page in pages {
        let Some(&size) = sizes.get(page) else { continue };
        let key = group_key(size);
        let at = match found.get(&key) {
            Some(&at) => at,
            None => {
                let at = groups.len();
                let paper = match key {
                    GroupKey::Paper(paper) => Some(paper),
                    _ => None,
                };
                found.insert(key, at);
                groups.push(SizeGroup {
                    paper,
                    size: normalised(size).unwrap_or((0.0, 0.0)),
                    pages: Vec::new(),
                    route: route(paper, rules, printers),
                });
                at
            }
        };
        if let Some(group) = groups.get_mut(at) {
            group.pages.push(page);
        }
    }
    Plan { groups }
}

fn route(paper: Option<usize>, rules: &PrinterRules, printers: Option<&[&str]>) -> Route {
    match paper.and_then(|p| rules.printer_for(p)) {
        None => Route::NoRule,
        Some(printer) if printers.is_none_or(|list| list.contains(&printer)) => Route::Ready(printer.to_string()),
        Some(printer) => Route::Missing(printer.to_string()),
    }
}

/// What to preselect for one size of page, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// The printer, when a rule gives an installed one. Otherwise the person chooses: there is no
    /// fallback to the default printer or the last one used.
    pub printer: Option<String>,
    /// The paper. The size detected from the PDF when it is a standard paper; only a size that
    /// isn't one takes the last-used paper, and [`Choice::reason`] says so.
    pub paper: usize,
    /// Print at actual size: wide-format drawings keep their scale.
    pub actual: bool,
    pub reason: String,
}

/// The choice for `group`, with `last_paper` the paper used last.
pub fn choose(group: &SizeGroup, last_paper: usize) -> Choice {
    let printer = match &group.route {
        Route::Ready(printer) => Some(printer.clone()),
        Route::NoRule | Route::Missing(_) => None,
    };
    let mut reason = group.reason();
    if group.paper.is_none() {
        reason.push_str(&format!(" The paper shown is the last one used: {}.", paper_label(last_paper)));
    }
    Choice { printer, paper: group.paper.unwrap_or(last_paper), actual: group.wide_format(), reason }
}

/// A printer and paper picked for a job.
#[derive(Clone, Copy, Debug)]
pub struct Selected<'a> {
    pub printer: &'a str,
    pub paper: usize,
    /// One page per sheet at the paper's size, so the paper should be the pages' own. Multiple,
    /// Booklet, Poster and Label put pages on other papers on purpose.
    pub one_page_per_sheet: bool,
}

/// Why what is selected goes against the rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub message: String,
    /// The printer and paper the rules want, when that printer is installed.
    pub fix: Option<(String, usize)>,
}

impl Conflict {
    /// What the one-click fix does.
    pub fn fix_label(&self) -> Option<String> {
        self.fix.as_ref().map(|(printer, paper)| format!("Use {printer}, {}", paper_label(*paper)))
    }
}

/// Whether `selected` goes against the rules, for the pages in `group` when they are of one size.
/// Sizes the rules don't cover, and rules whose printer is gone, were already put to the person;
/// their choice stands unless the paper chosen has its own rule.
pub fn conflict(group: Option<&SizeGroup>, selected: Selected<'_>, rules: &PrinterRules, printers: Option<&[&str]>) -> Option<Conflict> {
    let chosen = format!("{}, {}", selected.printer, paper_label(selected.paper));
    if selected.one_page_per_sheet
        && let Some(group) = group
        && let (Route::Ready(wanted), Some(paper)) = (&group.route, group.paper)
        && (selected.printer != wanted || selected.paper != paper)
    {
        let size = size_name(paper);
        return Some(Conflict {
            message: format!("These pages are {size}, which the printer rules send to {wanted}. You have selected {chosen}."),
            fix: Some((wanted.clone(), paper)),
        });
    }
    let wanted = rules.printer_for(selected.paper)?;
    if wanted == selected.printer || !printers.is_none_or(|list| list.contains(&wanted)) {
        return None;
    }
    Some(Conflict {
        message: format!("The printer rules send {} to {wanted}. You have selected {chosen}.", size_name(selected.paper)),
        fix: Some((wanted.to_string(), selected.paper)),
    })
}

/// The explanation to show, once, when Windows moved the default printer. `manages` is whether
/// "Let Windows manage my default printer" is on; `before` is the default when Atlas PDF last
/// sent a job, and `now` the default at present. Atlas PDF never changes the default itself.
pub fn default_printer_note(manages: bool, before: Option<&str>, now: Option<&str>, shown: bool) -> Option<String> {
    let (Some(before), Some(now)) = (before, now) else { return None };
    (manages && !shown && before != now).then(|| {
        format!(
            "Windows changed your default printer from {before} to {now}, because \"Let Windows manage my default printer\" is on. Atlas PDF didn't change it. You can turn that off in Windows Settings under Bluetooth & devices, Printers & scanners."
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARCH_C: (f64, f64) = (1296.0, 1728.0);
    const ARCH_D: (f64, f64) = (1728.0, 2592.0);
    const LETTER: (f64, f64) = (612.0, 792.0);
    const CANON_C: &str = "Canon TX-3200 18x24";
    const CANON_D: &str = "Canon TX-3200";
    const OFFICE: &str = "HP LaserJet M607 [F35101]";
    const INSTALLED: &[&str] = &[CANON_C, CANON_D, OFFICE];

    fn paper(name: &str) -> usize {
        paper_named(name).unwrap()
    }

    fn starter_plan(sizes: &[(f64, f64)]) -> Plan {
        let pages: Vec<usize> = (0..sizes.len()).collect();
        plan(sizes, &pages, &PrinterRules::starter(), Some(INSTALLED))
    }

    #[test]
    fn a_page_is_its_paper_either_way_round_within_the_tolerance() {
        for (size, name) in [
            (ARCH_C, "ARCH C"),
            ((1728.0, 1296.0), "ARCH C"),
            (ARCH_D, "ARCH D"),
            (LETTER, "US Letter"),
            ((1295.0, 1729.5), "ARCH C"),
            ((595.0, 842.0), "A4"),
        ] {
            assert_eq!(detect_paper(size), Some(paper(name)), "{size:?}");
        }
        for size in [(1296.0, 1731.0), (1000.0, 1728.0), (0.0, 0.0), (-612.0, -792.0), (f64::NAN, 792.0), (612.0, f64::INFINITY)] {
            assert_eq!(detect_paper(size), None, "{size:?}");
        }
    }

    /// The tolerance can't make a page two papers.
    #[test]
    fn no_two_papers_are_within_the_tolerance_of_each_other() {
        for (i, (_, a)) in PAPERS.iter().enumerate() {
            assert_eq!(detect_paper(*a), Some(i));
            for (_, b) in PAPERS.iter().skip(i + 1) {
                assert!((a.0 - b.0).abs().max((a.1 - b.1).abs()) > 2.0 * SIZE_TOLERANCE, "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn sizes_have_the_names_people_use() {
        assert_eq!(size_name(paper("ARCH C")), "18 x 24");
        assert_eq!(size_name(paper("ARCH D")), "24 x 36");
        assert_eq!(size_name(paper("Letter")), "Letter");
        assert_eq!(size_name(paper("A4")), "A4");
        assert_eq!(size_name(usize::MAX), "unknown size");
        assert_eq!(inches((1440.0, 2160.0)), "20 x 30 in");
        assert_eq!(inches((1476.0, 2178.0)), "20.5 x 30.25 in");
    }

    #[test]
    fn the_starter_rules_route_the_size_detected() {
        let reason = |sizes: &[(f64, f64)]| starter_plan(sizes).groups[0].reason();
        assert_eq!(reason(&[ARCH_C]), "18 x 24 detected: Canon TX-3200 18x24");
        assert_eq!(reason(&[(1728.0, 1296.0)]), "18 x 24 detected: Canon TX-3200 18x24", "landscape is the same size");
        assert_eq!(reason(&[ARCH_D]), "24 x 36 detected: Canon TX-3200");
        assert_eq!(reason(&[LETTER]), "Letter detected: HP LaserJet M607 [F35101]");
        assert_eq!(reason(&[(612.0, 1008.0)]), "Legal detected: HP LaserJet M607 [F35101]");
        assert_eq!(reason(&[(792.0, 1224.0)]), "Tabloid detected: HP LaserJet M607 [F35101]");
    }

    #[test]
    fn rules_are_data_not_hardcoded_printers() {
        let rules = PrinterRules { rules: vec![PrinterRule { paper: "ARCH C".into(), printer: "Plotter 7".into() }] };
        let got = plan(&[ARCH_C, ARCH_D], &[0, 1], &rules, Some(&["Plotter 7"]));
        assert_eq!(got.groups[0].route, Route::Ready("Plotter 7".into()));
        assert_eq!(got.groups[1].route, Route::NoRule);
    }

    /// The last-used paper never stands in for a size read from the PDF.
    #[test]
    fn the_last_paper_is_used_only_for_a_size_no_standard_paper_matches() {
        let letter = paper("Letter");
        let choice = choose(&starter_plan(&[ARCH_C]).groups[0], letter);
        assert_eq!((choice.paper, choice.printer.as_deref(), choice.actual), (paper("ARCH C"), Some(CANON_C), true));
        assert!(!choice.reason.contains("last"), "{}", choice.reason);

        // A standard size no rule covers keeps its own paper, and asks for the printer.
        let ansi_d = choose(&starter_plan(&[(1584.0, 2448.0)]).groups[0], letter);
        assert_eq!((ansi_d.paper, ansi_d.printer), (paper("ANSI D"), None));
        assert_eq!(ansi_d.reason, "22 x 34 detected, and no printer rule covers it. Choose a printer.");

        // A size that is no paper at all takes the last one, and says so.
        let odd = choose(&starter_plan(&[(1440.0, 2160.0)]).groups[0], letter);
        assert_eq!((odd.paper, odd.printer), (letter, None));
        assert!(odd.reason.contains("20 x 30 in detected") && odd.reason.contains("the last one used: US Letter"), "{}", odd.reason);
    }

    #[test]
    fn a_rule_for_a_printer_that_is_gone_asks_instead_of_falling_back() {
        let group = &plan(&[ARCH_C], &[0], &PrinterRules::starter(), Some(&[OFFICE])).groups[0];
        assert_eq!(group.route, Route::Missing(CANON_C.into()));
        let choice = choose(group, paper("Letter"));
        assert_eq!((choice.printer, choice.paper), (None, paper("ARCH C")));
        assert!(choice.reason.contains("isn't installed"), "{}", choice.reason);
        // While the list isn't in, nothing is called missing yet.
        assert_eq!(plan(&[ARCH_C], &[0], &PrinterRules::starter(), None).groups[0].route, Route::Ready(CANON_C.into()));
    }

    #[test]
    fn only_wide_drawings_default_to_actual_size() {
        for size in [ARCH_C, ARCH_D, (1440.0, 2160.0), (864.0, 1296.0)] {
            assert!(is_wide_format(size), "{size:?}");
        }
        for size in [LETTER, (612.0, 1008.0), (792.0, 1224.0), (f64::NAN, 3000.0)] {
            assert!(!is_wide_format(size), "{size:?}");
        }
    }

    #[test]
    fn mixed_sizes_group_in_the_order_they_appear_keeping_print_order() {
        let sizes = [ARCH_C, LETTER, (1728.0, 1296.0), ARCH_D, LETTER];
        let got = plan(&sizes, &[4, 3, 2, 1, 0], &PrinterRules::starter(), Some(INSTALLED));
        assert!(got.is_mixed() && got.can_split() && got.single().is_none());
        let groups: Vec<(&str, &[usize])> = got.groups.iter().map(|g| (paper_label(g.paper.unwrap()), g.pages.as_slice())).collect();
        assert_eq!(groups, [("US Letter", &[4, 1][..]), ("ARCH D (24 x 36 in)", &[3]), ("ARCH C (18 x 24 in)", &[2, 0])]);
        assert_eq!(got.breakdown(), "2 pages Letter, 1 page 24 x 36, 2 pages 18 x 24");
    }

    /// The input is a file's page tree: hostile sizes group safely and in linear-ish time.
    #[test]
    fn hostile_page_sizes_are_harmless() {
        let mut sizes: Vec<(f64, f64)> = (0..20_000).map(|i| (100.0 + f64::from(i), 90_000.0)).collect();
        sizes.extend([(f64::NAN, f64::NAN), (f64::INFINITY, 1.0), (-1.0, 5.0), (0.0, 0.0), (1e300, 1e300)]);
        let pages: Vec<usize> = (0..sizes.len()).chain([usize::MAX]).collect();
        let got = plan(&sizes, &pages, &PrinterRules::starter(), Some(INSTALLED));
        assert_eq!(got.groups.iter().map(|g| g.pages.len()).sum::<usize>(), sizes.len(), "every real page counted, the page past the end not");
        assert!(!got.can_split(), "too many sizes to send apart");
        assert!(got.breakdown().len() < 300);
        for group in &got.groups {
            assert_eq!(group.route, Route::NoRule);
            let _ = choose(group, usize::MAX);
        }
        assert_eq!(plan(&sizes, &[], &PrinterRules::starter(), None), Plan::default());
    }

    #[test]
    fn the_wrong_printer_for_the_paper_is_a_conflict_with_a_fix() {
        let rules = PrinterRules::starter();
        let plan_c = starter_plan(&[ARCH_C]);
        let group = plan_c.single();
        let select = |printer, name| Selected { printer, paper: paper(name), one_page_per_sheet: true };
        let conflict_of = |printer, name| conflict(group, select(printer, name), &rules, Some(INSTALLED));

        assert_eq!(conflict_of(CANON_C, "ARCH C"), None, "what the rules chose");
        // The reported waste: ARCH C on the printer set up for the 36 inch roll.
        let wrong = conflict_of(CANON_D, "ARCH C").unwrap();
        assert_eq!(wrong.fix, Some((CANON_C.to_string(), paper("ARCH C"))));
        assert_eq!(wrong.fix_label().unwrap(), "Use Canon TX-3200 18x24, ARCH C (18 x 24 in)");
        // The reported bug: Letter on the 24 inch roll.
        let letter = conflict_of(CANON_C, "US Letter").unwrap();
        assert_eq!(letter.fix, Some((CANON_C.to_string(), paper("ARCH C"))));
        assert!(letter.message.contains("These pages are 18 x 24"), "{}", letter.message);
        // The office printer on its own paper is still not where these pages go.
        assert!(conflict_of(OFFICE, "Tabloid").is_some());
    }

    #[test]
    fn layouts_that_change_the_paper_on_purpose_answer_only_to_the_papers_own_rule() {
        let rules = PrinterRules::starter();
        let plan_c = starter_plan(&[ARCH_C]);
        let imposed = |printer, name| Selected { printer, paper: paper(name), one_page_per_sheet: false };
        assert_eq!(conflict(plan_c.single(), imposed(OFFICE, "Tabloid"), &rules, Some(INSTALLED)), None);
        let wrong = conflict(plan_c.single(), imposed(CANON_D, "Tabloid"), &rules, Some(INSTALLED)).unwrap();
        assert_eq!(wrong.fix, Some((OFFICE.to_string(), paper("Tabloid"))));
        // A paper with no rule: nothing to go against.
        assert_eq!(conflict(plan_c.single(), imposed(CANON_D, "ANSI D"), &rules, Some(INSTALLED)), None);
    }

    #[test]
    fn a_choice_the_person_was_asked_for_is_no_conflict() {
        let rules = PrinterRules::starter();
        // No rule for ANSI D: whichever printer they pick stands.
        let ansi = starter_plan(&[(1584.0, 2448.0)]);
        let select = Selected { printer: CANON_D, paper: paper("ANSI D"), one_page_per_sheet: true };
        assert_eq!(conflict(ansi.single(), select, &rules, Some(INSTALLED)), None);
        // The rule's printer is gone: the replacement they picked stands too.
        let gone = plan(&[ARCH_C], &[0], &rules, Some(&[CANON_D, OFFICE]));
        let select = Selected { printer: CANON_D, paper: paper("ARCH C"), one_page_per_sheet: true };
        assert_eq!(conflict(gone.single(), select, &rules, Some(&[CANON_D, OFFICE])), None);
        // A size no paper matches on a paper with its own rule: that rule still speaks.
        let odd = starter_plan(&[(1440.0, 2160.0)]);
        let select = Selected { printer: CANON_D, paper: paper("ARCH C"), one_page_per_sheet: true };
        assert_eq!(conflict(odd.single(), select, &rules, Some(INSTALLED)).unwrap().fix, Some((CANON_C.to_string(), paper("ARCH C"))));
    }

    #[test]
    fn a_rules_file_round_trips_and_reports_what_it_could_not_use() {
        let rules = PrinterRules::starter();
        assert_eq!(PrinterRules::parse(&rules.to_json()), Ok((rules, None)));
        let (got, problem) = PrinterRules::parse(
            r#"{"rules":[{"paper":"ARCH C","printer":" Plotter "},{"paper":"Quarto","printer":"X"},{"paper":"Letter","printer":"  "}], "future":1}"#,
        )
        .unwrap();
        assert_eq!(got.rules, [PrinterRule { paper: "ARCH C (18 x 24 in)".into(), printer: "Plotter".into() }]);
        assert_eq!(problem.unwrap(), "2 rule(s) were left out: Quarto, Letter");
        for bad in ["", "not json", "[]", "{}", r#"{"rules":3}"#, r#"{"rules":[{"paper":1,"printer":2}]}"#] {
            assert!(PrinterRules::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_rule_count_from_a_file_is_capped() {
        let many: Vec<PrinterRule> = (0..MAX_RULES + 50).map(|_| PrinterRule { paper: "A4".into(), printer: "P".into() }).collect();
        let text = serde_json::to_string(&RulesFile { version: 1, rules: many }).unwrap();
        let (got, problem) = PrinterRules::parse(&text).unwrap();
        assert_eq!(got.rules.len(), MAX_RULES);
        assert!(problem.unwrap().starts_with("50 rule(s) were left out"));
    }

    #[test]
    fn loading_starts_from_the_starter_rules_and_never_guesses_from_a_bad_file() {
        let dir = std::env::temp_dir().join(format!("atlas-print-rules-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        let _ = std::fs::remove_file(&path);
        assert_eq!(PrinterRules::load(&path), LoadedRules { rules: PrinterRules::starter(), problem: None }, "no file yet");

        std::fs::write(&path, "{ broken").unwrap();
        let bad = PrinterRules::load(&path);
        assert!(bad.rules.rules.is_empty());
        assert!(bad.problem.unwrap().contains("can't be used"));

        std::fs::write(&path, vec![b' '; MAX_FILE_BYTES as usize + 10]).unwrap();
        assert!(PrinterRules::load(&path).problem.unwrap().contains("too large"));

        std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        assert!(PrinterRules::load(&path).rules.rules.is_empty(), "not text");

        let custom = PrinterRules { rules: vec![PrinterRule { paper: "A4".into(), printer: "P".into() }] };
        std::fs::write(&path, custom.to_json()).unwrap();
        assert_eq!(PrinterRules::load(&path), LoadedRules { rules: custom, problem: None });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_default_printer_note_appears_only_when_windows_moved_it_and_only_once() {
        let note = default_printer_note(true, Some(OFFICE), Some(CANON_C), false).unwrap();
        assert!(note.contains(OFFICE) && note.contains(CANON_C) && note.contains("Let Windows manage my default printer"), "{note}");
        assert_eq!(default_printer_note(true, Some(OFFICE), Some(CANON_C), true), None, "said once");
        assert_eq!(default_printer_note(false, Some(OFFICE), Some(CANON_C), false), None, "the user manages it");
        assert_eq!(default_printer_note(true, Some(OFFICE), Some(OFFICE), false), None, "it didn't move");
        assert_eq!(default_printer_note(true, None, Some(OFFICE), false), None, "nothing printed yet");
    }
}
