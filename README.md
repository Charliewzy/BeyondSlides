# BeyondSlides

把一堂课，变成可探索的内容。BeyondSlides 将中文课堂转写与幻灯片结合，帮助你找到老师在讲义之外补充的解释、例子与提醒。

这是一个**本地运行的课程项目 / 预览版**，不是多用户云服务。录音转写和幻灯片检索在本机 CPU 上运行；文本恢复、语义分段和重要性 / 新颖度比较可使用你提供的 OpenAI-compatible 模型 API，或复用本机 Codex CLI 的 ChatGPT 登录。

## 先看效果

下载本仓库后，用浏览器打开 [examples/demo/report.html](examples/demo/report.html)，或在应用侧栏点击 **查看示例报告**。

示例包含一堂 Rust 课程的 80 页幻灯片和完整分析文本，无需 API key、模型下载或分析等待。图片已嵌入 HTML，可离线打开；**不包含录音**。这是一次真实分析的存档，不代表所有讲座都能达到相同效果。来源见 [示例说明](examples/demo/README.md)。

## 可以做什么

- 分别选择书面材料和课堂内容：幻灯片可上传 PDF 或从雨课堂导入；讲述内容可上传转写、上传录音 / 视频，或从雨课堂导入录像。支持 JSON、SRT、VTT 和 UTF-8 TXT。
- 在本机 CPU 上识别雨课堂的图片型课件，并保留原 PDF 作为可视书面来源。
- 使用本地 CPU 将录音转写，再恢复标点和连贯表达，保留原始文本的来源范围。
- 比较全讲座中的段落，分别评估重要性与相对幻灯片的新颖度。
- 开始、停止继续派发任务、恢复处理；查看阶段进度、预计剩余时间、token 用量和调试日志。
- 在连续文本中用粗体表示重要性、下划线表示新颖度，自行调整显示阈值。
- 文本与幻灯片双向定位，配合讲座概览条浏览；有可靠时间戳和录音时可按段落播放音频。
- 导出可分享的 HTML、幻灯片图片及可用录音 ZIP，无需让朋友重新运行模型。

重要性是**同一讲座内部的相对排名**，新颖度也是结合幻灯片证据的相对判断；它们不是客观分数，也不适合跨讲座比较。幻灯片位置由文本检索和序列对齐推断，并非从视频中观察得到。

## 安装与启动

目前主要在 **Linux / WSL** 上开发和验证。仓库可以生成一个 Linux x86-64 独立预览包；macOS、原生 Windows 的完整流程尚未验证。

### Linux 独立预览包

发布者可用一个命令构建可下载的完整目录与压缩包：

```sh
scripts/package_standalone_linux.sh
```

解压 `dist/beyond-slides-linux-x86_64.tar.gz` 后，双击或运行其中的
`BeyondSlides`。启动器会运行本地服务、用随包附带的 Chromium 打开应用窗口，并在窗口关闭后干净地关闭控制服务。讲座、检查点、登录状态和首次下载的模型默认保存在 `~/.local/share/beyond-slides`，所以以后替换程序目录不会删除数据。

本机验证生成的目录约 **424 MiB**，压缩下载约 **283 MiB**；具体大小会随 Rust 和运行时版本变化。包内含 BeyondSlides、完整的 ungoogled-Chromium AppImage、PDFium、FFmpeg、`ffprobe` 与许可证，不要求另行安装这些组件。OCR、ASR 和嵌入模型仍在第一次需要时校验下载。当前包面向具有图形桌面的现代 glibc Linux x86-64；详情和验证命令见 [独立应用说明](docs/standalone.md)。

### 1. 从源码安装

从源码构建只需要 Rust / Cargo 和 C/C++ 构建工具。PDFium、FFmpeg 与
`ffprobe` 由 BeyondSlides 按固定版本管理，不需要通过系统包管理器安装。
Ubuntu / Debian 示例：

```sh
sudo apt-get update
sudo apt-get install build-essential pkg-config
```

