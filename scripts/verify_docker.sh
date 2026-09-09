#!/usr/bin/env bash
set -euo pipefail

repository=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
image=${1:-beyond-slides:deployment-smoke}
host_port=${BEYOND_SLIDES_DOCKER_SMOKE_PORT:-18080}
container=beyond-slides-deployment-smoke

cleanup() {
    docker rm -f "$container" >/dev/null 2>&1 || true
}
trap cleanup EXIT
cleanup

if [[ ${BEYOND_SLIDES_SKIP_DOCKER_BUILD:-0} != 1 ]]; then
    docker build --pull=false -t "$image" "$repository"
fi

docker run --rm --detach \
    --name "$container" \
    --publish "127.0.0.1:${host_port}:7842" \
    --volume "$repository/tests/fixtures:/fixtures:ro" \
    "$image" >/dev/null

for _ in {1..30}; do
    if curl --noproxy '*' --fail --silent "http://127.0.0.1:${host_port}/api/jobs" >/dev/null; then
        break
    fi
    sleep 1
done
curl --noproxy '*' --fail --silent "http://127.0.0.1:${host_port}/" >/dev/null

docker exec "$container" sh -c '
    ! command -v ffmpeg >/dev/null
    ! command -v ffprobe >/dev/null
    ! command -v pdftotext >/dev/null
    ! command -v pdftoppm >/dev/null
'

job=$(curl --noproxy '*' --fail --silent \
    --header 'X-BeyondSlides: local-ui' \
    --form 'name=Docker smoke test' \
    --form "slides=@${repository}/tests/fixtures/pdf_import.pdf;type=application/pdf" \
    --form "transcript=@${repository}/examples/tiny_course/transcript.json;type=application/json" \
    --form "recording=@${repository}/tests/fixtures/visual_alignment.mp4;type=video/mp4" \
    "http://127.0.0.1:${host_port}/api/jobs")
python3 -c '
import json, sys
job = json.load(sys.stdin)
assert job["preview"]["slide_count"] == 4, job
assert job["preview"]["segment_count"] == 15, job
assert job["preview"]["recording_duration_ms"] is not None, job
' <<<"$job"

ffmpeg=$(docker exec "$container" sh -c \
    "find /opt/beyond-slides/runtime-tools -type f -name ffmpeg | head -n 1")
docker exec "$container" "$ffmpeg" \
    -nostdin -hide_banner -loglevel error \
    -i /fixtures/visual_alignment.mp4 -f null -

echo "Docker HTTP, PDF, ffprobe, and FFmpeg smoke tests passed on 127.0.0.1:${host_port}."
