#!/usr/bin/env bash
set -euo pipefail

repository=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
output=${1:-"$repository/dist/beyond-slides-linux-x86_64"}
output_parent=$(dirname "$output")
output_name=$(basename "$output")

if [[ $(uname -s) != Linux || $(uname -m) != x86_64 ]]; then
    echo "The current standalone prototype targets Linux x86-64." >&2
    exit 1
fi
if [[ -e $output || -e ${output}.tar.gz ]]; then
    echo "Refusing to replace existing package output: $output" >&2
    exit 1
fi

mkdir -p "$output_parent"
staging=$(mktemp -d "$output_parent/.${output_name}.XXXXXX")
archive_staging=
cleanup() {
    if [[ -d ${staging:-} ]]; then
        rm -rf -- "$staging"
    fi
    if [[ -n ${archive_staging:-} && -e $archive_staging ]]; then
        rm -f -- "$archive_staging"
    fi
}
trap cleanup EXIT

cd "$repository"
cargo build --release --locked
install -m 0755 target/release/beyond-slides "$staging/beyond-slides"
if command -v strip >/dev/null 2>&1; then
    strip "$staging/beyond-slides"
fi
install -m 0755 packaging/linux/BeyondSlides "$staging/BeyondSlides"
install -m 0644 packaging/linux/README.txt "$staging/README.txt"
install -m 0644 LICENSE "$staging/LICENSE.beyond-slides.txt"
install -m 0644 NOTICE "$staging/NOTICE.txt"

"$staging/beyond-slides" install-runtime-tools "$staging/runtime-tools"
mkdir -p "$staging/chromium"
if [[ -n ${BEYOND_SLIDES_CHROMIUM_APPIMAGE:-} ]]; then
    install -m 0755 "$BEYOND_SLIDES_CHROMIUM_APPIMAGE" \
        "$staging/chromium/ungoogled-chromium.AppImage"
fi
"$staging/beyond-slides" install-chromium "$staging/chromium"

mv "$staging" "$output"
staging=
archive_staging=$(mktemp "$output_parent/.${output_name}.tar.gz.XXXXXX")
tar -C "$output_parent" -czf "$archive_staging" "$output_name"
mv "$archive_staging" "${output}.tar.gz"
archive_staging=

echo "Standalone directory: $output"
echo "Download archive:     ${output}.tar.gz"
du -sh "$output" "${output}.tar.gz"
