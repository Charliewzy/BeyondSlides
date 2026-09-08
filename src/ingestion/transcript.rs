//! Import user-authored transcripts without inventing recording timestamps.
use std::error::Error;

use crate::{SlideDeck, Transcript, TranscriptSegment, TranscriptSegmentId, ValidatedSources};

#[derive(Clone, Copy, Debug)]
pub enum TranscriptFormat {
    Json,
    SubRip,
    WebVtt,
    PlainText,
}

pub fn import(
    input: &str,
    format: TranscriptFormat,
) -> Result<Transcript, Box<dyn Error + Send + Sync>> {
    let input = input.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let transcript = match format {
        TranscriptFormat::Json => serde_json::from_str(&input)?,
        TranscriptFormat::SubRip => {
            let subtitles = subtp::srt::SubRip::parse(&format!("{}\n", input.trim_end()))?;
            let rows = subtitles.subtitles.into_iter().map(|cue| {
                let start = cue.start;
                let end = cue.end;
                (
                    Some(milliseconds(
                        start.hours,
                        start.minutes,
                        start.seconds,
                        start.milliseconds,
                    )),
                    Some(milliseconds(
                        end.hours,
                        end.minutes,
                        end.seconds,
                        end.milliseconds,
                    )),
                    cue.text.join("\n"),
                )
            });
            from_rows(rows)?
        }
        TranscriptFormat::WebVtt => {
            let subtitles = subtp::vtt::WebVtt::parse(&format!("{}\n", input.trim_end()))?;
            let rows = subtitles.blocks.into_iter().filter_map(|block| {
                let subtp::vtt::VttBlock::Que(cue) = block else {
                    return None;
                };
                let start = cue.timings.start;
                let end = cue.timings.end;
                Some((
                    Some(milliseconds(
                        start.hours,
                        start.minutes,
                        start.seconds,
                        start.milliseconds,
                    )),
                    Some(milliseconds(
                        end.hours,
                        end.minutes,
                        end.seconds,
                        end.milliseconds,
                    )),
                    cue.payload.join("\n"),
                ))
            });
            from_rows(rows)?
        }
        TranscriptFormat::PlainText => {
            // These are ingestion units, not semantic passages. Bound very long
            // unpunctuated input so restoration can process it with context.
            let mut units = Vec::new();
            for part in input.split_inclusive(['。', '！', '？', '!', '?', '\n']) {
                let characters: Vec<_> = part.chars().collect();
                for chunk in characters.chunks(500) {
                    let text: String = chunk.iter().collect();
                    if !text.trim().is_empty() {
                        units.push((None, None, text));
                    }
                }
            }
            from_rows(units)?
        }
    };
    if transcript.segments.is_empty() {
        return Err("The transcript contains no text".into());
    }
    ValidatedSources::new(transcript.clone(), SlideDeck { slides: Vec::new() })?;
    Ok(transcript)
}

fn milliseconds(hours: u8, minutes: u8, seconds: u8, milliseconds: u16) -> u64 {
    ((u64::from(hours) * 60 + u64::from(minutes)) * 60 + u64::from(seconds)) * 1000
        + u64::from(milliseconds)
}

fn from_rows(
    rows: impl IntoIterator<Item = (Option<u64>, Option<u64>, String)>,
) -> Result<Transcript, std::num::TryFromIntError> {
    let segments = rows
        .into_iter()
        .enumerate()
        .map(|(index, (start_ms, end_ms, text))| {
            Ok(TranscriptSegment {
                id: TranscriptSegmentId(u32::try_from(index)?),
                start_ms,
                end_ms,
                text,
            })
        })
        .collect::<Result<Vec<_>, std::num::TryFromIntError>>()?;
    Ok(Transcript { segments })
}
