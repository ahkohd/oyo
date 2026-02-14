use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use clap::{ArgAction, Parser, ValueEnum};
use serde_json::Value;

const DEFAULT_IDLE_PATTERNS: &[&str] = &[
    "crossterm::event::poll",
    "crossterm::event::read",
    "crossterm::event::source::",
    "EventSource::try_read",
    "clock_gettime",
    "epoll_pwait",
    "epoll_wait",
    "kevent",
    "nanosleep",
    "pselect",
    "poll",
    "ppoll",
    "select",
    "std::thread::park",
    "std::thread::sleep",
    "parking_lot::park",
    "futex",
];

#[derive(ValueEnum, Clone, Copy, Debug)]
enum CountMode {
    Leaf,
    Inclusive,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum Metric {
    Auto,
    Samples,
    Weight,
    Time,
}

#[derive(Parser, Debug)]
#[command(author, version, about = "Summarize samply/Firefox JSON profiles")]
struct Args {
    /// Path to profile.json or profile.json.gz
    #[arg(value_name = "PROFILE")]
    input: PathBuf,

    /// Thread index or name substring (case-insensitive)
    #[arg(long, value_name = "THREAD")]
    thread: Option<String>,

    /// Top N functions to show
    #[arg(long, default_value_t = 20)]
    top: usize,

    /// Count mode for functions
    #[arg(long, value_enum, default_value_t = CountMode::Inclusive)]
    mode: CountMode,

    /// Metric to rank samples (auto selects weight/time when available)
    #[arg(long, value_enum, default_value_t = Metric::Auto)]
    metric: Metric,

    /// Print a short thread summary (0 to disable)
    #[arg(long, default_value_t = 5)]
    top_threads: usize,

    /// Print a module-level report summary
    #[arg(long)]
    report: bool,

    /// Show full function tables with report output
    #[arg(long)]
    verbose: bool,

    /// Exclude idle samples from report output
    #[arg(long)]
    no_idle: bool,

    /// Mark leaf functions as idle when they contain this substring (case-insensitive)
    #[arg(long, value_name = "PATTERN", action = ArgAction::Append)]
    idle_pattern: Vec<String>,

