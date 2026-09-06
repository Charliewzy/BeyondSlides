//! Checkpoint persistence and execution for boundary-first passage preparation.

use std::{
    error::Error,
    path::Path,
    time::{Duration, Instant},
};

use beyond_slides::processing::{BatchRunError, run_bounded};
use beyond_slides::{
    BoundaryBatchResult, BoundarySegmentationPlan, ChatCompletionsClient,
    PASSAGE_BOUNDARY_INSTRUCTIONS, RestoredTranscript, SlideScorer, ValidatedRestoredAnalysis,
    ValidatedSources,
};
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::run_support::{
    ProviderSettings, initialize_run_directory, read_json, sha256, write_json_atomically,
};

/// Only inputs affecting model classification belong here. Length-selection
/// policy, slide retrieval, scheduling, and comparative prompts do not.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BoundaryRunIdentity {
    format_version: u32,
    task_plan_sha256: String,
    prompt_sha256: String,
    api_base_url: String,
    model: String,
    extra_body: Option<Value>,
    max_output_tokens: u32,
}

impl BoundaryRunIdentity {
    fn new(
        provider: &ProviderSettings,
        plan: &BoundarySegmentationPlan,
        max_output_tokens: u32,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            format_version: 1,
            task_plan_sha256: sha256(&serde_json::to_vec(plan.tasks())?),
            prompt_sha256: sha256(PASSAGE_BOUNDARY_INSTRUCTIONS.as_bytes()),
            api_base_url: provider.base_url().into(),
            model: provider.model().into(),
            extra_body: provider.extra_body().cloned(),
            max_output_tokens,
        })
    }

    fn fingerprint(&self) -> Result<String, serde_json::Error> {
        Ok(sha256(&serde_json::to_vec(self)?))
    }
}

pub(crate) struct BoundaryPreparation {
    pub analysis: ValidatedRestoredAnalysis,
    pub identity: BoundaryRunIdentity,
}

