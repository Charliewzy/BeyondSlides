use std::{collections::HashMap, error::Error, fmt};

use askama::Template;

use super::visual_alignment::VisualAlignment;
use crate::{SlideDeck, SlideId, SlideScore};

const CELL_WIDTH: usize = 8;
const CELL_HEIGHT: usize = 7;
const CHART_LEFT: usize = 54;
const CHART_TOP: usize = 22;
const CHART_RIGHT: usize = 18;
const CHART_BOTTOM: usize = 38;

pub struct AlignmentVisualizationWindow<'a> {
    pub number: usize,
    pub start_ms: u64,
    pub end_ms: u64,
    pub transcript: &'a str,
    pub slide_scores: &'a [SlideScore],
    pub slide_position: SlideId,
}

pub fn render_alignment_visualization(
    slide_deck: &SlideDeck,
    windows: &[AlignmentVisualizationWindow<'_>],
    visual_alignment: Option<&VisualAlignment>,
) -> Result<String, AlignmentVisualizationError> {
    if slide_deck.slides.is_empty() {
        return Err(AlignmentVisualizationError::EmptySlideDeck);
    }
    if windows.is_empty() {
        return Err(AlignmentVisualizationError::NoWindows);
    }

    let slide_positions: HashMap<_, _> = slide_deck
        .slides
        .iter()
        .enumerate()
        .map(|(position, slide)| (slide.id, position))
        .collect();
    let expected_ids: Vec<_> = slide_deck.slides.iter().map(|slide| slide.id).collect();
    let mut cells = Vec::with_capacity(windows.len() * slide_deck.slides.len());
    let mut dp_points = Vec::with_capacity(windows.len());
    let mut visual_points = Vec::new();
    let mut mismatch_connectors = Vec::new();
    let mut mismatch_markers = Vec::new();
    let mut window_targets = Vec::with_capacity(windows.len());
    let mut details = Vec::with_capacity(windows.len());
    let mut exact_visual_matches = 0;
    let mut visual_window_count = 0;

    for (window_position, window) in windows.iter().enumerate() {
        validate_window(window, &expected_ids, &slide_positions)?;
        let x = CHART_LEFT + window_position * CELL_WIDTH;
        let normalized = normalize(window.slide_scores);
        for (slide_position, (score, strength)) in
            window.slide_scores.iter().zip(&normalized).enumerate()
        {
            cells.push(HeatmapCell {
                x,
                y: CHART_TOP + slide_position * CELL_HEIGHT,
                fill: heat_color(*strength),
                window_number: window.number,
                slide_id: score.slide_id.0,
                score: format!("{:.6}", score.score),
            });
        }

        let inferred_position = slide_positions[&window.slide_position];
        dp_points.push(point(x, inferred_position));
        let visual_slide = visual_alignment
            .and_then(|alignment| visual_slide_for_window(window, alignment, &slide_positions));
        if let Some(visual_slide) = visual_slide {
            let visual_position = slide_positions[&visual_slide];
            visual_points.push(point(x, visual_position));
            visual_window_count += 1;
            if visual_slide == window.slide_position {
                exact_visual_matches += 1;
            } else {
                mismatch_connectors.push(MismatchConnector {
                    x: x + CELL_WIDTH / 2,
                    inferred_y: chart_y(inferred_position),
                    visual_y: chart_y(visual_position),
                });
                mismatch_markers.push(ChartPoint {
                    x: x + CELL_WIDTH / 2,
                    y: chart_y(visual_position),
                });
            }
        }

        window_targets.push(WindowTarget {
            number: window.number,
            x,
            width: CELL_WIDTH,
            height: slide_deck.slides.len() * CELL_HEIGHT,
            label: format!(
                "窗口 {}，{}，推断幻灯片 {}",
                window.number,
                format_range(window.start_ms, window.end_ms),
                window.slide_position.0
            ),
        });
        details.push(build_detail(
            window,
            window_position == 0,
            visual_slide,
            slide_deck,
            inferred_position,
            &normalized,
        ));
    }

    let chart_width = CHART_LEFT + windows.len() * CELL_WIDTH + CHART_RIGHT;
    let chart_height = CHART_TOP + slide_deck.slides.len() * CELL_HEIGHT + CHART_BOTTOM;
    let window_ticks = window_ticks(windows);
    let slide_ticks = slide_ticks(slide_deck);
    let has_visual_reference = !visual_points.is_empty();
    let visual_agreement = if visual_window_count == 0 {
        String::new()
    } else {
        format!(
            "{:.1}%",
            exact_visual_matches as f64 * 100.0 / visual_window_count as f64
        )
    };

    AlignmentVisualizationTemplate {
        window_count: windows.len(),
        slide_count: slide_deck.slides.len(),
        chart_width,
        chart_height,
        heatmap_width: windows.len() * CELL_WIDTH,
        heatmap_height: slide_deck.slides.len() * CELL_HEIGHT,
        cells,
        dp_path: dp_points.join(" "),
        visual_path: visual_points.join(" "),
        has_visual_reference,
        visual_agreement,
        mismatch_count: mismatch_connectors.len(),
        mismatch_connectors,
        mismatch_markers,
        window_targets,
        window_ticks,
        slide_ticks,
        details,
    }
    .render()
    .map_err(AlignmentVisualizationError::Render)
}

fn validate_window(
    window: &AlignmentVisualizationWindow<'_>,
    expected_ids: &[SlideId],
    slide_positions: &HashMap<SlideId, usize>,
) -> Result<(), AlignmentVisualizationError> {
    if window.start_ms > window.end_ms {
        return Err(AlignmentVisualizationError::ReversedWindow {
            window: window.number,
        });
    }
    if window
        .slide_scores
        .iter()
        .map(|score| score.slide_id)
        .ne(expected_ids.iter().copied())
    {
        return Err(AlignmentVisualizationError::InconsistentSlideScores {
            window: window.number,
        });
    }
    if let Some(score) = window
        .slide_scores
        .iter()
        .find(|score| !score.score.is_finite())
    {
        return Err(AlignmentVisualizationError::NonFiniteScore {
            window: window.number,
            slide_id: score.slide_id,
        });
    }
    if !slide_positions.contains_key(&window.slide_position) {
        return Err(AlignmentVisualizationError::UnknownSlidePosition {
            window: window.number,
            slide_id: window.slide_position,
        });
    }
    Ok(())
}

fn normalize(scores: &[SlideScore]) -> Vec<f64> {
    let minimum = scores
        .iter()
        .map(|score| score.score)
        .fold(f64::INFINITY, f64::min);
    let maximum = scores
        .iter()
        .map(|score| score.score)
        .fold(f64::NEG_INFINITY, f64::max);
    let range = maximum - minimum;
    if range == 0.0 {
        vec![0.0; scores.len()]
    } else {
        scores
            .iter()
            .map(|score| (score.score - minimum) / range)
            .collect()
    }
}

fn heat_color(strength: f64) -> String {
    let color = colorous::PLASMA.eval_continuous(strength);
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

fn point(x: usize, slide_position: usize) -> String {
    format!("{},{}", x + CELL_WIDTH / 2, chart_y(slide_position))
}

fn chart_y(slide_position: usize) -> usize {
    CHART_TOP + slide_position * CELL_HEIGHT + CELL_HEIGHT / 2
}

fn visual_slide_for_window(
    window: &AlignmentVisualizationWindow<'_>,
    alignment: &VisualAlignment,
    slide_positions: &HashMap<SlideId, usize>,
) -> Option<SlideId> {
    let frames: Vec<_> = alignment
        .frame_matches
        .iter()
        .filter(|frame| {
            frame.timestamp_ms >= window.start_ms && frame.timestamp_ms <= window.end_ms
        })
        .collect();
    let frames = if frames.is_empty() {
        let midpoint = window.start_ms + (window.end_ms - window.start_ms) / 2;
        let nearest = alignment
            .frame_matches
            .iter()
            .min_by_key(|frame| frame.timestamp_ms.abs_diff(midpoint))?;
        (nearest.timestamp_ms.abs_diff(midpoint) <= alignment.sample_period_ms)
            .then_some(vec![nearest])?
    } else {
        frames
    };

    let mut counts = HashMap::new();
    for frame in frames {
        if slide_positions.contains_key(&frame.best.slide_id) {
            *counts.entry(frame.best.slide_id).or_insert(0_usize) += 1;
        }
    }
    counts
        .into_iter()
        .max_by(|(left_id, left_count), (right_id, right_count)| {
            left_count
                .cmp(right_count)
                .then_with(|| slide_positions[right_id].cmp(&slide_positions[left_id]))
        })
        .map(|(slide_id, _)| slide_id)
}

fn build_detail<'a>(
    window: &AlignmentVisualizationWindow<'a>,
    selected: bool,
    visual_slide: Option<SlideId>,
    slide_deck: &'a SlideDeck,
    inferred_position: usize,
    normalized: &[f64],
) -> WindowDetail<'a> {
    let nearby_start = inferred_position.saturating_sub(3);
    let nearby_end = (inferred_position + 4).min(slide_deck.slides.len());
    let nearby_slides = slide_deck.slides[nearby_start..nearby_end]
        .iter()
        .map(|slide| NearbySlide {
            id: slide.id.0,
            text: slide.text.as_str(),
            inferred: slide.id == window.slide_position,
            visual: visual_slide == Some(slide.id),
        })
        .collect();

    let mut ranked: Vec<_> = window
        .slide_scores
        .iter()
        .zip(normalized)
        .enumerate()
        .collect();
    ranked.sort_by(|left, right| right.1.0.score.total_cmp(&left.1.0.score));
    let top_scores = ranked
        .into_iter()
        .take(5)
        .map(|(position, (score, strength))| ScoreBar {
            slide_id: score.slide_id.0,
            text: slide_deck.slides[position].text.as_str(),
            score: format!("{:.6}", score.score),
            width: (strength * 100.0).round() as u8,
        })
        .collect();

    WindowDetail {
        number: window.number,
        hidden: !selected,
        timestamp: format_range(window.start_ms, window.end_ms),
        transcript: window.transcript,
        inferred_slide: window.slide_position.0,
        has_visual_slide: visual_slide.is_some(),
        visual_slide: visual_slide.map_or(0, |slide| slide.0),
        mismatch: visual_slide.is_some_and(|slide| slide != window.slide_position),
        nearby_slides,
        top_scores,
    }
}

