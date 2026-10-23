# Atlas PDF

Atlas Switch's PDF workbench for construction drawings: a fork of
[PdfCraft](https://github.com/storytold/pdfcraft), the open-source, clean-room Acrobat
reimplementation in Rust (MIT OR Apache-2.0), with tools for quoting switchgear from electrical
drawing sets.

Everything PdfCraft does still works: viewing, search, comments, forms, organizing pages,
redaction, signatures, printing, the command-line tool and the opt-in MCP server. Atlas adds the
features below. For the upstream feature list, see
[PdfCraft's README](https://github.com/storytold/pdfcraft#readme).

**To install it,** download `AtlasSwitch.AtlasPDF-win-Setup.exe` from the
[latest release](https://github.com/jp117/atlas-pdf/releases/latest) and run it. No administrator
rights are needed, and it keeps itself up to date. See
[Installing and releasing Atlas PDF](docs/releasing-atlas.md).

## What Atlas adds

### Takeoff marks

Box equipment on a drawing to record whether Atlas quotes it:

| Key | Status                   | Colour |
| --- | ------------------------ | ------ |
| `1` | Included                 | green  |
| `2` | Included w/ comments     | yellow |
| `3` | Excluded                 | red    |

- Press `1`, `2` or `3`, or click the green, yellow or red box at the bottom of the tool strip
  beside the page (also in **Menu ▸ Atlas**), and drag a box over the equipment. Boxes are
  filled with a see-through tint of their colour, so the drawing stays readable.
- The box is labelled with the text inside it, including AutoCAD SHX text, so a disconnect boxed
  on an AutoCAD sheet arrives labelled `1600A D.S.#1 1500A FU`.
- Every box goes on in one drag, yellow included: comments are written on the quote, not on the
  boxes.
- Click a box (without dragging) to select it: `Delete` removes it, and `1`/`2`/`3` re-mark
  it. With nothing selected, `1`/`2`/`3` pick the tool.
- The **Takeoff** panel lists every mark sheet by sheet with a count per status. You can filter
  it and edit a mark's status and label, and **Export takeoff as CSV** saves the list (page,
  sheet, status, label, author).

Marks are standard PDF Square annotations with the subject `Atlas Takeoff: Included` (etc.), so
Acrobat and Bluebeam show them in their markup lists. Atlas's own data lives in a private
`/AtlasTakeoff` dictionary that other programs keep. A Square markup drawn in another program
with an Atlas subject counts as a mark too. Saving appends an incremental update, so the
original drawing's bytes are never rewritten.

### AutoCAD SHX text is text, not comments

AutoCAD exports SHX-font text as vector strokes plus an invisible annotation for each string
(author `AutoCAD SHX Text`). On electrical sheets that is most of the equipment text, so Acrobat
and upstream PdfCraft list hundreds or thousands of junk "comments" while search finds none of
the equipment. Atlas keeps those annotations out of the comment list and adds their text to the
page, so search, copy and the text tools all find it.

### Layers for construction drawings

The Layers panel groups layers:
- **by trade**, from NCS layer names (`E-POWR-RISE` is Electrical, `A-WALL` Architectural), with
  a keyword fallback for non-standard names;
- **by drawing**, from xref prefixes (`ARCH-BG|A-WALL`, `1ST FLOOR BG$0$BG`).

It also:
- merges same-named layers from combined files into one row;
- has a search box, a per-group eye and **Only**;
- has **Electrical focus**, which hides the architectural, interiors, structural, civil,
  landscape, mechanical and plumbing backgrounds in one click.

`Page N` sheet containers from combined files stay on through every preset.

### Takeoff for agents

`pdfcraft-cli run` and the MCP server gain `takeoff_list`, `takeoff_add`, `takeoff_update` and
`takeoff_export`, so a quoting agent can read an estimator's marks or draft its own. For
example:

```sh
pdfcraft-cli run --root "C:\jobs\2500363" --script takeoff.json
# takeoff.json: [{"tool":"doc_open","args":{"path":"E-600.pdf"}},
#                {"tool":"takeoff_export","args":{"doc":1,"out":"E-600 takeoff.json"}}]
```

## Building on Windows

Requirements:
- Rust 1.95 or later (`winget install Rustlang.Rustup`);
- Visual Studio Build Tools with the C++ workload
  (`winget install Microsoft.VisualStudio.BuildTools --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"`).

```sh
cargo run -p pdfcraft -- drawing.pdf        # the desktop app (debug build)
cargo build --release -p pdfcraft -p pdfcraft-cli
cargo test -p pdfcraft-atlas                # Atlas's own rules
```

Scope builds and tests to the crates you touched (see `CLAUDE.md`). A full workspace build takes
about 11 minutes on a 12-core machine.

### Installing a local build

Everyone else installs Setup from the latest release (above). To try a build of your own:

```sh
cargo build --release -p pdfcraft -p pdfcraft-cli
cargo xtask models                      # OCR models for Scan & OCR (optional)
powershell -ExecutionPolicy Bypass -File packaging\windows\install-atlas.ps1
```

This installs Atlas PDF for the current user in `%LOCALAPPDATA%\Programs\Atlas PDF`, without
administrator rights. It adds a Start menu shortcut and an "Open with Atlas PDF" entry on PDFs,
and registers the app so it can be the PDF default. Windows leaves that choice to you: the
script opens **Settings ▸ Default apps ▸ Atlas PDF**, where you set `.pdf` to Atlas PDF. Run it
again after a new build to update (close Atlas PDF first). Add `-Uninstall` to remove it; your
settings in `%APPDATA%\AtlasPDF` are kept. A script install doesn't update itself, and it shares
its registry entries with a Setup install: keep one or the other.

To publish a release, see [Cutting a release](docs/releasing-atlas.md#cutting-a-release).

## Working on the fork

PdfCraft moves fast: hundreds of commits a day, with `crates/ui-egui` the most contested. Atlas
code is kept out of upstream files so merges stay cheap:

| Where                                     | What                                                              |
| ----------------------------------------- | ----------------------------------------------------------------- |
| `crates/atlas` (`pdfcraft-atlas`, L4)     | Layer grouping and takeoff marks: pure rules with unit tests     |
| `crates/render/src/shx.rs`                | SHX text as page text                                             |
| `crates/ui-egui/src/atlas/`               | Takeoff tool, Takeoff panel, Layers panel, Atlas menu             |
| `crates/automation/src/atlas.rs`          | `takeoff_*` tools                                                 |

Upstream files carry one- to three-line hooks, each marked `Atlas fork`:
- `engine/src/lib.rs`: the takeoff edits and SHX filtering;
- `render/src/{text,inspect,lib}.rs`;
- `ui-egui/src/{lib,canvas,chrome,control,editing,docking,panels}.rs`;
- `automation/src/{lib,tools}.rs`;
- `xtask/src/layers.rs`.

Search for `Atlas fork` to find them all.

Rules for Atlas code:
- **Plain English strings** in Atlas UI. Every `tl!("…")` literal must be translated in about
  16 upstream catalogues, or tests fail.
- **Hooks, not edits.** Add a call into `atlas/`, not logic in an upstream file.
- **PdfCraft's rules still apply** (`AGENTS.md`, `CLAUDE.md`): never panic, clean-room,
  attribution for every asset, and quality gates before each commit (`cargo fmt --check`,
  clippy with `-D warnings`, the tests, `cargo xtask layers`, `cargo xtask assets`).
- **Clean-room.** Never look inside Adobe Acrobat's install folder or reverse engineer it.
  Behaviour comes from ISO 32000, public documentation and the drawings themselves.

### Taking changes from upstream

Our `main` is Atlas PDF's own line of development, not a copy of PdfCraft. PdfCraft is a
fetch-only `upstream` remote that we take changes from when they don't conflict with Atlas work:

```sh
git fetch upstream
git merge-tree --write-tree --name-only main upstream/main   # exit 0: merges cleanly; otherwise lists the conflicts
git merge upstream/main                                       # only when it merges cleanly
```

When a merge would conflict, leave it. To take one particular fix whose commit applies cleanly,
use `git cherry-pick <commit>`. After taking anything from upstream, run the quality gates on the
crates it touched and `cargo test -p pdfcraft-ui-egui` before pushing.

## Next

- Submittal review: filter returned engineer comments and stamps by reviewer and sheet, track
  approved / revise-and-resubmit, export a comment log.
- Revision compare for REV-0 / REV-1 shop drawings.
- Markup and respond: clouds, callouts and stamps on layers.
- Remember layer choices per file.
- An Atlas icon (the installer and app still use PdfCraft's).
- Code-sign the installer, so Windows stops warning on the first install.
- Hand takeoff marks to the switchgear quoting agents (`takeoff_export` JSON).

## Licence and credits

Atlas PDF is a modified version of PdfCraft, © the PdfCraft contributors, used under
MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`). Atlas's changes are under the
same licences. Every bundled asset is listed in `ATTRIBUTION.md`.

The ArtCraft name and logos are trademarks of the ArtCraft Team and are not part of this fork.
