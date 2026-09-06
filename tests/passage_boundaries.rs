use std::error::Error;

use beyond_slides::{
    AnnotationDiagnostics, BoundaryBatchResult, BoundaryDecision, BoundaryError,
    BoundarySegmentationPlan, BoundaryStrength, BoundaryWindowDecision, ProposedBoundaryBatch,
    RestoredTranscript, RestoredTranscriptSpan, SearchError, Slide, SlideDeck, SlideId, SlideScore,
    SlideScorer, Transcript, TranscriptSegment, TranscriptSegmentId, ValidatedSources,
};

fn restored(text: &str) -> RestoredTranscript {
    RestoredTranscript {
        spans: vec![RestoredTranscriptSpan::Text {
            source_start: TranscriptSegmentId(0),
            source_end: TranscriptSegmentId(0),
            text: text.into(),
        }],
    }
}

fn results(
    plan: &BoundarySegmentationPlan,
    strength: BoundaryStrength,
) -> Vec<BoundaryBatchResult> {
    plan.tasks()
        .iter()
        .map(|task| {
            let input = serde_json::to_value(task).unwrap();
            let windows = input["windows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|window| BoundaryWindowDecision {
                    window_index: window["window_index"].as_u64().unwrap() as usize,
                    boundaries: window["owned_boundary_after_atom_ids"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|id| BoundaryDecision {
                            after_atom: id.as_u64().unwrap() as usize,
                            strength,
                        })
                        .collect(),
                })
                .collect();
            BoundaryBatchResult {
                batch_index: task.batch_index(),
                decisions: ProposedBoundaryBatch { windows },
                diagnostics: AnnotationDiagnostics::default(),
            }
        })
        .collect()
}

#[test]
fn passage_can_cross_both_window_and_batch_edges() {
    let source = restored(&"甲。".repeat(98));
    let plan = BoundarySegmentationPlan::new(&source);
    assert_eq!(plan.tasks().len(), 2);
    let mut decisions = results(&plan, BoundaryStrength::Continue);
    // Completion order is unrelated to transcript order.
    decisions.reverse();
    let partition = plan.partition(&decisions).unwrap();
    assert_eq!(partition, vec![0..source.text().len()]);
    for task in plan.tasks() {
        let input = serde_json::to_value(task).unwrap();
        for window in input["windows"].as_array().unwrap() {
            let visible: Vec<_> = window["atoms"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a["atom_id"].as_u64().unwrap())
                .collect();
            for id in window["owned_boundary_after_atom_ids"].as_array().unwrap() {
                let id = id.as_u64().unwrap();
                assert!(visible.contains(&id) && visible.contains(&(id + 1)));
            }
        }
    }
}

#[test]
fn required_breaks_are_constraints_even_when_they_create_short_passages() {
    let source = restored("你好。再见。");
    let plan = BoundarySegmentationPlan::new(&source);
    let partition = plan
        .partition(&results(&plan, BoundaryStrength::RequiredBreak))
        .unwrap();
    let text = source.text();
    assert_eq!(
        partition
            .iter()
            .map(|r| &text[r.clone()])
            .collect::<Vec<_>>(),
        vec!["你好。", "再见。"]
    );
}

#[test]
fn immutable_utf8_partition_preserves_spaces_emoji_and_code_paths() {
    let text = " 类型是 Option<Self::Item>，不是别的：说明在这里。\n🦀 e\u{301}。";
    let source = restored(text);
    let plan = BoundarySegmentationPlan::new(&source);
    let parts = plan
        .partition(&results(&plan, BoundaryStrength::PreferredBreak))
        .unwrap();
    assert_eq!(
        parts.iter().map(|r| &text[r.clone()]).collect::<String>(),
        text
    );
    let input = serde_json::to_string(plan.tasks()).unwrap();
    assert!(input.contains("Option<Self::Item>"));
    for range in parts {
        assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
    }
}

