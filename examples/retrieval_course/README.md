# Chinese retrieval comparison fixture

This synthetic fixture compares slide retrieval modes without pretending that
their scores are exact alignment labels. It contains 12 concise slides and 18
queries split evenly between:

- semantic paraphrases with deliberately limited wording overlap;
- exact technical terms, identifiers, and formulas;
- mixed Chinese and English technical language.

Each query names one or more human-selected relevant slide IDs. As normalized
source references, those IDs are canonical zero-based slide positions. Run the
comparison with:

```sh
cargo run --example compare_retrieval
```

The first dense run downloads `BAAI/bge-small-zh-v1.5` into
`.fastembed_cache`; the downloaded model is about 91 MiB and is gitignored.

## Baseline result

Recorded on 2026-08-28 with FastEmbed 6.0.2 and
`BAAI/bge-small-zh-v1.5`:

| Mode | Hit@1 | Hit@3 | MRR |
| --- | ---: | ---: | ---: |
| BM25 | 77.8% | 83.3% | 0.807 |
| Dense | 66.7% | 77.8% | 0.769 |
| Hybrid RRF | 72.2% | 83.3% | 0.794 |

The exact and mixed groups were easy for BM25: it reached 100% Hit@1 on both.
All modes struggled on the six semantic cases; BM25 reached 33.3% Hit@1 while
dense and hybrid each reached 16.7%. Equal-weight fusion recovered BM25's
mixed-query wins but did not improve the overall baseline.

This is evidence against assuming that dense or hybrid retrieval is
automatically better. It is not evidence for selecting BM25 permanently: the
fixture is small and synthetic, so model and fusion choices must be revisited
on labeled excerpts from real Chinese lectures.
