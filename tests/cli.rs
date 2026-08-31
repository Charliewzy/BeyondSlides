use std::{fs, path::PathBuf, process::Command};

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

fn beyond_slides_binary() -> PathBuf {
    option_env!("CARGO_BIN_EXE_beyond-slides")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CARGO_BIN_EXE_beyond-slides").map(PathBuf::from))
        .expect("Cargo should provide the BeyondSlides binary path")
}