    /// Only list threads and exit
    #[arg(long)]
    list_threads: bool,
}

#[derive(Debug)]
enum ThreadSelector {
    Index(usize),
    Name(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MetricKind {
    Samples,
    Weight,
    Time,
}

#[derive(Debug)]
struct MetricInfo {
    kind: MetricKind,
    label: String,
    column_label: String,
    note: Option<String>,
}

#[derive(Debug)]
struct Sample {
    stack: Option<usize>,
    weight: f64,
    is_idle: bool,
}

#[derive(Debug)]
struct ThreadStats {
    sample_count: usize,
    weight_sum: Option<f64>,
    time_sum: Option<f64>,
    weight_type: Option<String>,
    cpu_percent: Option<f64>,
    cpu_ms: Option<f64>,
    elapsed_ms: Option<f64>,
}

struct IdleClassifier {
    patterns: Vec<String>,
}

#[derive(Clone, Debug)]
struct SampleUnits {
    time_label: Option<String>,
    time_scale_ms: Option<f64>,
    cpu_scale_ms: Option<f64>,
}

#[derive(Clone, Debug)]
struct FunctionInfo {
    name: String,
    resource: Option<String>,
    label: String,
}

struct FunctionCache {
    infos: Vec<FunctionInfo>,
}

#[derive(Debug)]
struct IdleStats {
    enabled: bool,
    total_weight: f64,
    idle_weight: f64,
    active_weight: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SampleFilter {
    All,
    Active,
}

#[derive(Clone, Debug)]
struct ThreadSummary {
    index: usize,
    name: String,
    process_type: String,
    pid: String,
    metric_value: f64,
    sample_count: usize,
    cpu_percent: Option<f64>,
}

impl IdleClassifier {
    fn new(patterns: Vec<String>) -> Self {
        Self {
            patterns: patterns
                .into_iter()
                .map(|pattern| pattern.to_ascii_lowercase())
                .collect(),
        }
    }

    fn is_idle(&self, info: &FunctionInfo) -> bool {
        let label = info.label.to_ascii_lowercase();
        self.patterns.iter().any(|pattern| label.contains(pattern))
    }
}

impl FunctionCache {
    fn new(tables: &ThreadTables<'_>) -> Self {
        let mut infos = Vec::with_capacity(tables.func_name.len());
        for index in 0..tables.func_name.len() {
            infos.push(func_info(tables, index));
        }
        Self { infos }
    }

    fn get(&self, index: usize) -> Option<&FunctionInfo> {
        self.infos.get(index)
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let profile = read_profile(&args.input)?;

    if let Some(meta) = profile.get("meta") {
        let product = meta.get("product").and_then(Value::as_str).unwrap_or("-");
        let version = meta.get("version").and_then(Value::as_u64).unwrap_or(0);
        let interval = meta.get("interval").and_then(Value::as_f64).unwrap_or(0.0);
        println!("profile: {}", args.input.display());
        println!(
            "meta: product={} version={} interval={}ms",
            product, version, interval
        );
    } else {
        println!("profile: {}", args.input.display());
    }

    let threads = profile
        .get("threads")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("profile missing threads array"))?;

    let units = sample_units(&profile);
    let idle_classifier = if args.report && args.no_idle {
        let patterns = if args.idle_pattern.is_empty() {
            DEFAULT_IDLE_PATTERNS
                .iter()
                .map(|pattern| pattern.to_string())
                .collect()
        } else {
            args.idle_pattern.clone()
        };
        Some(IdleClassifier::new(patterns))
    } else {
        None
    };
    let stats = collect_thread_stats(threads, &units);
    let global_metric = resolve_global_metric(args.metric, &stats);
    let thread_summary = if args.top_threads > 0 {
        Some(collect_thread_summary(
            threads,
            &stats,
            global_metric,
            args.top_threads,
        ))
    } else {
        None
    };

    if args.list_threads {
        print_threads(threads, &stats, &units)?;
        return Ok(());
    }

    if !args.report {
        if let Some(summary) = thread_summary.as_ref() {
            print_thread_summary(summary, global_metric, &units, "threads")?;
        }
    }

    let selector = args.thread.as_deref().map(parse_thread_selector);
    let (thread_index, thread) = select_thread(threads, selector, args.metric, &stats)?;
    let thread_name = thread
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("<unnamed>");
    let sample_count = thread.get("samples").map(sample_count).unwrap_or(0);

    let metric_info = resolve_metric_info(args.metric, thread, &units);
    println!(
        "thread[{}]: {} (samples={})",
        thread_index, thread_name, sample_count
    );
    println!(
        "metric: {}{}",
        metric_info.label,
        metric_info
            .note
            .as_deref()
            .map(|note| format!(" ({})", note))
            .unwrap_or_default()
    );
    println!("mode: {:?} top: {}", args.mode, args.top);
    if let Some(stat) = stats.get(thread_index) {
        if let (Some(cpu_percent), Some(cpu_ms), Some(elapsed_ms)) =
            (stat.cpu_percent, stat.cpu_ms, stat.elapsed_ms)
        {
            println!(
                "cpu: {:.1}% (thread {:.2}ms / {:.2}ms)",
                cpu_percent, cpu_ms, elapsed_ms
            );
        }
    }

    if sample_count == 0 {
        println!("no samples in this thread");
        return Ok(());
    }

    let tables = ThreadTables::from_profile_thread(&profile, thread)?;
    let function_cache = FunctionCache::new(&tables);
    let mut samples = extract_samples(thread, &metric_info, &units)?;
    let idle_stats = classify_idle_samples(
        &mut samples,
        &tables,
        &function_cache,
        idle_classifier.as_ref(),
    );

    let counts_inclusive_all =
        count_functions(&tables, &samples, CountMode::Inclusive, SampleFilter::All);
    let counts_leaf_all = count_functions(&tables, &samples, CountMode::Leaf, SampleFilter::All);
    let counts_inclusive_active = count_functions(
        &tables,
        &samples,
        CountMode::Inclusive,
        SampleFilter::Active,
    );
    let counts_leaf_active =
        count_functions(&tables, &samples, CountMode::Leaf, SampleFilter::Active);

    let mut entries_inclusive_all = build_entries(&function_cache, counts_inclusive_all);
    let mut entries_leaf_all = build_entries(&function_cache, counts_leaf_all);
    let mut entries_inclusive_active = build_entries(&function_cache, counts_inclusive_active);
    let mut entries_leaf_active = build_entries(&function_cache, counts_leaf_active);
    sort_entries(&mut entries_inclusive_all);
    sort_entries(&mut entries_leaf_all);
    sort_entries(&mut entries_inclusive_active);
    sort_entries(&mut entries_leaf_active);

    if args.report {
        print_report(ReportArgs {
            entries_inclusive: &entries_inclusive_active,
            entries_leaf: &entries_leaf_active,
            metric_info: &metric_info,
            global_metric,
            thread_summary: thread_summary.as_deref(),
            units: &units,
            idle_stats: &idle_stats,
            top: args.top,
        })?;
    }

    if !args.report || args.verbose {
        let entries = if args.report {
            match args.mode {
                CountMode::Inclusive => &entries_inclusive_active,
                CountMode::Leaf => &entries_leaf_active,
            }
        } else {
            match args.mode {
                CountMode::Inclusive => &entries_inclusive_all,
                CountMode::Leaf => &entries_leaf_all,
            }
        };

        println!("{:>4} {:>12} function", "rank", metric_info.column_label);
        for (rank, (info, count)) in entries.iter().take(args.top).enumerate() {
            let formatted = format_metric_value(*count, metric_info.kind);
            println!("{:>4} {:>12} {}", rank + 1, formatted, info.label);
        }
    }

    Ok(())
}

fn read_profile(path: &Path) -> Result<Value> {
    let bytes = read_profile_bytes(path)?;
    let profile = serde_json::from_slice(&bytes).context("failed to parse JSON profile")?;
    Ok(profile)
}

fn read_profile_bytes(path: &Path) -> Result<Vec<u8>> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut buf = Vec::new();
    if path.extension().and_then(|ext| ext.to_str()) == Some("gz") {
        let mut decoder = flate2::read::GzDecoder::new(file);
        decoder
            .read_to_end(&mut buf)
            .context("failed to read gz profile")?;
    } else {
        let mut reader = io::BufReader::new(file);
        reader
            .read_to_end(&mut buf)
            .context("failed to read profile")?;
    }
    Ok(buf)
}

fn parse_thread_selector(value: &str) -> ThreadSelector {
    if value.chars().all(|c| c.is_ascii_digit()) {
        if let Ok(index) = value.parse::<usize>() {
            return ThreadSelector::Index(index);
        }
    }
    ThreadSelector::Name(value.to_string())
}

fn collect_thread_stats(threads: &[Value], units: &SampleUnits) -> Vec<ThreadStats> {
    threads
        .iter()
        .map(|thread| thread_stats(thread, units))
        .collect()
}

fn thread_stats(thread: &Value, units: &SampleUnits) -> ThreadStats {
    let samples = thread.get("samples");
    let sample_count = samples.map(sample_count).unwrap_or(0);
    let weight_sum = samples
        .and_then(extract_weight_values)
        .map(|values| values.into_iter().fold(0.0, |acc, value| acc + value));
    let time_values = samples.and_then(|samples| extract_time_values(samples, units));
    let time_sum = time_values
        .as_ref()
        .map(|values| values.iter().fold(0.0, |acc, value| acc + value));
    let elapsed_ms = thread_elapsed_ms(thread).or_else(|| {
        time_sum.and_then(|sum| {
            if units.time_scale_ms.is_some() {
                Some(sum)
            } else {
                None
            }
        })
    });
    let cpu_ms = samples.and_then(extract_cpu_values).and_then(|values| {
        units
            .cpu_scale_ms
            .map(|scale| values.into_iter().sum::<f64>() * scale)
    });
    let cpu_percent = match (cpu_ms, elapsed_ms) {
        (Some(cpu_ms), Some(elapsed_ms)) if elapsed_ms > 0.0 => Some(cpu_ms / elapsed_ms * 100.0),
        _ => None,
    };
    let weight_type = samples
        .and_then(|samples| samples.get("weightType"))
        .and_then(Value::as_str)
        .map(|value| value.to_string());

    ThreadStats {
        sample_count,
        weight_sum,
        time_sum,
        weight_type,
        cpu_percent,
        cpu_ms,
        elapsed_ms,
    }
}

fn thread_elapsed_ms(thread: &Value) -> Option<f64> {
    thread_time_range_ms(thread, "registerTime", "unregisterTime")
        .or_else(|| thread_time_range_ms(thread, "processStartupTime", "processShutdownTime"))
}

fn thread_time_range_ms(thread: &Value, start_key: &str, end_key: &str) -> Option<f64> {
    let start = thread.get(start_key).and_then(Value::as_f64)?;
    let end = thread.get(end_key).and_then(Value::as_f64)?;
    let delta = end - start;
    if delta.is_sign_positive() {
        Some(delta)
    } else {
        None
    }
}

fn classify_idle_samples(
    samples: &mut [Sample],
    tables: &ThreadTables<'_>,
    cache: &FunctionCache,
    classifier: Option<&IdleClassifier>,
) -> IdleStats {
    let total_weight: f64 = samples.iter().map(|sample| sample.weight).sum();
    let Some(classifier) = classifier else {
        return IdleStats {
            enabled: false,
            total_weight,
            idle_weight: 0.0,
            active_weight: total_weight,
        };
    };

    let mut func_is_idle = vec![false; cache.infos.len()];
    for (index, info) in cache.infos.iter().enumerate() {
        func_is_idle[index] = classifier.is_idle(info);
    }
    let frame_to_func: Vec<Option<usize>> = tables
        .frame_func
        .iter()
        .map(|value| value.as_u64().map(|value| value as usize))
        .collect();

    let mut idle_weight = 0.0;
    for sample in samples.iter_mut() {
        let Some(stack_index) = sample.stack else {
            continue;
        };
        let mut is_idle = false;
        walk_stack(tables, stack_index, |frame_index| {
            if is_idle {
                return;
            }
            let func_index = frame_to_func.get(frame_index).and_then(|value| *value);
            if let Some(func_index) = func_index {
                if func_is_idle.get(func_index).copied().unwrap_or(false) {
                    is_idle = true;
                }
            }
        });
        if is_idle {
            sample.is_idle = true;
            idle_weight += sample.weight;
        }
    }

    let active_weight = (total_weight - idle_weight).max(0.0);
    IdleStats {
        enabled: true,
        total_weight,
        idle_weight,
        active_weight,
    }
}

fn resolve_global_metric(request: Metric, stats: &[ThreadStats]) -> MetricKind {
    match request {
        Metric::Samples => MetricKind::Samples,
        Metric::Weight => MetricKind::Weight,
        Metric::Time => MetricKind::Time,
        Metric::Auto => {
            if stats.iter().any(|stat| stat.weight_sum.is_some()) {
                MetricKind::Weight
            } else if stats.iter().any(|stat| stat.time_sum.is_some()) {
                MetricKind::Time
            } else {
                MetricKind::Samples
            }
        }
    }
}

fn resolve_metric_info(request: Metric, thread: &Value, units: &SampleUnits) -> MetricInfo {
    let samples = thread.get("samples");
    let has_weight = samples.and_then(extract_weight_values).is_some();
    let has_time = samples
        .and_then(|samples| extract_time_values(samples, units))
        .is_some();
    let weight_type = samples
        .and_then(|samples| samples.get("weightType"))
        .and_then(Value::as_str)
        .map(|value| value.to_string());

    let mut notes = Vec::new();
    if request == Metric::Auto {
        notes.push("auto".to_string());
    }

    let kind = match request {
        Metric::Samples => MetricKind::Samples,
        Metric::Weight => {
            if has_weight {
                MetricKind::Weight
            } else {
                notes.push("weight missing, using samples".to_string());
                MetricKind::Samples
            }
        }
        Metric::Time => {
            if has_time {
                MetricKind::Time
            } else {
                notes.push("time missing, using samples".to_string());
                MetricKind::Samples
            }
        }
        Metric::Auto => {
            if has_weight {
                MetricKind::Weight
            } else if has_time {
                MetricKind::Time
            } else {
                MetricKind::Samples
            }
        }
    };

    if kind == MetricKind::Weight {
        if let Some(weight_type) = weight_type.as_deref() {
            notes.push(format!("weightType={}", weight_type));
        }
    }
    if kind == MetricKind::Time {
        if let Some(label) = units.time_label.as_deref() {
            if units.time_scale_ms.is_some() && label != "ms" {
                notes.push(format!("timeUnit={} -> ms", label));
            } else if units.time_scale_ms.is_none() {
                notes.push(format!("timeUnit={}", label));
            }
        }
    }

    MetricInfo {
        kind,
        label: metric_label(kind).to_string(),
        column_label: metric_column_label(kind, units),
        note: if notes.is_empty() {
            None
        } else {
            Some(notes.join(", "))
        },
    }
}

fn metric_label(kind: MetricKind) -> &'static str {
    match kind {
        MetricKind::Samples => "samples",
        MetricKind::Weight => "weight",
        MetricKind::Time => "time",
    }
}

fn metric_column_label(kind: MetricKind, units: &SampleUnits) -> String {
    match kind {
        MetricKind::Samples => "samples".to_string(),
        MetricKind::Weight => "weight".to_string(),
        MetricKind::Time => {
            if units.time_scale_ms.is_some() {
                "time_ms".to_string()
            } else if let Some(label) = units.time_label.as_deref() {
                format!("time_{}", label)
            } else {
                "time".to_string()
            }
        }
    }
}

fn metric_value(stats: &ThreadStats, metric: MetricKind) -> f64 {
    match metric {
        MetricKind::Samples => stats.sample_count as f64,
        MetricKind::Weight => stats.weight_sum.unwrap_or(stats.sample_count as f64),
        MetricKind::Time => stats.time_sum.unwrap_or(stats.sample_count as f64),
    }
}

fn sample_units(profile: &Value) -> SampleUnits {
    let units = profile.get("meta").and_then(|meta| meta.get("sampleUnits"));
    let time_label = units
        .and_then(|units| units.get("time"))
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    SampleUnits {
        time_scale_ms: time_label.as_deref().and_then(unit_scale_ms),
        time_label,
        cpu_scale_ms: units
            .and_then(|units| units.get("threadCPUDelta"))
            .and_then(Value::as_str)
            .and_then(unit_scale_ms),
    }
}

fn unit_scale_ms(unit: &str) -> Option<f64> {
    match unit {
        "ms" | "millisecond" | "milliseconds" => Some(1.0),
        "us" | "µs" | "microsecond" | "microseconds" => Some(0.001),
        "ns" | "nanosecond" | "nanoseconds" => Some(0.000001),
        "s" | "sec" | "second" | "seconds" => Some(1000.0),
        _ => None,
    }
}

fn select_thread<'a>(
    threads: &'a [Value],
    selector: Option<ThreadSelector>,
    metric: Metric,
    stats: &[ThreadStats],
) -> Result<(usize, &'a Value)> {
    if threads.is_empty() {
        bail!("profile has no threads");
    }

    match selector {
        None => {
            let metric_kind = resolve_global_metric(metric, stats);
            let mut best_index = 0usize;
            let mut best_value = metric_value(&stats[0], metric_kind);
            for (index, stat) in stats.iter().enumerate().skip(1) {
                let value = metric_value(stat, metric_kind);
                if value > best_value {
                    best_value = value;
                    best_index = index;
                }
            }
            Ok((best_index, &threads[best_index]))
        }
        Some(ThreadSelector::Index(index)) => threads
            .get(index)
            .map(|thread| (index, thread))
            .ok_or_else(|| anyhow!("thread index {} out of range", index)),
        Some(ThreadSelector::Name(name)) => {
            let needle = name.to_ascii_lowercase();
            let found = threads.iter().enumerate().find(|(_, thread)| {
                let thread_name = thread.get("name").and_then(Value::as_str).unwrap_or("");
                let process_name = thread
                    .get("processName")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                thread_name.to_ascii_lowercase().contains(&needle)
                    || process_name.to_ascii_lowercase().contains(&needle)
            });
            found.ok_or_else(|| anyhow!("no thread matching '{}'", name))
        }
    }
}

struct ReportArgs<'a> {
    entries_inclusive: &'a [(FunctionInfo, f64)],
    entries_leaf: &'a [(FunctionInfo, f64)],
    metric_info: &'a MetricInfo,
    global_metric: MetricKind,
    thread_summary: Option<&'a [ThreadSummary]>,
    units: &'a SampleUnits,
    idle_stats: &'a IdleStats,
    top: usize,
}

