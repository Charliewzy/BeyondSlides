use std::{
    error::Error,
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use beyond_slides::{
    ChatCompletionsClient, ModelExchangeTrace, RestorationProgressError, RestoredTranscriptSpan,
    TranscriptRestorationConfig, TranscriptRestorationSession, WindowingConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::run_support::{
    ProviderSettings, checkpoint_path, display_token_count, initialize_run_directory_with,
    open_run_model_trace, read_json, read_json_with_hash, sha256, window_progress_bar,
    write_json_atomically, write_text_atomically,
};

const RESTORATION_RUN_FORMAT_VERSION: u32 = 1;
const MAX_OWNED_CHARACTERS: usize = 400;
const MAX_OWNED_DURATION_SECONDS: u64 = 60;
const CONTEXT_CHARACTERS: usize = 150;
const MAX_FINAL_ANSWER_REPAIRS: usize = 2;
const MAX_PROVIDER_RETRIES: usize = 2;
const MAX_OUTPUT_TOKENS: u32 = 8_192;
pub(crate) const RESTORED_TRANSCRIPT_FILE: &str = "restored-transcript.json";
const RESTORED_TEXT_FILE: &str = "restored-transcript.txt";
const DIAGNOSTICS_FILE: &str = "diagnostics.json";
const CANARY_TEXT_FILE: &str = "canary.txt";

pub async fn run_canary(
    transcript_path: &OsStr,
    run_directory: &OsStr,
) -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    let transcript_path = PathBuf::from(transcript_path);
    let run_directory = PathBuf::from(run_directory);
    let provider = ProviderSettings::from_restoration_environment()?;
    let (transcript, transcript_hash) = read_json_with_hash(&transcript_path, "transcript")?;
    let manifest = RestorationRunManifest::new(&provider, transcript_hash);
    initialize_run_directory_with(
        &run_directory,
        &manifest,
        "restoration",
        RestorationRunManifest::compatible,
    )?;
    provider.record_execution_settings(&run_directory)?;
    let client = restoration_client(&provider, open_run_model_trace(&run_directory)?)?;
    let mut session =
        TranscriptRestorationSession::prepare(&client, transcript, restoration_config(&provider)?)?;
    restore_checkpoints(&mut session, &run_directory)?;

    eprintln!("Sending transcript window 1 as the restoration canary...");
    let canary = session.restore_canary().await?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot run a restoration canary for an empty transcript",
        )
    })?;
    write_json_atomically(
        &checkpoint_path(&run_directory, 1),
        canary,
        "restoration window checkpoint",
    )?;
    let text = restored_span_text(&canary.restoration.spans);
    write_text_atomically(
        &run_directory.join(CANARY_TEXT_FILE),
        &text,
        "restoration canary text",
    )?;

    println!(
        "Restored canary window to {} in {:.1?} ({} spans, {} provider retries, {} final-answer repairs, prompt tokens: {}, completion tokens: {})",
        run_directory.display(),
        started.elapsed(),
        canary.restoration.spans.len(),
        canary.diagnostics.provider_retries,
        canary.diagnostics.final_answer_repairs,
        display_token_count(canary.diagnostics.prompt_tokens),
        display_token_count(canary.diagnostics.completion_tokens),
    );
    Ok(())
}

pub async fn run_complete(
    transcript_path: &OsStr,
    run_directory: &OsStr,
    shared_scheduler: Option<beyond_slides::RequestScheduler>,
) -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    let transcript_path = PathBuf::from(transcript_path);
    let run_directory = PathBuf::from(run_directory);
    let mut provider = ProviderSettings::from_restoration_environment()?;
    if let Some(scheduler) = shared_scheduler {
        provider = provider.with_scheduler(scheduler);
    }
    let (transcript, transcript_hash) = read_json_with_hash(&transcript_path, "transcript")?;
    let manifest = RestorationRunManifest::new(&provider, transcript_hash);
    initialize_run_directory_with(
        &run_directory,
        &manifest,
        "restoration",
        RestorationRunManifest::compatible,
    )?;
    provider.record_execution_settings(&run_directory)?;
    let client = restoration_client(&provider, open_run_model_trace(&run_directory)?)?;
    let mut session =
        TranscriptRestorationSession::prepare(&client, transcript, restoration_config(&provider)?)?;
    restore_checkpoints(&mut session, &run_directory)?;

    let progress = window_progress_bar(session.window_count(), session.completed_window_count())?;
    if session.window_count() > 0 {
        if session.completed_window_count() == 0 {
            progress.set_message("running canary");
        }
        let canary = session.restore_canary().await;
        provider.record_scheduling(&run_directory)?;
        let canary = canary?.expect("a nonempty transcript has a restoration canary");
        write_json_atomically(
            &checkpoint_path(&run_directory, 1),
            canary,
            "restoration window checkpoint",
        )?;
        progress.set_position(session.completed_window_count() as u64);
    }

    progress.set_message("restoring transcript windows");
    let result = session
        .complete_restoration_with_progress(|event| {
            let window_number = event.window_index + 1;
            write_json_atomically(
                &checkpoint_path(&run_directory, window_number),
                event.result,
                "restoration window checkpoint",
            )
            .map_err(|error| Box::new(error) as RestorationProgressError)?;
            progress.set_position(event.completed_windows as u64);
            provider
                .record_scheduling(&run_directory)
                .map_err(|error| Box::new(error) as RestorationProgressError)?;
            progress.set_message(
                provider.progress_message(&format!("completed window {window_number}")),
            );
            Ok(())
        })
        .await;
    provider.record_scheduling(&run_directory)?;
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            progress
                .abandon_with_message("restoration interrupted; validated checkpoints preserved");
            return Err(error.into());
        }
    };

    write_json_atomically(
        &run_directory.join(RESTORED_TRANSCRIPT_FILE),
        result.transcript(),
        "restored transcript",
    )?;
    write_text_atomically(
        &run_directory.join(RESTORED_TEXT_FILE),
        &result.transcript().text(),
        "restored transcript text",
    )?;
    write_json_atomically(
        &run_directory.join(DIAGNOSTICS_FILE),
        result.window_diagnostics(),
        "restoration diagnostics",
    )?;
    progress.finish_with_message("restoration complete");
    println!(
        "Wrote complete restored transcript to {} in {:.1?}",
        run_directory.display(),
        started.elapsed()
    );
    Ok(())
}

