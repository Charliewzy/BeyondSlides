# Use canonical zero-based source IDs

Normalized transcript segments and slides use typed `u32` IDs equal to their zero-based collection positions. Arbitrary stable IDs imposed lookup maps and scans without serving a current product requirement; canonical IDs make source lookup direct while preserving type safety in serialized references. IDs are stable only within one normalized source snapshot, human-facing slide and PDF page numbers remain separately one-based, and any future cross-normalization identity must use a separate source reference.
