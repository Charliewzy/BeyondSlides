#!/usr/bin/env bash
set -euo pipefail

package=${1:?usage: verify_standalone_linux.sh <standalone-directory>}
package=$(cd "$package" && pwd)
application="$package/beyond-slides"
chromium="$package/chromium/ungoogled-chromium.AppImage"

for required in "$package/BeyondSlides" "$application" "$chromium" "$package/runtime-tools"; do
    [[ -e $required ]] || {
        echo "Missing standalone component: $required" >&2
        exit 1
    }
done

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
    "$package/BeyondSlides"
grep -Fx -- "--app=http://127.0.0.1:$port" "$temporary/browser-arguments" >/dev/null
if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
    echo "The standalone launcher left its controller running after the browser exited." >&2
    exit 1
fi

APPIMAGE_EXTRACT_AND_RUN=1 "$chromium" --version | grep -F 'Chromium 152.0.7977.82' >/dev/null
ffmpeg=$(find "$package/runtime-tools" -type f -name ffmpeg | head -n 1)
ffprobe=$(find "$package/runtime-tools" -type f -name ffprobe | head -n 1)
pdfium=$(find "$package/runtime-tools" -type f -name 'libpdfium.so' | head -n 1)
"$ffmpeg" -version >/dev/null
"$ffprobe" -version >/dev/null
[[ -s $pdfium ]]

rain_port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')
BEYOND_SLIDES_RUNTIME_TOOLS_DIR="$package/runtime-tools" \
    "$application" serve "$temporary/rain-data" "$rain_port" \
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

echo "Standalone launcher, managed runtimes, Chromium, and Rain Classroom browser smoke tests passed."