pub(crate) async fn prepare(
    client: &ChatCompletionsClient,
    provider: &ProviderSettings,
    sources: ValidatedSources,
    restored: RestoredTranscript,
    scorer: &dyn SlideScorer,
    run_directory: &Path,
    max_output_tokens: u32,
) -> Result<BoundaryPreparation, Box<dyn Error>> {
    provider.worker.check_stop()?;
    provider
        .worker
        .progress(crate::worker_control::Stage::Passages, 0, None)?;
    let started = Instant::now();
    let plan = BoundarySegmentationPlan::new(&restored);
    let identity = BoundaryRunIdentity::new(provider, &plan, max_output_tokens)?;
    let directory = run_directory
        .join("boundaries")
        .join(identity.fingerprint()?);
    initialize_run_directory(&directory, &identity, "boundary classification")?;
    let tasks = plan.tasks();
    let mut results = vec![None; tasks.len()];
    for task in tasks {
        let path = directory.join(format!("batch-{:04}.json", task.batch_index() + 1));
        if path.exists() {
            let result: BoundaryBatchResult = read_json(&path, "boundary checkpoint")?;
            if result.batch_index != task.batch_index() {
                return Err(format!(
                    "boundary checkpoint {} has the wrong batch index",
                    path.display()
                )
                .into());
            }
            task.validate(&result.decisions)?;
            results[task.batch_index()] = Some(result);
        }
    }
    let restored_count = results.iter().flatten().count();
    provider.worker.baseline(
        crate::worker_control::Stage::Passages,
        restored_count,
        tasks.len(),
    )?;
    if restored_count > 0 {
        eprintln!(
            "Restored {restored_count}/{} validated boundary batches",
            tasks.len()
        );
    }
    let progress = ProgressBar::new(tasks.len() as u64);
    progress.set_style(ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} boundary batches {msg}"
    )?.progress_chars("=>-"));
    progress.set_position(restored_count as u64);
    progress.enable_steady_tick(Duration::from_millis(100));

    let execution = async {
        if let Some(task) = tasks.first()
            && results[0].is_none()
        {
            provider.worker.check_stop()?;
            progress.set_message("running boundary canary");
            let result = client.classify_passage_boundaries(task).await?;
            write_json_atomically(
                &directory.join("batch-0001.json"),
                &result,
                "boundary checkpoint",
            )?;
            results[0] = Some(result);
            progress.inc(1);
            provider.worker.progress(
                crate::worker_control::Stage::Passages,
                progress.position() as usize,
                Some(tasks.len()),
            )?;
            provider.record_scheduling(run_directory)?;
        }
        let pending = tasks
            .iter()
            .filter(|t| results[t.batch_index()].is_none())
            .collect::<Vec<_>>();
        run_bounded(
            pending,
            std::num::NonZeroUsize::new(provider.max_concurrency()).expect("validated concurrency"),
            &provider.worker.stop_signal(),
            |task| async move {
                client
                    .classify_passage_boundaries(task)
                    .await
                    .map_err(|error| Box::new(error) as Box<dyn Error>)
            },
            |result| {
                let index = result.batch_index;
                write_json_atomically(
                    &directory.join(format!("batch-{:04}.json", index + 1)),
                    &result,
                    "boundary checkpoint",
                )?;
                results[index] = Some(result);
                progress.inc(1);
                provider.worker.progress(
                    crate::worker_control::Stage::Passages,
                    progress.position() as usize,
                    Some(tasks.len()),
                )?;
                progress.set_message(
                    provider.progress_message(&format!("classified batch {}", index + 1)),
                );
                provider.record_scheduling(run_directory)?;
                Ok(())
            },
        )
        .await
        .map_err(|error| match error {
            BatchRunError::Stopped => Box::new(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "boundary classification stopped; completed checkpoints preserved",
            )) as Box<dyn Error>,
            BatchRunError::Work(error) => error,
        })?;
        Ok::<(), Box<dyn Error>>(())
    }
    .await;
    provider.record_scheduling(run_directory)?;
    if let Err(error) = execution {
        progress.abandon_with_message("classification interrupted; checkpoints preserved");
        return Err(error);
    }
    progress.finish_with_message("boundary classification complete");
    let results: Vec<_> = results.into_iter().flatten().collect();
    let classification_seconds = started.elapsed().as_secs_f64();
    eprintln!("Selecting global passages and inferring their slide positions...");
    let analysis = plan.assemble(sources, restored, scorer, &results)?;
    let lengths = analysis
        .passages()
        .iter()
        .map(|p| p.text.chars().count())
        .collect::<Vec<_>>();
    let diagnostics = serde_json::json!({
        "mode": "boundaries", "classification_identity": identity,
        "batch_count": results.len(), "restored_batch_count": restored_count,
        "classification_seconds_this_invocation": classification_seconds,
        "preparation_seconds_this_invocation": started.elapsed().as_secs_f64(),
        "passage_count": lengths.len(), "passage_character_counts": lengths,
        "exact_source_coverage": true,
        "batch_diagnostics": results.iter().map(|r| &r.diagnostics).collect::<Vec<_>>(),
        "unassessed_fields": ["connection_strength", "related_slides", "summary", "comparison_note"],
    });
    write_json_atomically(
        &run_directory.join("boundary-preparation.json"),
        &diagnostics,
        "boundary diagnostics",
    )?;
    Ok(BoundaryPreparation { analysis, identity })
}

#[cfg(test)]
mod tests {
    use super::*;
    use beyond_slides::{RestoredTranscriptSpan, TranscriptSegmentId};

    #[test]
    fn identity_tracks_model_inputs_not_scheduling_or_keys() -> Result<(), Box<dyn Error>> {
        let transcript = |text: &str| RestoredTranscript {
            spans: vec![RestoredTranscriptSpan::Text {
                source_start: TranscriptSegmentId(0),
                source_end: TranscriptSegmentId(0),
                text: text.into(),
            }],
        };
        let plan = BoundarySegmentationPlan::new(&transcript("第一句。第二句。"));
        let provider = ProviderSettings::new("https://example.test/v1", "secret", "model");
        let identity = BoundaryRunIdentity::new(&provider, &plan, 16_384)?;
        let other_key =
            ProviderSettings::new("https://example.test/v1", "different-secret", "model");
        assert_eq!(
            identity,
            BoundaryRunIdentity::new(&other_key, &plan, 16_384)?
        );
        assert_ne!(identity, BoundaryRunIdentity::new(&provider, &plan, 8192)?);
        let different = BoundarySegmentationPlan::new(&transcript("第一句。第二个句子。"));
        assert_ne!(
            identity,
            BoundaryRunIdentity::new(&provider, &different, 16_384)?
        );
        assert!(!serde_json::to_string(&identity)?.contains("secret"));
        Ok(())
    }
}