fn print_report(args: ReportArgs<'_>) -> Result<()> {
    let ReportArgs {
        entries_inclusive,
        entries_leaf,
        metric_info,
        global_metric,
        thread_summary,
        units,
        idle_stats,
        top,
    } = args;

    let total_samples = idle_stats.total_weight;
    let active_total = if idle_stats.enabled {
        idle_stats.active_weight
    } else {
        total_samples
    };

    println!("report:");
    if let Some(summary) = thread_summary {
        print_thread_summary(summary, global_metric, units, "hot threads")?;
    }

    println!(
        "  total {} (samples): {}",
        metric_info.label,
        format_metric_value(total_samples, metric_info.kind)
    );
    if idle_stats.enabled {
        let idle_percent = if total_samples > 0.0 {
            idle_stats.idle_weight / total_samples * 100.0
        } else {
            0.0
        };
        let active_percent = if total_samples > 0.0 {
            active_total / total_samples * 100.0
        } else {
            0.0
        };
        println!(
            "  idle (leaf match): {} ({:.1}%), active: {} ({:.1}%)",
            format_metric_value(idle_stats.idle_weight, metric_info.kind),
            idle_percent,
            format_metric_value(active_total, metric_info.kind),
            active_percent
        );
        println!("  percent columns are of active samples");
    } else {
        println!("  idle filtering disabled; percent columns are of total samples");
    }

    print_hotspots(
        "hot paths (inclusive)",
        entries_inclusive,
        active_total,
        metric_info,
        top,
    );
    print_hotspots(
        "hot spots (leaf)",
        entries_leaf,
        active_total,
        metric_info,
        top,
    );
    print_module_summary(entries_leaf, active_total, metric_info, top);
    println!();
    Ok(())
}

