# Profiling

This repo includes a small Rust helper, `oy-profile`, to summarize samply/Firefox
JSON profiles.

## Record with samply

Build a release binary and record a profile:

```bash
cargo build -p oyo --release
samply record --save-only -o profile.json.gz -- ./target/release/oy --range HEAD...HEAD
```

Notes:
- Use a `.json.gz` output name to keep files small.
- If you need perf permissions on Linux:

```bash
sudo sysctl -w kernel.perf_event_paranoid=1
```

Reset it when you are done:

```bash
sudo sysctl -w kernel.perf_event_paranoid=2
```

## Summarize profiles

Quick report (hot threads/paths/spots/modules):

```bash
cargo run -p oyo --bin oy-profile -- profile.json.gz --report --top 15
```

Add `--verbose` to include the full ranked function table with `--report`.
By default the report includes idle samples (percent columns are total). To
exclude idle samples (poll/sleep/etc), pass `--no-idle`. Customize the idle
matcher with:

```bash
cargo run -p oyo --bin oy-profile -- profile.json.gz --report --no-idle
cargo run -p oyo --bin oy-profile -- profile.json.gz --report --idle-pattern "my_idle_fn"
```

List threads:

```bash
cargo run -p oyo --bin oy-profile -- profile.json.gz --list-threads
```

Pick a thread and mode (inclusive = hot paths, leaf = self time):

```bash
cargo run -p oyo --bin oy-profile -- profile.json.gz --thread 0 --mode inclusive
cargo run -p oyo --bin oy-profile -- profile.json.gz --thread oy --mode leaf --top 30
```

By default the tool selects the hottest thread (based on the chosen metric) and
prints a short thread summary. Control the metric and summary size with:

```bash
cargo run -p oyo --bin oy-profile -- profile.json.gz --metric weight
cargo run -p oyo --bin oy-profile -- profile.json.gz --metric time --top-threads 0
```

Interpretation notes:
- CPU% is per-thread (threadCPUDelta / thread lifetime), not whole-system CPU.
- Short-lived threads can show near-100% CPU even with few samples.
- If `threadCPUDelta` or timing data is missing, CPU% is omitted.

## Debug logs

For diff UI debug logs (extent markers, navigation, etc.), see `docs/DEBUG.md`.
