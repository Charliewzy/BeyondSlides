use std::{
    error::Error,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::Instant,
};

use beyond_slides::{
    TimedTranscript, TimedTranscriptToken, Transcript, TranscriptSegment, TranscriptSegmentId,
};
use bzip2::read::BzDecoder;
use fs2::FileExt;
use reqwest::Client;
use serde_json::{Value, json};
use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig, SileroVadModelConfig,
    VadModelConfig, VoiceActivityDetector, Wave,
};

use super::model_assets::{download_verified, file_matches};

const SAMPLE_RATE: i32 = 16_000;
const VAD_WINDOW_SIZE: usize = 512;
// Padding protects quiet syllables at VAD boundaries. Overlapping padded
// regions are merged before recognition, so audio is never transcribed twice.
const VAD_PADDING_SAMPLES: usize = SAMPLE_RATE as usize;
const MODEL_DIRECTORY: &str = "sensevoice-small-int8-2024-07-17";
const MODEL_ARCHIVE_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2";
const VAD_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx";
const MODEL_ARCHIVE_BYTES: u64 = 163_002_883;
const VAD_BYTES: u64 = 643_854;
const MODEL_ARCHIVE_SHA256: &str =
    "7d1efa2138a65b0b488df37f8b89e3d91a60676e416f515b952358d83dfd347e";
const VAD_SHA256: &str = "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6";
const MODEL_SHA256: &str = "c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51";
const TOKENS_SHA256: &str = "f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc";

pub(super) struct RecognitionOutput {
    pub transcript: Transcript,
    pub timed_tokens: Option<TimedTranscript>,
    pub timing_warning: Option<String>,
    pub metadata: Value,
}

pub(super) struct RecognitionProgress {
    pub phase: &'static str,
    pub completed_regions: usize,
    pub total_regions: usize,
    pub completed_speech_ms: u64,
    pub total_speech_ms: u64,
}

pub(super) struct ModelPaths {
    model: PathBuf,
    tokens: PathBuf,
    vad: PathBuf,
}

