# BeyondSlides

把一堂课，变成可探索的内容。BeyondSlides 将中文课堂转写与幻灯片结合，帮助你找到老师在讲义之外补充的解释、例子与提醒。

这是一个**本地运行的课程项目 / 预览版**，不是多用户云服务。录音转写和幻灯片检索在本机 CPU 上运行；文本恢复、语义分段和重要性 / 新颖度比较使用你提供的 OpenAI-compatible 模型 API。

## 先看效果

下载本仓库后，用浏览器打开 [examples/demo/report.html](examples/demo/report.html)，或在应用侧栏点击 **查看示例报告**。

示例包含一堂 Rust 课程的 80 页幻灯片和完整分析文本，无需 API key、模型下载或分析等待。图片已嵌入 HTML，可离线打开；**不包含录音**。这是一次真实分析的存档，不代表所有讲座都能达到相同效果。来源见 [示例说明](examples/demo/README.md)。

## 可以做什么

- 分别选择书面材料和课堂内容：幻灯片可上传 PDF 或从雨课堂导入；讲述内容可上传转写、上传录音 / 视频，或从雨课堂导入录像。支持 JSON、FunASR 时间戳 TSV、SRT、VTT 和 UTF-8 TXT。
- 使用本地 CPU 将录音转写，再恢复标点和连贯表达，保留原始文本的来源范围。
- 比较全讲座中的段落，分别评估重要性与相对幻灯片的新颖度。
- 开始、停止继续派发任务、恢复处理；查看阶段进度、预计剩余时间、token 用量和调试日志。
- 在连续文本中用粗体表示重要性、下划线表示新颖度，自行调整显示阈值。
- 文本与幻灯片双向定位，配合讲座概览条浏览；有可靠时间戳和录音时可按段落播放音频。
- 导出可分享的 HTML、幻灯片图片及可用录音 ZIP，无需让朋友重新运行模型。

重要性是**同一讲座内部的相对排名**，新颖度也是结合幻灯片证据的相对判断；它们不是客观分数，也不适合跨讲座比较。幻灯片位置由文本检索和序列对齐推断，并非从视频中观察得到。

## 安装与启动

目前主要在 **Linux / WSL** 上开发和验证。尚未提供一键安装包；macOS、原生 Windows 的完整流程尚未验证。

### 1. 安装基础依赖

需要 Rust / Cargo、C/C++ 构建工具，以及 Poppler。附带录音或从视频转写时还需要 FFmpeg（包含 `ffprobe`）。Ubuntu / Debian 示例：

```sh
sudo apt-get update
sudo apt-get install build-essential pkg-config libssl-dev poppler-utils ffmpeg
```

