<h1 align="center">
    <b>env-lib</b><br>
    <sub><sup>PERFORMANCE</sup></sub>
</h1>
<div align="center">
    <sup>
        <a href="../README.md" title="Project Home"><b>HOME</b></a>
        <span>&nbsp;&mdash;&nbsp;</span>
        <a href="./API.md" title="API Reference"><b>API</b></a>
        <span>&nbsp;&mdash;&nbsp;</span>
        <a href="./GUIDELINES.md" title="Developer Guidelines"><b>GUIDELINES</b></a>
        <span>&nbsp;&mdash;&nbsp;</span>
        <a href="./STABILITY-1.0.md" title="Stability Contract"><b>STABILITY</b></a>
        <span>&nbsp;&mdash;&nbsp;</span>
        <span>PERFORMANCE</span>
    </sup>
</div>
<br>

> **Status:** Numbers below are the **1.0.0** baseline. Re-run the suite locally to compare on your hardware.

## Methodology

- **Harness:** [`criterion`](https://crates.io/crates/criterion) 0.5, `cargo bench --bench env_bench`.
- **Profile:** `release` with `lto = "fat"`, `codegen-units = 1`.
- **Machine:** Windows 11, x86_64, stable Rust. Each number is the criterion median of a 3-second measurement after a 1-second warm-up.
- **Input:** generated `.env` text with a comment header and `n` entries of the form `KEY_i=value-i`. Every fifth entry is followed by a double-quoted entry with an escape and an inline comment, so a file of `n` entries holds `1.2 n` variables in total.

## Results

### Parsing

| Operation | Median |
|---|---|
| `Env::from_text`, 10 entries (12 variables) | 3.95 µs |
| `Env::from_text`, 1000 entries (1200 variables) | 450 µs |

Parsing costs roughly 0.3 to 0.4 µs per variable and grows linearly with the file. It covers the whole load: splitting lines, validating names and values, decoding quoted values, keeping each line's original text for lossless saves, and building the lookup table. `Env::open` adds one file read on top of this. A load happens once per `open` or `reload`, not per lookup.

### Lookups

Measured on an `Env` loaded from the 100-entry input.

| Operation | Median |
|---|---|
| `get("KEY_50")`, held by the `Env` | 15.0 ns |
| `get("JG_KEY_50")`, prefixed name | 15.9 ns |
| `get("NOT_THERE")`, falls back to the process environment | 156 ns |
| `parse::<u32>("KEY_50")`, value does not parse | 73 ns |

A hit is one hash lookup and returns a borrowed value without allocating. Accepting the prefixed form of a name adds under a nanosecond. A miss validates the name, builds the prefixed name, and asks the operating system for `JG_NOT_THERE`; the system call is most of the 156 ns. Code that reads the same missing variable in a loop should read it once.

The `parse` benchmark's value is `value-50`, which is not a `u32`, so it measures the failing path, including building `Error::Parse`. A successful parse costs the lookup plus the `FromStr` implementation.

### Updates

| Operation | Median |
|---|---|
| `set("KEY_50", n.to_string())`, replacing an existing value | 87 ns |

The number includes formatting the new value with `to_string` in the benchmark loop. `set` validates the name and the value, then replaces the value in place.

### Saving

`save` is not benchmarked. It reads and parses the file again, at the parsing cost above, applies the session's changes, renders the result in memory, and writes it with `path_lib::write_atomic`. The write's cost is dominated by flushing to disk (`sync_all` on the new file and, on Unix, its directory). That time depends on the storage device and filesystem, not on this crate, so a number measured on one machine would say nothing about another.

## Running the Benchmarks

```bash
cargo bench --bench env_bench
```

## Interpreting the Numbers

An application loads its `.env` file at startup and reads a few dozen variables, most of them once. At these costs, loading a typical file takes a few microseconds and reading a variable takes nanoseconds, well below the cost of the file read and the system calls that surround them. Keep the `Env` and read from it; there is no need to cache values outside it.
