use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{ModelTraceEvent, ModelTraceRecord, ModelWorkflow, RestoredAnalysisArtifact};

const PARTITION_REJECTION_CATEGORIES: &[&str] = &[
    "passage_text_difference",
    "no_passages",
    "empty_passage",
    "invalid_restored_analysis",
];

/// Quality measurements for model-proposed passage partitions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnnotationQualitySummary {
    pub analyzed_windows: usize,
    pub exact_accepted_attempts: usize,
    pub fuzzy_accepted_attempts: usize,
    pub rejected_attempts: usize,
    pub rejected_window_count: usize,
    pub candidate_attempt_count: usize,
    pub exact_attempt_percent: f64,
    pub fuzzy_attempt_percent: f64,
    pub rejected_attempt_percent: f64,
    pub changed_characters: usize,
    pub compared_characters: usize,
    pub final_answer_repairs: usize,
}

/// Combines accepted projection diagnostics with rejected attempts in the trace.
pub fn summarize_annotation_quality(
    artifact: &RestoredAnalysisArtifact,
    trace: &[ModelTraceRecord],
) -> AnnotationQualitySummary {
    let exact_accepted_attempts = artifact
        .window_projections
        .iter()
        .filter(|projection| projection.is_exact())
        .count();
    let fuzzy_accepted_attempts = artifact.window_projections.len() - exact_accepted_attempts;
    let mut rejected_attempts = 0;
    let mut rejected_windows = BTreeSet::new();
    for record in trace {
        let ModelTraceEvent::Validation {
            accepted: false,
            category: Some(category),
            ..
        } = &record.event
        else {
            continue;
        };
        if record.workflow == ModelWorkflow::Annotation
            && PARTITION_REJECTION_CATEGORIES.contains(&category.as_str())
        {
            rejected_attempts += 1;
            rejected_windows.insert(record.window_index);
        }
    }

    let candidate_attempt_count =
        exact_accepted_attempts + fuzzy_accepted_attempts + rejected_attempts;
    AnnotationQualitySummary {
        analyzed_windows: artifact.window_projections.len(),
        exact_accepted_attempts,
        fuzzy_accepted_attempts,
        rejected_attempts,
        rejected_window_count: rejected_windows.len(),
        candidate_attempt_count,
        exact_attempt_percent: percentage(exact_accepted_attempts, candidate_attempt_count),
        fuzzy_attempt_percent: percentage(fuzzy_accepted_attempts, candidate_attempt_count),
        rejected_attempt_percent: percentage(rejected_attempts, candidate_attempt_count),
        changed_characters: artifact
            .window_projections
            .iter()
            .map(|projection| projection.changed_characters)
            .sum(),
        compared_characters: artifact
            .window_projections
            .iter()
            .map(|projection| projection.compared_characters)
            .sum(),
        final_answer_repairs: artifact
            .window_diagnostics
            .iter()
            .map(|diagnostics| diagnostics.final_answer_repairs)
            .sum(),
    }
}

pub fn render_annotation_quality(summary: &AnnotationQualitySummary) -> String {
    format!(
        "Passage partition quality\n\
         Windows: {}\n\
         Candidate attempts: {}\n\
           exact accepted: {} ({:.1}%)\n\
           fuzzy accepted: {} ({:.1}%)\n\
           rejected: {} ({:.1}%) across {} windows\n\
         Accepted copy differences: {}/{} characters ({:.2}%)\n\
         Final-answer repair turns: {}\n",
        summary.analyzed_windows,
        summary.candidate_attempt_count,
        summary.exact_accepted_attempts,
        summary.exact_attempt_percent,
        summary.fuzzy_accepted_attempts,
        summary.fuzzy_attempt_percent,
        summary.rejected_attempts,
        summary.rejected_attempt_percent,
        summary.rejected_window_count,
        summary.changed_characters,
        summary.compared_characters,
        percentage(summary.changed_characters, summary.compared_characters),
        summary.final_answer_repairs,
    )
}

fn percentage(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 * 100.0 / denominator as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AnnotationDiagnostics, MODEL_TRACE_FORMAT_VERSION, ModelRequestKind,
        PassageProjectionDiagnostics, RestoredTranscript, SlideId,
    };

    #[test]
    fn separates_final_exact_and_fuzzy_outcomes_from_rejected_attempts() {
        let artifact = RestoredAnalysisArtifact {
            restored_transcript: RestoredTranscript { spans: Vec::new() },
            passages: Vec::new(),
            slide_positions: vec![SlideId(0), SlideId(1)],
            window_diagnostics: vec![diagnostics(0), diagnostics(1)],
            window_projections: vec![projection(0, 80), projection(2, 100)],
        };
        let trace = vec![
            validation(0, false, Some("passage_text_difference")),
            validation(0, false, Some("unknown_related_slide")),
            validation(1, true, None),
            restoration_validation(1, false, Some("empty_passage")),
        ];

        let summary = summarize_annotation_quality(&artifact, &trace);

        assert_eq!(summary.analyzed_windows, 2);
        assert_eq!(summary.exact_accepted_attempts, 1);
        assert_eq!(summary.fuzzy_accepted_attempts, 1);
        assert_eq!(summary.rejected_attempts, 1);
        assert_eq!(summary.rejected_window_count, 1);
        assert_eq!(summary.candidate_attempt_count, 3);
        assert_eq!(summary.changed_characters, 2);
        assert_eq!(summary.compared_characters, 180);
        assert_eq!(summary.final_answer_repairs, 1);
    }

    fn diagnostics(final_answer_repairs: usize) -> AnnotationDiagnostics {
        AnnotationDiagnostics {
            provider_retries: 0,
            tool_rounds: 0,
            final_answer_repairs,
            prompt_tokens: Some(0),
            completion_tokens: Some(0),
            accepted_json_fence: false,
        }
    }

    fn projection(
        changed_characters: usize,
        compared_characters: usize,
    ) -> PassageProjectionDiagnostics {
        PassageProjectionDiagnostics {
            changed_characters,
            compared_characters,
        }
    }

    fn validation(window_index: usize, accepted: bool, category: Option<&str>) -> ModelTraceRecord {
        trace_record(ModelWorkflow::Annotation, window_index, accepted, category)
    }

    fn restoration_validation(
        window_index: usize,
        accepted: bool,
        category: Option<&str>,
    ) -> ModelTraceRecord {
        trace_record(ModelWorkflow::Restoration, window_index, accepted, category)
    }

    fn trace_record(
        workflow: ModelWorkflow,
        window_index: usize,
        accepted: bool,
        category: Option<&str>,
    ) -> ModelTraceRecord {
        ModelTraceRecord {
            format_version: MODEL_TRACE_FORMAT_VERSION,
            event_index: 0,
            timestamp_unix_ms: 0,
            exchange_id: 0,
            workflow,
            window_index,
            conversation_turn: 0,
            request_kind: ModelRequestKind::Initial,
            event: ModelTraceEvent::Validation {
                accepted,
                category: category.map(str::to_owned),
                error: None,
            },
        }
    }
}
