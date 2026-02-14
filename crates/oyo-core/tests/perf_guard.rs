use oyo_core::diff::DiffEngine;
use oyo_core::step::DiffNavigator;
use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Inputs {
    old: Arc<str>,
    new: Arc<str>,
    diff: oyo_core::diff::DiffResult,
}

fn perf_tests_enabled() -> bool {
    std::env::var("OYO_PERF_TESTS")
        .ok()
        .filter(|value| value != "0")
        .is_some()
}

fn build_inputs(hunks: usize, changes_per_hunk: usize, context_lines: usize) -> Inputs {
    let (old, new) = make_text(hunks, changes_per_hunk, context_lines);
    let engine = DiffEngine::new().with_context(context_lines);
    let diff = engine.diff_strings(&old, &new);
    Inputs {
        old: Arc::from(old),
        new: Arc::from(new),
        diff,
    }
}

fn make_text(hunks: usize, changes_per_hunk: usize, context_lines: usize) -> (String, String) {
    let mut old = String::new();
    let mut new = String::new();
    let gap = context_lines + 2;
    for hunk in 0..hunks {
        for idx in 0..gap {
            let line = format!("ctx {hunk} {idx}\n");
            old.push_str(&line);
            new.push_str(&line);
        }
        for change in 0..changes_per_hunk {
            old.push_str(&format!("old {hunk} {change}\n"));
            new.push_str(&format!("new {hunk} {change}\n"));
        }
    }
    (old, new)
}

fn measure_is_applied(inputs: &Inputs) -> Duration {
    let mut nav = DiffNavigator::new(
        inputs.diff.clone(),
        inputs.old.clone(),
        inputs.new.clone(),
        false,
    );
    nav.goto_end();
    let sample_ids: Vec<usize> = inputs
        .diff
        .changes
        .iter()
        .take(1000.min(inputs.diff.changes.len()))
        .map(|change| change.id)
        .collect();
    let state = nav.state();
    let start = Instant::now();
    for _ in 0..50 {
        for id in sample_ids.iter() {
            black_box(state.is_applied(*id));
        }
    }
    start.elapsed()
}

fn measure_hunk_index_for_change_id(inputs: &Inputs) -> Duration {
    let nav = DiffNavigator::new(
        inputs.diff.clone(),
        inputs.old.clone(),
        inputs.new.clone(),
        false,
    );
    let sample_ids: Vec<usize> = inputs
        .diff
        .changes
        .iter()
        .take(1000.min(inputs.diff.changes.len()))
        .map(|change| change.id)
        .collect();
    let start = Instant::now();
    for _ in 0..50 {
        for id in sample_ids.iter() {
            black_box(nav.hunk_index_for_change_id(*id));
        }
    }
    start.elapsed()
}

#[test]
fn perf_is_applied_scaling() {
    if !perf_tests_enabled() {
        return;
    }
    let small = build_inputs(20, 50, 3);
    let large = build_inputs(200, 50, 3);
    let small_time = measure_is_applied(&small);
    let large_time = measure_is_applied(&large);
    assert!(
        large_time.as_nanos() <= small_time.as_nanos() * 20,
        "is_applied scaled poorly: small={:?} large={:?}",
        small_time,
        large_time
    );
}

#[test]
fn perf_hunk_index_for_change_id_scaling() {
    if !perf_tests_enabled() {
        return;
    }
    let small = build_inputs(20, 50, 3);
    let large = build_inputs(200, 50, 3);
    let small_time = measure_hunk_index_for_change_id(&small);
    let large_time = measure_hunk_index_for_change_id(&large);
    assert!(
        large_time.as_nanos() <= small_time.as_nanos() * 20,
        "hunk_index_for_change_id scaled poorly: small={:?} large={:?}",
        small_time,
        large_time
    );
}
