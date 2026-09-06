use std::{error::Error, time::Duration};

use beyond_slides::{
    SlideDeck, ValidatedSources, WindowingConfig, build_windows,
    ingestion::transcript::{TranscriptFormat, import},
};

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

#[test]
fn timed_json_keeps_its_existing_wire_format() -> TestResult {
    let input =
        serde_json::json!({"segments": [{"id":0,"start_ms":0,"end_ms":1000,"text":"你好。"}]});
    let transcript = import(&input.to_string(), TranscriptFormat::Json)?;
    assert!(transcript.has_timestamps());
    assert_eq!(serde_json::to_value(transcript)?, input);
    Ok(())
}

#[test]
fn missing_timing_is_distinct_from_zero_and_must_be_consistent() -> TestResult {
    let transcript = import(
        r#"{"segments":[{"id":0,"text":"你好。"}]}"#,
        TranscriptFormat::Json,
    )?;
    assert!(!transcript.has_timestamps());
    assert_eq!(transcript.segments[0].start_ms, None);
    for input in [
        r#"{"segments":[{"id":0,"start_ms":0,"text":"你好。"}]}"#,
        r#"{"segments":[{"id":0,"start_ms":0,"end_ms":1,"text":"你好。"},{"id":1,"text":"再见。"}]}"#,
    ] {
        assert!(import(input, TranscriptFormat::Json).is_err());
    }
    Ok(())
}

#[test]
fn plain_text_is_unicode_safe_and_uses_character_only_windows() -> TestResult {
    let text = format!("{}。第二句话。", "汉".repeat(1200));
    let transcript = import(&text, TranscriptFormat::PlainText)?;
    assert_eq!(
        transcript
            .segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<String>(),
        text
    );
    assert!(
        transcript
            .segments
            .iter()
            .all(|s| s.start_ms.is_none() && s.end_ms.is_none() && s.text.chars().count() <= 500)
    );
    let sources = ValidatedSources::new(transcript, SlideDeck { slides: vec![] })?;
    let windows = build_windows(
        &sources,
        WindowingConfig::new(600, Duration::from_millis(1), 0)?,
    );
    assert_eq!(windows.len(), 3);
    assert!(import(" \n\r\n", TranscriptFormat::PlainText).is_err());
    Ok(())
}

#[test]
fn subrip_preserves_multiline_text_and_normalizes_cue_ids() -> TestResult {
    let transcript = import(
        "\u{feff}8\r\n00:00:01,250 --> 00:00:03,500\r\n第一行\r\n第二行\r\n\r\n12\r\n00:00:04,000 --> 00:00:05,000\r\n下一句。",
        TranscriptFormat::SubRip,
    )?;
    assert_eq!(transcript.segments.len(), 2);
    assert_eq!(transcript.segments[0].id.0, 0);
    assert_eq!(transcript.segments[1].id.0, 1);
    assert_eq!(transcript.segments[0].text, "第一行\n第二行");
    assert_eq!(transcript.segments[0].start_ms, Some(1250));
    assert_eq!(transcript.segments[0].end_ms, Some(3500));
    Ok(())
}

#[test]
fn webvtt_ignores_notes_and_preserves_cue_timing() -> TestResult {
    let transcript = import(
        "WEBVTT\n\nNOTE not lecture text\n\nfirst\n00:01.250 --> 00:03.500\n你好。\n\n",
        TranscriptFormat::WebVtt,
    )?;
    assert_eq!(transcript.segments.len(), 1);
    assert_eq!(transcript.segments[0].text, "你好。");
    assert_eq!(transcript.segments[0].start_ms, Some(1250));
    assert_eq!(transcript.segments[0].end_ms, Some(3500));
    Ok(())
}
