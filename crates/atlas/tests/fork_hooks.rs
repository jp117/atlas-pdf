//! The Atlas side of every file Atlas PDF changed in PdfCraft's code, counted.
//!
//! Atlas marks each change it makes to a file upstream also edits with a comment naming the fork
//! (see `MARKER`). Resolving a merge conflict to upstream's side drops those changes silently: the
//! code still builds, and the feature just stops being wired in. This test counts the marked lines
//! in every file and compares them with `HOOKS`, so a merge that loses one fails here.
//!
//! When the count changes on purpose (a new hook, or upstream split a file and the hook moved),
//! update `HOOKS` in the same commit. Never lower a count to make a merge pass without finding
//! where the hook went (README, "Taking changes from upstream").

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Spelled in two parts so this file doesn't count itself.
const MARKER: &str = concat!("Atlas", " fork");

/// Every file holding `MARKER`, with how many of its lines do. Sorted by path.
const HOOKS: &[(&str, usize)] = &[
    (".github/workflows/atlas-release.yml", 1),
    ("AGENTS.md", 1),
    ("NOTICE", 1),
    ("README.md", 3),
    ("ROADMAP.md", 3),
    ("apps/pdfcraft-cli/src/run.rs", 1),
    ("apps/pdfcraft/Cargo.toml", 3),
    ("apps/pdfcraft/src/atlas_install.rs", 1),
    ("apps/pdfcraft/src/atlas_register.rs", 1),
    ("apps/pdfcraft/src/main.rs", 6),
    ("apps/pdfcraft/src/settings.rs", 2),
    ("apps/pdfcraft/src/single_instance.rs", 2),
    ("apps/pdfcraft/src/updates.rs", 2),
    ("crates/annot/src/tests.rs", 1),
    ("crates/atlas/src/lib.rs", 1),
    ("crates/automation/README.md", 1),
    ("crates/automation/src/atlas.rs", 1),
    ("crates/automation/src/atlas_print.rs", 1),
    ("crates/automation/src/comments.rs", 1),
    ("crates/automation/src/lib.rs", 3),
    ("crates/automation/src/printing.rs", 2),
    ("crates/automation/src/tools.rs", 1),
    ("crates/automation/tests/it/atlas_print.rs", 1),
    ("crates/engine/src/comments.rs", 1),
    ("crates/engine/src/links.rs", 1),
    ("crates/engine/src/tests.rs", 1),
    ("crates/print/README.md", 1),
    ("crates/render/src/inspect/annotations.rs", 1),
    ("crates/render/src/lib.rs", 1),
    ("crates/render/src/raster.rs", 1),
    ("crates/render/src/raster/tests.rs", 11),
    ("crates/render/src/shx.rs", 1),
    ("crates/ui-egui/src/app/control.rs", 1),
    ("crates/ui-egui/src/app/prefs.rs", 1),
    ("crates/ui-egui/src/atlas/mod.rs", 1),
    ("crates/ui-egui/src/atlas/print_routing.rs", 1),
    ("crates/ui-egui/src/atlas/print_rules_ui.rs", 1),
    ("crates/ui-egui/src/atlas/printer_papers.rs", 1),
    ("crates/ui-egui/src/canvas.rs", 1),
    ("crates/ui-egui/src/canvas/image_pick.rs", 1),
    ("crates/ui-egui/src/canvas/page_cursor.rs", 1),
    ("crates/ui-egui/src/canvas/page_input.rs", 1),
    ("crates/ui-egui/src/chrome.rs", 1),
    ("crates/ui-egui/src/dialogs.rs", 3),
    ("crates/ui-egui/src/dialogs/preferences.rs", 1),
    ("crates/ui-egui/src/editing.rs", 1),
    ("crates/ui-egui/src/hotbar.rs", 1),
    ("crates/ui-egui/src/lib.rs", 3),
    ("crates/ui-egui/src/menus.rs", 4),
    ("crates/ui-egui/src/panels/right.rs", 3),
    ("crates/ui-egui/src/persist/recovery.rs", 1),
    ("crates/ui-egui/src/platform/portable.rs", 1),
    ("crates/ui-egui/src/platform/updates.rs", 6),
    ("crates/ui-egui/src/state.rs", 2),
    ("crates/ui-egui/src/windows.rs", 3),
    ("crates/ui-egui/src/workflows/print_ui.rs", 5),
    ("crates/ui-egui/src/workflows/print_ui/body.rs", 7),
    ("crates/ui-egui/src/workflows/print_ui/draft.rs", 1),
    ("crates/ui-egui/tests/atlas.rs", 1),
    ("crates/ui-egui/tests/it/links.rs", 1),
    ("crates/ui-egui/tests/it/panel_snapshots.rs", 2),
    ("crates/ui-egui/tests/it/print_rules.rs", 1),
    ("deny.toml", 1),
    ("packaging/windows/install-atlas.ps1", 1),
    ("packaging/windows/package-atlas.ps1", 1),
    ("packaging/windows/print-hold-lib.ps1", 1),
    ("packaging/windows/test-print-hold.ps1", 1),
    ("parity/acrobat-features.toml", 2),
    ("vendor/README.md", 2),
    ("vendor/hayro-interpret/src/cache.rs", 5),
    ("vendor/hayro-interpret/src/x_object.rs", 2),
    ("vendor/hayro-syntax/Cargo.toml", 1),
    ("vendor/hayro-syntax/src/filter/dct.rs", 3),
    ("vendor/hayro-syntax/src/filter/lzw_flate.rs", 2),
    ("vendor/hayro/src/renderer.rs", 20),
    ("xtask/src/layers.rs", 2),
];

/// Folders that are never part of the source: build output, local-only notes, checkouts and
/// corpora (`.gitignore`).
const SKIP: &[&str] = &[".git", ".claude", "target", "plan", "dist", "corpus", "log", "fuzz-out", "craft-fonts", "models"];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn walk(dir: &Path, root: &Path, found: &mut BTreeMap<String, usize>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            if !SKIP.iter().any(|s| name == *s) {
                walk(&path, root, found);
            }
        } else if kind.is_file() {
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let text = String::from_utf8_lossy(&bytes);
            let lines = text.lines().filter(|l| l.contains(MARKER)).count();
            if lines > 0 {
                let relative = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                found.insert(relative, lines);
            }
        }
    }
}

#[test]
fn every_atlas_hook_survives_a_merge() {
    let root = workspace_root();
    let mut found = BTreeMap::new();
    walk(&root, &root, &mut found);
    let expected: BTreeMap<String, usize> = HOOKS.iter().map(|(p, n)| ((*p).to_owned(), *n)).collect();

    let mut problems = Vec::new();
    for (path, want) in &expected {
        match found.get(path) {
            None => problems.push(format!("{path}: no hooks left (expected {want}); the file is gone or lost Atlas's code")),
            Some(got) if got < want => problems.push(format!("{path}: {got} hooks, expected {want}; a merge dropped Atlas's code")),
            Some(got) if got > want => problems.push(format!("{path}: {got} hooks, expected {want}; add the new hook to HOOKS")),
            Some(_) => {}
        }
    }
    for (path, got) in &found {
        if !expected.contains_key(path) {
            problems.push(format!("{path}: {got} hooks not in HOOKS; add it (or the file a hook moved from is listed)"));
        }
    }
    assert!(problems.is_empty(), "Atlas hooks changed:\n  {}", problems.join("\n  "));
}

#[test]
fn hooks_are_sorted_and_listed_once() {
    let paths: Vec<&str> = HOOKS.iter().map(|(p, _)| *p).collect();
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(paths, sorted);
}