/// Downloads and verifies the native CPU models once, then reuses the cache.
pub(super) async fn ensure_models(
    data_root: &Path,
    mut report: impl FnMut(u64, u64) -> Result<(), Box<dyn Error>>,
) -> Result<ModelPaths, Box<dyn Error>> {
    let directory = data_root.join("models").join(MODEL_DIRECTORY);
    fs::create_dir_all(&directory)?;
    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(directory.join("download.lock"))?;
    lock_file.lock_exclusive()?;

    let paths = ModelPaths {
        model: directory.join("model.int8.onnx"),
        tokens: directory.join("tokens.txt"),
        vad: directory.join("silero_vad.onnx"),
    };
    let total = MODEL_ARCHIVE_BYTES + VAD_BYTES;
    if model_files_are_valid(&paths)? {
        report(total, total)?;
        return Ok(paths);
    }

    let client = Client::builder()
        .user_agent(concat!("BeyondSlides/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let archive = directory.join("sensevoice.tar.bz2");
    download_verified(
        &client,
        MODEL_ARCHIVE_URL,
        MODEL_ARCHIVE_BYTES,
        MODEL_ARCHIVE_SHA256,
        &archive,
        |bytes| report(bytes, total),
    )
    .await?;
    download_verified(
        &client,
        VAD_URL,
        VAD_BYTES,
        VAD_SHA256,
        &paths.vad,
        |bytes| report(MODEL_ARCHIVE_BYTES + bytes, total),
    )
    .await?;
    extract_model_files(&archive, &paths)?;
    if !model_files_are_valid(&paths)? {
        return Err("downloaded SenseVoice model files failed integrity validation".into());
    }
    fs::remove_file(archive)?;
    report(total, total)?;
    Ok(paths)
}

fn extract_model_files(archive: &Path, paths: &ModelPaths) -> Result<(), Box<dyn Error>> {
    let decoder = BzDecoder::new(File::open(archive)?);
    let mut archive = tar::Archive::new(decoder);
    let model_part = paths.model.with_extension("part");
    let tokens_part = paths.tokens.with_extension("part");
    let _ = fs::remove_file(&model_part);
    let _ = fs::remove_file(&tokens_part);
    let mut found_model = false;
    let mut found_tokens = false;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let output = match name {
            "model.int8.onnx" => {
                found_model = true;
                &model_part
            }
            "tokens.txt" => {
                found_tokens = true;
                &tokens_part
            }
            _ => continue,
        };
        let mut file = File::create(output)?;
        io::copy(&mut entry, &mut file)?;
        file.flush()?;
    }
    if !found_model || !found_tokens {
        return Err("SenseVoice archive did not contain the expected model and tokens".into());
    }
    if !file_matches(&model_part, MODEL_SHA256)? || !file_matches(&tokens_part, TOKENS_SHA256)? {
        return Err("extracted SenseVoice files failed SHA-256 validation".into());
    }
    if paths.model.exists() {
        fs::remove_file(&paths.model)?;
    }
    if paths.tokens.exists() {
        fs::remove_file(&paths.tokens)?;
    }
    fs::rename(model_part, &paths.model)?;
    fs::rename(tokens_part, &paths.tokens)?;
    Ok(())
}

fn model_files_are_valid(paths: &ModelPaths) -> Result<bool, io::Error> {
    Ok(file_matches(&paths.model, MODEL_SHA256)?
        && file_matches(&paths.tokens, TOKENS_SHA256)?
        && file_matches(&paths.vad, VAD_SHA256)?)
}

struct SpeechRegion {
    start_sample: usize,
    end_sample: usize,
}

struct RecognizedRegion {
    start_ms: u64,
    end_ms: u64,
    text: String,
    tokens: Vec<TimedTranscriptToken>,
    has_complete_token_timing: bool,
}

pub(super) fn transcribe(
    audio: &Path,
    models: &ModelPaths,
    mut report: impl FnMut(RecognitionProgress) -> Result<(), Box<dyn Error>>,
    mut check_stop: impl FnMut() -> Result<(), Box<dyn Error>>,
) -> Result<RecognitionOutput, Box<dyn Error>> {
    let total_started = Instant::now();
    let wave = Wave::read(path_text(audio)?).ok_or("Sherpa could not read the extracted WAV")?;
    if wave.sample_rate() != SAMPLE_RATE {
        return Err(format!("expected {SAMPLE_RATE} Hz mono WAV input").into());
    }
    let loading_started = Instant::now();
    let recognizer = create_recognizer(&models.model, &models.tokens)?;
    let loading_seconds = loading_started.elapsed().as_secs_f64();
    report(RecognitionProgress {
        phase: "detecting_speech",
        completed_regions: 0,
        total_regions: 0,
        completed_speech_ms: 0,
        total_speech_ms: 0,
    })?;
    let vad_started = Instant::now();
    let regions = detect_speech(&wave, &models.vad, &mut check_stop)?;
    let vad_seconds = vad_started.elapsed().as_secs_f64();
    if regions.is_empty() {
        return Err("no speech was detected in the recording".into());
    }
    let total_speech_ms = regions
        .iter()
        .map(|region| samples_to_ms(region.end_sample - region.start_sample))
        .sum();
    report(RecognitionProgress {
        phase: "recognizing",
        completed_regions: 0,
        total_regions: regions.len(),
        completed_speech_ms: 0,
        total_speech_ms,
    })?;

    let recognition_started = Instant::now();
    let mut completed_speech_ms = 0_u64;
    let mut recognized_regions = Vec::with_capacity(regions.len());
    for (index, region) in regions.iter().enumerate() {
        check_stop()?;
        let stream = recognizer.create_stream();
        stream.accept_waveform(
            SAMPLE_RATE,
            &wave.samples()[region.start_sample..region.end_sample],
        );
        recognizer.decode(&stream);
        let result = stream
            .get_result()
            .ok_or("Sherpa returned no recognition result")?;
        completed_speech_ms = completed_speech_ms
            .saturating_add(samples_to_ms(region.end_sample - region.start_sample));
        if !result.text.trim().is_empty() {
            recognized_regions.push(convert_result(
                samples_to_ms(region.start_sample),
                samples_to_ms(region.end_sample),
                result,
            ));
        }
        report(RecognitionProgress {
            phase: "recognizing",
            completed_regions: index + 1,
            total_regions: regions.len(),
            completed_speech_ms,
            total_speech_ms,
        })?;
    }
    let recognition_seconds = recognition_started.elapsed().as_secs_f64();
    let timing_complete = recognized_regions
        .iter()
        .all(|region| region.has_complete_token_timing);
    let (transcript, timed_tokens) = assemble_transcript(&recognized_regions)?;
    Ok(RecognitionOutput {
        transcript,
        timed_tokens: timing_complete.then_some(timed_tokens),
        timing_warning: (!timing_complete)
            .then(|| "SenseVoice omitted token timing for at least one speech region".into()),
        metadata: json!({
            "runtime": "sherpa-onnx 1.13.7",
            "model": "SenseVoiceSmall INT8 + Silero VAD",
            "device": "cpu",
            "timings_seconds": {
                "loading_models": loading_seconds,
                "detecting_speech": vad_seconds,
                "recognizing": recognition_seconds,
                "native_transcription": total_started.elapsed().as_secs_f64(),
            }
        }),
    })
}

fn detect_speech(
    wave: &Wave,
    model: &Path,
    check_stop: &mut impl FnMut() -> Result<(), Box<dyn Error>>,
) -> Result<Vec<SpeechRegion>, Box<dyn Error>> {
    let config = VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model: Some(path_text(model)?.to_owned()),
            threshold: 0.25,
            min_silence_duration: 0.5,
            min_speech_duration: 0.5,
            window_size: VAD_WINDOW_SIZE as i32,
            max_speech_duration: 30.0,
        },
        ten_vad: Default::default(),
        sample_rate: SAMPLE_RATE,
        num_threads: 1,
        provider: Some("cpu".into()),
        debug: false,
    };
    let detector = VoiceActivityDetector::create(&config, 60.0)
        .ok_or("could not create the Silero voice activity detector")?;
    let mut detected = Vec::new();
    for (index, samples) in wave.samples().chunks(VAD_WINDOW_SIZE).enumerate() {
        if index % 100 == 0 {
            check_stop()?;
        }
        detector.accept_waveform(samples);
        drain_regions(&detector, &mut detected);
    }
    detector.flush();
    drain_regions(&detector, &mut detected);

    let mut padded: Vec<SpeechRegion> = Vec::with_capacity(detected.len());
    for (start, end) in detected {
        let start = start.saturating_sub(VAD_PADDING_SAMPLES);
        let end = end
            .saturating_add(VAD_PADDING_SAMPLES)
            .min(wave.samples().len());
        if let Some(previous) = padded.last_mut()
            && start <= previous.end_sample
        {
            previous.end_sample = previous.end_sample.max(end);
        } else {
            padded.push(SpeechRegion {
                start_sample: start,
                end_sample: end,
            });
        }
    }
    Ok(padded)
}

