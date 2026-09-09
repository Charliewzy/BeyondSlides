use std::{collections::BTreeMap, error::Error, fs, io, path::Path, time::Instant};

use super::{
    jobs::{Job, save_job},
    speech_recognition,
};
use crate::{
    run_support::{read_json, write_json_atomically},
    worker_control::{Stage, WorkerControl},
};
use beyond_slides::{SlideDeck, TimedTranscript, Transcript, ValidatedSources, runtime_tools};
use serde::{Deserialize, Serialize};

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
    pub downloaded_model_bytes: u64,
    pub total_model_bytes: Option<u64>,
    pub timings_seconds: BTreeMap<String, f64>,
    pub reused: bool,
    /// Filled by the controller, never trusted from a worker checkpoint.
    #[serde(skip_deserializing)]
    pub recognition_eta_ms: Option<u64>,
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
        observer_version: 1,
        phase: "checking_recording".into(),
        attempt_started_ms: attempt,
        ..Progress::default()
    };
    publish_progress(run, &progress)?;
    let recording = directory.join(job.recording.as_ref().ok_or("Missing recording")?);
    let recording_sha256 = super::model_assets::file_hash(&recording)?;
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
        let ffmpeg = runtime_tools::ffmpeg_path()
            .map_err(|error| format!("Could not prepare FFmpeg: {error}"))?;
        let status = tokio::process::Command::new(ffmpeg)
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
        progress.phase = "downloading_models".into();
        publish_progress(run, &progress)?;
        control.check_stop()?;
        let data_root = directory
            .parent()
            .ok_or("lecture directory has no application data root")?;
        let models = speech_recognition::ensure_models(data_root, |downloaded, total| {
            progress.downloaded_model_bytes = downloaded;
            progress.total_model_bytes = Some(total);
            publish_progress(run, &progress)?;
            control.check_stop()?;
            Ok(())
        })
        .await?;
        progress.phase = "loading_models".into();
        publish_progress(run, &progress)?;
        let output = speech_recognition::transcribe(
            &audio,
            &models,
            |observed| {
                progress.phase = observed.phase.into();
                progress.completed_regions = observed.completed_regions;
                progress.total_regions = Some(observed.total_regions);
                progress.completed_speech_ms = observed.completed_speech_ms;
                progress.total_speech_ms = Some(observed.total_speech_ms);
                publish_progress(run, &progress)?;
                Ok(())
            },
            || {
                control.check_stop()?;
                Ok(())
            },
        )?;
        let mut result = Transcription {
            transcript: output.transcript,
            timed_tokens: output.timed_tokens,
            timing_warning: output.timing_warning,
            metadata: output.metadata,
        };
        progress.timings_seconds =
            serde_json::from_value(result.metadata["timings_seconds"].clone()).unwrap_or_default();
        progress
            .timings_seconds
            .insert("extracting_audio".into(), extraction_seconds);
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
    if progress.reused {
        control.baseline(Stage::Transcription, 1, 1)?;
    } else {
        control.progress(Stage::Transcription, 1, Some(1))?;
    }
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
