# /// script
# requires-python = ">=3.11"
# dependencies = ["httpx>=0.28,<0.29"]
# ///
"""Exercise the actual local server/worker using a deterministic local model.

Requires a built binary, Poppler, and the already-cached dense retrieval model.
No paid model endpoint is used. Artifacts are retained for browser inspection.
"""
import argparse
import io
import json
import subprocess
import threading
import time
import zipfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import httpx


class Model(BaseHTTPRequestHandler):
    calls = 0

    def log_message(self, *_):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["content-length"])))
        task = json.loads(body["messages"][1]["content"])
        Model.calls += 1
        time.sleep(0.5)
        if "output_contract" in task:
            answer = {"spans": [dict(kind="text", source_start=s["id"], source_end=s["id"], text=s["text"]) for s in task["owned_region"]]}
        elif "windows" in task:
            answer = {"windows": [dict(window_index=w["window_index"], boundaries=[dict(after_atom=i, strength="preferred_break") for i in w["owned_boundary_after_atom_ids"]]) for w in task["windows"]]}
        elif "comparisons" in task:
            answer = {"comparisons": [dict(comparison_id=c["comparison_id"], most="A", least=list(c["candidates"])[-1]) for c in task["comparisons"]]}
        else:
            raise AssertionError(f"Unexpected task keys: {list(task)}")
        payload = json.dumps({"id": "local-test", "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": json.dumps(answer, ensure_ascii=False)}}], "usage": {"prompt_tokens": 100, "completion_tokens": 50}}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/beyond-slides"))
    parser.add_argument("--recording", type=Path, help="Exercise real local ASR on this recording instead of uploading a transcript")
    parser.add_argument("--untimed", action="store_true", help="Upload plain text and verify that no recording timeline is invented")
    args = parser.parse_args()
    if args.untimed and args.recording:
        parser.error("--untimed and --recording are mutually exclusive")
    args.output.mkdir(parents=True, exist_ok=False)
    model = ThreadingHTTPServer(("127.0.0.1", 0), Model)
    threading.Thread(target=model.serve_forever, daemon=True).start()
    server = None
    client = None
    job_id = None

    def start_server():
        process = subprocess.Popen([str(args.binary.resolve()), "serve", str(args.output.resolve()), "0"], stdout=subprocess.PIPE, text=True)
        line = process.stdout.readline().strip()
        assert line.startswith("BeyondSlides application: "), line
        return process, httpx.Client(base_url=line.split(": ", 1)[1], timeout=60, headers={"X-BeyondSlides": "local-ui"}, trust_env=False)

    def status():
        result = client.get(f"/api/jobs/{job_id}")
        result.raise_for_status()
        return result.json()

    def wait(predicate, timeout=180):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            value = status()
            if predicate(value):
                return value
            if value["state"] == "failed":
                raise AssertionError(value["error"])
            time.sleep(0.15)
        raise AssertionError(f"Timed out: {status()}")

    try:
        server, client = start_server()
        assert client.get("/").status_code == 200
        assert client.get("/api/jobs", headers={"Host": "attacker.example"}).status_code == 403
        assert client.post("/api/jobs", headers={"Origin": "https://attacker.example"}).status_code == 403
        text = "泛型允许我们用统一的形式表达不同类型上的相同操作。这一段讲解通过编译期检查保证类型安全，同时避免重复实现同样的逻辑。"
        transcript = {"segments": [dict(id=i, start_ms=i * 10000, end_ms=(i + 1) * 10000, text=text) for i in range(24)]}
        files = {
            "slides": ("slides.pdf", Path("tests/fixtures/pdf_import.pdf").read_bytes(), "application/pdf"),
            "transcript": ("transcript.json", json.dumps(transcript, ensure_ascii=False).encode(), "application/json"),
            "recording": ("recording.mp4", Path("tests/fixtures/visual_alignment.mp4").read_bytes(), "video/mp4"),
        }
        if args.recording:
            del files["transcript"]
            files["recording"] = (args.recording.name, args.recording.read_bytes(), "application/octet-stream")
        elif args.untimed:
            files["transcript"] = ("transcript.txt", (text * 24).encode(), "text/plain")
        response = client.post("/api/jobs", data={"name": "本地应用完整流程验证"}, files=files)
        response.raise_for_status()
        job = response.json()
        job_id = job["id"]
        assert job["preview"]["segment_count"] == (0 if args.recording else 48 if args.untimed else 24)
        if args.untimed:
            assert job["preview"]["duration_ms"] is None
        assert status()["state"] == "ready"
        payload = {"api_key": "local-test-secret", "settings": {
            "base_url": f"http://127.0.0.1:{model.server_port}/v1", "model": "local-test-model", "extra_body": None,
            "max_concurrency": 2, "request_interval_ms": 0, "adaptive": False, "boundary_passages": True,
        }}
        response = client.post(f"/api/jobs/{job_id}/start", json=payload)
        response.raise_for_status()
        assert client.post(f"/api/jobs/{job_id}/start", json=payload).status_code == 409
        wait(lambda s: s["usage"]["active_requests"] > 0)
        client.post(f"/api/jobs/{job_id}/stop").raise_for_status()
        paused = wait(lambda s: s["state"] == "paused")
        assert paused["usage"]["responses"] >= 1
        checkpoints = list((args.output / job_id / "run-0001/analysis/restoration").glob("window-*.json"))
        assert checkpoints
        before = {p.name: p.read_bytes() for p in checkpoints}
        asr_checkpoint = args.output / job_id / "transcription/checkpoint.json"
        asr_before = asr_checkpoint.read_bytes() if args.recording else None

        payload["settings"]["request_interval_ms"] = 5
        client.post(f"/api/jobs/{job_id}/start", json=payload).raise_for_status()
        # Restart the controller while its separately-owned worker continues.
        client.close()
        server.terminate()
        server.wait(timeout=10)
        server, client = start_server()
        done = wait(lambda s: s["state"] == "complete")
        if args.recording:
            assert done["job"]["preview"]["segment_count"] > 0
            assert asr_checkpoint.read_bytes() == asr_before
        assert done["usage"]["known_input_tokens"] == Model.calls * 100
        assert done["usage"]["known_output_tokens"] == Model.calls * 50
        assert done["usage"]["active_requests"] == 0
        for filename, content in before.items():
            assert (args.output / job_id / "run-0001/analysis/restoration" / filename).read_bytes() == content
        report = client.get(done["report_url"])
        assert report.status_code == 200 and "data-passage" in report.text
        assert client.get(f"/reports/{job_id}/manifest.json").status_code == 404
        extension = args.recording.suffix if args.recording else ".mp4"
        audio = client.get(f"/reports/{job_id}/report.assets/lecture-audio{extension}", headers={"Range": "bytes=0-31"})
        if args.untimed:
            assert audio.status_code == 404
            assert 'data-time="未提供时间戳"' in report.text
            assert 'data-audio-start-ms=""' in report.text
        else:
            assert audio.status_code == 206 and len(audio.content) == 32
        exported = client.get(f"/api/jobs/{job_id}/export")
        exported.raise_for_status()
        with zipfile.ZipFile(io.BytesIO(exported.content)) as archive:
            assert "report.html" in archive.namelist()
            assert (f"report.assets/lecture-audio{extension}" in archive.namelist()) == (not args.untimed)
            assert all(name in ("report.html", "README.txt") or name.startswith("report.assets/") for name in archive.namelist())
            assert archive.read("report.html") == report.content
        saved = (args.output / job_id / "job.json").read_text()
        assert "local-test-secret" not in saved
        payload["settings"]["boundary_passages"] = False
        changed = client.post(f"/api/jobs/{job_id}/start", json=payload)
        assert changed.status_code == 409 and "keep restoration" in changed.json()["error"]
        summary = {"job_id": job_id, "model_calls": Model.calls, "status": done, "verified": ["import", "preview", "duplicate-start rejection", "graceful stop", "checkpoint resume", "controller restart while worker runs", "token totals", "report and ranged media", "shareable ZIP excludes private run files", "cross-origin rejection", "settings-change confirmation", "key absent from metadata"]}
        if args.recording:
            summary["verified"].append("real CPU ASR and transcription checkpoint reuse")
        (args.output / "verification.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2))
        print(json.dumps({"job_id": job_id, "model_calls": Model.calls, "state": done["state"]}))
    finally:
        if client is not None:
            if job_id:
                try:
                    if status()["state"] in ("running", "stopping"):
                        client.post(f"/api/jobs/{job_id}/stop")
                        wait(lambda s: s["state"] not in ("running", "stopping"), 60)
                except Exception as error:
                    print(f"Cleanup warning: {error}")
            client.close()
        if server is not None and server.poll() is None:
            server.terminate()
            server.wait(timeout=10)
        model.shutdown()


if __name__ == "__main__":
    main()
