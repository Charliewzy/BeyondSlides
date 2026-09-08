"use strict";
const $ = (id) => document.getElementById(id);
let currentId = null;
let pollingTimer;
let jobs = [];
let debugLoading = false;
let reviewJob = null;
let reviewLoading = false;
let reviewLoaded = false;
let rainLoginTimer;
let rainSessionChecked = false;
let rainDownloadTimer;
let activeRainImportId;
let rainLectures = [];
let rainSelectedLessonId = "";
let rainLectureRequest = 0;

function showReviewPage(page, source) {
  $("review-page-title").textContent = `第 ${page.page} 页`;
  $("review-page-warning").textContent = page.warnings.join("；");
  $("review-page-image").src = source;
  $("review-page-image").alt = `第 ${page.page} 页原始幻灯片`;
  $("review-page-text").textContent = page.text || "（未提取到文字）";
  $("review-dialog").showModal();
}
async function loadSlideReview() {
  if (!currentId || !$("source-preview").open || reviewLoading || reviewLoaded) return;
  const id = currentId;
  reviewLoading = true;
  $("review-status").textContent = "正在检查已保存的幻灯片文字…";
  $("review-retry").hidden = true;
  try {
    const pages = await api(`/api/jobs/${id}/slide-review`);
    if (id !== currentId) return;
    $("review-strip").replaceChildren();
    for (const page of pages) {
      const source = `/api/jobs/${id}/slide-review/${page.page}`;
      const button = document.createElement("button"); button.type = "button"; button.className = "review-card";
      button.setAttribute("aria-label", `检查第 ${page.page} 页`);
      const image = document.createElement("img"); image.src = source; image.alt = `第 ${page.page} 页预览`; image.loading = "lazy";
      image.addEventListener("error", () => { image.alt = "预览暂不可用；点击仍可查看提取文字"; });
      const title = document.createElement("strong"); title.textContent = `第 ${page.page} 页`;
      const warning = document.createElement("small"); warning.textContent = page.warnings.join("\n");
      button.append(image, title, warning); button.addEventListener("click", () => showReviewPage(page, source));
      $("review-strip").append(button);
    }
    reviewLoaded = true;
    $("review-status").textContent = pages.length ? `${pages.length} 页建议检查 · 可左右滚动` : "未发现文字稀少或疑似异常字符的页面。此检查不保证提取完整。";
    updateReviewArrows();
  } catch (error) {
    if (id === currentId) { $("review-status").textContent = `预览暂不可用：${error.message}。不影响开始分析。`; $("review-retry").hidden = false; reviewLoaded = true; }
  } finally { if (id === currentId) reviewLoading = false; }
}
function updateReviewArrows() {
  const strip = $("review-strip");
  $("review-left").disabled = strip.scrollLeft <= 1;
  $("review-right").disabled = strip.scrollLeft + strip.clientWidth >= strip.scrollWidth - 1;
}

