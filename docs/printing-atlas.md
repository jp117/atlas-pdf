# Printing drawings in Atlas PDF

Atlas PDF chooses the printer and paper from each PDF's page size, so wide-format roll paper isn't
wasted on a job sent to the wrong roll. This applies to the Print window and to `doc_print`.

## How Atlas's printers are set up

A Canon imagePROGRAF TX-3200 holds two rolls and is set up as two Windows printers. The settings
saved on each printer stay with it, and every job carries them through.

| Windows printer | Paper | Paper Source | Roll Paper Width | Rotate Page 90 degrees | Expected sheet |
|---|---|---|---|---|---|
| `Canon TX-3200` | ARCH D (24x36) | Roll 1 (36") | 36" | On | 36" across, cut at 24" |
| `Canon TX-3200 18x24` | ARCH C (18x24) | Roll 2 (24") | 24" | On | 24" across, cut at 18" |

The office printer is `HP LaserJet M607 [F35101]`, which is also the Windows default.

## Printer rules

**Preferences ▸ Printer rules** maps paper sizes to printers. Print's **Printer rules…** button goes
there too. A new install starts with:

| Paper | Printer |
|---|---|
| ARCH C | `Canon TX-3200 18x24` |
| ARCH D | `Canon TX-3200` |
| Letter, Legal, Tabloid | `HP LaserJet M607 [F35101]` |

The rules are saved in `print-rules.json` beside the app's settings
(`%APPDATA%\AtlasPDF\data`, or the `AtlasPDFData` folder of a portable copy), and the app and
`pdfcraft-cli` read the same file:

```json
{ "version": 1, "rules": [{ "paper": "ARCH C (18 x 24 in)", "printer": "Canon TX-3200 18x24" }] }
```

Short paper names such as `ARCH C` or `Letter` work in the file. A rule that can't be used is left
out and reported. A file that can't be read gives no rules, and Print says why and asks, rather than
guessing. Editing the rules in Preferences rewrites the file.

## What Print does

When Print opens, it reads the size of the pages being printed, after their CropBox and `/Rotate`,
with portrait and landscape counting as the same size. A page within 2 points (0.7 mm) of a standard
paper is that paper. Changing which pages to print chooses again.

| The pages are | Printer | Paper | What Print says |
|---|---|---|---|
| A size with a rule | The rule's | The size detected | `18 x 24 detected: Canon TX-3200 18x24` |
| A standard size with no rule | You choose | The size detected | `22 x 34 detected, and no printer rule covers it. Choose a printer.` |
| No standard size | You choose | The last paper used | `20 x 30 in detected. … The paper shown is the last one used: ARCH D (24 x 36 in).` |
| A size whose rule names a printer that isn't installed | You choose | The size detected | `18 x 24 detected, but the printer rule names …, which isn't installed. Choose a printer.` |
| Several sizes | One per size | One per size | `Mixed page sizes: 2 pages 18 x 24, 1 page Letter, 1 page 24 x 36.` |

- **Actual size** is the default for wide-format drawings (a short side wider than Tabloid's 11 in).
  Other pages keep the layout you last used.
- **The last-used paper never overrides the size detected.** It only fills in for a size that matches
  no standard paper, and Print says so.
- **There's no silent fallback.** When the rules can't choose, Print stays disabled until you pick a
  printer. It doesn't use the default printer or the last one used.
- **You can override the printer and paper for a job.** The next print starts from the rules again.

### When the printer and paper go against the rules

A warning appears above the Print button, which then reads **Print anyway**. Next to the warning, one
button puts back what the rules want, for example **Use Canon TX-3200 18x24, ARCH C (18 x 24 in)**.
Save as PDF never warns, and neither does a printer you were asked to choose. Multiple, Booklet,
Poster and Label put pages on other papers on purpose, so they only answer to the rule for the paper
selected.

### Mixed page sizes

Windows prints one paper size per job. Print shows the breakdown, with a printer and paper for each
size, and **Send 3 jobs** sends each size to its own printer as a separate job named after its size
(`Plans (18 x 24).pdf`). A size with no printer holds every job until you choose one, and nothing is
sent until every size can be laid out.

**Print every page on one paper** is the alternative. It's your choice, and it asks for a printer.
Pages of other sizes are then scaled or cut as Page Sizing says. Booklet and the other layouts always
work this way. More than 16 sizes can't be sent apart.

### The default printer

Atlas PDF names the printer in every job and never sets the Windows default. If **Let Windows manage
my default printer** is on and the default has moved since a job was last sent, Print explains that
once. Atlas PDF only reads that setting, and you can turn it off in Windows Settings under
**Bluetooth & devices ▸ Printers & scanners**.

## The command line and agents

`doc_print` takes the same decisions:

```json
{ "tool": "doc_print", "args": { "doc": 1, "paper": "auto", "printer": "auto" } }
```

The result reports `paper`, `printer`, `reason`, `layout` and a `jobs` list. `print_plan` reports the
same choice without sending anything. `print_rules` and `print_rules_set` read and replace the rules.

| Case | What `doc_print` does |
|---|---|
| `paper: "auto"` with `path` | Saves the print-ready PDF on the size detected |
| `paper: "ARCH D"`, `printer: "auto"` | Every page on that paper, on the printer its rule gives |
| `paper: "auto"`, `printer: "<name>"` | The size detected, on that printer, if the rules agree |
| No rule, a missing printer or no standard size | An error saying what to pass; nothing is sent |
| Mixed sizes | An error with the breakdown, unless `mixed: "split"` sends one job per size |
| An explicit printer and paper that go against the rules | An error with the fix, unless `allow_conflict: true` |

A call can pass its own `rules`. `print_rules_set` is refused in a session confined with `--root`, since
the rules file is outside the folder.

## Testing without wasting paper

`packaging/windows/test-print-hold.ps1` sends real jobs to real drivers, through the command line and
the real Print window, and holds them:

1. Pause the queue and check that it reports Paused, or stop.
2. Send the job, and read the held job's DEVMODE with `GetJob` level 2. (`Win32_PrintJob` reports
   UNKNOWN paper for these jobs.)
3. Convert the DEVMODE to a print ticket through the driver, and check the paper, source, roll width
   and rotate.
4. Delete the job and resume the queue, also when the test fails. A job that can't be deleted leaves
   its queue paused, so it can't print.

```powershell
cargo build -p pdfcraft -p pdfcraft-cli
powershell -File packaging/windows/test-print-hold.ps1              # or -Only cli, -Only window
```

A held job can't prove the cut length or how the drawing sits on the sheet, so a change to this path
still needs one real sheet each of 18x24 and 24x36.

## Could one Windows printer serve both rolls?

Not needed today, since routing to the two printers works, but it looks possible. Nothing in the
product does this yet.

The Canon driver lists these features in `http://www.canon.com/ns/printschema/inkjet/v100`:
`JobInputBin` under the standard `psk` keyword (`RollPaperAuto`, `Manual`, `RollPaper1`, `RollPaper2`),
plus `JobRollWidth` and `JobRotate90`.

Asked to validate `RollPaper2` and `Roll24inch` on `Canon TX-3200`, without sending a job, the driver
accepted them and kept `JobRotate90=ON`. It reported the conflict as resolved, but left ARCH D as the
paper, so it doesn't stop an ARCH D job going to the 24" roll. A rule would have to check that itself.

To build it:
- A rule would need optional driver settings, and only a rule or the user could set them.
- `craft-print` would have to pass vendor features into the ticket. Today its `Job::options` are CUPS
  only, and on Windows the ticket holds schema keywords. That means extending a dependency.
- It would need its own held-job tests and real sheets, since those settings haven't been tried
  through to paper.

## Known limits

- **Color is always stated on the job.** `craft-print` writes `Color` or `Monochrome` into every
  ticket, from Print's grayscale box, so a color setting saved on a printer doesn't carry through.
  Paper source, roll width and rotate do.
- **A portrait and a landscape page of one size share a job**, sent in the orientation of its first
  page, with the others turned to match (`craft-print`).
