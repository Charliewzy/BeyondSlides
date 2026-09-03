use std::{fs, path::PathBuf, process::Command};

use beyond_slides::{
    MODEL_TRACE_FORMAT_VERSION, ModelRequestKind, ModelTraceEvent, ModelTraceRecord, ModelWorkflow,
};

#[test]
fn tiny_course_can_be_rendered_from_the_command_line() {
    let report_path = std::env::temp_dir().join(format!(
        "beyond-slides-tiny-course-{}.html",
        std::process::id()
    ));

    let output = Command::new(beyond_slides_binary())
        .args([
            "examples/tiny_course/transcript.json",
            "examples/tiny_course/slides.json",
            "examples/tiny_course/annotations.json",
        ])
        .arg(&report_path)
        .output()
        .expect("the BeyondSlides binary should run");
    let report = fs::read_to_string(&report_path)
        .expect("a successful render should create the requested report");
    fs::remove_file(&report_path).expect("the temporary report should be removable");

    assert!(
        output.status.success()
            && report.starts_with("<!doctype html>")
            && report.contains("按价值排序的口头补充")
            && report.contains("完整讲稿")
    );
}

#[test]
fn complete_analysis_requires_explicit_provider_configuration() {
    let output = Command::new(beyond_slides_binary())
        .args([
            "analyze",
            "missing-transcript.json",
            "missing-slides.json",
            "missing-run-directory",
        ])
        .env_remove("BEYOND_SLIDES_API_BASE_URL")
        .env_remove("BEYOND_SLIDES_API_KEY")
        .env_remove("BEYOND_SLIDES_MODEL")
        .output()
        .expect("the BeyondSlides binary should run");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("BEYOND_SLIDES_API_BASE_URL"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn model_trace_can_be_summarized_without_provider_configuration() {
    let directory = tempfile::tempdir().expect("temporary trace directory");
    let trace_path = directory.path().join("model-trace.jsonl");
    let record = ModelTraceRecord {
        format_version: MODEL_TRACE_FORMAT_VERSION,
        event_index: 0,
        timestamp_unix_ms: 1,
        exchange_id: 0,
        workflow: ModelWorkflow::Restoration,
        window_index: 12,
        conversation_turn: 0,
        request_kind: ModelRequestKind::Initial,
        event: ModelTraceEvent::Validation {
            accepted: false,
            category: Some("outside_owned_region".into()),
            error: Some("claimed context".into()),
        },
    };
    fs::write(
        &trace_path,
        format!(
            "{}\n",
            serde_json::to_string(&record).expect("serialize trace record")
        ),
    )
    .expect("write model trace");

    let output = Command::new(beyond_slides_binary())
        .args(["summarize-trace"])
        .arg(&trace_path)
        .env_remove("BEYOND_SLIDES_API_BASE_URL")
        .env_remove("BEYOND_SLIDES_API_KEY")
        .env_remove("BEYOND_SLIDES_MODEL")
        .output()
        .expect("the BeyondSlides binary should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("outside_owned_region: 1"));
    assert!(stdout.contains("restoration window 12"));
}

#[test]
fn saved_restored_analysis_can_be_rendered_and_evaluated_without_a_provider() {
    let directory = tempfile::tempdir().expect("temporary analysis directory");
    let transcript_path = directory.path().join("transcript.json");
    let slides_path = directory.path().join("slides.json");
    let analysis_path = directory.path().join("analysis.json");
    let trace_path = directory.path().join("model-trace.jsonl");
    let report_path = directory.path().join("report.html");
    fs::write(
        &transcript_path,
        r#"{"segments":[{"id":0,"start_ms":0,"end_ms":1000,"text":"原始文本"}]}"#,
    )
    .expect("write transcript");
    fs::write(&slides_path, r#"{"slides":[{"id":0,"text":"课程幻灯片"}]}"#).expect("write slides");
    fs::write(
        &analysis_path,
        r#"{
            "restored_transcript":{"spans":[{"kind":"text","source_start":0,"source_end":0,"text":"可读讲稿。"}]},
            "passages":[{"text":"可读讲稿。","source_start":0,"source_end":0,"novelty":3,"connection_strength":2,"importance":4,"related_slides":[0],"summary":null,"comparison_note":null}],
            "slide_positions":[0],
            "window_diagnostics":[{"provider_retries":0,"tool_rounds":0,"final_answer_repairs":1,"prompt_tokens":10,"completion_tokens":5,"accepted_json_fence":false}],
            "window_projections":[{"changed_characters":1,"compared_characters":20}]
        }"#,
    )
    .expect("write analysis");
    let rejected = ModelTraceRecord {
        format_version: MODEL_TRACE_FORMAT_VERSION,
        event_index: 0,
        timestamp_unix_ms: 1,
        exchange_id: 0,
        workflow: ModelWorkflow::Annotation,
        window_index: 0,
        conversation_turn: 0,
        request_kind: ModelRequestKind::Initial,
        event: ModelTraceEvent::Validation {
            accepted: false,
            category: Some("passage_text_difference".into()),
            error: Some("difference exceeded threshold".into()),
        },
    };
    fs::write(
        &trace_path,
        format!(
            "{}\n",
            serde_json::to_string(&rejected).expect("serialize trace record")
        ),
    )
    .expect("write trace");

    let render = Command::new(beyond_slides_binary())
        .arg("render-analysis")
        .args([&transcript_path, &slides_path, &analysis_path, &report_path])
        .env_remove("BEYOND_SLIDES_API_BASE_URL")
        .env_remove("BEYOND_SLIDES_API_KEY")
        .env_remove("BEYOND_SLIDES_MODEL")
        .output()
        .expect("run restored analysis renderer");
    assert!(
        render.status.success(),
        "{}",
        String::from_utf8_lossy(&render.stderr)
    );
    let report = fs::read_to_string(&report_path).expect("read report");
    assert!(report.contains("可读讲稿。"));
    assert!(report.contains("data-importance=\"4\""));

    let evaluate = Command::new(beyond_slides_binary())
        .arg("evaluate-analysis")
        .args([&analysis_path, &trace_path])
        .env_remove("BEYOND_SLIDES_API_BASE_URL")
        .env_remove("BEYOND_SLIDES_API_KEY")
        .env_remove("BEYOND_SLIDES_MODEL")
        .output()
        .expect("run restored analysis evaluator");
    assert!(
        evaluate.status.success(),
        "{}",
        String::from_utf8_lossy(&evaluate.stderr)
    );
    let quality = String::from_utf8_lossy(&evaluate.stdout);
    assert!(quality.contains("fuzzy accepted: 1 (50.0%)"));
    assert!(quality.contains("rejected: 1 (50.0%) across 1 windows"));
}

fn beyond_slides_binary() -> PathBuf {
    option_env!("CARGO_BIN_EXE_beyond-slides")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CARGO_BIN_EXE_beyond-slides").map(PathBuf::from))
        .expect("Cargo should provide the BeyondSlides binary path")
}
