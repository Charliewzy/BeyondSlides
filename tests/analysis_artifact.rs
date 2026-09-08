use std::error::Error;

use beyond_slides::{
    ComparativeScore, RestoredAnalysisArtifact, RestoredAnalysisArtifactError, SlideDeck, SlideId,
    Transcript, ValidatedSources,
};

#[test]
fn persisted_analysis_is_revalidated_before_rendering() -> Result<(), Box<dyn Error>> {
    let artifact = artifact()?;
    let analysis = artifact.validate(sources()?)?;

    assert_eq!(
        analysis.restored_transcript().text(),
        "所有权保证内存安全。"
    );
    assert_eq!(analysis.passages().len(), 2);
    Ok(())
}

#[test]
fn persisted_analysis_rejects_metadata_with_a_different_window_count() -> Result<(), Box<dyn Error>>
{
    let mut artifact = artifact()?;
    artifact.window_projections.clear();

    let error = artifact
        .validate(sources()?)
        .expect_err("window metadata must describe the same run");

    assert!(matches!(
        error,
        RestoredAnalysisArtifactError::WindowCountMismatch {
            field: "window_projections",
            expected: 1,
            actual: 0,
        }
    ));
    Ok(())
}

#[test]
fn persisted_analysis_rejects_text_that_is_not_the_restored_transcript()
-> Result<(), Box<dyn Error>> {
    let mut artifact = artifact()?;
    artifact.passages[1].text = "内存并不安全。".into();

    let error = artifact
        .validate(sources()?)
        .expect_err("rendering must not trust modified passage text");

    assert!(error.to_string().contains("authoritative"));
    Ok(())
}

#[test]
fn persisted_analysis_rejects_an_unknown_passage_slide_position() -> Result<(), Box<dyn Error>> {
    let mut artifact = artifact()?;
    artifact.passages[0].slide_position = SlideId(9);

    let error = artifact
        .validate(sources()?)
        .expect_err("rendering must not trust an unknown inferred slide position");

    assert!(
        error
            .to_string()
            .contains("unknown inferred slide position 9")
    );
    Ok(())
}

#[test]
fn persisted_analysis_rejects_a_display_level_that_disagrees_with_comparative_evidence()
-> Result<(), Box<dyn Error>> {
    let mut artifact = artifact()?;
    artifact.passages[0].comparative_importance = Some(ComparativeScore::new(8, 8, 0, 10_000)?);
    artifact.passages[0].comparative_novelty = Some(ComparativeScore::new(8, 0, 8, 0)?);

    let error = artifact
        .validate(sources()?)
        .expect_err("the display level must be derived from comparative evidence");

    assert!(error.to_string().contains("display level 5"));
    assert!(error.to_string().contains("score field is 4"));
    Ok(())
}

fn artifact() -> Result<RestoredAnalysisArtifact, serde_json::Error> {
    serde_json::from_value(serde_json::json!({
        "restored_transcript": {
            "spans": [{
                "kind": "text",
                "source_start": 0,
                "source_end": 0,
                "text": "所有权保证内存安全。"
            }]
        },
        "passages": [
            {
                "text": "所有权",
                "source_start": 0,
                "source_end": 0,
                "slide_position": 0,
                "novelty": 1,
                "importance": 4,
                "related_slides": [0],
                "summary": null,
                "comparison_note": null
            },
            {
                "text": "保证内存安全。",
                "source_start": 0,
                "source_end": 0,
                "slide_position": 0,
                "novelty": 2,
                "importance": 5,
                "related_slides": [0],
                "summary": null,
                "comparison_note": null
            }
        ],
        "window_diagnostics": [{
            "provider_retries": 0,
            "tool_rounds": 0,
            "final_answer_repairs": 0,
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "accepted_json_fence": false
        }],
        "window_projections": [{
            "changed_characters": 0,
            "compared_characters": 10
        }]
    }))
}

fn sources() -> Result<ValidatedSources, beyond_slides::ValidationError> {
    let transcript: Transcript = serde_json::from_value(serde_json::json!({
        "segments": [{
            "id": 0,
            "start_ms": 0,
            "end_ms": 1_000,
            "text": "所有权保证内存安全"
        }]
    }))
    .expect("valid test transcript");
    let slides: SlideDeck = serde_json::from_value(serde_json::json!({
        "slides": [{"id": 0, "text": "所有权与内存安全"}]
    }))
    .expect("valid test slides");
    ValidatedSources::new(transcript, slides)
}