Rust 可通过 [rustup](https://rustup.rs/) 安装；最低支持版本与 CI 使用的版本为 1.94.0。首次构建会下载 Rust 依赖和 ONNX Runtime；首次分析还可能下载本地中文检索模型 `BAAI/bge-small-zh-v1.5`。

```sh
git clone https://github.com/Charliewzy/BeyondSlides.git
cd BeyondSlides
cargo build --release --locked
./target/release/beyond-slides serve
```

打开 **http://127.0.0.1:7842**。请从仓库目录启动；默认数据目录是当前目录下的 `run/application/`。

也可以指定数据目录和端口：

```sh
./target/release/beyond-slides serve run/my-lectures 7842
```

### 2. 可选：启用本地录音转写

**已有转写文件时可以跳过这一节。** 从录音 / 视频转写需要 Python 3.11、CPU PyTorch 和 FunASR。安装 [uv](https://docs.astral.sh/uv/getting-started/installation/) 后，在仓库根目录运行：

```sh
uv venv --python 3.11 .venv
uv pip install --python .venv/bin/python torch torchaudio --index-url https://download.pytorch.org/whl/cpu
uv pip install --python .venv/bin/python funasr==1.4.5
```

如果已有 `.venv`，无需重新创建。程序会优先使用仓库的 `.venv`；也可在启动服务器前设置 `BEYOND_SLIDES_ASR_PYTHON`，指定另一环境的 Python 可执行文件。

转写使用 SenseVoiceSmall、语音活动检测及标点模型，首次使用可能下载权重。**不需要 GPU**。速度取决于 CPU、录音长度和模型加载情况；LLM 分析还受服务商速度、限流和思考模式影响，不保证固定完成时间。

从雨课堂导入还需要本机安装 Google Chrome 或 Chromium。BeyondSlides 会在后台使用独立的浏览器配置，并把扫码登录页面显示在应用自己的对话框中；登录状态有效时会自动复用，无需每次扫码。可以分别选择一份雨课堂课件和一堂课的录像，两者不必来自同一讲次。若雨课堂在后台将一堂课存为多份回放，BeyondSlides 会自动合并为一份录像，不要求用户选择或理解这些内部片段。雨课堂目前只提供逐页图片，因此导入的课件会组装为图片型 PDF；现阶段 `pdftotext` 通常提取不到有用文字，后续需要 OCR。此功能使用雨课堂网页自身的私有接口，网站更新后可能需要同步适配。

### 3. 导入并分析

1. 点击 **导入讲座**，分别选择“书面材料”和“课堂内容”的来源。前者可上传 PDF 或选择雨课堂课件；后者可上传转写、上传录音 / 视频并本地转写，或选择雨课堂录像。
2. 检查第一页文字、转写预览和建议检查的幻灯片。图片页或标题页文字少并不一定是错误。
3. 填入 API base URL、服务商提供的准确模型名称和 API key。
4. 点击 **开始分析**，完成后打开阅读报告。

例如使用课程提供的 GLM-5 服务时：

```text
API base URL: https://lab.cs.tsinghua.edu.cn/ai-platform/api/v1
模型名称: glm-5
```

需要自行取得有权限的 API key；该服务不是本项目提供的公共接口。为加快处理，可尝试在 **高级设置 → 服务商额外请求字段** 中填写：

```json
{"thinking":{"type":"disabled"}}
```

这是 GLM-5 的示例，**不是所有模型 / 服务商通用的设置**。本应用不会默认替你关闭思考。模型 API 可能产生费用；页面 token 用量随已完成响应更新，不是实时费用估算。

## 停止、恢复与分享

- **关闭浏览器或重启网页服务器不会停止已运行的后台 worker。** 恢复网页连接时使用同一个数据目录。
- 要暂停处理，点击 **完成当前任务后停止**。已经开始的请求会继续完成并保存；本地转写需要等当前整段录音处理结束。
- 恢复时重新输入 API key。已完成结果经过校验后复用；并发 / 请求间隔变化不需要重做模型分析。更改模型或语义设置时，应用会提示需要重新处理的阶段。
- 硬中断会丢失尚未保存的工作；录音转写尚未生成完整检查点时需要重新转写。
- **下载分享包（含录音）** 后，先解压整个 ZIP，再打开 `report.html`。保持 `report.assets/` 与 HTML 相邻；只发送 HTML 会丢失外部图片 / 音频。无时间戳的转写不提供按段落播放，也不导出未使用的录音。

## 隐私与限制

- 原始 PDF、录音和视频保存在本地，不作为附件上传给分析模型；**转写和幻灯片文字会发送到你配置的模型端点**。
- 雨课堂登录信息保存在应用数据目录下的独立浏览器配置中；短期签名的录像与课件页面地址不会写入讲座配置。请像保护普通浏览器登录一样保护该数据目录。
- API key 不写入讲座配置或浏览器持久存储，但会传给本地后台进程。调试日志有凭据过滤，不等于自动清除所有敏感内容。
- 本地检查点、模型请求 / 响应 traces 和日志可能包含完整课程文本。默认 `run/`、`data/` 被 Git 忽略；不要把自己的数据目录提交到仓库。
- 新写入的模型 traces 和服务商错误会过滤配置的 API key（含 JSON / URL 编码形式）。修复前的 traces 不会自动清理；不要分享原始 traces。过滤不保证识别任意混淆方式或其他秘密。
- 服务仅绑定 loopback，没有多用户鉴权。**不要直接通过反向代理或端口转发将它公开到互联网。**
- PDF 目前依赖可提取文字，不自动 OCR 图片型幻灯片；检查警告不能保证提取内容完整。
- ASR、文本恢复、分段、排名和幻灯片对齐都可能出错；音频定位可能退回较粗的转写区间。请对重要内容回看原始讲义 / 录音。
- 软件使用 [MIT 许可证](LICENSE)。内置示例的课程文字和幻灯片不在 MIT 授权范围内；按项目所有者决定，为当前课程提交保留。公开发布前仍需确认授权或替换示例，见 [NOTICE](NOTICE)。

## 开发与验证

```sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features -- --test-threads=1
uv run --isolated --python 3.11 --with-requirements scripts/tests/requirements.txt python -m unittest discover -s scripts/tests
```

GitHub Actions 在 `main` 推送和 pull request 上运行上述四项检查，也支持手动触发。CI 不运行付费模型、完整 ASR 或浏览器测试。PDF / 视频相关测试需要 Poppler 和 FFmpeg；下载中文检索模型的测试默认忽略，可单独运行 `cargo test --test retrieval -- --ignored`。浏览器与本地应用端到端检查使用真实服务器 / worker 和本地模拟模型，不产生付费 API 请求：

```sh
cargo build --locked
# 输出目录必须是尚不存在的新目录。
uv run scripts/verify_local_application.py run/release-check
uv run --with playwright python -m playwright install chromium
uv run scripts/verify_application_browser.py run/release-check
```

运行验证脚本前先完成构建；不要在脚本运行过程中替换其使用的二进制。端到端验证需要已缓存的中文检索模型；详细要求见 [本地应用说明](docs/local-application.md#verification)。模拟模型测试验证流程，不验证真实模型的判断质量。

## 项目导航

- [本地应用：配置、数据与恢复机制](docs/local-application.md)
- [命令行分析与检查点说明](docs/running.md)
- [架构](ARCHITECTURE.md)、[领域词汇](CONTEXT.md)、[架构决策](docs/adr/)
- [实验与评估记录](docs/evaluation/)
- [发布就绪检查与待解决问题](docs/release-readiness.md)
- `src/`：Rust 流程、验证、模型通信、检索和本地服务器
- `templates/`：应用与报告的 HTML / CSS / JavaScript
- `prompts/`：文本恢复、分段和比较任务的模型指令
- `scripts/`：CPU 转写桥接、实验与验证工具
- `tests/`、`examples/`：回归测试、固定样例与离线演示
