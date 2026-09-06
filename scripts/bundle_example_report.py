"""Bundle a freshly rendered, audio-free BeyondSlides report into one HTML file.

The resulting checked-in example is usable offline and can be embedded in the
application binary. This is for our generated reports, not arbitrary websites.
"""
import argparse
import base64
import html
from pathlib import Path
import re


def bundle(source: Path) -> str:
    document = source.read_text(encoding="utf-8")
    if "<audio" in document:
        raise ValueError("Render the example without --audio before bundling")
    root = source.parent.resolve()

    def inline_image(match):
        path = (root / html.unescape(match[2])).resolve()
        if not path.is_relative_to(root) or path.suffix.lower() != ".png":
            raise ValueError("Example images must be report-local PNGs")
        encoded = base64.b64encode(path.read_bytes()).decode("ascii")
        return f'{match[1]}data:image/png;base64,{encoded}{match[3]}'

    document = re.sub(r'(<img\b[^>]*\bsrc=")([^"]+)(")', inline_image, document)
    document = document.replace("<title>BeyondSlides · 连续讲稿价值分布</title>",
                                "<title>BeyondSlides · 示例报告</title>")
    notice = '<p data-example-notice style="padding:1rem;border:1px solid #d4cbe8;border-radius:.6rem;background:#f4f0fb">示例报告：来自一堂真实的 Rust 讲座，展示已生成的讲稿、幻灯片与评分交互。查看示例不会发起模型请求。本示例未附录音。</p>'
    return document.replace("<main>", f"<main>\n    {notice}", 1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    document = bundle(args.source)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(document, encoding="utf-8")
    print(f"Bundled {args.output} ({args.output.stat().st_size:,} bytes)")


if __name__ == "__main__":
    main()
