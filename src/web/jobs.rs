use std::{
    fs::{self, File, TryLockError},
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use beyond_slides::{
    ChatCompletionsConfig, Transcript, ValidatedSources,
    ingestion::{
        pdf,
        transcript::{TranscriptFormat, import},
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::run_support::{ModelBackendKind, read_json, write_json_atomically};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Job {
    pub id: String,
    pub name: String,
    pub created_ms: u64,
    pub preview: Preview,
    pub recording: Option<String>,
    #[serde(default)]
    pub transcribe_recording: bool,
    pub runs: Vec<Run>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Preview {
    pub slide_count: usize,
    pub segment_count: usize,
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub recording_duration_ms: Option<u64>,
    pub transcript_sample: String,
    pub slide_sample: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Settings {
    #[serde(default)]
    pub backend: ModelBackendKind,
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub codex_reasoning_effort: Option<String>,
    #[serde(default)]
    pub codex_service_tier: Option<String>,
    pub extra_body: Option<Value>,
    pub max_concurrency: usize,
    pub request_interval_ms: u64,
    pub adaptive: bool,
    pub boundary_passages: bool,
}

impl Settings {
    pub fn validate(&self, key: &str) -> Result<(), String> {
        if self.model.trim().is_empty() {
            return Err("Model name cannot be empty".into());
        }
        if self.backend == ModelBackendKind::Codex {
            if self
                .codex_reasoning_effort
                .as_ref()
                .is_some_and(|effort| effort.trim().is_empty())
            {
                return Err("Codex reasoning effort cannot be empty".into());
            }
            if self
                .codex_service_tier
                .as_ref()
                .is_some_and(|tier| tier.trim().is_empty())
            {
                return Err("Codex service tier cannot be empty".into());
            }
            if self.extra_body.is_some() {
                return Err(
                    "Codex app-server does not accept provider extra request fields".into(),
                );
            }
            if !(1..=32).contains(&self.max_concurrency) || self.request_interval_ms > 60_000 {
                return Err("Concurrency must be 1–32 and request spacing 0–60000 ms".into());
            }
            return Ok(());
        }
        if self.codex_reasoning_effort.is_some() || self.codex_service_tier.is_some() {
            return Err("Codex reasoning and speed settings require the Codex backend".into());
        }
        let config = ChatCompletionsConfig::new(&self.base_url, key, &self.model)
            .map_err(|e| e.to_string())?;
        let url = url::Url::parse(&self.base_url).map_err(|e| e.to_string())?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("Put credentials in the API key field, not in the endpoint URL".into());
        }
        if let Some(body) = &self.extra_body {
            config
                .with_extra_body(body.clone())
                .map_err(|e| e.to_string())?;
            if body.to_string().contains(key) {
                return Err("Do not put the API key in saved request options".into());
            }
        }
        if !(1..=32).contains(&self.max_concurrency) || self.request_interval_ms > 60_000 {
            return Err("Concurrency must be 1–32 and request spacing 0–60000 ms".into());
        }
        Ok(())
    }

    pub fn same_restoration(&self, other: &Self) -> bool {
        self.backend == other.backend
            && self.model == other.model
            && self.codex_reasoning_effort == other.codex_reasoning_effort
            && (self.backend == ModelBackendKind::Codex
                || (self.base_url == other.base_url && self.extra_body == other.extra_body))
    }

    pub fn same_analysis(&self, other: &Self) -> bool {
        self.same_restoration(other) && self.boundary_passages == other.boundary_passages
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Run {
    pub number: usize,
    pub settings: Settings,
    pub started_ms: u64,
    pub elapsed_before_ms: u64,
}

impl Run {
    pub fn directory(&self, job: &Path) -> PathBuf {
        job.join(format!("run-{:04}", self.number))
    }

    /// A lower bound after a hard interruption, never time spent offline.
    pub fn checkpointed_elapsed(&self, job: &Path) -> Result<u64, io::Error> {
        let path = self.directory(job).join("control/elapsed.json");
        if !path.exists() {
            return Ok(self.elapsed_before_ms);
        }
        let checkpoint: ElapsedCheckpoint = read_json(&path, "worker elapsed checkpoint")?;
        Ok(if checkpoint.started_ms == self.started_ms {
            checkpoint.elapsed_ms.max(self.elapsed_before_ms)
        } else {
            self.elapsed_before_ms
        })
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct ElapsedCheckpoint {
    pub started_ms: u64,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum OutcomeStatus {
    Complete,
    Paused,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Outcome {
    pub status: OutcomeStatus,
    pub error: Option<String>,
    pub finished_ms: u64,
    pub elapsed_ms: u64,
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(super) fn job_path(root: &Path, id: &str) -> Result<PathBuf, io::Error> {
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid lecture ID",
        ));
    }
    Ok(root.join(id))
}

pub(super) fn read_job(path: &Path) -> Result<Job, io::Error> {
    read_json(&path.join("job.json"), "lecture job")
}

pub(super) fn save_job(path: &Path, job: &Job) -> Result<(), io::Error> {
    write_json_atomically(&path.join("job.json"), job, "lecture job")
}

/// The lock is the live-process evidence, not a PID or a persisted status flag.
pub(super) fn worker_lock(path: &Path) -> Result<File, io::Error> {
    File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.join("worker.lock"))
}

pub(super) fn is_worker_running(path: &Path) -> Result<bool, io::Error> {
    let file = worker_lock(path)?;
    match file.try_lock() {
        Ok(()) => Ok(false), // Closing the file releases the acquired probe lock.
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

pub(super) fn import_job(
    directory: &Path,
    id: String,
    name: String,
    transcript_extension: &str,
    recording: Option<String>,
) -> Result<Job, String> {
    let imported = pdf::import(&directory.join("slides.pdf")).map_err(|e| e.to_string())?;
    if imported.slide_deck.slides.is_empty() {
        return Err("The PDF contains no slides".into());
    }
    let transcribe_recording = transcript_extension.is_empty();
    let recording_duration_ms = if let Some(recording) = &recording {
        Some(probe_recording(
            &directory.join(recording),
            transcribe_recording,
        )?)
    } else {
        None
    };
    if transcribe_recording && recording.is_none() {
        return Err("Provide a transcript or a recording to transcribe".into());
    }
    let input = if transcribe_recording {
        String::new()
    } else {
        fs::read_to_string(directory.join("transcript-upload")).map_err(|e| e.to_string())?
    };
    let transcript: Transcript = match transcript_extension {
        "" => Transcript {
            segments: Vec::new(),
        },
        "json" => import(&input, TranscriptFormat::Json).map_err(|e| e.to_string())?,
        "tsv" => import(&input, TranscriptFormat::Tsv).map_err(|e| e.to_string())?,
        "srt" => import(&input, TranscriptFormat::SubRip).map_err(|e| e.to_string())?,
        "vtt" => import(&input, TranscriptFormat::WebVtt).map_err(|e| e.to_string())?,
        "txt" => import(&input, TranscriptFormat::PlainText).map_err(|e| e.to_string())?,
        _ => return Err("Supported transcript formats: JSON, TSV, SRT, VTT and plain text".into()),
    };
    if !transcribe_recording && transcript.segments.is_empty() {
        return Err("The transcript contains no segments".into());
    }
    let sources =
        ValidatedSources::new(transcript, imported.slide_deck).map_err(|e| e.to_string())?;
    let transcript = sources.transcript();
    let deck = sources.slide_deck();
    let mut preview = Preview {
        slide_count: deck.slides.len(),
        segment_count: transcript.segments.len(),
        duration_ms: transcript.segments.last().and_then(|s| s.end_ms),
        recording_duration_ms,
        transcript_sample: transcript
            .segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(700)
            .collect(),
        slide_sample: deck.slides[0].text.chars().take(700).collect(),
        warnings: import_warnings(&imported.warnings),
    };
    if !transcribe_recording && !transcript.has_timestamps() {
        preview.warnings.push("此转写未提供时间戳。可以分析文本，但无法按段落定位或播放录音；不会凭文字长度猜测时间。".into());
    }
    if !transcribe_recording {
        write_json_atomically(
            &directory.join("transcript.json"),
            transcript,
            "normalized transcript",
        )
        .map_err(|e| e.to_string())?;
    }
    write_json_atomically(&directory.join("slides.json"), deck, "normalized slides")
        .map_err(|e| e.to_string())?;
    let job = Job {
        id,
        name,
        created_ms: now_ms(),
        preview,
        recording,
        transcribe_recording,
        runs: Vec::new(),
    };
    save_job(directory, &job).map_err(|e| e.to_string())?;
    Ok(job)
}

fn import_warnings(warnings: &[pdf::ImportWarning]) -> Vec<String> {
    let mut messages = Vec::new();
    let mut sparse_pages = Vec::new();
    for warning in warnings {
        match warning {
            pdf::ImportWarning::SparseText {
                page,
                non_whitespace_characters,
            } => {
                sparse_pages.push(format!("{page}（{non_whitespace_characters} 字符）"));
            }
            pdf::ImportWarning::SuspiciousGlyphs { page, glyphs } => {
                messages.push(format!(
                    "第 {page} 页包含疑似无法正确提取的字符：{}。请对照原始幻灯片检查。",
                    glyphs.iter().collect::<String>()
                ));
            }
        }
    }
    if !sparse_pages.is_empty() {
        messages.insert(0, format!("以下页面提取到的文字较少（不计空白）：{}。标题页或图片页可能正常；请对照原始幻灯片检查，不代表导入失败。", sparse_pages.join("、")));
    }
    messages
}

fn probe_recording(path: &Path, require_audio: bool) -> Result<u64, String> {
    let output = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=codec_type",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .map_err(|e| format!("Could not inspect recording with ffprobe: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "Invalid recording: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let data: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    if require_audio
        && !data["streams"]
            .as_array()
            .is_some_and(|streams| streams.iter().any(|s| s["codec_type"] == "audio"))
    {
        return Err("The recording has no audio track to transcribe".into());
    }
    let seconds: f64 = data["format"]["duration"]
        .as_str()
        .ok_or("Recording duration is unavailable")?
        .parse()
        .map_err(|_| "Invalid recording duration")?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err("Recording duration must be positive".into());
    }
    Ok((seconds * 1000.0).round() as u64)
}

/// Copy only reusable restoration artifacts. The pipeline revalidates every
/// checkpoint; never adopt arbitrary annotation files under a new identity.
pub(super) fn copy_restoration(previous: &Path, next: &Path) -> Result<(), io::Error> {
    if !previous.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(next)?;
    for entry in fs::read_dir(previous)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::copy(entry.path(), next.join(entry.file_name()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(backend: ModelBackendKind) -> Settings {
        Settings {
            backend,
            base_url: "http://localhost/v1".into(),
            model: "test-model".into(),
            codex_reasoning_effort: None,
            codex_service_tier: None,
            extra_body: None,
            max_concurrency: 2,
            request_interval_ms: 0,
            adaptive: false,
            boundary_passages: true,
        }
    }

    #[test]
    fn codex_settings_need_no_endpoint_or_api_key() {
        let mut settings = settings(ModelBackendKind::Codex);
        settings.base_url.clear();
        assert_eq!(settings.validate(""), Ok(()));
    }

    #[test]
    fn codex_settings_reject_openai_specific_extra_body() {
        let mut settings = settings(ModelBackendKind::Codex);
        settings.extra_body = Some(serde_json::json!({"thinking": {"type": "disabled"}}));
        assert!(settings.validate("").unwrap_err().contains("extra request"));
    }

    #[test]
    fn codex_checkpoint_reuse_ignores_the_hidden_endpoint_field() {
        let left = settings(ModelBackendKind::Codex);
        let mut right = left.clone();
        right.base_url = "https://a-hidden-old-value.example/v1".into();
        assert!(left.same_restoration(&right));
    }

    #[test]
    fn reasoning_changes_checkpoint_identity_but_service_tier_does_not() {
        let left = settings(ModelBackendKind::Codex);
        let mut right = left.clone();
        right.codex_service_tier = Some("priority".into());
        assert!(left.same_restoration(&right));
        right.codex_reasoning_effort = Some("high".into());
        assert!(!left.same_restoration(&right));
    }

    #[test]
    fn backend_wire_names_match_the_browser_and_environment_contract() {
        assert_eq!(
            serde_json::to_value(ModelBackendKind::OpenAiCompatible).unwrap(),
            "openai_compatible"
        );
        assert_eq!(
            serde_json::to_value(ModelBackendKind::Codex).unwrap(),
            "codex"
        );
    }

    #[test]
    fn settings_without_backend_keep_openai_compatibility() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "base_url": "http://localhost/v1",
            "model": "test-model",
            "extra_body": null,
            "max_concurrency": 2,
            "request_interval_ms": 0,
            "adaptive": false,
            "boundary_passages": true
        }))
        .expect("legacy settings deserialize");
        assert_eq!(settings.backend, ModelBackendKind::OpenAiCompatible);
    }

    #[test]
    fn pdf_import_warnings_group_pages_and_explain_the_caveat() {
        let warnings = import_warnings(&[
            pdf::ImportWarning::SparseText {
                page: 3,
                non_whitespace_characters: 5,
            },
            pdf::ImportWarning::SparseText {
                page: 11,
                non_whitespace_characters: 7,
            },
            pdf::ImportWarning::SuspiciousGlyphs {
                page: 12,
                glyphs: vec!['\u{fffd}'],
            },
        ]);
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("3（5 字符）、11（7 字符）"));
        assert!(warnings[0].contains("不代表导入失败"));
        assert!(warnings[1].contains("第 12 页"));
        assert!(warnings[1].contains('\u{fffd}'));
    }
    #[test]
    fn job_paths_cannot_escape_the_application_root() {
        for invalid in [
            "../lecture",
            "/tmp",
            "..",
            "abcd",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/",
        ] {
            assert!(job_path(Path::new("jobs"), invalid).is_err());
        }
        assert!(job_path(Path::new("jobs"), &"a".repeat(32)).is_ok());
    }
    #[test]
    fn live_worker_detection_uses_an_os_lock() -> Result<(), io::Error> {
        let directory = tempfile::tempdir()?;
        assert!(!is_worker_running(directory.path())?);
        let worker = worker_lock(directory.path())?;
        worker.lock()?;
        assert!(is_worker_running(directory.path())?);
        drop(worker);
        assert!(!is_worker_running(directory.path())?);
        Ok(())
    }

    #[test]
    fn interrupted_elapsed_uses_only_the_current_attempt_checkpoint() -> Result<(), io::Error> {
        let directory = tempfile::tempdir()?;
        let run = Run {
            number: 1,
            settings: Settings {
                backend: ModelBackendKind::OpenAiCompatible,
                base_url: "http://localhost/v1".into(),
                model: "test".into(),
                codex_reasoning_effort: None,
                codex_service_tier: None,
                extra_body: None,
                max_concurrency: 2,
                request_interval_ms: 0,
                adaptive: false,
                boundary_passages: false,
            },
            started_ms: 1000,
            elapsed_before_ms: 500,
        };
        assert_eq!(run.checkpointed_elapsed(directory.path())?, 500);
        let path = run.directory(directory.path()).join("control/elapsed.json");
        fs::create_dir_all(path.parent().unwrap())?;
        write_json_atomically(
            &path,
            &ElapsedCheckpoint {
                started_ms: 1000,
                elapsed_ms: 1750,
            },
            "test",
        )?;
        assert_eq!(run.checkpointed_elapsed(directory.path())?, 1750);
        write_json_atomically(
            &path,
            &ElapsedCheckpoint {
                started_ms: 999,
                elapsed_ms: 8000,
            },
            "test",
        )?;
        assert_eq!(run.checkpointed_elapsed(directory.path())?, 500);
        Ok(())
    }
}
