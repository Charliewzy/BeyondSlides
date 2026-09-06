# Example lecture report

Open `report.html` directly in a browser, or choose **查看示例报告** in the
application. It is self-contained: slide images, styles and interactions are
embedded, with no API key, uploads, model calls or network assets required.
The application also embeds this HTML in its binary, so the button does not
depend on the checkout or the application's job directory at runtime.

This is an archived result from the real-course Rust lecture used during
development (80 slides), not synthetic scores or a guarantee of model accuracy.
Source run: `run/real_course/passage-quality-20260906/production/analysis.json`.
The full recording is deliberately excluded to avoid adding hundreds of
megabytes to Git. No model traces, credentials or private run configuration
are bundled. Slides retain their original attribution.

## Refreshing the example

With the original development inputs available locally:

```sh
cargo run -- render-analysis \
  run/real_course/transcript.json \
  run/real_course/slides.json \
  run/real_course/passage-quality-20260906/production/analysis.json \
  run/demo-build/report.html \
  --slides-pdf data/real_course/slides.pdf

uv run scripts/bundle_example_report.py \
  run/demo-build/report.html examples/demo/report.html
```

This only renders existing analysis; it does not run inference. The original
inputs are intentionally not required to view the checked-in example. Regenerate
the snapshot when adopting newer reader UI changes.