fn collect_thread_summary(
    threads: &[Value],
    stats: &[ThreadStats],
    metric: MetricKind,
    top: usize,
) -> Vec<ThreadSummary> {
    let mut indices: Vec<usize> = (0..threads.len()).collect();
    indices.sort_by(|a, b| {
        metric_value(&stats[*b], metric)
            .partial_cmp(&metric_value(&stats[*a], metric))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    indices
        .into_iter()
        .take(top)
        .map(|index| {
            let thread = &threads[index];
            let name = thread
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("<unnamed>")
                .to_string();
            let process_type = thread
                .get("processType")
                .and_then(Value::as_str)
                .unwrap_or("-")
                .to_string();
            let pid = thread
                .get("pid")
                .and_then(Value::as_str)
                .unwrap_or("-")
                .to_string();
            ThreadSummary {
                index,
                name,
                process_type,
                pid,
                metric_value: metric_value(&stats[index], metric),
                sample_count: stats[index].sample_count,
                cpu_percent: stats[index].cpu_percent,
            }
        })
        .collect()
}

fn print_thread_summary(
    summary: &[ThreadSummary],
    metric: MetricKind,
    units: &SampleUnits,
    title: &str,
) -> Result<()> {
    let metric_label = metric_column_label(metric, units);
    println!("  {} (top {} by {}):", title, summary.len(), metric_label);
    for entry in summary {
        let cpu = entry
            .cpu_percent
            .map(|value| format!("cpu={:.1}%", value))
            .unwrap_or_else(|| "cpu=?".to_string());
        let metric_value = format_metric_value(entry.metric_value, metric);
        println!(
            "    [{}] {} ({}={}, samples={}, {}, process={}, pid={})",
            entry.index,
            entry.name,
            metric_label,
            metric_value,
            entry.sample_count,
            cpu,
            entry.process_type,
            entry.pid
        );
    }
    Ok(())
}

fn print_hotspots(
    label: &str,
    entries: &[(FunctionInfo, f64)],
    total: f64,
    metric_info: &MetricInfo,
    top: usize,
) {
    println!("  {}:", label);
    println!(
        "  {:>4} {:>12} {:>7} function",
        "rank", metric_info.column_label, "%"
    );
    for (rank, (info, count)) in entries.iter().take(top).enumerate() {
        let percent = if total > 0.0 {
            count / total * 100.0
        } else {
            0.0
        };
        let formatted = format_metric_value(*count, metric_info.kind);
        println!(
            "  {:>4} {:>12} {:>6.1}% {}",
            rank + 1,
            formatted,
            percent,
            info.label
        );
    }
}

fn print_module_summary(
    entries: &[(FunctionInfo, f64)],
    total: f64,
    metric_info: &MetricInfo,
    top: usize,
) {
    let mut module_counts: HashMap<String, f64> = HashMap::new();
    let mut module_top: HashMap<String, (String, f64)> = HashMap::new();

    for (info, count) in entries {
        let key = module_key(info);
        *module_counts.entry(key.clone()).or_insert(0.0) += *count;
        let entry = module_top
            .entry(key)
            .or_insert_with(|| (info.label.clone(), *count));
        if *count > entry.1 {
            *entry = (info.label.clone(), *count);
        }
    }

    let mut modules: Vec<(String, f64)> = module_counts.into_iter().collect();
    modules.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    println!("  top modules (leaf):");
    println!(
        "  {:>4} {:>12} {:>7} module",
        "rank", metric_info.column_label, "%"
    );
    for (rank, (module, value)) in modules.into_iter().take(top).enumerate() {
        let percent = if total > 0.0 {
            value / total * 100.0
        } else {
            0.0
        };
        let formatted = format_metric_value(value, metric_info.kind);
        let top_fn = module_top
            .get(&module)
            .map(|(label, _)| label.as_str())
            .unwrap_or("-");
        println!(
            "  {:>4} {:>12} {:>6.1}% {} | {}",
            rank + 1,
            formatted,
            percent,
            module,
            top_fn
        );
    }
}

fn print_threads(threads: &[Value], stats: &[ThreadStats], units: &SampleUnits) -> Result<()> {
    println!("threads:");
    for (index, thread) in threads.iter().enumerate() {
        let name = thread
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("<unnamed>");
        let process_type = thread
            .get("processType")
            .and_then(Value::as_str)
            .unwrap_or("-");
        let pid = thread.get("pid").and_then(Value::as_str).unwrap_or("-");
        let stat = &stats[index];
        let mut metrics = vec![format!("samples={}", stat.sample_count)];
        if let Some(weight_sum) = stat.weight_sum {
            let weight_label = stat
                .weight_type
                .as_deref()
                .map(|ty| format!("weight({})", ty))
                .unwrap_or_else(|| "weight".to_string());
            metrics.push(format!(
                "{}={}",
                weight_label,
                format_metric_value(weight_sum, MetricKind::Weight)
            ));
        }
        if let Some(time_sum) = stat.time_sum {
            metrics.push(format!(
                "{}={}",
                metric_column_label(MetricKind::Time, units),
                format_metric_value(time_sum, MetricKind::Time)
            ));
        }
        if let Some(cpu_percent) = stat.cpu_percent {
            metrics.push(format!("cpu={:.1}%", cpu_percent));
        }
        println!(
            "  [{}] {} ({}, process={}, pid={})",
            index,
            name,
            metrics.join(", "),
            process_type,
            pid
        );
    }
    Ok(())
}

fn sample_count(samples: &Value) -> usize {
    if let Some(stack) = samples.get("stack").and_then(Value::as_array) {
        return stack.len();
    }
    if let Some(data) = samples.get("data").and_then(Value::as_array) {
        return data.len();
    }
    if let Some(count) = samples.get("length").and_then(Value::as_u64) {
        return count as usize;
    }
    0
}

fn extract_samples(
    thread: &Value,
    metric_info: &MetricInfo,
    units: &SampleUnits,
) -> Result<Vec<Sample>> {
    let samples = thread
        .get("samples")
        .ok_or_else(|| anyhow!("thread missing samples"))?;
    let stacks = extract_sample_stacks(thread)?;
    let len = stacks.len();

    let mut weights = match metric_info.kind {
        MetricKind::Samples => vec![1.0; len],
        MetricKind::Weight => extract_weight_values(samples).unwrap_or_else(|| vec![1.0; len]),
        MetricKind::Time => extract_time_values(samples, units).unwrap_or_else(|| vec![1.0; len]),
    };
    weights = align_weights(weights, len);

    let samples = stacks
        .into_iter()
        .zip(weights)
        .map(|(stack, weight)| Sample {
            stack,
            weight,
            is_idle: false,
        })
        .collect();
    Ok(samples)
}

fn extract_sample_stacks(thread: &Value) -> Result<Vec<Option<usize>>> {
    let samples = thread
        .get("samples")
        .ok_or_else(|| anyhow!("thread missing samples"))?;

    if let Some(stack) = samples.get("stack").and_then(Value::as_array) {
        return Ok(stack.iter().map(to_opt_index).collect());
    }

    let schema = samples
        .get("schema")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("samples missing stack and schema"))?;
    let stack_index = schema
        .get("stack")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("samples schema missing stack column"))?
        as usize;
    let data = samples
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("samples missing data"))?;

    let stacks = data
        .iter()
        .map(|row| {
            row.as_array()
                .and_then(|row| row.get(stack_index))
                .and_then(to_opt_index)
        })
        .collect();
    Ok(stacks)
}

