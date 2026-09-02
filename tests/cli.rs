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

fn beyond_slides_binary() -> PathBuf {
    option_env!("CARGO_BIN_EXE_beyond-slides")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CARGO_BIN_EXE_beyond-slides").map(PathBuf::from))
        .expect("Cargo should provide the BeyondSlides binary path")
}
