# SCIP C fixture

Minimal C source used by `scip_import.rs`.

The test hand-crafts a tiny valid `index.scip` with prost because this cut is
decode-only and must not require `scip-clang` on PATH.