Rust 可通过 [rustup](https://rustup.rs/) 安装；最低支持版本与 CI 使用的版本为 1.94.0。首次处理 PDF 或媒体时，会下载并校验约 63 MiB 的 PDFium、FFmpeg 和 `ffprobe` 压缩资源，解压后缓存约 167 MiB。首次构建会下载 Rust 依赖和 ONNX Runtime；首次分析还可能下载本地中文检索模型 `BAAI/bge-small-zh-v1.5`。首次导入雨课堂课件会下载并校验约 21 MiB 的 PP-OCRv5 模型，之后直接复用。

```sh
git clone https://github.com/Charliewzy/BeyondSlides.git
cd BeyondSlides
cargo build --release --locked
./target/release/beyond-slides serve
```

默认情况下，托管运行时写入操作系统缓存。制作离线包时可提前安装到
可执行文件旁边；应用会优先使用该目录：

```sh
./target/release/beyond-slides install-runtime-tools target/release/runtime-tools
```

打开 **http://127.0.0.1:7842**。请从仓库目录启动；默认数据目录是当前目录下的 `run/application/`。

也可以指定数据目录和端口：

```sh
./target/release/beyond-slides serve run/my-lectures 7842
```

### Docker

Docker 镜像同样从仓库源码编译，并预装项目锁定版本的 PDFium、FFmpeg 和
`ffprobe`；运行镜像不通过系统包管理器安装 FFmpeg 或 Poppler：

```sh
docker build --pull=false -t beyond-slides .
docker volume create beyond-slides-data
docker run --rm --name beyond-slides \
  -p 127.0.0.1:80:7842 \
  -v beyond-slides-data:/data \
  beyond-slides
```

打开 **http://127.0.0.1/**。容器内监听 `0.0.0.0:7842`，因此宿主端口可以任意映射；上例特意验证了 `80:7842`，并只向宿主 loopback 发布。模型、登录资料、讲座与检查点都保存在命名卷中。完整的端口、可信来源及冒烟测试说明见 [部署说明](docs/deployment.md)。默认镜像不含可选的 Chromium，因此雨课堂导入在该镜像中不可用。

### 2. 可选：启用本地录音转写

**已有转写文件时可以跳过这一节。** 录音 / 视频转写由 Rust 内的 sherpa-onnx、INT8 SenseVoiceSmall 和 Silero VAD 在本机 CPU 上完成，不需要 Python、PyTorch、GPU 或系统安装的 FFmpeg。BeyondSlides 使用经过完整性校验的托管 FFmpeg 提取 16 kHz 单声道音频。

首次转写会自动下载并校验约 156 MiB 的压缩模型资源；解压后的模型缓存约 230 MiB，位于应用数据目录的 `models/` 下，之后的讲座会直接复用。速度取决于 CPU、录音长度和模型加载情况；LLM 分析还受服务商速度、限流和思考模式影响，不保证固定完成时间。

源码运行或默认 Docker 镜像从雨课堂导入时，还需要本机提供 Google Chrome 或 Chromium；Linux 独立预览包已自带固定版本。BeyondSlides 会在后台使用独立的浏览器配置，并把扫码登录页面显示在应用自己的对话框中；登录状态有效时会自动复用，无需每次扫码。课件和录像选择会绑定到同一门课程、同一讲次，并分别提示该讲次是否缺少可导入的课件或录像；只有存在多份课件时才要求选择。若雨课堂在后台将一堂课存为多份回放，BeyondSlides 会自动合并为一份录像，不要求用户选择或理解这些内部片段。雨课堂逐页图片会组装为图片型 PDF，并由本机 CPU 上的 PP-OCRv5 提取文字；无需 Python、PaddlePaddle、GPU 或在线 OCR 服务。此功能使用雨课堂网页自身的私有接口，网站更新后可能需要同步适配。

### 3. 导入并分析

1. 点击 **导入讲座**，分别选择“书面材料”和“课堂内容”的来源。前者可上传 PDF 或选择雨课堂课件；后者可上传转写、上传录音 / 视频并本地转写，或选择雨课堂录像。
2. 检查第一页文字、转写预览和建议检查的幻灯片。图片页或标题页文字少并不一定是错误。
3. 选择模型连接方式。OpenAI-compatible 模式需填写 API base URL、准确模型名称和 API key；Codex 模式需先安装 Codex CLI 并运行 `codex login`，应用随后会读取账号可用的模型、推理强度和 Fast 模式，无需 API key。
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

- 原始 PDF、录音和视频保存在本地，不作为附件上传给分析模型；**转写和幻灯片文字会发送到你配置的 API，或由本机 Codex 转发给 OpenAI**。
- Codex 模式复用 Codex CLI 自己缓存的登录，不读取或保存 ChatGPT 凭据。模型任务在空临时目录、只读沙箱中运行；若 Codex 仍尝试调用工具，BeyondSlides 会终止该任务。
- 雨课堂登录信息保存在应用数据目录下的独立浏览器配置中；短期签名的录像与课件页面地址不会写入讲座配置。请像保护普通浏览器登录一样保护该数据目录。
- API key 不写入讲座配置或浏览器持久存储，但会传给本地后台进程。调试日志有凭据过滤，不等于自动清除所有敏感内容。
- 本地检查点、模型请求 / 响应 traces 和日志可能包含完整课程文本。默认 `run/`、`data/` 被 Git 忽略；不要把自己的数据目录提交到仓库。
- 新写入的模型 traces 和服务商错误会过滤配置的 API key（含 JSON / URL 编码形式）。修复前的 traces 不会自动清理；不要分享原始 traces。过滤不保证识别任意混淆方式或其他秘密。
- 源码启动默认只绑定 loopback；Docker 镜像在容器内绑定 `0.0.0.0`，但示例只向宿主 loopback 发布。服务没有多用户鉴权，**不要直接通过反向代理或端口转发将它公开到互联网。**
- 本地上传的 PDF 目前仍依赖可提取文字；PP-OCRv5 只自动处理雨课堂导入的逐页图片。检查警告与 OCR 都不能保证提取内容完整。
- ASR、文本恢复、分段、排名和幻灯片对齐都可能出错；音频定位可能退回较粗的转写区间。请对重要内容回看原始讲义 / 录音。
- 软件使用 [MIT 许可证](LICENSE)。托管的 FFmpeg/ffprobe 是独立的 GPLv3 程序，预装包会同时保留上游许可证与构建说明；PDFium 和独立包中的 ungoogled-Chromium 也保留各自的上游许可证与来源。内置示例的课程文字和幻灯片不在 MIT 授权范围内；按项目所有者决定，为当前课程提交保留。公开发布前仍需确认授权或替换示例，见 [NOTICE](NOTICE)。

## 开发与验证

```sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features -- --test-threads=1
uv run --isolated --python 3.11 --with-requirements scripts/tests/requirements.txt python -m unittest discover -s scripts/tests
```

GitHub Actions 在 `main` 推送和 pull request 上运行上述四项检查，也支持手动触发。CI 不运行付费模型、完整 ASR 或浏览器测试。PDF / 视频测试会使用与应用相同的托管 PDFium、FFmpeg 和 `ffprobe`；下载中文检索模型的测试默认忽略，可单独运行 `cargo test --test retrieval -- --ignored`。浏览器与本地应用端到端检查使用真实服务器 / worker 和本地模拟模型，不产生付费 API 请求：

```sh
cargo build --locked
# 输出目录必须是尚不存在的新目录。
uv run scripts/verify_local_application.py run/release-check
uv run --with playwright python -m playwright install chromium
uv run scripts/verify_application_browser.py run/release-check
# 构建独立包后，验证启动、关闭、运行时和雨课堂浏览器：
scripts/verify_standalone_linux.sh dist/beyond-slides-linux-x86_64
```

运行验证脚本前先完成构建；不要在脚本运行过程中替换其使用的二进制。端到端验证需要已缓存的中文检索模型；详细要求见 [本地应用说明](docs/local-application.md#verification)。模拟模型测试验证流程，不验证真实模型的判断质量。

## 项目导航

- [本地应用：配置、数据与恢复机制](docs/local-application.md)
- [Docker 与网络部署](docs/deployment.md)
- [Linux 独立应用包](docs/standalone.md)
- [命令行分析与检查点说明](docs/running.md)
- [架构](ARCHITECTURE.md)、[领域词汇](CONTEXT.md)、[架构决策](docs/adr/)
- [实验与评估记录](docs/evaluation/)
- [发布就绪检查与待解决问题](docs/release-readiness.md)
- `src/`：Rust 流程、验证、模型通信、检索和本地服务器
- `templates/`：应用与报告的 HTML / CSS / JavaScript
- `prompts/`：文本恢复、分段和比较任务的模型指令
- `scripts/`：CPU 转写桥接、实验与验证工具
- `tests/`、`examples/`：回归测试、固定样例与离线演示