fn restoration_client(
    provider: &ProviderSettings,
    model_trace: ModelExchangeTrace,
) -> Result<ChatCompletionsClient, Box<dyn Error>> {
    let config = provider
        .chat_config()?
        .with_model_trace(model_trace)
        .with_max_provider_retries(MAX_PROVIDER_RETRIES)
        .with_max_final_answer_repairs(MAX_FINAL_ANSWER_REPAIRS)?
        .with_max_output_tokens(MAX_OUTPUT_TOKENS)?;
    Ok(ChatCompletionsClient::new(config))
}

fn restoration_config(
    provider: &ProviderSettings,
) -> Result<TranscriptRestorationConfig, Box<dyn Error>> {
    let windowing = WindowingConfig::new(
        MAX_OWNED_CHARACTERS,
        Duration::from_secs(MAX_OWNED_DURATION_SECONDS),
        CONTEXT_CHARACTERS,
    )?;
    Ok(TranscriptRestorationConfig::new(
        windowing,
        provider.max_concurrency(),
    )?)
}

fn restore_checkpoints(
    session: &mut TranscriptRestorationSession<'_>,
    run_directory: &Path,
) -> Result<(), Box<dyn Error>> {
    for window_index in 0..session.window_count() {
        let path = checkpoint_path(run_directory, window_index + 1);
        if path.exists() {
            let result = read_json(&path, "restoration window checkpoint")?;
            session.restore_window_checkpoint(window_index, result)?;
        }
    }
    let restored = session.completed_window_count();
    if restored > 0 {
        eprintln!(
            "Restored {restored}/{} validated restoration checkpoints",
            session.window_count()
        );
    }
    Ok(())
}

fn restored_span_text(spans: &[RestoredTranscriptSpan]) -> String {
    spans
        .iter()
        .filter_map(|span| match span {
            RestoredTranscriptSpan::Text { text, .. } => Some(text.as_str()),
            RestoredTranscriptSpan::OmittedDisfluency { .. } => None,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RestorationRunManifest {
    format_version: u32,
    transcript_sha256: String,
    restoration_prompt_sha256: String,
    api_base_url: String,
    model: String,
    #[serde(default)]
    chat_extra_body: Option<Value>,
    max_owned_characters: usize,
    max_owned_duration_seconds: u64,
    context_characters: usize,
    max_concurrent_windows: usize,
    max_final_answer_repairs: usize,
    max_provider_retries: usize,
    max_output_tokens: u32,
}

impl RestorationRunManifest {
    fn new(provider: &ProviderSettings, transcript_sha256: String) -> Self {
        Self {
            format_version: RESTORATION_RUN_FORMAT_VERSION,
            transcript_sha256,
            restoration_prompt_sha256: sha256(include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/restoration.md"
            ))),
            api_base_url: provider.base_url().into(),
            model: provider.model().into(),
            chat_extra_body: provider.extra_body().cloned(),
            max_owned_characters: MAX_OWNED_CHARACTERS,
            max_owned_duration_seconds: MAX_OWNED_DURATION_SECONDS,
            context_characters: CONTEXT_CHARACTERS,
            max_concurrent_windows: provider.max_concurrency(),
            max_final_answer_repairs: MAX_FINAL_ANSWER_REPAIRS,
            max_provider_retries: MAX_PROVIDER_RETRIES,
            max_output_tokens: MAX_OUTPUT_TOKENS,
        }
    }

    fn compatible(&self, expected: &Self) -> bool {
        let mut actual = self.clone();
        actual.max_concurrent_windows = expected.max_concurrent_windows;
        actual.max_provider_retries = expected.max_provider_retries;
        actual.max_final_answer_repairs = expected.max_final_answer_repairs;
        actual == *expected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restoration_resume_allows_scheduling_but_rejects_semantic_changes() {
        let provider = ProviderSettings::new("https://example.test/v1", "secret", "model");
        let original = RestorationRunManifest::new(&provider, "source".into());
        let mut changed = original.clone();
        changed.max_concurrent_windows = 20;
        changed.max_provider_retries = 6;
        changed.max_final_answer_repairs = 3;
        assert!(original.compatible(&changed));
        for field in [
            "transcript_sha256",
            "restoration_prompt_sha256",
            "model",
            "max_owned_characters",
            "context_characters",
            "max_output_tokens",
            "format_version",
        ] {
            let mut value = serde_json::to_value(&changed).unwrap();
            value[field] = if value[field].is_number() {
                serde_json::json!(999)
            } else {
                serde_json::json!("different")
            };
            assert!(
                !original.compatible(&serde_json::from_value(value).unwrap()),
                "{field}"
            );
        }
    }
}
