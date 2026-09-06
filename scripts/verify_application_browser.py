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
            page.goto(f"{url}/#{job}")
            page.locator("#open-report").wait_for(state="visible")
            assert page.locator("#run-state").inner_text() == "处理完成"
            assert page.locator("#api-key").input_value() == ""
            page.reload()
            page.locator("#open-report").wait_for(state="visible")
            page.screenshot(path=str(args.workspace / "browser-desktop.png"), full_page=True)
            with page.expect_popup() as opened:
                page.locator("#open-report").click()
            reader = opened.value
            reader.wait_for_load_state()
            assert reader.locator("[data-passage]").count() > 0
            assert reader.locator(".slide-item img").first.evaluate("img => img.complete && img.naturalWidth > 0")
            reader.close()
            with page.expect_download() as download:
                page.locator("#export-report").click()
            assert download.value.failure() is None
            page.set_viewport_size({"width": 390, "height": 844})
            assert page.evaluate("document.documentElement.scrollWidth <= innerWidth")
            page.screenshot(path=str(args.workspace / "browser-mobile.png"), full_page=True)
            page.locator("#new-lecture").click()
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
            assert not errors, errors
            browser.close()
        print("Browser checks passed: reload, reader, export, mobile, source mode, untimed import, no JS errors")
    finally:
        server.terminate()
        server.wait(timeout=10)


if __name__ == "__main__":
    main()
