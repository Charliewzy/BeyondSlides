# Classify passage boundaries independently of request windows

Window-owned passage preparation forces cuts through explanations, so an
opt-in boundary-first mode evaluates punctuation-derived candidate gaps with
overlapping context and selects a global partition in Rust; request boundaries
do not constrain passage boundaries. This remains an alternative to ADR-0003's
copied-text projection and preserves ADR-0002's coarse provenance by slicing
authoritative restored text directly.

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

The mode infers new slide positions and reruns comparative rankings without
transferring old passage judgments. The legacy window-owned mode and its
checkpoints remain available. Changing the prompt and serialized decision shape
selects a new checkpoint identity, so categorical checkpoints are not reused.