fn extract_weight_values(samples: &Value) -> Option<Vec<f64>> {
    extract_sample_array(samples, "weight").or_else(|| extract_data_column(samples, "weight"))
}

fn extract_cpu_values(samples: &Value) -> Option<Vec<f64>> {
    extract_sample_array(samples, "threadCPUDelta")
        .or_else(|| extract_data_column(samples, "threadCPUDelta"))
}

fn extract_time_values(samples: &Value, units: &SampleUnits) -> Option<Vec<f64>> {
    let mut values = if let Some(values) = extract_sample_array(samples, "timeDeltas") {
        values
    } else if let Some(values) = extract_data_column(samples, "timeDeltas") {
        values
    } else {
        extract_data_column(samples, "time").map(|times| time_deltas_from_times(&times))?
    };

    if let Some(scale) = units.time_scale_ms {
        for value in &mut values {
            *value *= scale;
        }
    }
    Some(values)
}

fn extract_sample_array(samples: &Value, key: &str) -> Option<Vec<f64>> {
    samples.get(key).and_then(Value::as_array).map(|arr| {
        arr.iter()
            .map(|value| value_to_f64(value).unwrap_or(0.0))
            .collect()
    })
}

fn extract_data_column(samples: &Value, column: &str) -> Option<Vec<f64>> {
    let schema = samples.get("schema").and_then(Value::as_object)?;
    let index = schema.get(column).and_then(Value::as_u64)? as usize;
    let data = samples.get("data").and_then(Value::as_array)?;
    let mut values = Vec::with_capacity(data.len());
    for row in data {
        let value = row
            .as_array()
            .and_then(|row| row.get(index))
            .and_then(value_to_f64)
            .unwrap_or(0.0);
        values.push(value);
    }
    Some(values)
}

