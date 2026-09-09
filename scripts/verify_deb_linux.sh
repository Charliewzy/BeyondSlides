#!/usr/bin/env bash
set -euo pipefail

package=${1:?usage: verify_deb_linux.sh <beyond-slides.deb>}
for command in apparmor_parser curl dpkg-deb python3 realpath; do
    command -v "$command" >/dev/null 2>&1 || {
        echo "The package verifier requires $command." >&2
        exit 1
    }
done
[[ -f $package ]] || {
    echo "Debian package does not exist: $package" >&2
    exit 1
}
package=$(realpath "$package")
temporary=$(mktemp -d)
server_pid=
cleanup() {
    if [[ -n $server_pid ]] && kill -0 "$server_pid" 2>/dev/null; then
        kill -INT "$server_pid" 2>/dev/null || true
        wait "$server_pid" 2>/dev/null || true
    fi
    rm -rf -- "$temporary"
}
trap cleanup EXIT

root="$temporary/root"
control="$temporary/control"
mkdir -p "$root" "$control"
dpkg-deb --extract "$package" "$root"
dpkg-deb --control "$package" "$control"

application="$root/opt/beyond-slides"
browser="$application/chromium/chrome"
profile="$root/etc/apparmor.d/opt.beyond-slides.chromium.chrome"
for required in \
    "$application/BeyondSlides" \
    "$application/beyond-slides" \
    "$application/runtime-tools" \
    "$browser" \
    "$profile" \
    "$root/usr/share/applications/beyond-slides.desktop" \
    "$root/usr/share/icons/hicolor/scalable/apps/beyond-slides.svg"; do
    [[ -e $required ]] || {
        echo "Missing Debian-package component: $required" >&2
        exit 1
    }
done
[[ ! -e $application/chromium/ungoogled-chromium.AppImage ]]
[[ ! -e $application/chromium/chromedriver ]]
[[ -x $application/BeyondSlides && -x $application/beyond-slides && -x $browser ]]
[[ $(readlink "$root/usr/bin/beyond-slides") == /opt/beyond-slides/beyond-slides ]]
grep -F '/opt/beyond-slides/chromium/chrome flags=(unconfined)' "$profile" >/dev/null
grep -Fx '  userns,' "$profile" >/dev/null
apparmor_parser -Q -T "$profile"
grep -Fx 'Package: beyond-slides' "$control/control" >/dev/null
grep -Fx 'Architecture: amd64' "$control/control" >/dev/null
grep -F 'Depends: apparmor (>= 4.0)' "$control/control" >/dev/null
grep -F '/etc/apparmor.d/opt.beyond-slides.chromium.chrome' "$control/conffiles" >/dev/null
"$browser" --version | grep -F 'Chromium 152.0.7977.82' >/dev/null

cat >"$temporary/fake-browser" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$@" >"$BEYOND_SLIDES_FAKE_BROWSER_RECORD"
sleep 0.2
EOF
chmod +x "$temporary/fake-browser"
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')
BEYOND_SLIDES_BROWSER_EXECUTABLE="$temporary/fake-browser" \
BEYOND_SLIDES_FAKE_BROWSER_RECORD="$temporary/browser-arguments" \
BEYOND_SLIDES_DATA_DIR="$temporary/launcher-data" \
BEYOND_SLIDES_PORT="$port" \
    "$application/BeyondSlides"
grep -Fx -- "--app=http://127.0.0.1:$port" "$temporary/browser-arguments" >/dev/null
if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
    echo "The Debian launcher left its controller running after the browser exited." >&2
    exit 1
fi

ffmpeg=$(find "$application/runtime-tools" -type f -name ffmpeg | head -n 1)
ffprobe=$(find "$application/runtime-tools" -type f -name ffprobe | head -n 1)
pdfium=$(find "$application/runtime-tools" -type f -name 'libpdfium.so' | head -n 1)
"$ffmpeg" -version >/dev/null
"$ffprobe" -version >/dev/null
[[ -s $pdfium ]]

rain_port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')
BEYOND_SLIDES_RUNTIME_TOOLS_DIR="$application/runtime-tools" \
    "$application/beyond-slides" serve "$temporary/rain-data" "$rain_port" \
    >"$temporary/rain-controller.log" 2>&1 &
server_pid=$!
for _ in {1..100}; do
    if curl --noproxy '*' --fail --silent "http://127.0.0.1:$rain_port/api/jobs" >/dev/null; then
        break
    fi
    sleep 0.1
done
curl --noproxy '*' --fail --silent \
    --max-time 60 \
    --header 'X-BeyondSlides: local-ui' \
    --request POST \
    "http://127.0.0.1:$rain_port/api/rain-classroom/connect" >/dev/null
curl --noproxy '*' --fail --silent \
    --max-time 60 \
    "http://127.0.0.1:$rain_port/api/rain-classroom/login-view" \
    >"$temporary/rain-login.png"
python3 -c 'import pathlib,sys; assert pathlib.Path(sys.argv[1]).read_bytes().startswith(b"\x89PNG\r\n\x1a\n")' \
    "$temporary/rain-login.png"
kill -INT "$server_pid"
wait "$server_pid"
server_pid=

if dpkg-deb --contents "$package" | awk '$6 ~ /^\.\/opt\/|^\.\/etc\/|^\.\/usr\// && $2 != "root/root" { exit 1 }'; then
    :
else
    echo "The Debian package contains application files not owned by root." >&2
    exit 1
fi

echo "Debian layout, AppArmor profile, launcher, runtimes, and Rain Classroom browser smoke tests passed."
