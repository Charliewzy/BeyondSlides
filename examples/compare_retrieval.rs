use std::{collections::HashMap, error::Error};

use beyond_slides::{
    DenseSlideSearcher, HybridSlideSearcher, LexicalSlideSearcher, SlideDeck, SlideId,
    SlideSearcher, Transcript, ValidatedSources,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct RetrievalCases {
    queries: Vec<RetrievalCase>,
}

#[derive(Deserialize)]
struct RetrievalCase {
    id: String,
    kind: String,
    query: String,
    relevant_slides: Vec<SlideId>,
}

struct QueryResult {
    first_relevant_rank: Option<usize>,
    top_three: Vec<SlideId>,
}

struct ModeResults {
    name: &'static str,
    queries: HashMap<String, QueryResult>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let slide_deck: SlideDeck = serde_json::from_str(include_str!("retrieval_course/slides.json"))?;
    let cases: RetrievalCases =
        serde_json::from_str(include_str!("retrieval_course/queries.json"))?;
    let sources = ValidatedSources::new(Transcript { sentences: vec![] }, slide_deck)?;

    eprintln!("Loading BAAI/bge-small-zh-v1.5 and indexing slides...");
    let lexical = LexicalSlideSearcher::new(&sources);
    let dense = DenseSlideSearcher::try_new(&sources)?;
    let hybrid = HybridSlideSearcher::new(&lexical, &dense);
    let searchers: [(&str, &dyn SlideSearcher); 3] =
        [("BM25", &lexical), ("Dense", &dense), ("Hybrid", &hybrid)];

    let mut all_results = Vec::new();
    for (name, searcher) in searchers {
        let mut queries = HashMap::new();
        for case in &cases.queries {
            let hits = searcher.search(&case.query, sources.slide_deck().slides.len())?;
            let first_relevant_rank = hits
                .iter()
                .position(|hit| case.relevant_slides.contains(&hit.slide_id))
                .map(|position| position + 1);
            let top_three = hits.iter().take(3).map(|hit| hit.slide_id).collect();
            queries.insert(
                case.id.clone(),
                QueryResult {
                    first_relevant_rank,
                    top_three,
                },
            );
        }
        all_results.push(ModeResults { name, queries });
    }

    println!("| Mode | Cases | Hit@1 | Hit@3 | MRR |");
    println!("| --- | ---: | ---: | ---: | ---: |");
    for results in &all_results {
        print_metrics(results, &cases.queries, "all");
        for kind in ["semantic", "exact", "mixed"] {
            let matching: Vec<_> = cases
                .queries
                .iter()
                .filter(|case| case.kind == kind)
                .collect();
            print_metrics(results, matching.iter().copied(), kind);
        }
    }

    println!("\nTop-three slide IDs by query (`*` means the expected slide is first):");
    for case in &cases.queries {
        print!("{} [{}]", case.id, case.kind);
        for results in &all_results {
            let result = &results.queries[&case.id];
            let marker = if result.first_relevant_rank == Some(1) {
                "*"
            } else {
                ""
            };
            let ids: Vec<_> = result.top_three.iter().map(|id| id.0).collect();
            print!("  {}{}={ids:?}", results.name, marker);
        }
        println!();
    }

    Ok(())
}

fn print_metrics<'a, I>(results: &ModeResults, cases: I, label: &str)
where
    I: IntoIterator<Item = &'a RetrievalCase>,
{
    let cases: Vec<_> = cases.into_iter().collect();
    let ranks: Vec<_> = cases
        .iter()
        .filter_map(|case| results.queries[&case.id].first_relevant_rank)
        .collect();
    let hit_at_one = ranks.iter().filter(|&&rank| rank <= 1).count() as f64 / cases.len() as f64;
    let hit_at_three = ranks.iter().filter(|&&rank| rank <= 3).count() as f64 / cases.len() as f64;
    let reciprocal_rank =
        ranks.iter().map(|&rank| 1.0 / rank as f64).sum::<f64>() / cases.len() as f64;

    println!(
        "| {} ({label}) | {} | {:.1}% | {:.1}% | {:.3} |",
        results.name,
        cases.len(),
        hit_at_one * 100.0,
        hit_at_three * 100.0,
        reciprocal_rank,
    );
}
