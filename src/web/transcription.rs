use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::Stdio,
    time::Instant,
};

use beyond_slides::{SlideDeck, TimedTranscript, Transcript, ValidatedSources};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::jobs::{Job, save_job};
use crate::{
    run_support::{read_json, write_json_atomically},
    worker_control::{Stage, WorkerControl},
};

#[derive(Deserialize, Serialize)]
struct Transcription {
    transcript: Transcript,
    timed_tokens: Option<TimedTranscript>,
    timing_warning: Option<String>,
    metadata: serde_json::Value,
}

#[derive(Deserialize, Serialize)]
struct Checkpoint {
    recording_sha256: String,
    result: Transcription,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct Progress {
    pub observer_version: u32,
    pub phase: String,
    pub attempt_started_ms: u64,
    pub completed_regions: usize,
    pub total_regions: Option<usize>,
    pub completed_speech_ms: u64,
    pub total_speech_ms: Option<u64>,
    pub timings_seconds: BTreeMap<String, f64>,
    pub reused: bool,
}

pub(super) fn read_progress(run: &Path, attempt: u64) -> Result<Option<Progress>, io::Error> {
    let path = run.join("control/asr-progress.json");
    if !path.exists() {
        return Ok(None);
    }
    let progress: Progress = read_json(&path, "transcription progress")?;
    Ok((progress.attempt_started_ms == attempt).then_some(progress))
}

fn publish_progress(run: &Path, progress: &Progress) -> Result<(), io::Error> {
    write_json_atomically(
        &run.join("control/asr-progress.json"),
        progress,
        "transcription progress",
    )
}

pub(super) async fn prepare(
    directory: &Path,
    run: &Path,
    job: &mut Job,
) -> Result<(), Box<dyn Error>> {
    if !job.transcribe_recording {
        return Ok(());
    }
    let control = WorkerControl::from_environment()?;
    control.check_stop()?;
    control.progress(Stage::Transcription, 0, None)?;
    let attempt = job.runs.last().ok_or("Missing run metadata")?.started_ms;
    let mut progress = Progress {
        phase: "checking_recording".into(),
        attempt_started_ms: attempt,
        ..Progress::default()
    };
    publish_progress(run, &progress)?;
    let recording = directory.join(job.recording.as_ref().ok_or("Missing recording")?);
    let recording_sha256 = file_hash(&recording)?;
    let asr_directory = directory.join("transcription");
    fs::create_dir_all(&asr_directory)?;
    let checkpoint_path = asr_directory.join("checkpoint.json");
    let checkpoint: Checkpoint = if checkpoint_path.is_file() {
        let checkpoint: Checkpoint = read_json(&checkpoint_path, "transcription checkpoint")?;
        if checkpoint.recording_sha256 != recording_sha256 {
            return Err("Recording changed after transcription; import it as a new lecture".into());
        }
        progress.reused = true;
        progress.timings_seconds =
            serde_json::from_value(checkpoint.result.metadata["timings_seconds"].clone())
                .unwrap_or_default();
        checkpoint
    } else {
        progress.phase = "extracting_audio".into();
        publish_progress(run, &progress)?;
        let extracting = Instant::now();
        let audio = asr_directory.join("audio.wav");
        let status = tokio::process::Command::new("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(&recording)
            .args(["-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
            .arg(&audio)
            .status()
            .await
            .map_err(|e| format!("Could not extract audio with ffmpeg: {e}"))?;
        if !status.success() {
            return Err("Audio extraction failed; see worker.log".into());
        }
        let extraction_seconds = extracting.elapsed().as_secs_f64();
        progress
            .timings_seconds
            .insert("extracting_audio".into(), extraction_seconds);
        progress.phase = "loading_models".into();
        publish_progress(run, &progress)?;
        control.check_stop()?;
        let script = asr_directory.join("transcribe.py");
        fs::write(
            &script,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/scripts/transcribe_recording.py"
            )),
        )?;
        // Preserve even a rejected attempt's output for diagnosis. Only the
        // validated checkpoint below is eligible for reuse.
        let result_path = asr_directory.join(format!("result-{attempt}.json"));
        let python = python_executable();
        let mut command = tokio::process::Command::new(&python);
        command
            .arg("-u")
            .arg(&script)
            .arg(&audio)
            .arg(&result_path)
            .arg(run.join("control/asr-progress.json"))
            .arg("--attempt-started-ms")
            .arg(attempt.to_string())
            .arg("--extraction-seconds")
            .arg(extraction_seconds.to_string())
            .env_remove("BEYOND_SLIDES_API_KEY")
            .env("CUDA_VISIBLE_DEVICES", "")
            .stdin(Stdio::null());
        let key = std::env::var("BEYOND_SLIDES_API_KEY").unwrap_or_default();
        let status = super::logs::capture(&mut command, &asr_directory.join("asr-debug.log"), &key).await
            .map_err(|e| format!("Could not run local ASR Python {}: {e}. See docs/local-application.md for setup.", python.display()))?;
        if !status.success() {
            return Err(format!("Local CPU transcription failed ({status}). See {}. Install FunASR and CPU PyTorch in the configured Python environment.", asr_directory.join("asr-debug.log").display()).into());
        }
        let mut result: Transcription = read_json(&result_path, "local ASR result")?;
        if let Some(observed) = read_progress(run, attempt)? {
            progress = observed;
        }
        result.metadata["timings_seconds"] = serde_json::to_value(&progress.timings_seconds)?;
        let checkpoint = Checkpoint {
            recording_sha256,
            result,
        };
        validate(directory, &checkpoint.result)?;
        write_json_atomically(&checkpoint_path, &checkpoint, "transcription checkpoint")?;
        checkpoint
    };
    let saving = Instant::now();
    progress.phase = "finalizing".into();
    publish_progress(run, &progress)?;
    validate(directory, &checkpoint.result)?;
    write_json_atomically(
        &directory.join("transcript.json"),
        &checkpoint.result.transcript,
        "transcript",
    )?;
    if let Some(timing) = &checkpoint.result.timed_tokens {
        write_json_atomically(
            &directory.join("timed-tokens.json"),
            timing,
            "timed transcript",
        )?;
    }
    let segments = &checkpoint.result.transcript.segments;
    job.preview.segment_count = segments.len();
    job.preview.duration_ms = segments.last().and_then(|s| s.end_ms);
    job.preview.transcript_sample = segments
        .iter()
        .flat_map(|s| s.text.chars())
        .take(700)
        .collect();
    if let Some(warning) = checkpoint.result.timing_warning {
        let warning = format!(
            "Fine-grained ASR timing unavailable; playback uses coarse transcript segments: {warning}"
        );
        if !job.preview.warnings.contains(&warning) {
            job.preview.warnings.push(warning);
        }
    }
    save_job(directory, job)?;
    progress
        .timings_seconds
        .insert("finalizing".into(), saving.elapsed().as_secs_f64());
    progress.phase = "complete".into();
    publish_progress(run, &progress)?;
    control.progress(Stage::Transcription, 1, Some(1))?;
    control.check_stop()?;
    Ok(())
}

fn validate(directory: &Path, result: &Transcription) -> Result<(), Box<dyn Error>> {
    if result.transcript.segments.is_empty() {
        return Err("ASR returned no transcript segments".into());
    }
    if !result.transcript.has_timestamps() {
        return Err("ASR did not provide source timing".into());
    }
    let slides: SlideDeck = read_json(&directory.join("slides.json"), "slides")?;
    ValidatedSources::new(result.transcript.clone(), slides)?;
    Ok(())
}

fn python_executable() -> PathBuf {
    if let Some(path) = std::env::var_os("BEYOND_SLIDES_ASR_PYTHON") {
        return path.into();
    }
    let local = Path::new(env!("CARGO_MANIFEST_DIR")).join(if cfg!(windows) {
        ".venv/Scripts/python.exe"
    } else {
        ".venv/bin/python"
    });
    if local.is_file() {
        local
    } else {
        PathBuf::from("python3")
    }
}

fn file_hash(path: &Path) -> Result<String, io::Error> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 64 * 1024];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_from_a_previous_attempt_is_not_presented_as_live() -> io::Result<()> {
        let run = tempfile::tempdir()?;
        fs::create_dir(run.path().join("control"))?;
        publish_progress(
            run.path(),
            &Progress {
                phase: "recognizing".into(),
                attempt_started_ms: 10,
                ..Progress::default()
            },
        )?;
        assert!(read_progress(run.path(), 11)?.is_none());
        assert_eq!(read_progress(run.path(), 10)?.unwrap().phase, "recognizing");
        Ok(())
    }
}
