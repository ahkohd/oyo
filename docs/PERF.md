# Performance

This project includes Criterion benchmarks and lightweight perf guards to catch
algorithmic regressions (O(n^2) surprises) in the diff engine.

## Benchmarks

Run the Criterion suite:

```bash
cargo bench -p oyo-core --bench perf
```

Notes:
- Criterion will use gnuplot if available; otherwise it falls back to plotters.
- You can save a baseline for comparison:

```bash
cargo bench -p oyo-core --bench perf -- --save-baseline main
```

Then compare against it later with:

```bash
cargo bench -p oyo-core --bench perf -- --baseline main
```

## Perf guards (scaling checks)

These tests are off by default to avoid CI flakiness. Enable them with
`OYO_PERF_TESTS=1`:

```bash
OYO_PERF_TESTS=1 cargo test -p oyo-core --test perf_guard
```

The guards compare small vs large inputs to ensure linear scaling (not absolute
time), so they are resilient across machines.

## Profiling

Capture a Time Profiler trace and inspect it in Firefox Profiler:

```bash
xcrun xctrace record --template "Time Profiler" --launch -- ./target/release/oy
```

You can also use `samply`:

```bash
samply record ./target/release/oy --range d7857b3...HEAD
```
