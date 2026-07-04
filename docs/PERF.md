# Performance checks

Use the performance checks to catch slowdowns in the diff engine.

The project has:

- Criterion benchmarks
- optional scaling tests for algorithmic regressions
- profiling commands for local investigation

## Run benchmarks

Run the Criterion benchmark suite:

```sh
cargo bench -p oyo-core --bench perf
```

Criterion uses gnuplot when it is available. Otherwise, it falls back to plotters.

## Compare with a baseline

Save a baseline before you change performance-sensitive code:

```sh
cargo bench -p oyo-core --bench perf -- --save-baseline main
```

Compare against it later:

```sh
cargo bench -p oyo-core --bench perf -- --baseline main
```

## Run scaling tests

Scaling tests are off by default because timings can be noisy in CI.

Enable them with `OYO_PERF_TESTS=1`:

```sh
OYO_PERF_TESTS=1 cargo test -p oyo-core --test perf_guard
```

These tests compare small and large inputs. They check scaling, not absolute time, so they are less sensitive to machine speed.

Use them to catch unexpected `O(n^2)` behaviour.

## Profile locally

On macOS, capture a Time Profiler trace:

```sh
xcrun xctrace record --template "Time Profiler" --launch -- ./target/release/oy
```

Open the trace in Firefox Profiler.

You can also use `samply`:

```sh
samply record ./target/release/oy --range d7857b3...HEAD
```
