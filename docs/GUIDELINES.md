<h1 align="center">
    <b>env-lib</b><br>
    <sub><sup>DEVELOPER GUIDELINES</sup></sub>
</h1>
<div align="center">
    <sup>
        <a href="../README.md" title="Project Home"><b>HOME</b></a>
        <span>&nbsp;&mdash;&nbsp;</span>
        <a href="./API.md" title="API Reference"><b>API</b></a>
        <span>&nbsp;&mdash;&nbsp;</span>
        <a href="./PERFORMANCE.md" title="Performance"><b>PERFORMANCE</b></a>
        <span>&nbsp;&mdash;&nbsp;</span>
        <a href="./STABILITY-1.0.md" title="Stability Contract"><b>STABILITY</b></a>
        <span>&nbsp;&mdash;&nbsp;</span>
        <span>GUIDELINES</span>
    </sup>
</div>
<br>

This document describes the engineering standards this project is built to. [REPS](../REPS.md) is the governing standard; this page records how it applies here.

## Standards

- **MSRV:** Rust 1.85
- **Edition:** 2024
- **License:** Apache-2.0 OR MIT (dual)
- **Cross-platform:** Linux, macOS, Windows tested in CI; FreeBSD type-checked

## Quality Bar

- **One runtime dependency.** `path-lib`, a first-party crate with no dependencies of its own, for home expansion, the application config directory, and atomic writes. `criterion` and `proptest` are dev-dependencies only.
- **`unsafe` confined to `export`.** The crate root denies `unsafe_code`; only `Env::export` opts out, to call `std::env::set_var` and `remove_var`. It is an `unsafe fn` with a `# Safety` section, and each `unsafe` block carries a `SAFETY` comment.
- **No panics in library code** &mdash; no `unwrap`, `expect`, `todo`, `unimplemented`, or `unreachable`.
- **Values stay out of output.** No error message and no `Debug` output may contain a variable's value. Tests check this for parse errors, value errors, and `Debug`.
- **Reject, don't guess.** Invalid names, invalid prefixes, and malformed lines are errors with the line number, never silently changed or skipped.
- **Lossless files.** A save must not change anything the program did not change, including edits made to the file on disk after it was loaded. Property tests check that any set of variables round-trips through a file and that saving an unchanged file is byte-identical.
- **Property-tested parser.** The parser is run against arbitrary text and must return an error, never panic.
- **Benchmark-backed claims** &mdash; performance statements are verified by committed benchmarks.

## Contributing

1. `cargo fmt --all -- --check` passes.
2. `cargo clippy --all-targets --all-features -- -D warnings` is clean, also with `--target` set to a macOS and a Linux triple.
3. `cargo test --all-features` passes.
4. New public items include documentation and examples.
5. A change to the accepted file syntax or to what a save preserves updates the parser documentation in `src/dotenv.rs`, the [file format section](./API.md#the-file-format) of the API reference, and the tests together. The format is part of the [1.x stability contract](./STABILITY-1.0.md): a file that loads under 1.0 must load the same way under every 1.x release.
6. Tests that change the process environment go in their own test binary, as `tests/export.rs` does, so no other test runs concurrently with them.
7. A change to the public API is checked against the [stability contract](./STABILITY-1.0.md) before it is merged.