fn window_ticks(windows: &[AlignmentVisualizationWindow<'_>]) -> Vec<AxisTick> {
    let interval = windows.len().div_ceil(10).max(1);
    windows
        .iter()
        .enumerate()
        .filter(|(position, _)| position % interval == 0 || position + 1 == windows.len())
        .map(|(position, window)| AxisTick {
            x: CHART_LEFT + position * CELL_WIDTH + CELL_WIDTH / 2,
            y: CHART_TOP + 4,
            label: window.number.to_string(),
        })
        .collect()
}

fn slide_ticks(slide_deck: &SlideDeck) -> Vec<AxisTick> {
    let interval = slide_deck.slides.len().div_ceil(8).max(1);
    slide_deck
        .slides
        .iter()
        .enumerate()
        .filter(|(position, _)| position % interval == 0 || position + 1 == slide_deck.slides.len())
        .map(|(position, slide)| AxisTick {
            x: CHART_LEFT - 8,
            y: CHART_TOP + position * CELL_HEIGHT + CELL_HEIGHT / 2 + 3,
            label: slide.id.0.to_string(),
        })
        .collect()
}

fn format_range(start_ms: u64, end_ms: u64) -> String {
    format!("{}–{}", format_time(start_ms), format_time(end_ms))
}

fn format_time(milliseconds: u64) -> String {
    let seconds = milliseconds / 1_000;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3_600,
        (seconds / 60) % 60,
        seconds % 60
    )
}

