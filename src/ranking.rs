use std::cmp::Reverse;

use crate::{LecturePassage, ValidatedAnalysis};

pub fn rank_oral_additions(analysis: &ValidatedAnalysis) -> Vec<&LecturePassage> {
    let mut passages: Vec<_> = analysis
        .passages()
        .iter()
        .filter(|passage| qualifies_as_oral_addition(passage))
        .collect();

    passages.sort_by_key(|passage| Reverse((passage.importance.get(), passage.novelty.get())));
    passages
}

fn qualifies_as_oral_addition(passage: &LecturePassage) -> bool {
    passage.importance.get() >= 3 && passage.novelty.get() >= 2
}