async function api(path, options = {}) {
  const response = await fetch(path, { ...options, headers: { "X-BeyondSlides": "local-ui", ...(typeof options.body === "string" ? { "Content-Type": "application/json" } : {}), ...options.headers } });
  const body = await response.json();
  if (!response.ok) { const error = new Error(body.error || `HTTP ${response.status}`); error.status = response.status; throw error; }
  return body;
}
function notice(message) { $("notice").textContent = message; $("notice").hidden = !message; }
function duration(ms) { const seconds = Math.floor(ms / 1000); return `${Math.floor(seconds / 60)}分${String(seconds % 60).padStart(2, "0")}秒`; }
function fileSize(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KiB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MiB`;
  return `${(bytes / 1024 ** 3).toFixed(2)} GiB`;
}
function renderRainDownloadProgress(observed) {
  const container = $("rain-download"), progress = $("rain-download-bar");
  const resource = observed.resource === "slides" ? "雨课堂课件" : observed.resource === "recording" ? "雨课堂录像" : "雨课堂内容";
  container.hidden = false;
  if (observed.phase === "assembling") {
    $("rain-download-phase").textContent = `正在整理${resource}…`;
    $("rain-download-percent").textContent = "下载完成";
    progress.max = 1; progress.value = 1;
  } else if (observed.phase === "downloading") {
    $("rain-download-phase").textContent = `正在下载${resource}…`;
    const completed = observed.total_items > 0 ? observed.completed_items : observed.downloaded_bytes;
    const total = observed.total_items > 0 ? observed.total_items : observed.total_bytes;
    if (total > 0) {
      const percent = Math.min(100, Math.floor(completed / total * 100));
      $("rain-download-percent").textContent = `${percent}%`;
      progress.max = total; progress.value = completed;
    } else {
      $("rain-download-percent").textContent = "";
      progress.removeAttribute("value");
    }
  } else {
    $("rain-download-phase").textContent = "正在准备雨课堂导入…";
    $("rain-download-percent").textContent = "";
    progress.removeAttribute("value");
  }
  $("rain-download-bytes").textContent = observed.total_items > 0
    ? `${observed.completed_items} / ${observed.total_items} 页 · 已下载 ${fileSize(observed.downloaded_bytes)}`
    : observed.total_bytes > 0
    ? `${fileSize(observed.downloaded_bytes)} / ${fileSize(observed.total_bytes)}`
    : observed.downloaded_bytes > 0 ? `已下载 ${fileSize(observed.downloaded_bytes)}` : "正在获取资源信息…";
}
async function pollRainDownload(importId) {
  if (activeRainImportId !== importId) return;
  try {
    renderRainDownloadProgress(await api(`/api/rain-classroom/imports/${importId}`));
  } catch (error) {
    if (error.status !== 404) notice(error.message);
  }
  if (activeRainImportId === importId) rainDownloadTimer = setTimeout(() => pollRainDownload(importId), 500);
}
function stopRainDownloadProgress() {
  activeRainImportId = null; clearTimeout(rainDownloadTimer); $("rain-download").hidden = true;
}
// Old jobs keep their original metadata; translate legacy diagnostics only for display.
function importWarnings(warnings) {
  const sparse = [], messages = [];
  for (const warning of warnings) {
    const match = /^SparseText \{ page: (\d+), non_whitespace_characters: (\d+) \}$/.exec(warning);
    const glyphs = /^SuspiciousGlyphs \{ page: (\d+), glyphs: (.+) \}$/.exec(warning);
    if (match) sparse.push(`${match[1]}（${match[2]} 字符）`);
    else if (glyphs) messages.push(`第 ${glyphs[1]} 页包含疑似无法正确提取的字符：${glyphs[2]}。请对照原始幻灯片检查。`);
    else messages.push(warning);
  }
  if (sparse.length) messages.unshift(`以下页面提取到的文字较少（不计空白）：${sparse.join("、")}。标题页或图片页可能正常；请对照原始幻灯片检查，不代表导入失败。`);
  return messages.join("\n");
}
function readableLog(text) {
  return text.replace(/^--- attempt (\d+) ---$/gm, (_, ms) => {
    const date = new Date(Number(ms));
    return Number.isNaN(date.getTime()) ? _ : `--- 日志记录开始：${date.toLocaleString()}（本次子进程输出，非模型重试）---`;
  });
}
function stageTiming(data, isCurrent) {
  if (data.elapsed_ms == null) return "耗时未记录（旧版运行）";
  const elapsed = `累计用时 ${duration(data.elapsed_ms)}`;
  if (data.reused) return `${elapsed} · 复用已保存结果`;
  if (data.total !== null && data.completed >= data.total) return `${elapsed} · 完成`;
  if (!isCurrent) return elapsed;
  if (data.eta_ms == null) return `${elapsed} · 暂无剩余时间估计`;
  return `${elapsed} · 预计剩余 ${duration(data.eta_ms)} · 预计阶段总用时 ${duration(data.elapsed_ms + data.eta_ms)}`;
}
function transcriptionTiming(data, observed, active) {
  const elapsed = data.elapsed_ms == null ? "累计用时未记录（旧版运行）" : `累计用时 ${duration(data.elapsed_ms)}`;
  if (observed.reused) return `${elapsed} · 复用已保存结果`;
  if (observed.phase === "complete") return `${elapsed} · 完成`;
  if (!active) return elapsed;
  if (observed.phase !== "recognizing") return `${elapsed} · 此步骤暂无剩余时间估计`;
  if (!(observed.total_speech_ms > 0)) return `${elapsed} · 暂无可用的语音识别工作量`;
  if (observed.completed_speech_ms >= observed.total_speech_ms) return `${elapsed} · 语音识别已完成，等待后续步骤`;
  return observed.recognition_eta_ms == null
    ? `${elapsed} · 正在采样语音识别速度…`
    : `${elapsed} · 预计语音识别剩余 ${duration(observed.recognition_eta_ms)}（不含后续标点与保存）`;
}
function setSettings(settings) {
  if (!settings) return;
  $("base-url").value = settings.base_url; $("model").value = settings.model;
  $("extra-body").value = settings.extra_body ? JSON.stringify(settings.extra_body, null, 2) : "";
  $("concurrency").value = settings.max_concurrency; $("spacing").value = settings.request_interval_ms;
  $("adaptive").checked = settings.adaptive; $("boundaries").checked = settings.boundary_passages;
}
async function refreshLibrary() {
  jobs = await api("/api/jobs");
  $("lectures").replaceChildren();
  for (const job of jobs) {
    const entry = document.createElement("div"); entry.className = "lecture-entry"; entry.dataset.jobId = job.id;
    entry.classList.toggle("selected", currentId === job.id);
    const open = document.createElement("button"); open.type = "button"; open.className = "lecture-open"; open.textContent = job.name;
    open.addEventListener("click", () => select(job.id).catch(e => notice(e.message)));
    const remove = document.createElement("button"); remove.type = "button"; remove.className = "lecture-delete"; remove.textContent = "×";
    remove.title = "删除"; remove.setAttribute("aria-label", `删除讲座：${job.name}`);
    remove.addEventListener("click", () => deleteLecture(job));
    entry.append(open, remove); $("lectures").append(entry);
  }
}
async function deleteLecture(job) {
  if (!window.confirm(`确认删除“${job.name}”吗？该讲座的转写、分析结果和报告将从本机永久删除。`)) return;
  const deletingCurrent = currentId === job.id;
  if (deletingCurrent) clearTimeout(pollingTimer);
  try {
    await api(`/api/jobs/${job.id}`, { method: "DELETE" });
    if (deletingCurrent) {
      currentId = null; history.replaceState(null, "", "/");
      $("workspace").hidden = true; $("import-panel").hidden = false;
    }
    await refreshLibrary(); notice("");
  } catch (error) {
    notice(error.message);
    if (deletingCurrent) poll();
  }
}
async function select(id) {
  currentId = id; clearTimeout(pollingTimer); notice("");
  history.replaceState(null, "", `#${id}`);
  $("import-panel").hidden = true; $("workspace").hidden = false;
  $("api-key").value = "";
  $("debug-output").textContent = ""; $("debug-status").textContent = "";
  $("debug-download").hidden = true;
  const job = jobs.find(j => j.id === id);
  setSettings(job?.runs.at(-1)?.settings);
  $("source-preview").open = !job?.runs.length;
  await refreshLibrary(); await poll();
}
function tokenUsage(known, missing, responses) {
  if (!responses) return "等待响应";
  if (missing === responses) return "服务商未提供";
  return `${missing ? "≥ " : ""}${known.toLocaleString()}${missing ? `（${missing}次缺失）` : ""}`;
}
const asrPhases = { checking_recording: "检查录音", extracting_audio: "提取音频", loading_models: "加载模型", detecting_speech: "检测语音", recognizing: "识别语音", punctuating: "添加标点", saving: "保存转写", finalizing: "验证并保存", complete: "完成" };
function transcriptionProgress(row, progress, count, observed, active) {
  const measurable = observed.phase === "recognizing" && observed.total_speech_ms > 0 && observed.total_regions > 0;
  const ratio = measurable ? Math.min(1, observed.completed_speech_ms / observed.total_speech_ms) : null;
  const label = asrPhases[observed.phase] || "处理中";
  count.textContent = `${label}${ratio === null ? "" : ` · ${Math.floor(ratio * 100)}%`}${observed.reused ? "（复用已保存结果）" : ""}`;
  if (observed.phase === "complete") { progress.max = 1; progress.value = 1; }
  else if (ratio !== null) { progress.max = observed.total_speech_ms; progress.value = observed.completed_speech_ms; }
  else if (active) { progress.removeAttribute("value"); }
  else { progress.max = 1; progress.value = 0; }
  if (observed.total_regions !== null) {
    const detail = document.createElement("small");
    detail.textContent = `${observed.completed_regions} / ${observed.total_regions} 个语音区域 · ${duration(observed.completed_speech_ms)} / ${duration(observed.total_speech_ms)} 有声时长。百分比按有声时长计算，不按区域个数计算。`;
    row.append(detail);
  }
}
function render(status) {
  const { job, usage, state } = status;
  if (reviewJob !== job.id) {
    reviewJob = job.id; reviewLoading = false; reviewLoaded = false;
    $("review-strip").replaceChildren(); $("review-status").textContent = "";
    $("review-dialog").close();
  }
  loadSlideReview();
  $("lecture-title").textContent = job.name;
  $("lecture-meta").textContent = `${job.preview.slide_count} 张幻灯片 · ${job.preview.segment_count ? `${job.preview.segment_count} 个转写片段` : "等待本地转写"} · ${job.preview.duration_ms !== null ? `转写时长 ${duration(job.preview.duration_ms)}` : job.preview.segment_count ? "无转写时间戳" : `录音时长 ${duration(job.preview.recording_duration_ms || 0)}`}${job.recording ? " · 已附录音/视频" : " · 无录音回放"}`;
  $("transcript-preview").textContent = job.preview.transcript_sample || "开始处理后，先用本机 CPU 转写录音，再进行分析。";
  $("slide-preview").textContent = job.preview.slide_sample;
  $("source-warnings").textContent = importWarnings(job.preview.warnings);
  const active = state === "running" || state === "stopping";
  $("processing-panel").hidden = state === "ready";
  $("start-button").hidden = active;
  $("start-button").textContent = state === "ready" ? (job.transcribe_recording ? "开始转写与分析" : "开始分析") : state === "complete" ? "更改设置后重新分析" : "恢复处理";
  $("stop-button").hidden = !active; $("stop-button").disabled = state === "stopping";
  $("open-report").hidden = !status.report_url;
  if (status.report_url) $("open-report").href = status.report_url;
  $("export-report").hidden = !status.report_url;
  $("export-report").href = `/api/jobs/${job.id}/export`;
  $("export-report").textContent = job.recording && job.preview.duration_ms !== null ? "下载分享包（含录音）" : "下载分享包";
  const stateLabels = { running: "正在处理", stopping: "正在完成当前任务…", paused: "已暂停，可继续", failed: "处理失败，检查后可恢复", interrupted: "运行中断，可恢复", complete: "处理完成" };
  $("run-state").textContent = stateLabels[state] || "准备就绪";
  $("elapsed").textContent = `已处理 ${state === "interrupted" ? "至少 " : ""}${duration(status.elapsed_ms)}`;
  $("run-description").textContent = state === "stopping" ? "不再启动新任务。当前本地转写或模型窗口/批次会完成并保存结果，可能还需要一段时间。" : "关闭或刷新这个页面不会停止后端处理。恢复前需要重新输入 API key。";
  $("stage-progress").replaceChildren();
  const labels = { transcription: "本地 CPU 转写", restoration: "恢复可读转写", retrieval: "建立幻灯片检索", passages: "语义分段与幻灯片对齐", comparisons: "重要性与新颖度比较", rendering: "生成阅读报告" };
  for (const [stage, label] of Object.entries(labels)) {
    if (stage === "transcription" && !job.transcribe_recording) continue;
    const data = status.progress.stages[stage];
    const row = document.createElement("div"); row.className = "stage";
    const heading = document.createElement("div"); heading.className = "stage-label";
    const name = document.createElement("span"); name.textContent = label;
    const count = document.createElement("span"); count.textContent = !data ? "等待" : data.total === null ? "准备中" : data.total === 0 ? "无需处理" : `${data.completed} / ${data.total}`;
    heading.append(name, count); row.append(heading);
    const progress = document.createElement("progress"); progress.setAttribute("aria-label", label);
    if (data?.total !== null) { progress.max = data?.total || 1; progress.value = data?.total === 0 ? 1 : data?.completed || 0; }
    row.append(progress); $("stage-progress").append(row);
    if (stage === "transcription" && status.transcription) transcriptionProgress(row, progress, count, status.transcription, active);
    if (data) {
      const timing = document.createElement("small"); timing.className = "stage-timing";
      timing.textContent = stage === "transcription" && status.transcription
        ? transcriptionTiming(data, status.transcription, active)
        : stageTiming(data, active && status.progress.current === stage);
      row.append(timing);
    }
  }
  $("input-tokens").textContent = tokenUsage(usage.known_input_tokens, usage.missing_input_usage, usage.responses);
  $("output-tokens").textContent = tokenUsage(usage.known_output_tokens, usage.missing_output_usage, usage.responses);
  $("active-requests").textContent = usage.active_requests; $("retries").textContent = usage.retries;
  $("run-error").textContent = [status.error, status.usage_error].filter(Boolean).join("\n");
  const timings = status.transcription?.timings_seconds || {};
  $("asr-timings").textContent = Object.keys(asrPhases).filter(phase => phase in timings).map(phase => `${asrPhases[phase]}：${timings[phase].toFixed(2)} 秒`).join(" · ");
  if (status.transcription?.reused) $("asr-timings").textContent += "（模型阶段耗时来自复用的转写结果）";
}
async function pollDebug() {
  if (!currentId || !$("debug-panel").open || debugLoading) return;
  const id = currentId, kind = $("debug-kind").value;
  debugLoading = true;
  try {
    const log = await api(`/api/jobs/${id}/logs/${kind}`);
    if (id !== currentId || kind !== $("debug-kind").value) return;
    $("debug-download").href = `/api/jobs/${id}/logs/${kind}/download`;
    $("debug-download").hidden = !log.available;
    if ($("debug-follow").checked || !$("debug-output").textContent) {
      $("debug-output").textContent = readableLog(log.text);
      if ($("debug-follow").checked) $("debug-output").scrollTop = $("debug-output").scrollHeight;
    }
    $("debug-status").textContent = !log.available ? "尚无此版本捕获的日志。旧日志仅保存在本地；下次启动或恢复后开始捕获。" : !$("debug-follow").checked ? "显示已暂停，后台继续记录。勾选跟随可查看最新输出。" : log.truncated ? "显示最近 128 KiB；更早的输出请下载完整日志。" : "实时更新（约每秒）；后台持续保存日志。";
  } catch (error) { if (id === currentId) $("debug-status").textContent = `日志暂不可用：${error.message}`; }
  finally { debugLoading = false; }
}
async function poll() {
  clearTimeout(pollingTimer);
  const id = currentId;
  if (!id) return;
  try { const status = await api(`/api/jobs/${id}`); if (id === currentId) { render(status); await pollDebug(); } }
  catch (error) { if (id === currentId) notice(`无法连接工作台：${error.message}。页面会继续尝试连接。`); }
  if (id === currentId) { clearTimeout(pollingTimer); pollingTimer = setTimeout(poll, 1000); }
}
$("debug-panel").addEventListener("toggle", pollDebug);
$("source-preview").addEventListener("toggle", () => { loadSlideReview(); updateReviewArrows(); });
$("review-retry").addEventListener("click", () => { reviewLoaded = false; loadSlideReview(); });
$("review-close").addEventListener("click", () => $("review-dialog").close());
$("review-page-image").addEventListener("error", () => { $("review-page-image").alt = "图片预览暂不可用，请稍后重试；仍可检查右侧提取文字。"; });
$("review-strip").addEventListener("scroll", updateReviewArrows, { passive: true });
window.addEventListener("resize", updateReviewArrows);
for (const [id, direction] of [["review-left", -1], ["review-right", 1]]) {
  $(id).addEventListener("click", () => $("review-strip").scrollBy({ left: direction * $("review-strip").clientWidth * .8, behavior: "smooth" }));
}
$("debug-kind").addEventListener("change", () => { $("debug-output").textContent = ""; $("debug-download").hidden = true; pollDebug(); });
$("debug-follow").addEventListener("change", pollDebug);
$("debug-output").addEventListener("scroll", () => {
  const output = $("debug-output");
  if (output.scrollHeight - output.clientHeight - output.scrollTop > 20) $("debug-follow").checked = false;
});
$("new-lecture").addEventListener("click", () => {
  currentId = null; clearTimeout(pollingTimer); history.replaceState(null, "", "/");
  $("workspace").hidden = true; $("import-panel").hidden = false; notice("");
});
function syncImportSources() {
  const slidesFromRain = $("slides-source-mode").value === "rain";
  const lectureMode = $("lecture-source-mode").value;
  const recordingFromRain = lectureMode === "rain";
  const fromRecording = lectureMode === "recording";
  const transcript = document.querySelector('[name="transcript"]');
  const recording = document.querySelector('[name="recording"]');
  const slides = document.querySelector('[name="slides"]');
  slides.disabled = slidesFromRain; slides.required = !slidesFromRain;
  $("slides-upload-label").hidden = slidesFromRain;
  $("rain-slides-selection").hidden = !slidesFromRain;
  transcript.disabled = fromRecording || recordingFromRain; transcript.required = !fromRecording && !recordingFromRain;
  $("transcript-upload-label").hidden = fromRecording || recordingFromRain;
  recording.disabled = recordingFromRain; recording.required = fromRecording;
  $("recording-upload-label").hidden = recordingFromRain;
  $("rain-recording-selection").hidden = !recordingFromRain;
  const usesRain = slidesFromRain || recordingFromRain;
  $("rain-classroom-import").hidden = !usesRain;
  $("recording-label").textContent = fromRecording ? "② 录音 / 视频（必选，将在本机转写）" : "录音 / 视频（可选，用于回放）";
  if (usesRain && !rainSessionChecked) checkRainSession();
}
$("slides-source-mode").addEventListener("change", syncImportSources);
$("lecture-source-mode").addEventListener("change", syncImportSources);
function setRainAuthenticated(authenticated) {
  $("rain-authenticated").hidden = !authenticated;
  $("rain-logout").hidden = !authenticated;
  $("rain-connect").hidden = authenticated;
}
function clearRainSelections() {
  rainLectures = []; rainSelectedLessonId = ""; rainLectureRequest += 1;
  for (const prefix of ["rain-slides", "rain-recording"]) {
    $(`${prefix}-course`).disabled = true;
    $(`${prefix}-course`).replaceChildren(new Option("先加载课程", ""));
    $(`${prefix}-lecture`).disabled = true;
    $(`${prefix}-lecture`).replaceChildren(new Option("先选择课程", ""));
  }
  $("rain-slides-unavailable").hidden = true;
  $("rain-recording-unavailable").hidden = true;
  $("rain-presentation-label").hidden = true;
  $("rain-presentation").disabled = true;
  $("rain-presentation").replaceChildren(new Option("先选择讲次", ""));
}
function showRainCourses(courses) {
  rainLectures = []; rainSelectedLessonId = ""; rainLectureRequest += 1;
  for (const prefix of ["rain-slides", "rain-recording"]) {
    const select = $(`${prefix}-course`); select.replaceChildren(new Option("请选择课程", ""));
    for (const course of courses) {
      const suffix = course.classroom_name && course.classroom_name !== course.course_name ? ` · ${course.classroom_name}` : "";
      select.add(new Option(`${course.course_name}${suffix}`, String(course.classroom_id)));
    }
    select.disabled = false;
    $(`${prefix}-lecture`).disabled = true;
    $(`${prefix}-lecture`).replaceChildren(new Option("先选择课程", ""));
  }
  $("rain-slides-unavailable").hidden = true;
  $("rain-recording-unavailable").hidden = true;
  $("rain-presentation-label").hidden = true;
  $("rain-presentation").disabled = true;
  $("rain-presentation").replaceChildren(new Option("先选择讲次", ""));
  $("rain-status").textContent = courses.length ? "" : "当前账号没有可导入的课程。";
  setRainAuthenticated(true);
}
function refreshRainLoginView() {
  $("rain-login-view").src = `/api/rain-classroom/login-view?t=${Date.now()}`;
}
async function pollRainLogin() {
  if (!$("rain-login-dialog").open) return;
  try {
    const courses = await api("/api/rain-classroom/courses");
    showRainCourses(courses);
    $("rain-login-status").textContent = "登录成功。";
    $("rain-login-dialog").close();
    return;
  } catch (_) {
    $("rain-login-status").textContent = "等待扫码并在手机上确认…";
    refreshRainLoginView();
  }
  rainLoginTimer = setTimeout(pollRainLogin, 1500);
}
async function checkRainSession() {
  rainSessionChecked = true;
  $("rain-connect").disabled = true; $("rain-status").textContent = "正在检查雨课堂登录状态…";
  try {
    await api("/api/rain-classroom/connect", { method: "POST" });
    showRainCourses(await api("/api/rain-classroom/courses"));
  } catch (_) {
    setRainAuthenticated(false);
    $("rain-status").textContent = "尚未登录，请扫码后继续。";
  } finally { $("rain-connect").disabled = false; }
}
$("rain-connect").addEventListener("click", async () => {
  $("rain-connect").disabled = true; $("rain-status").textContent = "正在准备雨课堂登录…";
  try {
    await api("/api/rain-classroom/connect", { method: "POST" });
    $("rain-login-status").textContent = "等待扫码并在手机上确认…";
    refreshRainLoginView(); $("rain-login-dialog").showModal(); pollRainLogin();
  } catch (error) { $("rain-status").textContent = error.message; }
  finally { $("rain-connect").disabled = false; }
});
$("rain-logout").addEventListener("click", async () => {
  $("rain-logout").disabled = true;
  $("rain-status").textContent = "正在退出雨课堂…";
  try {
    await api("/api/rain-classroom/logout", { method: "POST" });
    clearRainSelections(); setRainAuthenticated(false); rainSessionChecked = false;
    $("rain-status").textContent = "已退出。下次使用雨课堂时需要重新扫码登录。";
  } catch (error) { $("rain-status").textContent = error.message; }
  finally { $("rain-logout").disabled = false; }
});
$("rain-login-close").addEventListener("click", () => $("rain-login-dialog").close());
$("rain-login-dialog").addEventListener("close", () => clearTimeout(rainLoginTimer));
function supportsRainSource(prefix, lecture) {
  return prefix === "rain-slides" ? lecture.presentation_count > 0 : lecture.has_recording;
}
function renderRainLectureSelect(prefix) {
  const select = $(`${prefix}-lecture`);
  const available = rainLectures.filter(lecture => supportsRainSource(prefix, lecture));
  const selected = rainLectures.find(lecture => lecture.lesson_id === rainSelectedLessonId);
  const placeholder = available.length ? "请选择讲次" : "无可导入讲次";
  select.replaceChildren(new Option(placeholder, ""));
  for (const lecture of available) {
    const option = new Option(lecture.title, lecture.lesson_id); option.dataset.title = lecture.title; select.add(option);
  }
  if (selected && !available.some(lecture => lecture.lesson_id === selected.lesson_id)) {
    const option = new Option(selected.title, selected.lesson_id); option.dataset.title = selected.title; select.add(option);
  }
  select.value = rainSelectedLessonId;
  select.disabled = !available.length && !selected;
}
function renderRainAvailability() {
  const lecture = rainLectures.find(candidate => candidate.lesson_id === rainSelectedLessonId);
  $("rain-slides-unavailable").hidden = !lecture || lecture.presentation_count > 0;
  $("rain-recording-unavailable").hidden = !lecture || lecture.has_recording;
}
async function loadRainLectures(classroomId) {
  const request = ++rainLectureRequest;
  rainLectures = []; rainSelectedLessonId = "";
  for (const prefix of ["rain-slides", "rain-recording"]) {
    const select = $(`${prefix}-lecture`); select.disabled = true;
    select.replaceChildren(new Option(classroomId ? "正在加载讲次…" : "先选择课程", ""));
  }
  renderRainAvailability();
  $("rain-presentation-label").hidden = true;
  $("rain-presentation").disabled = true;
  $("rain-presentation").replaceChildren(new Option("先选择讲次", ""));
  if (!classroomId) return;
  try {
    const lectures = await api(`/api/rain-classroom/courses/${classroomId}/lectures`);
    if (request !== rainLectureRequest) return;
    rainLectures = lectures;
    renderRainLectureSelect("rain-slides"); renderRainLectureSelect("rain-recording");
    $("rain-status").textContent = "";
  } catch (error) {
    if (request !== rainLectureRequest) return;
    setRainAuthenticated(false);
    for (const prefix of ["rain-slides", "rain-recording"]) {
      const select = $(`${prefix}-lecture`); select.replaceChildren(new Option("加载失败", ""));
    }
    $("rain-status").textContent = `${error.message}。请重新扫码登录。`;
  }
}
async function chooseRainCourse(prefix) {
  const classroomId = $(`${prefix}-course`).value;
  $("rain-slides-course").value = classroomId;
  $("rain-recording-course").value = classroomId;
  await loadRainLectures(classroomId);
}
async function loadRainPresentationOptions() {
  const classroomId = $("rain-slides-course").value;
  const lessonId = $("rain-slides-lecture").value;
  const expectedLessonId = lessonId;
  const label = $("rain-presentation-label"); label.hidden = true;
  const select = $("rain-presentation"); select.disabled = true;
  select.replaceChildren(new Option(lessonId ? "正在加载课件…" : "先选择讲次", ""));
  if (!classroomId || !lessonId) return;
  const lecture = rainLectures.find(candidate => candidate.lesson_id === lessonId);
  if (!lecture?.presentation_count) return;
  try {
    const presentations = await api(`/api/rain-classroom/courses/${classroomId}/lectures/${lessonId}/presentations`);
    if (rainSelectedLessonId !== expectedLessonId) return;
    select.replaceChildren(new Option("请选择课件", ""));
    for (const presentation of presentations) {
      select.add(new Option(`${presentation.title || "未命名课件"} · ${presentation.page_count} 页`, presentation.presentation_id));
    }
    if (presentations.length === 1) {
      select.value = presentations[0].presentation_id;
    } else if (presentations.length > 1) {
      select.disabled = false; label.hidden = false;
    } else {
      $("rain-slides-unavailable").hidden = false;
    }
    $("rain-status").textContent = "";
  } catch (error) {
    if (rainSelectedLessonId !== expectedLessonId) return;
    label.hidden = false; select.replaceChildren(new Option("加载失败", ""));
    $("rain-status").textContent = error.message;
  }
}
async function chooseRainLecture(prefix) {
  rainSelectedLessonId = $(`${prefix}-lecture`).value;
  renderRainLectureSelect("rain-slides"); renderRainLectureSelect("rain-recording");
  renderRainAvailability();
  const lecture = rainLectures.find(candidate => candidate.lesson_id === rainSelectedLessonId);
  const name = document.querySelector('[name="name"]');
  if (lecture && !name.value.trim()) name.value = lecture.title;
  await loadRainPresentationOptions();
}
$("rain-slides-course").addEventListener("change", () => chooseRainCourse("rain-slides"));
$("rain-recording-course").addEventListener("change", () => chooseRainCourse("rain-recording"));
$("rain-slides-lecture").addEventListener("change", () => chooseRainLecture("rain-slides"));
$("rain-recording-lecture").addEventListener("change", () => chooseRainLecture("rain-recording"));
$("import-form").addEventListener("submit", async (event) => {
  event.preventDefault(); notice(""); $("import-button").disabled = true;
  const slidesFromRain = $("slides-source-mode").value === "rain";
  const recordingFromRain = $("lecture-source-mode").value === "rain";
  const usesRain = slidesFromRain || recordingFromRain;
  $("import-status").textContent = usesRain ? "正在导入雨课堂内容并检查文件…" : "正在上传并检查文件…";
  try {
    const form = new FormData(event.target);
    if (!form.get("recording")?.size) form.delete("recording");
    if (slidesFromRain) {
      const classroomId = Number($("rain-slides-course").value), lessonId = $("rain-slides-lecture").value, presentationId = $("rain-presentation").value;
      if (!classroomId || !lessonId || !presentationId) throw new Error("请选择要导入的雨课堂课件");
      form.set("rain_slides", JSON.stringify({ classroom_id: classroomId, lesson_id: lessonId, presentation_id: presentationId }));
    }
    if (recordingFromRain) {
      const classroomId = Number($("rain-recording-course").value), lessonId = $("rain-recording-lecture").value;
      if (!classroomId || !lessonId) throw new Error("请选择要导入的雨课堂录像");
      form.set("rain_recording", JSON.stringify({ classroom_id: classroomId, lesson_id: lessonId }));
    }
    if (usesRain) {
      activeRainImportId = crypto.randomUUID();
      form.set("rain_import_id", activeRainImportId);
      renderRainDownloadProgress({ resource: "preparing", phase: "preparing", downloaded_bytes: 0, total_bytes: null });
      pollRainDownload(activeRainImportId);
    }
    const job = await api("/api/jobs", { method: "POST", body: form });
    await refreshLibrary(); await select(job.id); event.target.reset(); syncImportSources();
  } catch (error) { notice(error.message); }
  finally { stopRainDownloadProgress(); $("import-button").disabled = false; $("import-status").textContent = ""; }
});
$("start-form").addEventListener("submit", async (event) => {
  event.preventDefault(); notice(""); $("start-button").disabled = true;
  const id = currentId;
  try {
    const request = {
      settings: { base_url: $("base-url").value.trim(), model: $("model").value.trim(), extra_body: $("extra-body").value.trim() ? JSON.parse($("extra-body").value) : null,
        max_concurrency: Number($("concurrency").value), request_interval_ms: Number($("spacing").value), adaptive: $("adaptive").checked, boundary_passages: $("boundaries").checked },
      api_key: $("api-key").value, confirm_reprocessing: false,
    };
    const send = () => api(`/api/jobs/${id}/start`, { method: "POST", body: JSON.stringify(request) });
    try { await send(); }
    catch (error) {
      if (error.status !== 409 || !error.message.includes("Confirm to continue") || !window.confirm(error.message)) throw error;
      request.confirm_reprocessing = true; await send();
    }
    $("api-key").value = ""; $("source-preview").open = false;
    clearTimeout(pollingTimer); await poll();
  } catch (error) { notice(error.message); }
  finally { $("start-button").disabled = false; }
});
$("stop-button").addEventListener("click", async () => {
  try { await api(`/api/jobs/${currentId}/stop`, { method: "POST" }); clearTimeout(pollingTimer); await poll(); }
  catch (error) { notice(error.message); }
});
refreshLibrary().then(() => {
  const id = location.hash.slice(1);
  if (jobs.some(j => j.id === id)) return select(id);
}).catch(error => notice(error.message));