#[derive(Template)]
#[template(path = "alignment_visualize.html")]
struct AlignmentVisualizationTemplate<'a> {
    window_count: usize,
    slide_count: usize,
    chart_width: usize,
    chart_height: usize,
    heatmap_width: usize,
    heatmap_height: usize,
    cells: Vec<HeatmapCell>,
    dp_path: String,
    visual_path: String,
    has_visual_reference: bool,
    visual_agreement: String,
    mismatch_count: usize,
    mismatch_connectors: Vec<MismatchConnector>,
    mismatch_markers: Vec<ChartPoint>,
    window_targets: Vec<WindowTarget>,
    window_ticks: Vec<AxisTick>,
    slide_ticks: Vec<AxisTick>,
    details: Vec<WindowDetail<'a>>,
}

struct HeatmapCell {
    x: usize,
    y: usize,
    fill: String,
    window_number: usize,
    slide_id: u32,
    score: String,
}

struct ChartPoint {
    x: usize,
    y: usize,
}

struct MismatchConnector {
    x: usize,
    inferred_y: usize,
    visual_y: usize,
}

struct WindowTarget {
    number: usize,
    x: usize,
    width: usize,
    height: usize,
    label: String,
}

struct AxisTick {
    x: usize,
    y: usize,
    label: String,
}

struct WindowDetail<'a> {
    number: usize,
    hidden: bool,
    timestamp: String,
    transcript: &'a str,
    inferred_slide: u32,
    has_visual_slide: bool,
    visual_slide: u32,
    mismatch: bool,
    nearby_slides: Vec<NearbySlide<'a>>,
    top_scores: Vec<ScoreBar<'a>>,
}

struct NearbySlide<'a> {
    id: u32,
    text: &'a str,
    inferred: bool,
    visual: bool,
}

struct ScoreBar<'a> {
    slide_id: u32,
    text: &'a str,
    score: String,
    width: u8,
}

#[derive(Debug)]
pub enum AlignmentVisualizationError {
    EmptySlideDeck,
    NoWindows,
    ReversedWindow { window: usize },
    InconsistentSlideScores { window: usize },
    NonFiniteScore { window: usize, slide_id: SlideId },
    UnknownSlidePosition { window: usize, slide_id: SlideId },
    Render(askama::Error),
}

impl fmt::Display for AlignmentVisualizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySlideDeck => write!(formatter, "cannot render alignment without slides"),
            Self::NoWindows => write!(formatter, "cannot render alignment without windows"),
            Self::ReversedWindow { window } => {
                write!(formatter, "alignment window {window} ends before it starts")
            }
            Self::InconsistentSlideScores { window } => write!(
                formatter,
                "alignment window {window} does not score every slide in presentation order"
            ),
            Self::NonFiniteScore { window, slide_id } => write!(
                formatter,
                "alignment window {window} has a non-finite score for slide {}",
                slide_id.0
            ),
            Self::UnknownSlidePosition { window, slide_id } => write!(
                formatter,
                "alignment window {window} selects unknown slide {}",
                slide_id.0
            ),
            Self::Render(error) => {
                write!(
                    formatter,
                    "could not render alignment visualization HTML: {error}"
                )
            }
        }
    }
}

impl Error for AlignmentVisualizationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Render(error) => Some(error),
            _ => None,
        }
    }
}
