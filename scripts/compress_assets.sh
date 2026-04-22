#!/usr/bin/env bash
# Compress FHIR definition JSON files for embedding in the binary.
# Requires zstd (brew install zstd).
#
# Usage: bash scripts/compress_assets.sh
set -euo pipefail

ASSETS="$(dirname "$0")/../assets"

for json_file in "$ASSETS"/fhir_*.json; do
    out="${json_file%.json}.json.zst"
    zstd --force -19 -q "$json_file" -o "$out"
    orig=$(wc -c < "$json_file")
    comp=$(wc -c < "$out")
    ratio=$(awk "BEGIN { printf \"%.1f\", $orig / $comp }")
    printf "%-30s %6d KB → %4d KB  (%.1fx)\n" \
        "$(basename "$out")" \
        $(( orig / 1024 )) \
        $(( comp / 1024 )) \
        "$ratio"
done