fn time_deltas_from_times(times: &[f64]) -> Vec<f64> {
    if times.is_empty() {
        return Vec::new();
    }
    let mut deltas = Vec::with_capacity(times.len());
    let mut prev = times[0];
    deltas.push(0.0);
    for &time in times.iter().skip(1) {
        let delta = time - prev;
        deltas.push(if delta >= 0.0 { delta } else { 0.0 });
        prev = time;
    }
    deltas
}

fn value_to_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(value) => value.parse::<f64>().ok(),
        _ => None,
    }
}

fn align_weights(mut weights: Vec<f64>, len: usize) -> Vec<f64> {
    if weights.len() > len {
        weights.truncate(len);
    } else if weights.len() < len {
        weights.resize(len, 0.0);
    }
    weights
}

fn to_opt_index(value: &Value) -> Option<usize> {
    match value {
        Value::Number(number) => number.as_u64().map(|n| n as usize),
        Value::Null => None,
        _ => None,
    }
}

struct ThreadTables<'a> {
    strings: &'a [Value],
    frame_func: &'a [Value],
    func_name: &'a [Value],
    func_resource: Option<&'a [Value]>,
    resource_name: Option<&'a [Value]>,
    stack_prefix: &'a [Value],
    stack_frame: &'a [Value],
}

