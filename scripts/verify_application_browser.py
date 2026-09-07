# /// script
# requires-python = ">=3.11"
# dependencies = ["playwright>=1.50,<2"]
# ///
"""Browser smoke test against a completed local-application verifier workspace."""
import argparse
import json
import subprocess
from pathlib import Path

from playwright.sync_api import sync_playwright


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("workspace", type=Path)
    parser.add_argument("--chromium", type=Path, help="Use an existing Chromium executable")
    args = parser.parse_args()
    job = json.loads((args.workspace / "verification.json").read_text())["job_id"]
    server = subprocess.Popen(
        [str(Path("target/debug/beyond-slides").resolve()), "serve", str(args.workspace.resolve()), "0"],
        stdout=subprocess.PIPE, text=True,
    )
    try:
        line = server.stdout.readline().strip()
        assert line.startswith("BeyondSlides application: "), line
        url = line.split(": ", 1)[1]
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(executable_path=str(args.chromium) if args.chromium else None)
            page = browser.new_page(viewport={"width": 1440, "height": 1000})
            errors = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(url)
            jobs_before_example = page.request.get(f"{url}/api/jobs").json()
            with page.expect_popup() as opened_example:
                page.locator("#open-example").click()
            example = opened_example.value
            example.on("pageerror", lambda error: errors.append(str(error)))
            example.wait_for_load_state()
            assert example.url == f"{url}/example/report.html"
            assert example.locator("[data-example-notice]").is_visible()
            assert example.locator("[data-slide]").count() == 80
            assert example.locator("[data-passage]").count() > 100
            assert example.locator("audio").count() == 0
            example.locator("[data-minimap-prototype]").wait_for(state="visible")
            assert example.locator("[data-slide] img").first.evaluate("image => image.naturalWidth > 0 && image.src.startsWith('data:image/png;base64,')")
            example.locator("#importance-threshold").evaluate("input => { input.value = 6; input.dispatchEvent(new Event('input', {bubbles: true})); }")
            assert example.locator(".importance-emphasized").count() == 0
            assert not example.evaluate("performance.getEntriesByType('resource').some(entry => entry.name.startsWith('http'))")
            example.screenshot(path=str(args.workspace / "browser-example-report.png"))
            example.close()
            assert page.request.get(f"{url}/api/jobs").json() == jobs_before_example
            offline_context = browser.new_context(offline=True, viewport={"width": 1440, "height": 1000})
            offline = offline_context.new_page()
            offline.on("pageerror", lambda error: errors.append(str(error)))
            offline.goto(Path("examples/demo/report.html").resolve().as_uri())
            offline.locator("[data-minimap-prototype]").wait_for(state="visible")
            assert offline.locator("[data-slide] img").first.evaluate("image => image.naturalWidth > 0")
            offline_context.close()
            page.goto(f"{url}/#{job}")
            # Moving from / to /#job is hash-only navigation; reload to exercise
            # opening a saved job URL as a fresh application entry.
            page.reload()
            page.locator("#open-report").wait_for(state="visible")
            assert page.locator("#run-state").inner_text() == "处理完成"
            assert page.locator("#api-key").input_value() == ""
            disclosure = page.locator("#provider-panel .disclosure")
            assert disclosure.locator("strong").is_visible()
            assert "建议尝试关闭模型的思考模式" in disclosure.locator("strong").inner_text()
            assert "GLM-5" in disclosure.inner_text()
            assert json.loads(disclosure.locator("code").inner_text()) == {"thinking": {"type": "disabled"}}
            assert not page.locator("#start-form details").evaluate("element => element.open")
            page.reload()
            page.locator("#open-report").wait_for(state="visible")
            # Existing jobs need no migration or new analysis to review sources.
            page.get_by_text("检查导入内容", exact=True).click()
            review_card = page.get_by_role("button", name="检查第 3 页", exact=True)
            review_card.wait_for(state="visible")
            page.wait_for_function("document.querySelector('.review-card img')?.naturalWidth > 0")
            assert "仅提取到 0 个字符" in review_card.inner_text()
            assert page.request.get(f"{url}/api/jobs/{job}/slide-review/0").status == 404
            assert page.request.get(f"{url}/api/jobs/{job}/slide-review/1").status == 404
            review_card.click()
            assert page.locator("#review-dialog").is_visible()
            assert page.locator("#review-page-title").inner_text() == "第 3 页"
            assert page.locator("#review-page-text").inner_text() == "（未提取到文字）"
            page.wait_for_function("document.getElementById('review-page-image').naturalWidth > 0")
            page.screenshot(path=str(args.workspace / "browser-slide-review.png"), full_page=True)
            page.keyboard.press("Escape")
            assert not page.locator("#review-dialog").is_visible()
            page.get_by_text("检查导入内容", exact=True).click()
            page.locator("#debug-panel summary").click()
            page.wait_for_function("document.getElementById('debug-status').textContent.length > 0")
            assert "local-test-secret" not in page.locator("#debug-output").inner_text()
            if page.locator("#debug-download").is_visible():
                with page.expect_download() as log_download:
                    page.locator("#debug-download").click()
                assert log_download.value.failure() is None
            else:
                assert "尚无" in page.locator("#debug-status").inner_text()
            # Deterministic UI-only cases: escaping, bounded-tail notice,
            # scrolling pauses display, and follow reconnects to latest output.
            log_text = ["<script>window.logExecuted = true</script>\n" + "debug line\n" * 200]
            page.route("**/api/jobs/*/logs/worker", lambda route: route.fulfill(json={"text": log_text[0], "truncated": True, "available": True}))
            page.wait_for_function("document.getElementById('debug-output').textContent.includes('<script>')")
            assert page.evaluate("window.logExecuted === undefined")
            page.locator("#debug-output").evaluate("element => { element.scrollTop = 0; }")
            page.wait_for_function("!document.getElementById('debug-follow').checked")
            frozen = page.locator("#debug-output").inner_text()
            log_text[0] += "new output after pause\n"
            page.wait_for_timeout(1200)
            assert page.locator("#debug-output").inner_text() == frozen
            page.locator("#debug-follow").check()
            page.wait_for_function("document.getElementById('debug-output').textContent.includes('new output after pause')")
            page.unroute("**/api/jobs/*/logs/worker")
            page.locator("#debug-kind").select_option("transcription")
            page.wait_for_timeout(1200)
            assert page.locator("#debug-status").inner_text()
            page.locator("#debug-kind").select_option("worker")
            page.screenshot(path=str(args.workspace / "browser-desktop.png"), full_page=True)
            # UI-only progress cases: a weighted recognition percentage must
            # not be confused with an overall job percentage or model loading.
            status = page.request.get(f"{url}/api/jobs/{job}").json()
            observed = dict(observer_version=1, phase="recognizing", attempt_started_ms=0,
                            completed_regions=1, total_regions=3, completed_speech_ms=1000,
                            total_speech_ms=4000, timings_seconds={}, reused=False)
            status.update(state="running", transcription=observed)
            status["job"]["transcribe_recording"] = True
            status["progress"]["stages"]["transcription"] = dict(completed=0, total=None)
            status["progress"]["current"] = "passages"
            status["progress"]["stages"]["passages"] = dict(completed=21, total=147, elapsed_ms=397000, eta_ms=613000, reused=False)
            status["job"]["preview"]["warnings"] = ["SparseText { page: 3, non_whitespace_characters: 5 }", "SparseText { page: 11, non_whitespace_characters: 7 }"]
            page.route(f"**/api/jobs/{job}", lambda route: route.fulfill(json=status))
            page.wait_for_function("document.getElementById('stage-progress').textContent.includes('识别语音 · 25%')")
            bar = page.get_by_role("progressbar", name="本地 CPU 转写")
            assert bar.get_attribute("value") == "1000"
            assert bar.get_attribute("max") == "4000"
            assert "正在采样语音识别速度" in page.locator("#stage-progress").inner_text()
            observed["recognition_eta_ms"] = 123000
            page.wait_for_function("document.getElementById('stage-progress').textContent.includes('预计语音识别剩余 2分03秒')")
            assert "不含后续标点与保存" in page.locator("#stage-progress").inner_text()
            assert "预计剩余 10分13秒" in page.locator("#stage-progress").inner_text()
            assert "预计阶段总用时 16分50秒" in page.locator("#stage-progress").inner_text()
            page.get_by_text("检查导入内容", exact=True).click()
            assert "3（5 字符）、11（7 字符）" in page.locator("#source-warnings").inner_text()
            assert "SparseText" not in page.locator("#source-warnings").inner_text()
            assert "非模型重试" in page.evaluate("readableLog('--- attempt 1788696565049 ---')")
            page.screenshot(path=str(args.workspace / "browser-stage-timing.png"), full_page=True)
            observed["phase"] = "loading_models"
            page.wait_for_function("document.querySelector('progress[aria-label=\"本地 CPU 转写\"]').getAttribute('value') === null")
            assert "预计语音识别剩余" not in page.locator("#stage-progress").inner_text()
            observed["phase"] = "punctuating"
            page.wait_for_function("document.getElementById('stage-progress').textContent.includes('添加标点')")
            assert "预计语音识别剩余" not in page.locator("#stage-progress").inner_text()
            status["state"] = "paused"
            page.wait_for_function("document.getElementById('run-state').textContent === '已暂停，可继续'")
            page.wait_for_function("document.querySelector('progress[aria-label=\"本地 CPU 转写\"]').value === 0")
            assert "预计剩余" not in page.locator("#stage-progress").inner_text()
            page.unroute(f"**/api/jobs/{job}")
            page.wait_for_function("document.getElementById('run-state').textContent === '处理完成'")
            with page.expect_popup() as opened:
                page.locator("#open-report").click()
            reader = opened.value
            reader.wait_for_load_state()
            assert reader.locator("[data-passage]").count() > 0
            assert reader.locator(".slide-item img").first.evaluate("img => img.complete && img.naturalWidth > 0")
            assert "prototype=" not in reader.url
            reader.locator("[data-minimap-prototype]").wait_for(state="visible")
            assert reader.locator(".minimap-prototype-passage").count() == reader.locator("[data-passage]").count()
            reader.set_viewport_size({"width": 390, "height": 844})
            assert reader.locator("[data-minimap-prototype]").is_hidden()
            reader.close()
            with page.expect_download() as download:
                page.locator("#export-report").click()
            assert download.value.failure() is None
            page.set_viewport_size({"width": 390, "height": 844})
            assert page.evaluate("document.documentElement.scrollWidth <= innerWidth")
            page.screenshot(path=str(args.workspace / "browser-mobile.png"), full_page=True)
            page.locator("#new-lecture").click()
            page.route("**/api/rain-classroom/connect", lambda route: route.fulfill(json={"opened": True}))
            page.route("**/api/rain-classroom/courses", lambda route: route.fulfill(json=[{
                "classroom_id": 3195306, "course_name": "程序设计训练", "classroom_name": "Rust 语言",
            }]))
            page.route("**/api/rain-classroom/courses/3195306/lectures", lambda route: route.fulfill(json=[{
                "lesson_id": "1765600343592779520", "title": "⑥并发编程",
            }]))
            page.locator("#source-mode").select_option("rain")
            assert page.locator("#rain-classroom-import").is_visible()
            assert page.locator('[name="transcript"]').is_disabled()
            assert page.locator('[name="recording"]').is_disabled()
            page.locator("#rain-authenticated").wait_for(state="visible")
            assert page.locator("#rain-connect").is_hidden()
            page.locator("#rain-course").select_option("3195306")
            page.locator("#rain-lecture").select_option("1765600343592779520")
            assert page.locator('[name="name"]').input_value() == "⑥并发编程"
            page.evaluate("renderRainDownloadProgress({phase: 'downloading', downloaded_bytes: 26214400, total_bytes: 104857600})")
            assert page.locator("#rain-download").is_visible()
            assert page.locator("#rain-download-percent").inner_text() == "25%"
            assert page.locator("#rain-download-bytes").inner_text() == "25.0 MiB / 100.0 MiB"
            assert page.locator("#rain-download-bar").evaluate("progress => progress.value / progress.max") == 0.25
            page.evaluate("stopRainDownloadProgress()")
            assert page.locator("#rain-download").is_hidden()
            page.unroute("**/api/rain-classroom/connect")
            page.unroute("**/api/rain-classroom/courses")
            page.unroute("**/api/rain-classroom/courses/3195306/lectures")
            page.locator("#source-mode").select_option("recording")
            assert page.locator('[name="transcript"]').is_disabled()
            assert page.locator('[name="recording"]').evaluate("input => input.required")
            page.locator("#source-mode").select_option("transcript")
            page.locator('[name="name"]').fill("浏览器无时间戳导入检查")
            page.locator('[name="slides"]').set_input_files("tests/fixtures/pdf_import.pdf")
            page.locator('[name="transcript"]').set_input_files({
                "name": "lecture.txt", "mimeType": "text/plain", "buffer": "泛型帮助我们表达共同的操作。借用避免不必要的数据复制。".encode(),
            })
            page.locator("#import-button").click()
            page.locator("#workspace").wait_for(state="visible")
            assert "无转写时间戳" in page.locator("#lecture-meta").inner_text()
            assert "借用" in page.locator("#transcript-preview").inner_text()
            assert page.locator("#start-button").is_visible()
            page.get_by_role("button", name="检查第 3 页", exact=True).wait_for(state="visible")
            page.get_by_role("button", name="检查第 3 页", exact=True).click()
            assert page.locator("#review-dialog").is_visible()
            assert page.locator("#review-dialog").evaluate("element => element.scrollWidth <= element.clientWidth")
            page.locator("#review-close").click()
            # UI-only many-page case verifies horizontal navigation on mobile.
            preview_png = page.request.get(f"{url}/api/jobs/{job}/slide-review/3").body()
            page.route("**/slide-review/*", lambda route: route.fulfill(content_type="image/png", body=preview_png))
            page.route("**/slide-review", lambda route: route.fulfill(json=[dict(page=i, text="检查文字", warnings=["仅提取到 4 个字符"]) for i in range(1, 13)]))
            page.evaluate("reviewLoaded = false; loadSlideReview()")
            page.wait_for_function("document.querySelectorAll('.review-card').length === 12")
            page.locator("#review-right").click()
            page.wait_for_function("document.getElementById('review-strip').scrollLeft > 0")
            assert page.evaluate("document.documentElement.scrollWidth <= innerWidth")
            page.screenshot(path=str(args.workspace / "browser-slide-review-mobile.png"), full_page=True)
            page.unroute("**/slide-review")
            page.route("**/slide-review", lambda route: route.fulfill(status=503, json={"error": "Temporary preview failure"}))
            page.evaluate("reviewLoaded = false; loadSlideReview()")
            page.locator("#review-retry").wait_for(state="visible")
            assert page.locator("#start-button").is_enabled()
            page.unroute("**/slide-review")
            page.route("**/slide-review", lambda route: route.fulfill(json=[]))
            page.locator("#review-retry").click()
            page.wait_for_function("document.getElementById('review-status').textContent.includes('未发现')")
            assert page.locator(".review-card").count() == 0
            assert not errors, errors
            browser.close()
        print("Browser checks passed: reload, debug logs/download/escaping/follow, reader, export, mobile, source mode, untimed import, no JS errors")
    finally:
        server.terminate()
        server.wait(timeout=10)


if __name__ == "__main__":
    main()
