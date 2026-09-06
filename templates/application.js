"use strict";
const $ = (id) => document.getElementById(id);
let currentId = null;
let pollingTimer;
let jobs = [];

async function api(path, options = {}) {
  const response = await fetch(path, { ...options, headers: { "X-BeyondSlides": "local-ui", ...(typeof options.body === "string" ? { "Content-Type": "application/json" } : {}), ...options.headers } });
  const body = await response.json();
  if (!response.ok) { const error = new Error(body.error || `HTTP ${response.status}`); error.status = response.status; throw error; }
  return body;
}
function notice(message) { $("notice").textContent = message; $("notice").hidden = !message; }
function duration(ms) { const seconds = Math.floor(ms / 1000); return `${Math.floor(seconds / 60)}分${String(seconds % 60).padStart(2, "0")}秒`; }
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
    const button = document.createElement("button"); button.textContent = job.name;
    button.classList.toggle("selected", currentId === job.id);
    button.addEventListener("click", () => select(job.id).catch(e => notice(e.message)));
    $("lectures").append(button);
  }
}
async function select(id) {
  currentId = id; clearTimeout(pollingTimer); notice("");
  history.replaceState(null, "", `#${id}`);
  $("import-panel").hidden = true; $("workspace").hidden = false;
  $("api-key").value = "";
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
function render(status) {
  const { job, usage, state } = status;
  $("lecture-title").textContent = job.name;
  $("lecture-meta").textContent = `${job.preview.slide_count} 张幻灯片 · ${job.preview.segment_count ? `${job.preview.segment_count} 个转写片段` : "等待本地转写"} · ${job.preview.duration_ms !== null ? `转写时长 ${duration(job.preview.duration_ms)}` : job.preview.segment_count ? "无转写时间戳" : `录音时长 ${duration(job.preview.recording_duration_ms || 0)}`}${job.recording ? " · 已附录音/视频" : " · 无录音回放"}`;
  $("transcript-preview").textContent = job.preview.transcript_sample || "开始处理后，先用本机 CPU 转写录音，再进行分析。";
  $("slide-preview").textContent = job.preview.slide_sample;
  $("source-warnings").textContent = job.preview.warnings.join("\n");
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
  $("elapsed").textContent = `已处理 ${duration(status.elapsed_ms)}`;
  $("run-description").textContent = state === "stopping" ? "不再启动新任务。当前本地转写或模型窗口/批次会完成并保存结果，可能还需要一段时间。" : "关闭或刷新这个页面不会停止后端处理。恢复前需要重新输入 API key。";
  $("stage-progress").replaceChildren();
  const labels = { transcription: "本地 CPU 转写", restoration: "恢复可读转写", retrieval: "建立幻灯片检索", passages: "语义分段与幻灯片对齐", comparisons: "重要性与新颖度比较", rendering: "生成阅读报告" };
  for (const [stage, label] of Object.entries(labels)) {
    if (stage === "transcription" && !job.transcribe_recording) continue;
    const data = status.progress.stages[stage];
    const row = document.createElement("div"); row.className = "stage";
    const heading = document.createElement("div"); heading.className = "stage-label";
    const name = document.createElement("span"); name.textContent = label;
    const count = document.createElement("span"); count.textContent = !data ? "等待" : data.total === null ? "准备中" : `${data.completed} / ${data.total}`;
    heading.append(name, count); row.append(heading);
    const progress = document.createElement("progress"); progress.setAttribute("aria-label", label);
    if (data?.total !== null) { progress.max = data?.total || 1; progress.value = data?.completed || 0; }
    row.append(progress); $("stage-progress").append(row);
  }
  $("input-tokens").textContent = tokenUsage(usage.known_input_tokens, usage.missing_input_usage, usage.responses);
  $("output-tokens").textContent = tokenUsage(usage.known_output_tokens, usage.missing_output_usage, usage.responses);
  $("active-requests").textContent = usage.active_requests; $("retries").textContent = usage.retries;
  $("run-error").textContent = [status.error, status.usage_error].filter(Boolean).join("\n");
}
async function poll() {
  const id = currentId;
  if (!id) return;
  try { const status = await api(`/api/jobs/${id}`); if (id === currentId) render(status); }
  catch (error) { if (id === currentId) notice(`无法连接工作台：${error.message}。页面会继续尝试连接。`); }
  if (id === currentId) pollingTimer = setTimeout(poll, 1000);
}
$("new-lecture").addEventListener("click", () => {
  currentId = null; clearTimeout(pollingTimer); history.replaceState(null, "", "/");
  $("workspace").hidden = true; $("import-panel").hidden = false; notice("");
});
$("source-mode").addEventListener("change", () => {
  const fromRecording = $("source-mode").value === "recording";
  const transcript = document.querySelector('[name="transcript"]');
  transcript.disabled = fromRecording; transcript.required = !fromRecording;
  $("transcript-upload-label").hidden = fromRecording;
  document.querySelector('[name="recording"]').required = fromRecording;
  $("recording-label").textContent = fromRecording ? "② 录音 / 视频（必选，将在本机转写）" : "录音 / 视频（可选，用于回放）";
});
$("import-form").addEventListener("submit", async (event) => {
  event.preventDefault(); notice(""); $("import-button").disabled = true; $("import-status").textContent = "正在上传并检查文件…";
  try {
    const form = new FormData(event.target);
    if (!form.get("recording")?.size) form.delete("recording");
    const job = await api("/api/jobs", { method: "POST", body: form });
    await refreshLibrary(); await select(job.id); event.target.reset(); $("source-mode").dispatchEvent(new Event("change"));
  } catch (error) { notice(error.message); }
  finally { $("import-button").disabled = false; $("import-status").textContent = ""; }
});
$("start-form").addEventListener("submit", async (event) => {
  event.preventDefault(); notice(""); $("start-button").disabled = true;
  try {
    const request = {
      settings: { base_url: $("base-url").value.trim(), model: $("model").value.trim(), extra_body: $("extra-body").value.trim() ? JSON.parse($("extra-body").value) : null,
        max_concurrency: Number($("concurrency").value), request_interval_ms: Number($("spacing").value), adaptive: $("adaptive").checked, boundary_passages: $("boundaries").checked },
      api_key: $("api-key").value, confirm_reprocessing: false,
    };
    const send = () => api(`/api/jobs/${currentId}/start`, { method: "POST", body: JSON.stringify(request) });
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