impl<'a> ThreadTables<'a> {
    fn from_profile_thread(profile: &'a Value, thread: &'a Value) -> Result<Self> {
        let strings = array_at_opt(thread, &["stringArray"])
            .or_else(|| array_at_opt(profile, &["shared", "stringArray"]))
            .ok_or_else(|| anyhow!("missing stringArray array (thread or shared)"))?;
        let frame_func = array_at(thread, &["frameTable", "func"])?;
        let func_name = array_at(thread, &["funcTable", "name"])?;
        let func_resource = array_at_opt(thread, &["funcTable", "resource"]).map(Vec::as_slice);
        let resource_name = array_at_opt(thread, &["resourceTable", "name"]).map(Vec::as_slice);
        let stack_prefix = array_at(thread, &["stackTable", "prefix"])?;
        let stack_frame = array_at(thread, &["stackTable", "frame"])?;

        Ok(Self {
            strings,
            frame_func,
            func_name,
            func_resource,
            resource_name,
            stack_prefix,
            stack_frame,
        })
    }
}

fn array_at<'a>(value: &'a Value, path: &[&str]) -> Result<&'a Vec<Value>> {
    array_at_opt(value, path).ok_or_else(|| anyhow!("missing {} array", path.join(".")))
}

fn array_at_opt<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Vec<Value>> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_array()
}

