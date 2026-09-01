#!/usr/bin/env bash
# Regenerate the fonts embedded in the cmuxd binary.
#
# The browser page cannot rely on the viewing device having a Nerd Font — a
# phone certainly will not — so cmuxd carries its own and serves them. The
# symbols font is subset to the Nerd Fonts code point ranges and recompressed
# as woff2, which takes it from 2.6 MB to about 1 MB.
#
# Only needed when refreshing the vendored files; the build uses what is
# already committed under crates/cmuxd/assets/fonts.
#
# Needs: pyftsubset (python-fonttools, with brotli for woff2 output).
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$REPO/crates/cmuxd/assets/fonts"
mkdir -p "$OUT"

find_font() {
  for p in "$@"; do
    [ -f "$p" ] && { printf '%s' "$p"; return 0; }
  done
  echo "missing font, looked in: $*" >&2
  return 1
}

command -v pyftsubset >/dev/null || {
  echo "pyftsubset not found — pip install 'fonttools[woff]'" >&2; exit 2; }

# Every Nerd Fonts range, so any prompt or statusline renders, not just ours.
# The last range is Material Design Icons, which is most of the weight but is
# where common glyphs like the model and memory icons live.
RANGES='U+E000-E00A,U+E0A0-E0A3,U+E0B0-E0BF,U+E0C0-E0C8,U+E0CC-E0D7,'
RANGES+='U+E200-E2A9,U+E300-E3E3,U+E5FA-E6B7,U+E700-E8EF,U+EA60-EBEB,'
RANGES+='U+F000-F2FF,U+F300-F375,U+F400-F533,U+F0001-F1AF0'

SYMBOLS="$(find_font \
  /usr/share/fonts/TTF/SymbolsNerdFontMono-Regular.ttf \
  /usr/share/fonts/truetype/nerd-fonts/SymbolsNerdFontMono-Regular.ttf \
  ~/.local/share/fonts/SymbolsNerdFontMono-Regular.ttf)"

echo "subsetting $(basename "$SYMBOLS")"
pyftsubset "$SYMBOLS" \
  --unicodes="$RANGES" --flavor=woff2 --layout-features='' \
  --no-hinting --desubroutinize \
  --output-file="$OUT/symbols.woff2"

for pair in "Regular:mono.woff2" "Bold:mono-bold.woff2"; do
  weight="${pair%%:*}"; dest="${pair##*:}"
  src="$(find_font \
    "/usr/share/fonts/webfonts/JetBrainsMono-$weight.woff2" \
    "/usr/local/share/fonts/JetBrainsMono-$weight.woff2" \
    "$HOME/.local/share/fonts/JetBrainsMono-$weight.woff2")"
  echo "copying $(basename "$src") -> $dest"
  cp "$src" "$OUT/$dest"
done

for pair in "ttf-jetbrains-mono:LICENSE-JetBrainsMono.txt" \
            "ttf-nerd-fonts-symbols-mono:LICENSE-NerdFontsSymbols.txt"; do
  pkg="${pair%%:*}"; dest="${pair##*:}"
  if [ -f "/usr/share/licenses/$pkg/LICENSE" ]; then
    cp "/usr/share/licenses/$pkg/LICENSE" "$OUT/$dest"
    echo "copying license -> $dest"
  else
    echo "WARNING: no license found for $pkg; do not ship without it" >&2
  fi
done

echo
ls -l "$OUT"
