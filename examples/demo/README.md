# Example lecture report

The embedded lecture text and slide images are third-party course material,
excluded from the project's MIT license. This demo is retained for the current
course submission; public redistribution still requires permission or replacement.
See [NOTICE](../../NOTICE).

Open `report.html` directly in a browser, or choose **查看示例报告** in the
application. It is self-contained: slide images, styles and interactions are
embedded, with no API key, uploads, model calls or network assets required.
The application also embeds this HTML in its binary, so the button does not
depend on the checkout or the application's job directory at runtime.

This is an archived result from the real-course Rust lecture used during
development (80 slides), not synthetic scores or a guarantee of model accuracy.
It was refreshed from local application job
`672ff94298cb60d180900184eb4a060b`, run 1, on 2026-09-10. The full recording is
deliberately excluded to avoid adding hundreds of megabytes to Git. No model
traces, credentials or private run configuration are bundled. Slides retain
their original attribution.

## Refreshing the example

With the original development inputs available locally:

```sh
cargo run -- render-analysis \
  run/application/672ff94298cb60d180900184eb4a060b/transcript.json \
  run/application/672ff94298cb60d180900184eb4a060b/slides.json \
  run/application/672ff94298cb60d180900184eb4a060b/run-0001/analysis/analysis.json \
  run/demo-build/report.html \
  --slides-pdf run/application/672ff94298cb60d180900184eb4a060b/slides.pdf

uv run scripts/bundle_example_report.py \
  run/demo-build/report.html examples/demo/report.html
```

This only renders existing analysis; it does not run inference. The original
inputs are intentionally not required to view the checked-in example. Regenerate
the snapshot when adopting newer reader UI changes.