fn drain_regions(detector: &VoiceActivityDetector, regions: &mut Vec<(usize, usize)>) {
    while let Some(region) = detector.front() {
        let start = region.start().max(0) as usize;
        regions.push((start, start.saturating_add(region.samples().len())));
        detector.pop();
    }
}

fn create_recognizer(model: &Path, tokens: &Path) -> Result<OfflineRecognizer, Box<dyn Error>> {
    let mut config = OfflineRecognizerConfig {
        decoding_method: Some("greedy_search".into()),
        ..Default::default()
    };
    config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
        model: Some(path_text(model)?.to_owned()),
        language: Some("zh".into()),
        use_itn: true,
    };
    config.model_config.tokens = Some(path_text(tokens)?.to_owned());
    config.model_config.num_threads = 4;
    config.model_config.provider = Some("cpu".into());
    OfflineRecognizer::create(&config)
        .ok_or_else(|| "could not create the native SenseVoice recognizer".into())
}

fn convert_result(
    region_start_ms: u64,
    region_end_ms: u64,
    result: sherpa_onnx::OfflineRecognizerResult,
) -> RecognizedRegion {
    let timestamps = result.timestamps.unwrap_or_default();
    let durations = result.durations.unwrap_or_default();
    let complete = !result.tokens.is_empty() && timestamps.len() == result.tokens.len();
    let expected_timed_tokens = result
        .tokens
        .iter()
        .filter(|text| !text.trim().is_empty())
        .count();
    let mut previous_end = region_start_ms;
    let tokens = if complete {
        result
            .tokens
            .iter()
            .enumerate()
            .filter_map(|(index, text)| {
                if text.trim().is_empty() || previous_end >= region_end_ms {
                    return None;
                }
                let start = region_start_ms
                    .saturating_add((f64::from(timestamps[index]) * 1_000.0).round() as u64)
                    .max(previous_end)
                    .min(region_end_ms - 1);
                let suggested_end = durations
                    .get(index)
                    .map(|duration| {
                        start.saturating_add((f64::from(*duration) * 1_000.0).round() as u64)
                    })
                    .or_else(|| {
                        timestamps.get(index + 1).map(|next| {
                            region_start_ms
                                .saturating_add((f64::from(*next) * 1_000.0).round() as u64)
                        })
                    })
                    .unwrap_or(region_end_ms);
                let end = suggested_end.max(start + 1).min(region_end_ms);
                previous_end = end;
                Some(TimedTranscriptToken {
                    text: text.clone(),
                    start_ms: start,
                    end_ms: end,
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let token_text = tokens
        .iter()
        .map(|token| token.text.as_str())
        .collect::<String>();
    let has_complete_token_timing = complete && tokens.len() == expected_timed_tokens;
    let text = if has_complete_token_timing && !token_text.is_empty() {
        token_text
    } else {
        result.text
    };
    RecognizedRegion {
        start_ms: region_start_ms,
        end_ms: region_end_ms,
        text,
        tokens,
        has_complete_token_timing,
    }
}

fn assemble_transcript(
    regions: &[RecognizedRegion],
) -> Result<(Transcript, TimedTranscript), Box<dyn Error>> {
    let mut segments = Vec::new();
    let mut timed_tokens = Vec::new();
    for region in regions {
        if region.has_complete_token_timing {
            let mut sentence = Vec::new();
            for token in &region.tokens {
                sentence.push(token.clone());
                if token.text.chars().last().is_some_and(is_sentence_ending) {
                    push_timed_segment(&mut segments, &sentence)?;
                    sentence.clear();
                }
            }
            if !sentence.is_empty() {
                push_timed_segment(&mut segments, &sentence)?;
            }
            timed_tokens.extend(region.tokens.iter().cloned());
        } else if !region.text.trim().is_empty() {
            push_segment(
                &mut segments,
                region.start_ms,
                region.end_ms.max(region.start_ms + 1),
                region.text.clone(),
            )?;
        }
    }
    Ok((
        Transcript { segments },
        TimedTranscript {
            tokens: timed_tokens,
        },
    ))
}

fn push_timed_segment(
    segments: &mut Vec<TranscriptSegment>,
    tokens: &[TimedTranscriptToken],
) -> Result<(), Box<dyn Error>> {
    push_segment(
        segments,
        tokens
            .first()
            .expect("timed sentence is non-empty")
            .start_ms,
        tokens.last().expect("timed sentence is non-empty").end_ms,
        tokens.iter().map(|token| token.text.as_str()).collect(),
    )
}

fn push_segment(
    segments: &mut Vec<TranscriptSegment>,
    start_ms: u64,
    end_ms: u64,
    text: String,
) -> Result<(), Box<dyn Error>> {
    let id =
        u32::try_from(segments.len()).map_err(|_| "ASR produced too many transcript segments")?;
    segments.push(TranscriptSegment {
        id: TranscriptSegmentId(id),
        start_ms: Some(start_ms),
        end_ms: Some(end_ms),
        text,
    });
    Ok(())
}

fn is_sentence_ending(character: char) -> bool {
    matches!(character, '。' | '！' | '？' | '；' | '!' | '?' | ';')
}

fn samples_to_ms(samples: usize) -> u64 {
    samples as u64 * 1_000 / SAMPLE_RATE as u64
}

fn path_text(path: &Path) -> Result<&str, Box<dyn Error>> {
    path.to_str()
        .ok_or_else(|| "speech-recognition paths must be UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path},
    };

    #[tokio::test]
    async fn model_download_resumes_a_partial_file_and_publishes_atomically() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/asset"))
            .and(header("range", "bytes=6-"))
            .respond_with(ResponseTemplate::new(206).set_body_bytes(b"world"))
            .mount(&server)
            .await;
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("asset.bin");
        fs::write(destination.with_extension("part"), b"hello ").unwrap();
        let mut progress = Vec::new();
        download_verified(
            &Client::new(),
            &format!("{}/asset", server.uri()),
            11,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9",
            &destination,
            |downloaded| {
                progress.push(downloaded);
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(fs::read(destination).unwrap(), b"hello world");
        assert_eq!(progress.last(), Some(&11));
    }

    #[test]
    fn assembles_punctuated_segments_and_fine_timing() {
        let regions = vec![RecognizedRegion {
            start_ms: 0,
            end_ms: 900,
            text: "你好。继续".into(),
            tokens: vec![
                token("你", 10, 100),
                token("好", 100, 200),
                token("。", 200, 250),
                token("继续", 300, 700),
            ],
            has_complete_token_timing: true,
        }];
        let (transcript, timing) = assemble_transcript(&regions).unwrap();
        assert_eq!(transcript.segments.len(), 2);
        assert_eq!(transcript.segments[0].text, "你好。");
        assert_eq!(transcript.segments[1].text, "继续");
        assert_eq!(timing.tokens.len(), 4);
    }

    #[test]
    fn falls_back_to_a_coarse_region_when_token_timing_is_missing() {
        let regions = vec![RecognizedRegion {
            start_ms: 100,
            end_ms: 800,
            text: "一段文字".into(),
            tokens: Vec::new(),
            has_complete_token_timing: false,
        }];
        let (transcript, timing) = assemble_transcript(&regions).unwrap();
        assert_eq!(transcript.segments[0].start_ms, Some(100));
        assert_eq!(transcript.segments[0].end_ms, Some(800));
        assert!(timing.tokens.is_empty());
    }

    /// Manual production smoke test. It is ignored in CI because model weights
    /// and a five-minute recording are intentionally not repository assets.
    #[test]
    #[ignore = "set BEYOND_SLIDES_ASR_SMOKE_ROOT to a directory containing audio.wav, model.int8.onnx, tokens.txt, and silero_vad.onnx"]
    fn transcribes_a_real_recording_with_native_models() {
        let root = PathBuf::from(
            std::env::var_os("BEYOND_SLIDES_ASR_SMOKE_ROOT")
                .expect("BEYOND_SLIDES_ASR_SMOKE_ROOT must be set"),
        );
        let models = ModelPaths {
            model: root.join("model.int8.onnx"),
            tokens: root.join("tokens.txt"),
            vad: root.join("silero_vad.onnx"),
        };
        let started = Instant::now();
        let output = transcribe(&root.join("audio.wav"), &models, |_| Ok(()), || Ok(())).unwrap();
        assert!(output.transcript.segments.len() > 10);
        let timing = output.timed_tokens.unwrap();
        assert!(timing.tokens.len() > 100);
        assert!(
            timing
                .tokens
                .iter()
                .all(|token| token.end_ms > token.start_ms)
        );
        assert!(
            timing
                .tokens
                .windows(2)
                .all(|tokens| tokens[0].end_ms <= tokens[1].start_ms)
        );
        eprintln!(
            "native smoke test produced {} segments in {:.2}s",
            output.transcript.segments.len(),
            started.elapsed().as_secs_f64()
        );
    }

    fn token(text: &str, start_ms: u64, end_ms: u64) -> TimedTranscriptToken {
        TimedTranscriptToken {
            text: text.into(),
            start_ms,
            end_ms,
        }
    }
}
