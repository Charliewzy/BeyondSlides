# Classify passage boundaries independently of request windows

Window-owned passage preparation forces cuts through explanations, so passage
segmentation evaluates punctuation-derived candidate gaps with overlapping
context and selects a global partition in Rust; request boundaries do not
constrain passage boundaries. This is the standard path for new analyses and
preserves ADR-0002's coarse provenance by slicing authoritative restored text
directly. Completed artifacts created through ADR-0003's copied-text projection
remain readable, but that path is no longer offered for new execution.

The former categorical decisions made `continue` uncuttable and
`required_break` mandatory. A real lecture produced a coherent 451-character
region whose internal gaps were all `continue`, making the semantic constraint
incompatible with the 450-character hard limit and deterministically stopping
the complete product run. Prompt warnings could not guarantee feasibility, and
raising the limit would only move the same failure.

Each candidate gap now receives an ordinal `0..5` cut cost: zero is natural and
five is severely damaging, but no cost is an absolute prohibition. Passage
selection enforces a 300-character hard maximum and lexicographically minimizes
the number of selected cost-five cuts, then cost-four through cost-one cuts,
then passage count, and finally squared passage lengths. This does not convert
semantic judgments, character counts, and passage counts onto an arbitrary
shared numeric scale. Punctuation remains the ordinary atomization mechanism;
long punctuation-free atoms receive evenly distributed UTF-8-safe fallback
gaps so a source-preserving partition is always feasible.

The pipeline infers slide positions and runs comparative rankings after the
partition exists. Boundary classifications have their own checkpoint identity;
changing the prompt or serialized decision shape therefore does not silently
reuse incompatible checkpoints.
