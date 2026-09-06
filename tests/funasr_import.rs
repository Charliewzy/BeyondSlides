use std::error::Error;

use beyond_slides::ingestion::funasr::{ImportError, import_tsv};
use beyond_slides::{TranscriptSegment, TranscriptSegmentId};

#[test]
fn funasr_rows_become_individually_referenced_transcript_segments() -> Result<(), Box<dyn Error>> {
    let tsv = "start\tend\ttext\r\n8.800\t8.860\t 好 \r\n31.64\t31.700\t我们继续上课\r\n";

    let transcript = import_tsv(tsv)?;

    assert_eq!(
        transcript.segments,
        vec![
            TranscriptSegment {
                id: TranscriptSegmentId(0),
                start_ms: Some(8_800),
                end_ms: Some(8_860),
                text: "好".to_owned(),
            },
            TranscriptSegment {
                id: TranscriptSegmentId(1),
                start_ms: Some(31_640),
                end_ms: Some(31_700),
                text: "我们继续上课".to_owned(),
            },
        ]
    );
    Ok(())
}

#[test]
fn blank_lines_do_not_create_transcript_segments() -> Result<(), Box<dyn Error>> {
    let transcript = import_tsv("start\tend\ttext\n\n0\t0.060\t好\n   \n0.500\t1\t下一句\n")?;

    assert_eq!(transcript.segments.len(), 2);
    assert_eq!(transcript.segments[1].id, TranscriptSegmentId(1));
    Ok(())
}

#[test]
fn funasr_header_must_match_the_supported_format() {
    let error = import_tsv("begin\tend\ttext\n0.000\t1.000\t内容\n")
        .expect_err("an unknown TSV shape must be rejected");

    assert_eq!(
        error,
        ImportError::InvalidHeader {
            actual: "begin\tend\ttext".to_owned(),
        }
    );
}

#[test]
fn malformed_timestamp_reports_its_source_line_and_field() {
    let error = import_tsv("start\tend\ttext\nsoon\t1.000\t内容\n")
        .expect_err("a nonnumeric timestamp must be rejected");

    assert_eq!(
        error,
        ImportError::InvalidTimestamp {
            line: 2,
            field: "start",
            value: "soon".to_owned(),
        }
    );
}

#[test]
fn timestamp_precision_finer_than_milliseconds_is_not_silently_rounded() {
    let error = import_tsv("start\tend\ttext\n0.0001\t1.000\t内容\n")
        .expect_err("unsupported precision must be rejected");

    assert_eq!(
        error,
        ImportError::InvalidTimestamp {
            line: 2,
            field: "start",
            value: "0.0001".to_owned(),
        }
    );
}

#[test]
fn reversed_and_overlapping_evidence_is_rejected() {
    let reversed = import_tsv("start\tend\ttext\n2.000\t1.000\t内容\n")
        .expect_err("a reversed timestamp range must be rejected");
    assert_eq!(
        reversed,
        ImportError::EndBeforeStart {
            line: 2,
            start_ms: 2_000,
            end_ms: 1_000,
        }
    );

    let overlapping = import_tsv("start\tend\ttext\n0.000\t1.000\t第一句\n0.900\t2.000\t第二句\n")
        .expect_err("overlapping evidence must be rejected");
    assert_eq!(
        overlapping,
        ImportError::OverlappingRows {
            previous_line: 2,
            line: 3,
            previous_end_ms: 1_000,
            start_ms: 900,
        }
    );
}

#[test]
fn empty_text_is_rejected_without_dropping_the_row() {
    let error = import_tsv("start\tend\ttext\n0.000\t1.000\t   \n")
        .expect_err("empty evidence must be rejected");

    assert_eq!(error, ImportError::EmptyText { line: 2 });
}