fn format_metric_value(value: f64, kind: MetricKind) -> String {
    match kind {
        MetricKind::Samples => format!("{:.0}", value),
        MetricKind::Weight => {
            if (value.fract()).abs() < 0.01 {
                format!("{:.0}", value)
            } else {
                format!("{:.2}", value)
            }
        }
        MetricKind::Time => format!("{:.2}", value),
    }
}

fn module_key(info: &FunctionInfo) -> String {
    if info.name.contains("::") {
        let mut parts = info.name.split("::");
        let first = parts.next().unwrap_or(&info.name);
        if let Some(second) = parts.next() {
            return format!("{}::{}", first, second);
        }
        return first.to_string();
    }
    if let Some(resource) = info.resource.as_deref() {
        return resource.to_string();
    }
    info.name.clone()
}

fn build_entries(cache: &FunctionCache, counts: HashMap<usize, f64>) -> Vec<(FunctionInfo, f64)> {
    counts
        .into_iter()
        .filter_map(|(func, count)| cache.get(func).cloned().map(|info| (info, count)))
        .collect()
}

fn sort_entries(entries: &mut [(FunctionInfo, f64)]) {
    entries.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.label.cmp(&b.0.label))
    });
}

fn count_functions(
    tables: &ThreadTables<'_>,
    samples: &[Sample],
    mode: CountMode,
    filter: SampleFilter,
) -> HashMap<usize, f64> {
    let frame_to_func: Vec<Option<usize>> = tables
        .frame_func
        .iter()
        .map(|value| value.as_u64().map(|value| value as usize))
        .collect();

    let mut counts: HashMap<usize, f64> = HashMap::new();

    for sample in samples {
        if filter == SampleFilter::Active && sample.is_idle {
            continue;
        }
        let Some(stack_index) = sample.stack else {
            continue;
        };
        if stack_index >= tables.stack_frame.len() {
            continue;
        }
        match mode {
            CountMode::Leaf => {
                if let Some(frame_index) = tables
                    .stack_frame
                    .get(stack_index)
                    .and_then(|value| value.as_u64())
                    .map(|value| value as usize)
                {
                    if let Some(func_index) = frame_to_func.get(frame_index).and_then(|v| *v) {
                        *counts.entry(func_index).or_insert(0.0) += sample.weight;
                    }
                }
            }
            CountMode::Inclusive => {
                walk_stack(tables, stack_index, |frame_index| {
                    if let Some(func_index) = frame_to_func.get(frame_index).and_then(|v| *v) {
                        *counts.entry(func_index).or_insert(0.0) += sample.weight;
                    }
                });
            }
        }
    }

    counts
}

fn walk_stack(tables: &ThreadTables<'_>, start: usize, mut f: impl FnMut(usize)) {
    let mut current = Some(start);
    let mut depth = 0usize;
    while let Some(stack_index) = current {
        if stack_index >= tables.stack_frame.len() {
            break;
        }
        let frame_index = tables
            .stack_frame
            .get(stack_index)
            .and_then(Value::as_u64)
            .map(|value| value as usize);
        let Some(frame_index) = frame_index else {
            break;
        };
        f(frame_index);

        current = tables.stack_prefix.get(stack_index).and_then(to_opt_index);
        depth += 1;
        if depth > 8192 {
            break;
        }
    }
}

fn func_info(tables: &ThreadTables<'_>, func_index: usize) -> FunctionInfo {
    let name_index = tables
        .func_name
        .get(func_index)
        .and_then(Value::as_u64)
        .map(|value| value as usize);
    let name = name_index
        .and_then(|idx| tables.strings.get(idx).and_then(Value::as_str))
        .unwrap_or("<unknown>")
        .to_string();

    let resource = tables
        .func_resource
        .and_then(|resources| resources.get(func_index))
        .and_then(Value::as_u64)
        .and_then(|resource_index| {
            tables
                .resource_name
                .and_then(|names| names.get(resource_index as usize))
                .and_then(Value::as_u64)
        })
        .and_then(|string_index| tables.strings.get(string_index as usize))
        .and_then(Value::as_str)
        .map(|value| value.to_string());

    let label = match resource.as_deref() {
        Some(resource) if !resource.is_empty() => format!("{} ({})", name, resource),
        _ => name.clone(),
    };

    FunctionInfo {
        name,
        resource,
        label,
    }
}
