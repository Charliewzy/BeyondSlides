#!/usr/bin/env bash
set -euo pipefail

repository=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
version=$(awk -F '"' '/^version = "/ { print $2; exit }' "$repository/Cargo.toml")
output=${1:-"$repository/dist/beyond-slides_${version}-1_amd64.deb"}
output_parent=$(dirname "$output")

if [[ $(uname -s) != Linux || $(uname -m) != x86_64 ]]; then
    echo "The current Debian package targets Linux x86-64." >&2
    exit 1
fi
for command in cargo dpkg-deb awk install; do
    command -v "$command" >/dev/null 2>&1 || {
        echo "The package builder requires $command." >&2
        exit 1
    }
done
if [[ -e $output ]]; then
    echo "Refusing to replace existing package output: $output" >&2
    exit 1
fi

mkdir -p "$output_parent"
package_root=$(mktemp -d "$output_parent/.beyond-slides-deb-root.XXXXXX")
browser_work=$(mktemp -d "$output_parent/.beyond-slides-browser.XXXXXX")
deb_work=$(mktemp -d "$output_parent/.beyond-slides-deb-output.XXXXXX")
cleanup() {
    [[ ! -d ${package_root:-} ]] || rm -rf -- "$package_root"
    [[ ! -d ${browser_work:-} ]] || rm -rf -- "$browser_work"
    [[ ! -d ${deb_work:-} ]] || rm -rf -- "$deb_work"
}
trap cleanup EXIT

cd "$repository"
cargo build --release --locked

application="$package_root/opt/beyond-slides"
documentation="$package_root/usr/share/doc/beyond-slides"
mkdir -p \
    "$application/chromium" \
    "$documentation" \
    "$package_root/usr/bin" \
    "$package_root/usr/share/applications" \
    "$package_root/usr/share/icons/hicolor/scalable/apps" \
    "$package_root/etc/apparmor.d" \
    "$package_root/DEBIAN"

install -m 0755 target/release/beyond-slides "$application/beyond-slides"
if command -v strip >/dev/null 2>&1; then
    strip "$application/beyond-slides"
fi
install -m 0755 packaging/linux/BeyondSlides "$application/BeyondSlides"
install -m 0644 packaging/debian/README.Debian "$documentation/README.Debian"
install -m 0644 LICENSE "$documentation/LICENSE.beyond-slides"
install -m 0644 NOTICE "$documentation/NOTICE"
install -m 0644 third_party/chromium.LICENSE "$documentation/LICENSE.chromium"
install -m 0644 third_party/ungoogled-chromium-portablelinux.LICENSE \
    "$documentation/LICENSE.ungoogled-chromium"

"$application/beyond-slides" install-runtime-tools "$application/runtime-tools"
if [[ -n ${BEYOND_SLIDES_CHROMIUM_APPIMAGE:-} ]]; then
    install -m 0755 "$BEYOND_SLIDES_CHROMIUM_APPIMAGE" \
        "$browser_work/ungoogled-chromium.AppImage"
fi
"$application/beyond-slides" install-chromium "$browser_work"
(
    cd "$browser_work"
    ./ungoogled-chromium.AppImage --appimage-extract >/dev/null
)
chromium_source="$browser_work/squashfs-root/opt/ungoogled-chromium"
[[ -x $chromium_source/chrome ]] || {
    echo "The verified browser archive did not contain its Chromium executable." >&2
    exit 1
}
cp -a "$chromium_source/." "$application/chromium/"
# chromedriver is a developer utility and is not used by either application UI
# rendering or Rain Classroom automation.
rm -f -- "$application/chromium/chromedriver"

install -m 0644 packaging/debian/opt.beyond-slides.chromium.chrome \
    "$package_root/etc/apparmor.d/opt.beyond-slides.chromium.chrome"
install -m 0644 packaging/debian/beyond-slides.desktop \
    "$package_root/usr/share/applications/beyond-slides.desktop"
install -m 0644 packaging/debian/beyond-slides.svg \
    "$package_root/usr/share/icons/hicolor/scalable/apps/beyond-slides.svg"
ln -s /opt/beyond-slides/beyond-slides "$package_root/usr/bin/beyond-slides"

install -m 0755 packaging/debian/postinst "$package_root/DEBIAN/postinst"
install -m 0755 packaging/debian/postrm "$package_root/DEBIAN/postrm"
install -m 0644 packaging/debian/conffiles "$package_root/DEBIAN/conffiles"
installed_size=$(du -sk "$package_root" | awk '{ print $1 }')
sed \
    -e "s/@VERSION@/$version/g" \
    -e "s/@INSTALLED_SIZE@/$installed_size/g" \
    packaging/debian/control.in >"$package_root/DEBIAN/control"
(
    cd "$package_root"
    find etc opt usr -type f -print0 | sort -z | xargs -0 md5sum >DEBIAN/md5sums
)

staged_deb="$deb_work/$(basename "$output")"
dpkg-deb --root-owner-group --build -Zxz -z6 "$package_root" "$staged_deb"
mv "$staged_deb" "$output"

echo "Ubuntu package: $output"
du -h "$output"
dpkg-deb --field "$output" Package Version Architecture Installed-Size