#[test]
fn rejects_missing_duplicate_foreign_and_wrongly_numbered_checkpoint_entries() {
    let plan = BoundarySegmentationPlan::new(&restored(&"甲。".repeat(100)));
    let valid = results(&plan, BoundaryStrength::PreferredBreak);
    for mutation in 0..7 {
        let mut invalid = valid.clone();
        match mutation {
            0 => {
                invalid.pop();
            }
            1 => {
                invalid[1] = invalid[0].clone();
            }
            2 => {
                invalid[0].decisions.windows[0].boundaries.pop();
            }
            3 => {
                let duplicate = invalid[0].decisions.windows[0].boundaries[0].clone();
                invalid[0].decisions.windows[0].boundaries[1] = duplicate;
            }
            4 => {
                invalid[0].decisions.windows[0].boundaries[0].after_atom = 999;
            }
            5 => {
                invalid[0].decisions.windows[0].window_index = 999;
            }
            6 => {
                invalid[0].batch_index = 999;
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(
                plan.partition(&invalid),
                Err(BoundaryError::InvalidResponse(_))
            ),
            "mutation {mutation}"
        );
    }
}

#[test]
fn impossible_semantic_constraints_fail_instead_of_silently_cutting() {
    let plan = BoundarySegmentationPlan::new(&restored(&"甲。".repeat(230)));
    assert!(matches!(
        plan.partition(&results(&plan, BoundaryStrength::Continue)),
        Err(BoundaryError::NoLegalPartition { .. })
    ));
    let plan = BoundarySegmentationPlan::new(&restored(&"甲".repeat(451)));
    assert!(matches!(
        plan.partition(&[]),
        Err(BoundaryError::Unbreakable { .. })
    ));
}

#[test]
fn empty_and_single_atom_need_no_provider_request() {
    for text in ["", "没有标点但仍然完整"] {
        let plan = BoundarySegmentationPlan::new(&restored(text));
        assert!(plan.tasks().is_empty());
        let ranges = plan.partition(&[]).unwrap();
        assert_eq!(
            ranges.iter().map(|r| &text[r.clone()]).collect::<String>(),
            text
        );
    }
}

struct Scorer;
impl SlideScorer for Scorer {
    fn score_slides(&self, _: &str) -> Result<Vec<SlideScore>, SearchError> {
        Ok(vec![SlideScore {
            slide_id: SlideId(0),
            score: 1.0,
        }])
    }
}

#[test]
fn assembly_preserves_coarse_provenance_and_does_not_invent_evidence() -> Result<(), Box<dyn Error>>
{
    let sources = || {
        ValidatedSources::new(
            Transcript {
                segments: (0..3)
                    .map(|id| TranscriptSegment {
                        id: TranscriptSegmentId(id),
                        text: "原文".into(),
                        start_ms: id as u64 * 1000,
                        end_ms: (id + 1) as u64 * 1000,
                    })
                    .collect(),
            },
            SlideDeck {
                slides: vec![Slide {
                    id: SlideId(0),
                    text: "类型".into(),
                }],
            },
        )
    };
    let restored = RestoredTranscript {
        spans: vec![
            RestoredTranscriptSpan::OmittedDisfluency {
                source_start: TranscriptSegmentId(0),
                source_end: TranscriptSegmentId(0),
            },
            RestoredTranscriptSpan::Text {
                source_start: TranscriptSegmentId(1),
                source_end: TranscriptSegmentId(2),
                text: "第一个完整意思。第二个完整意思。".into(),
            },
        ],
    };
    let plan = BoundarySegmentationPlan::new(&restored);
    let decisions = results(&plan, BoundaryStrength::RequiredBreak);
    let analysis = plan.assemble(sources()?, restored.clone(), &Scorer, &decisions)?;
    assert_eq!(analysis.passages().len(), 2);
    for passage in analysis.passages() {
        assert_eq!(passage.source_start, TranscriptSegmentId(1));
        assert_eq!(passage.source_end, TranscriptSegmentId(2));
        assert!(passage.related_slides.is_empty() && passage.summary.is_none());
        assert!(passage.connection_strength.is_none());
        assert!(passage.comparative_importance.is_none() && passage.comparative_novelty.is_none());
    }
    assert_eq!(analysis.restored_transcript(), &restored);
    assert!(
        plan.assemble(sources()?, self::restored("别的文本"), &Scorer, &decisions)
            .is_err()
    );
    Ok(())
}
