use criterion::{black_box, BatchSize, BenchmarkId, Criterion};
use criterion::{criterion_group, criterion_main};
use oyo_core::diff::DiffEngine;
use oyo_core::step::{AnimationFrame, DiffNavigator};
use std::sync::Arc;
use std::time::Duration;

struct BenchInputs {
    old: Arc<str>,
    new: Arc<str>,
    diff: oyo_core::diff::DiffResult,
}

fn build_inputs(hunks: usize, changes_per_hunk: usize, context_lines: usize) -> BenchInputs {
    let (old, new) = make_text(hunks, changes_per_hunk, context_lines);
    let engine = DiffEngine::new().with_context(context_lines);
    let diff = engine.diff_strings(&old, &new);
    BenchInputs {
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

fn bench_prev_hunk(c: &mut Criterion) {
    let inputs = build_inputs(100, 20, 3);
    c.bench_function("prev_hunk/100x20", |b| {
        b.iter_batched(
            || {
                let mut nav = DiffNavigator::new(
                    inputs.diff.clone(),
                    inputs.old.clone(),
                    inputs.new.clone(),
                    false,
                );
                nav.goto_end();
                nav
            },
            |mut nav| {
                black_box(nav.prev_hunk());
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_next_hunk(c: &mut Criterion) {
    let inputs = build_inputs(100, 20, 3);
    c.bench_function("next_hunk/100x20", |b| {
        b.iter_batched(
            || {
                DiffNavigator::new(
                    inputs.diff.clone(),
                    inputs.old.clone(),
                    inputs.new.clone(),
                    false,
                )
            },
            |mut nav| {
                black_box(nav.next_hunk());
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_view_for_changes(c: &mut Criterion) {
    let inputs = build_inputs(100, 20, 3);
    c.bench_function("view_for_changes/100x20", |b| {
        b.iter_batched(
            || {
                let mut nav = DiffNavigator::new(
                    inputs.diff.clone(),
                    inputs.old.clone(),
                    inputs.new.clone(),
                    false,
                );
                let mid = nav.state().total_steps / 2;
                nav.goto(mid);
                nav
            },
            |nav| {
                black_box(nav.current_view_with_frame(AnimationFrame::Idle));
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_view_for_changes_large_hunk(c: &mut Criterion) {
    let inputs = build_inputs(1, 5000, 3);
    let mut group = c.benchmark_group("view_for_changes_large_hunk");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);
    group.bench_function("1x5000", |b| {
        b.iter_batched(
            || {
                let mut nav = DiffNavigator::new(
                    inputs.diff.clone(),
                    inputs.old.clone(),
                    inputs.new.clone(),
                    false,
                );
                let mid = nav.state().total_steps / 2;
                nav.goto(mid);
                nav
            },
            |nav| {
                black_box(nav.current_view_with_frame(AnimationFrame::Idle));
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

fn bench_hunk_index_for_change_id(c: &mut Criterion) {
    let inputs = build_inputs(200, 10, 3);
    let change_ids: Vec<usize> = inputs.diff.changes.iter().map(|c| c.id).collect();
    c.bench_with_input(
        BenchmarkId::new("hunk_index_for_change_id", change_ids.len()),
        &change_ids,
        |b, ids| {
            b.iter_batched(
                || {
                    DiffNavigator::new(
                        inputs.diff.clone(),
                        inputs.old.clone(),
                        inputs.new.clone(),
                        false,
                    )
                },
                |nav| {
                    for id in ids.iter().take(1000) {
                        black_box(nav.hunk_index_for_change_id(*id));
                    }
                },
                BatchSize::SmallInput,
            );
        },
    );
}

fn bench_is_applied(c: &mut Criterion) {
    let inputs = build_inputs(200, 10, 3);
    let change_ids: Vec<usize> = inputs.diff.changes.iter().map(|c| c.id).collect();
    c.bench_function("is_applied/200x10", |b| {
        b.iter_batched(
            || {
                let mut nav = DiffNavigator::new(
                    inputs.diff.clone(),
                    inputs.old.clone(),
                    inputs.new.clone(),
                    false,
                );
                nav.goto_end();
                nav
            },
            |nav| {
                let state = nav.state();
                for id in change_ids.iter().take(1000) {
                    black_box(state.is_applied(*id));
                }
            },
            BatchSize::SmallInput,
        );
    });
}

criterion_group!(
    benches,
    bench_prev_hunk,
    bench_next_hunk,
    bench_view_for_changes,
    bench_view_for_changes_large_hunk,
    bench_hunk_index_for_change_id,
    bench_is_applied
);
criterion_main!(benches);
