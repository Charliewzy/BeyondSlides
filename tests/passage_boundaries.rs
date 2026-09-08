use std::error::Error;

use beyond_slides::{
    AnnotationDiagnostics, BoundaryBatchResult, BoundaryDecision, BoundaryError,
    BoundarySegmentationPlan, BoundaryWindowDecision, ProposedBoundaryBatch, RestoredTranscript,
    RestoredTranscriptSpan, Score5, SearchError, Slide, SlideDeck, SlideId, SlideScore,
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

fn results(plan: &BoundarySegmentationPlan, cut_cost: u8) -> Vec<BoundaryBatchResult> {
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
                            cut_cost: Score5::try_from(cut_cost).unwrap(),
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
    let mut decisions = results(&plan, 5);
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
fn lowest_cut_cost_is_selected_when_a_cut_is_required() {
    let source = restored(&format!(
        "{}。{}。{}。{}。",
        "甲".repeat(99),
        "乙".repeat(99),
        "丙".repeat(99),
        "丁".repeat(99)
    ));
    let plan = BoundarySegmentationPlan::new(&source);
    let mut decisions = results(&plan, 5);
    decisions[0].decisions.windows[0].boundaries[1].cut_cost = Score5::ZERO;
    let partition = plan.partition(&decisions).unwrap();
    let text = source.text();
    assert_eq!(partition.len(), 2);
    assert_eq!(
        partition[0].clone(),
        0.."甲".repeat(99).len() + "。".len() + "乙".repeat(99).len() + "。".len()
    );
    assert_eq!(
        partition
            .iter()
            .map(|r| &text[r.clone()])
            .collect::<String>(),
        text
    );
}

#[test]
fn semantic_cut_quality_precedes_passage_count() {
    let source = restored(&format!(
        "{}。{}。{}。{}。{}。",
        "甲".repeat(99),
        "乙".repeat(99),
        "丙".repeat(99),
        "丁".repeat(99),
        "戊".repeat(99)
    ));
    let plan = BoundarySegmentationPlan::new(&source);
    let mut decisions = results(&plan, 5);
    let boundaries = &mut decisions[0].decisions.windows[0].boundaries;
    boundaries[0].cut_cost = Score5::ZERO;
    boundaries[1].cut_cost = Score5::try_from(1).unwrap();
    boundaries[2].cut_cost = Score5::try_from(1).unwrap();
    boundaries[3].cut_cost = Score5::ZERO;
    let partition = plan.partition(&decisions).unwrap();

    assert_eq!(partition.len(), 3);
}

#[test]
fn passage_count_and_balance_resolve_equal_cut_costs() {
    let source = restored(&format!(
        "{}。{}。{}。{}。",
        "甲".repeat(99),
        "乙".repeat(99),
        "丙".repeat(99),
        "丁".repeat(99)
    ));
    let plan = BoundarySegmentationPlan::new(&source);
    let partition = plan.partition(&results(&plan, 0)).unwrap();

    assert_eq!(partition.len(), 2);
    assert_eq!(
        source.text()[partition[0].clone()].chars().count(),
        source.text()[partition[1].clone()].chars().count()
    );
}

#[test]
fn immutable_utf8_partition_preserves_spaces_emoji_and_code_paths() {
    let text = " 类型是 Option<Self::Item>，不是别的：说明在这里。\n🦀 e\u{301}。";
    let source = restored(text);
    let plan = BoundarySegmentationPlan::new(&source);
    let parts = plan.partition(&results(&plan, 0)).unwrap();
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
    let valid = results(&plan, 0);
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
fn costly_boundaries_still_produce_an_exact_partition_under_300_characters() {
    let text = "甲。".repeat(230);
    let plan = BoundarySegmentationPlan::new(&restored(&text));
    let partition = plan.partition(&results(&plan, 5)).unwrap();
    assert!(
        partition
            .iter()
            .all(|range| text[range.clone()].chars().count() <= 300)
    );
    assert_eq!(
        partition
            .iter()
            .map(|range| &text[range.clone()])
            .collect::<String>(),
        text
    );
}

#[test]
fn real_course_451_character_region_no_longer_stops_partitioning() {
    let text = "并发的优点，其实它最大的优点就是有了并发以后，我们才有可能让我们程序的执行性能变得更快。比如说举个例子，一个很简单的例子，数组求和，对吧？非常简单的一个例子。这个数组求和如果是单线程去计算的话，就是我们写一个最简单的这样的程序，它只能单线程去计算，那么它只能是单核去参与。也就是说，哪怕你的计算机有双核、四核、十核，甚至服务器上，比如说有好几百核，对吧？你如果只是这样的一个程序的话，它只能用到其中的一个核，这样的话它的性能会比较低，对吧？当然它的优点就是编写起来肯定是最简单的，便于理解。那这个程序它是怎么写的？就这么两行，实际上核心的就是我定义了一个向量，是一到一千。然后我用这个 map 对它先做一个计算。当然这里面为了模拟一个相对耗时的计算，通过 thread sleep 让它睡一毫秒，让它的运行时间相对慢一点。那么算完了以后，实际上就是做了一个两倍的操作，然后我求一个和，求出来就是这个东西。那程序本身很简单，但是这样一个程序，我哪怕给你再多的计算资源，它也只能用其中的一个核去运行，对吧？";
    assert_eq!(text.chars().count(), 451);
    let plan = BoundarySegmentationPlan::new(&restored(text));
    let partition = plan.partition(&results(&plan, 5)).unwrap();
    assert_eq!(partition.len(), 2);
    assert!(
        partition
            .iter()
            .all(|range| text[range.clone()].chars().count() <= 300)
    );
    assert_eq!(
        partition
            .iter()
            .map(|range| &text[range.clone()])
            .collect::<String>(),
        text
    );
}

#[test]
fn unpunctuated_text_receives_balanced_utf8_safe_fallback_atoms() {
    let text = "甲".repeat(451);
    let plan = BoundarySegmentationPlan::new(&restored(&text));
    assert!(!plan.tasks().is_empty());
    let partition = plan.partition(&results(&plan, 5)).unwrap();
    assert_eq!(partition.len(), 2);
    assert!(
        partition
            .iter()
            .all(|range| text[range.clone()].chars().count() <= 300)
    );
    assert_eq!(
        partition
            .iter()
            .map(|range| &text[range.clone()])
            .collect::<String>(),
        text
    );
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
                        start_ms: Some(id as u64 * 1000),
                        end_ms: Some((id + 1) as u64 * 1000),
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
    let long_sentence = format!("{}。{}。", "第一部分".repeat(38), "第二部分".repeat(38));
    let restored = RestoredTranscript {
        spans: vec![
            RestoredTranscriptSpan::OmittedDisfluency {
                source_start: TranscriptSegmentId(0),
                source_end: TranscriptSegmentId(0),
            },
            RestoredTranscriptSpan::Text {
                source_start: TranscriptSegmentId(1),
                source_end: TranscriptSegmentId(2),
                text: long_sentence,
            },
        ],
    };
    let plan = BoundarySegmentationPlan::new(&restored);
    let decisions = results(&plan, 0);
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
