use std::{
    fs::{self, File, TryLockError},
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use beyond_slides::{
    ChatCompletionsConfig, Transcript, ValidatedSources,
    ingestion::{funasr, pdf},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::run_support::{read_json, write_json_atomically};

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
    pub duration_ms: u64,
    #[serde(default)]
    pub recording_duration_ms: Option<u64>,
    pub transcript_sample: String,
    pub slide_sample: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Settings {
    pub base_url: String,
    pub model: String,
    pub extra_body: Option<Value>,
    pub max_concurrency: usize,
    pub request_interval_ms: u64,
    pub adaptive: bool,
    pub boundary_passages: bool,
}

impl Settings {
    pub fn validate(&self, key: &str) -> Result<(), String> {
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
        self.base_url == other.base_url
            && self.model == other.model
            && self.extra_body == other.extra_body
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
    let transcript: Transcript =
        match transcript_extension {
            "" => Transcript {
                segments: Vec::new(),
            },
            "json" => serde_json::from_str(&input)
                .map_err(|e| format!("Invalid normalized transcript JSON: {e}"))?,
            "tsv" => funasr::import_tsv(&input).map_err(|e| e.to_string())?,
            _ => return Err(
                "This first import slice accepts normalized JSON or timestamped TSV transcripts"
                    .into(),
            ),
        };
    if !transcribe_recording && transcript.segments.is_empty() {
        return Err("The transcript contains no segments".into());
    }
    let sources =
        ValidatedSources::new(transcript, imported.slide_deck).map_err(|e| e.to_string())?;
    let transcript = sources.transcript();
    let deck = sources.slide_deck();
    let preview = Preview {
        slide_count: deck.slides.len(),
        segment_count: transcript.segments.len(),
        duration_ms: transcript.segments.last().map_or(0, |s| s.end_ms),
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
        warnings: imported.warnings.iter().map(|w| format!("{w:?}")).collect(),
    };
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
}
