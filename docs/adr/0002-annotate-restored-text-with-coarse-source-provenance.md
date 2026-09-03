# Annotate restored text with coarse source provenance

Lecture passages partition the readable restored transcript rather than raw
transcript segments. Each passage retains the first and last source segment of
the restored spans supporting it, but adjacent passages may share that source
range when a semantic boundary falls inside one restored span; this preserves
readable annotation granularity without inventing finer provenance than the
restoration stage produced.
