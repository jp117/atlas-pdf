# Installing and releasing Atlas PDF

Atlas PDF ships to Atlas Switch on Windows as a per-user installer, and on Linux as an AppImage.
Both keep themselves up to date.
Releases are GitHub Releases of this repository, made by `.github/workflows/atlas-release.yml`.
(Upstream PdfCraft's own pipeline, [`releasing.md`](releasing.md), builds PdfCraft-branded
packages and isn't used here.)

## Installing (for everyone at Atlas)

1. Download **`AtlasSwitch.AtlasPDF-win-Setup.exe`** from
   <https://github.com/jp117/atlas-pdf/releases/latest>.
2. Run it. No administrator rights are needed: it installs for you alone in
   `%LOCALAPPDATA%\AtlasSwitch.AtlasPDF`, adds Start menu and desktop shortcuts, opens Atlas PDF and offers it
   for PDFs ("Open with Atlas PDF" on any PDF's right-click menu).
   - The installer isn't code-signed yet, so Windows may say *Windows protected your PC*: choose
     **More info ▸ Run anyway**. Updates don't ask again.
   - To open PDFs in Atlas PDF by double-click: on the first start it opens *Settings ▸ Default
     apps* for Atlas PDF; set `.pdf` to Atlas PDF there. Windows doesn't let an app do this itself.
3. **Updates are automatic.** Each time Atlas PDF starts it looks for a newer release, downloads it
   in the background and says *Atlas PDF x.y.z is ready*. The new version installs when you close
   Atlas PDF; *Help ▸ Check for updates ▸ Restart now* installs it at once (unsaved documents are
   asked about first).
4. **Uninstall** from *Settings ▸ Apps ▸ Installed apps ▸ Atlas PDF*. Your settings
   (`%APPDATA%\AtlasPDF`) stay.

`AtlasSwitch.AtlasPDF-win-Portable.zip` on the same page is the app unzipped, to run from anywhere; it doesn't
update itself.

## Installing on Linux

1. Download **`AtlasSwitch.AtlasPDF.AppImage`** from
   <https://github.com/jp117/atlas-pdf/releases/latest> into a folder you own, such as
   `~/Applications`, and make it executable:

   ```sh
   mkdir -p ~/Applications && mv ~/Downloads/AtlasSwitch.AtlasPDF.AppImage ~/Applications/
   chmod +x ~/Applications/AtlasSwitch.AtlasPDF.AppImage
   ~/Applications/AtlasSwitch.AtlasPDF.AppImage
   ```

   It runs on Ubuntu 22.04 and anything newer (glibc 2.35 or later).
2. On every start it adds itself to the applications menu and to *Open with* for PDFs
   (`~/.local/share/applications/AtlasSwitch.AtlasPDF.desktop`, written again if you move the
   AppImage). To make it the PDF default:
   `xdg-mime default AtlasSwitch.AtlasPDF.desktop application/pdf`.
3. **Updates are automatic**, as on Windows: the new version replaces the AppImage when you close
   Atlas PDF. Keep the AppImage in a folder you can write to, or the updater asks for your password.
4. **Uninstall** by deleting the AppImage. Its menu entry hides itself once the file is gone. Your
   settings (`~/.config/atlaspdf`) stay.

Printing goes through CUPS. Printer rules name CUPS queues (`lpstat -p` lists them), which have
different names from the same printers on Windows, so set the rules up again on each machine. The
Print dialog can't read the papers a printer takes from a CUPS driver yet, as it does from a Windows
one.

## Cutting a release

1. **Bump the version** in a PR, as for any change. It lives in `[workspace.package] version` in
   the root `Cargo.toml`:

   ```sh
   cargo xtask version              # the current version
   cargo xtask version set 0.7.2    # updates Cargo.toml and Cargo.lock
   ```

   Every release needs a higher version: installed copies only move to a newer one. When merging
   from upstream, keep the higher of the two versions.
2. **Run the workflow** on `main`, once the PR is merged:

   ```sh
   gh workflow run atlas-release.yml --repo jp117/atlas-pdf
   gh run watch --repo jp117/atlas-pdf
   ```

   It builds on GitHub's Windows and Ubuntu 22.04 runners at the same time and packages both with
   Velopack. The Windows job publishes **`v<version>`** with `AtlasSwitch.AtlasPDF-win-Setup.exe`,
   `AtlasSwitch.AtlasPDF-<version>-full.nupkg`, `releases.win.json` and
   `AtlasSwitch.AtlasPDF-win-Portable.zip`. Then `publish-linux` adds `AtlasSwitch.AtlasPDF.AppImage`,
   the Linux package and `releases.linux.json` to the same release. It refuses a version that is
   already released. If the Linux build fails, the Windows release still goes out. Linux copies
   then report that no update can be found until the next release has Linux files.
3. That's all: installed copies find it the next time they start.

**Trying a build without releasing it:** `gh workflow run atlas-release.yml -f publish=false`
keeps the files as a workflow artifact instead. `-f version=0.7.2-rc.1` overrides the version
for that run; a version with a `-` suffix is published as a pre-release, which installed copies
ignore (they follow the latest full release).

**Building on your own machine:** `pwsh packaging/windows/package-atlas.ps1` writes the Windows
files to `dist\atlas`, and `packaging/linux/package-atlas.sh` the Linux ones to `dist/atlas-linux`. It needs the .NET 8 SDK and `dotnet tool install -g vpk --version 1.2.161`.
Set `CRAFT_FONTS_DIR` and `CRAFT_I18N_DIR` to checkouts of craft-fonts and of craft-i18n (at the commit
in `craft-i18n.rev`) first, as the workflow does: without them the build has no Japanese fonts and is
English only.

**Trying an update locally:** serve a folder holding a newer build's `releases.win.json` and
`.nupkg` (`python -m http.server 8000`), then start an installed copy with
`ATLASPDF_UPDATE_URL=http://localhost:8000`.

## How it fits together

| Piece | What it does |
|---|---|
| `packaging/linux/package-atlas.sh` | Builds `pdfcraft`, stages it as `AtlasPDF` with the CLI, licences and OCR models, and runs `vpk pack` for the AppImage (full packages only) |
| `crates/atlas/src/desktop_entry.rs` | The AppImage's launcher entry, which `atlas_install.rs` writes on Linux |
| `packaging/windows/package-atlas.ps1` | Builds `pdfcraft.exe` (static C runtime), stages it as `AtlasPDF.exe` with the CLI, licences and OCR models, and runs `vpk pack` (full packages only) |
| `apps/pdfcraft/src/atlas_install.rs` | Answers Setup's hooks first thing in `main` (Windows); for installed copies, reads `releases/latest/download/releases.win.json` (`releases.linux.json` on Linux) with the app's own HTTPS client and hands a downloaded update to Velopack's updater as the app closes |
| `apps/pdfcraft/src/atlas_register.rs` | The hooks' registry entries under `HKEY_CURRENT_USER`: the ProgID, Default Programs, App Paths and the PDF right-click entry; uninstall removes them |
| `crates/ui-egui/src/atlas/updates.rs` | The check at start, the background download, the *ready* notice and the Updates dialog |
| `packaging/windows/install-atlas.ps1` | Installs a local build for trying it out (no automatic updates); it runs the same registration hooks. Don't keep it next to a Setup install |

The `velopack` crate and `vpk` are pinned to the same version (1.2.161); bump them together.

## Not done yet

- **Code signing.** Without it SmartScreen warns on the first install. `sign.ps1` already handles
  Azure Trusted Signing (about $10 a month); `vpk pack` would take `--signParams`.
- **A macOS** build for Atlas (`release.yml` upstream already signs and notarizes PdfCraft's).
