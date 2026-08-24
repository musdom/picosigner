#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "Usage: $0 SOURCE_TTF [OUTPUT_TTF]" >&2
    echo "Defaults OUTPUT_TTF to picosigner/assets/subset-font.ttf." >&2
}

if [[ $# -lt 1 || $# -gt 2 ]]; then
    usage
    exit 2
fi

if ! command -v pyftsubset >/dev/null 2>&1; then
    echo "error: pyftsubset is required; install it with: python3 -m pip install fonttools" >&2
    exit 1
fi

source_font=$1
if [[ ! -f "$source_font" ]]; then
    echo "error: source font not found: $source_font" >&2
    exit 1
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
output_font=${2:-"$script_dir/subset-font.ttf"}
output_dir=$(dirname -- "$output_font")
if [[ ! -d "$output_dir" ]]; then
    echo "error: output directory not found: $output_dir" >&2
    exit 1
fi

temporary_font=$(mktemp "$output_dir/.subset-font.ttf.XXXXXX")
trap 'rm -f "$temporary_font"' EXIT

# PicoSigner's rendered text is printable ASCII. Disabling layout features
# prevents optional ligatures and their supporting glyphs from being pulled
# into the firmware. Hinting, glyph names, and license/version
# metadata are intentionally retained.
pyftsubset "$source_font" \
    --output-file="$temporary_font" \
    --unicodes=U+0020-007E \
    --layout-features='' \
    --glyph-names \
    --name-IDs='*'

chmod 0644 "$temporary_font"
mv -- "$temporary_font" "$output_font"
trap - EXIT

echo "Wrote $output_font ($(wc -c < "$output_font") bytes)"
