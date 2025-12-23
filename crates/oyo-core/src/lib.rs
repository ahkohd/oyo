//! Oyo Core - Diff engine with step-through support
//!
//! This library provides data structures and algorithms for computing
//! and navigating through diffs in a step-by-step manner.

pub mod change;
pub mod diff;
pub mod git;
pub mod multi;
pub mod step;

pub use change::{Change, ChangeKind, ChangeSpan};
pub use diff::{DiffEngine, DiffResult, FileDiff, Hunk};
pub use git::{ChangedFile, FileStatus};
pub use multi::{FileEntry, MultiFileDiff};
pub use step::{
    AnimationFrame, DiffNavigator, LineKind, StepDirection, StepState, ViewLine, ViewSpan,
    ViewSpanKind,
};
