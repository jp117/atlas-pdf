#!/usr/bin/env bash
# Atlas fork: build Atlas PDF's Linux AppImage and update package (Velopack).
#
# Produces, in $OUT (default: dist/atlas-linux):
#   AtlasSwitch.AtlasPDF.AppImage            the app: download, make executable, run. It updates
#                                            itself in place and adds itself to the applications
#                                            menu (apps/pdfcraft/src/atlas_install.rs)
#   AtlasSwitch.AtlasPDF-<version>-*.nupkg   the package AppImages update from
#   releases.linux.json                      the feed they read
#
# The version is [workspace.package] version in Cargo.toml. atlas-release.yml runs this on
# Ubuntu 22.04 (glibc 2.35), so the AppImage runs on that and anything newer; see
# docs/releasing-atlas.md.
#
# Needs: cargo, the network (the OCR models), and vpk at the same version as the velopack crate
# in apps/pdfcraft/Cargo.toml (.NET 8 SDK, then: dotnet tool install -g vpk --version 1.2.161).
#
# Usage: packaging/linux/package-atlas.sh [--skip-build]
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"

VPK_VERSION=1.2.161
PACK_ID=AtlasSwitch.AtlasPDF
SKIP_BUILD=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    -h | --help) sed -n '2,17p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
OUT="${OUT:-$ROOT/dist/atlas-linux}"

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
  echo "error: unexpected version '$VERSION' in Cargo.toml" >&2
  exit 1
fi
if ! vpk -h 2>/dev/null | grep -q "$VPK_VERSION"; then
  echo "warning: this vpk may not be $VPK_VERSION, the velopack crate's version (dotnet tool install -g vpk --version $VPK_VERSION)" >&2
fi
echo "==> Atlas PDF $VERSION for Linux $(uname -m)"

if [[ "$SKIP_BUILD" = 0 ]]; then
  (cd "$ROOT" && cargo build --release --locked -p pdfcraft -p pdfcraft-cli)
fi
BIN="$CARGO_TARGET_DIR/release"

# The app is AtlasPDF, so it never collides with an installed PdfCraft's pdfcraft. Velopack puts
# the folder's contents beside it inside the AppImage.
STAGE="$CARGO_TARGET_DIR/atlas-package-linux"
rm -rf "$STAGE"
mkdir -p "$STAGE"
install -m755 "$BIN/pdfcraft" "$STAGE/AtlasPDF"
install -m755 "$BIN/pdfcraft-cli" "$STAGE/pdfcraft-cli"
strip "$STAGE/AtlasPDF" "$STAGE/pdfcraft-cli" 2>/dev/null || true
copy_docs "$STAGE"
cp "$ROOT/assets/app-icon/LICENSE-document.txt" "$STAGE/"
# OCR models, where the app looks first: models/ beside the executable (crates/ocr).
stage_models "$STAGE/models"

# Full packages only (no deltas), as on Windows: AppImages read `releases/latest/download`, which
# holds the newest release's files and none of the older ones a delta chain would need.
mkdir -p "$OUT"
vpk pack --packId "$PACK_ID" --packVersion "$VERSION" --packDir "$STAGE" --mainExe AtlasPDF \
  --packTitle 'Atlas PDF' --packAuthors 'Atlas Switch' \
  --icon "$ROOT/assets/app-icon/hicolor/256x256/apps/ai.storyteller.pdfcraft.png" \
  --categories 'Office;Viewer' --delta None --outputDir "$OUT"

# The Windows job publishes into the same release: a Linux file of the same name would replace
# a Windows one.
for f in "$OUT"/*; do
  case "$(basename "$f")" in
    *-full.nupkg) [[ "$(basename "$f")" == *linux* ]] || { echo "error: $(basename "$f") would collide with the Windows package" >&2; exit 1; } ;;
  esac
done
if [[ ! -f "$OUT/releases.linux.json" || ! -f "$OUT/$PACK_ID.AppImage" ]]; then
  echo "error: vpk pack didn't write releases.linux.json and $PACK_ID.AppImage:" >&2
  ls -l "$OUT" >&2
  exit 1
fi
ls -l "$OUT"
