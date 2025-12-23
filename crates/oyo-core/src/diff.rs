//! Diff computation engine

use crate::change::{Change, ChangeKind, ChangeSpan};
use similar::{ChangeTag, TextDiff};
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DiffError {
    #[error("Failed to read file: {0}")]
    FileRead(#[from] std::io::Error),
    #[error("Diff computation failed: {0}")]
    ComputeFailed(String),
}

/// A hunk is a group of related changes that are close together
#[derive(Debug, Clone)]
pub struct Hunk {
    /// Unique ID for this hunk
    pub id: usize,
    /// IDs of changes in this hunk (in order)
    pub change_ids: Vec<usize>,
    /// Starting line number in old file
    pub old_start: Option<usize>,
    /// Starting line number in new file
    pub new_start: Option<usize>,
    /// Number of insertions in this hunk
    pub insertions: usize,
    /// Number of deletions in this hunk
    pub deletions: usize,
}

impl Hunk {
    /// Get the number of changes in this hunk
    pub fn len(&self) -> usize {
        self.change_ids.len()
    }

    /// Check if hunk is empty
    pub fn is_empty(&self) -> bool {
        self.change_ids.is_empty()
    }
}

/// Result of a diff operation
#[derive(Debug, Clone)]
pub struct DiffResult {
    /// All changes in order
    pub changes: Vec<Change>,
    /// Only the actual changes (excluding context)
    pub significant_changes: Vec<usize>,
    /// Hunks (groups of related changes)
    pub hunks: Vec<Hunk>,
    /// Total number of insertions
    pub insertions: usize,
    /// Total number of deletions
    pub deletions: usize,
}

impl DiffResult {
    /// Get only the significant (non-context) changes
    pub fn get_significant_changes(&self) -> Vec<&Change> {
        self.significant_changes
            .iter()
            .filter_map(|&id| self.changes.iter().find(|c| c.id == id))
            .collect()
    }

    /// Get a hunk by ID
    pub fn get_hunk(&self, hunk_id: usize) -> Option<&Hunk> {
        self.hunks.iter().find(|h| h.id == hunk_id)
    }

    /// Find which hunk a change belongs to
    pub fn hunk_for_change(&self, change_id: usize) -> Option<&Hunk> {
        self.hunks
            .iter()
            .find(|h| h.change_ids.contains(&change_id))
    }
}

/// A diff for a single file
#[derive(Debug, Clone)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub result: DiffResult,
}

/// The main diff engine
pub struct DiffEngine {
    /// Number of context lines to include
    context_lines: usize,
    /// Whether to do word-level diffing within changed lines
    word_level: bool,
}

impl Default for DiffEngine {
    fn default() -> Self {
        Self {
            context_lines: 3,
            word_level: true,
        }
    }
}

impl DiffEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_context(mut self, lines: usize) -> Self {
        self.context_lines = lines;
        self
    }

    pub fn with_word_level(mut self, enabled: bool) -> Self {
        self.word_level = enabled;
        self
    }

    /// Compute diff between two strings
    pub fn diff_strings(&self, old: &str, new: &str) -> DiffResult {
        let text_diff = TextDiff::from_lines(old, new);
        let mut changes = Vec::new();
        let mut significant_changes = Vec::new();
        let mut insertions = 0;
        let mut deletions = 0;
        let mut change_id = 0;

        let mut old_line_num = 1usize;
        let mut new_line_num = 1usize;

        // Group consecutive changes together for word-level diffing
        let mut pending_deletes: Vec<(String, usize)> = Vec::new();
        let mut pending_inserts: Vec<(String, usize)> = Vec::new();

        let ops: Vec<_> = text_diff.iter_all_changes().collect();

        for change in ops.iter() {
            match change.tag() {
                ChangeTag::Equal => {
                    // Flush any pending changes before processing equal
                    self.flush_pending_changes(
                        &mut pending_deletes,
                        &mut pending_inserts,
                        &mut changes,
                        &mut significant_changes,
                        &mut change_id,
                        &mut insertions,
                        &mut deletions,
                    );

                    let span = ChangeSpan::equal(change.value().trim_end_matches('\n'))
                        .with_lines(Some(old_line_num), Some(new_line_num));
                    changes.push(Change::single(change_id, span));
                    change_id += 1;
                    old_line_num += 1;
                    new_line_num += 1;
                }
                ChangeTag::Delete => {
                    pending_deletes.push((
                        change.value().trim_end_matches('\n').to_string(),
                        old_line_num,
                    ));
                    old_line_num += 1;
                }
                ChangeTag::Insert => {
                    pending_inserts.push((
                        change.value().trim_end_matches('\n').to_string(),
                        new_line_num,
                    ));
                    new_line_num += 1;
                }
            }
        }

        // Flush remaining changes
        self.flush_pending_changes(
            &mut pending_deletes,
            &mut pending_inserts,
            &mut changes,
            &mut significant_changes,
            &mut change_id,
            &mut insertions,
            &mut deletions,
        );

        // Compute hunks by grouping nearby changes
        let hunks = Self::compute_hunks(&significant_changes, &changes);

        DiffResult {
            changes,
            significant_changes,
            hunks,
            insertions,
            deletions,
        }
    }

    /// Compute hunks by grouping consecutive changes that are close together
    /// Changes within PROXIMITY_THRESHOLD lines are grouped into the same hunk
    fn compute_hunks(significant_changes: &[usize], changes: &[Change]) -> Vec<Hunk> {
        const PROXIMITY_THRESHOLD: usize = 3;

        let mut hunks = Vec::new();
        if significant_changes.is_empty() {
            return hunks;
        }

        let mut current_hunk_changes: Vec<usize> = Vec::new();
        let mut current_hunk_old_start: Option<usize> = None;
        let mut current_hunk_new_start: Option<usize> = None;
        let mut last_old_line: Option<usize> = None;
        let mut last_new_line: Option<usize> = None;
        let mut current_insertions = 0;
        let mut current_deletions = 0;
        let mut hunk_id = 0;

        for &change_id in significant_changes {
            let change = match changes.iter().find(|c| c.id == change_id) {
                Some(c) => c,
                None => continue,
            };

            // Get line numbers from first span
            let (old_line, new_line) = change
                .spans
                .first()
                .map(|s| (s.old_line, s.new_line))
                .unwrap_or((None, None));

            // Determine if this change is close to the previous one
            let is_close = match (last_old_line, last_new_line, old_line, new_line) {
                (Some(lo), _, Some(co), _) => co.saturating_sub(lo) <= PROXIMITY_THRESHOLD,
                (_, Some(ln), _, Some(cn)) => cn.saturating_sub(ln) <= PROXIMITY_THRESHOLD,
                _ => current_hunk_changes.is_empty(), // First change always starts a hunk
            };

            if is_close {
                // Add to current hunk
                current_hunk_changes.push(change_id);
                if current_hunk_old_start.is_none() {
                    current_hunk_old_start = old_line;
                }
                if current_hunk_new_start.is_none() {
                    current_hunk_new_start = new_line;
                }
            } else {
                // Save current hunk and start a new one
                if !current_hunk_changes.is_empty() {
                    hunks.push(Hunk {
                        id: hunk_id,
                        change_ids: current_hunk_changes.clone(),
                        old_start: current_hunk_old_start,
                        new_start: current_hunk_new_start,
                        insertions: current_insertions,
                        deletions: current_deletions,
                    });
                    hunk_id += 1;
                }

                // Start new hunk
                current_hunk_changes = vec![change_id];
                current_hunk_old_start = old_line;
                current_hunk_new_start = new_line;
                current_insertions = 0;
                current_deletions = 0;
            }

            // Update last line numbers
            if old_line.is_some() {
                last_old_line = old_line;
            }
            if new_line.is_some() {
                last_new_line = new_line;
            }

            // Count insertions/deletions in this change
            for span in &change.spans {
                match span.kind {
                    ChangeKind::Insert => current_insertions += 1,
                    ChangeKind::Delete => current_deletions += 1,
                    ChangeKind::Replace => {
                        current_insertions += 1;
                        current_deletions += 1;
                    }
                    ChangeKind::Equal => {}
                }
            }
        }

        // Don't forget the last hunk
        if !current_hunk_changes.is_empty() {
            hunks.push(Hunk {
                id: hunk_id,
                change_ids: current_hunk_changes,
                old_start: current_hunk_old_start,
                new_start: current_hunk_new_start,
                insertions: current_insertions,
                deletions: current_deletions,
            });
        }

        hunks
    }

    #[allow(clippy::too_many_arguments)]
    fn flush_pending_changes(
        &self,
        pending_deletes: &mut Vec<(String, usize)>,
        pending_inserts: &mut Vec<(String, usize)>,
        changes: &mut Vec<Change>,
        significant_changes: &mut Vec<usize>,
        change_id: &mut usize,
        insertions: &mut usize,
        deletions: &mut usize,
    ) {
        if pending_deletes.is_empty() && pending_inserts.is_empty() {
            return;
        }

        // Try to match deletes with inserts for replace operations
        if self.word_level && pending_deletes.len() == pending_inserts.len() {
            for ((old_text, old_line), (new_text, new_line)) in
                pending_deletes.iter().zip(pending_inserts.iter())
            {
                let spans = self.compute_word_diff(old_text, new_text, *old_line, *new_line);
                let change = Change::new(*change_id, spans);
                significant_changes.push(*change_id);
                changes.push(change);
                *change_id += 1;
                *insertions += 1;
                *deletions += 1;
            }
        } else {
            // Output as separate deletes and inserts
            for (text, line) in pending_deletes.iter() {
                let span = ChangeSpan::delete(text.clone()).with_lines(Some(*line), None);
                significant_changes.push(*change_id);
                changes.push(Change::single(*change_id, span));
                *change_id += 1;
                *deletions += 1;
            }
            for (text, line) in pending_inserts.iter() {
                let span = ChangeSpan::insert(text.clone()).with_lines(None, Some(*line));
                significant_changes.push(*change_id);
                changes.push(Change::single(*change_id, span));
                *change_id += 1;
                *insertions += 1;
            }
        }

        pending_deletes.clear();
        pending_inserts.clear();
    }
}

/// Tokenize code for word-level diffing
/// Separates identifiers from punctuation for accurate diffs
fn tokenize_code(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut buf = String::new();
    let mut in_word = false;

    for ch in line.chars() {
        let is_word = ch.is_alphanumeric() || ch == '_';
        if is_word {
            if !in_word {
                if !buf.is_empty() {
                    tokens.push(std::mem::take(&mut buf));
                }
                in_word = true;
            }
            buf.push(ch);
        } else {
            if in_word {
                if !buf.is_empty() {
                    tokens.push(std::mem::take(&mut buf));
                }
                in_word = false;
            }
            if ch.is_whitespace() {
                // Group consecutive whitespace
                if !buf.is_empty() && !buf.chars().all(char::is_whitespace) {
                    tokens.push(std::mem::take(&mut buf));
                }
                buf.push(ch);
            } else {
                // Each punctuation char is its own token
                if !buf.is_empty() {
                    tokens.push(std::mem::take(&mut buf));
                }
                tokens.push(ch.to_string());
            }
        }
    }
    if !buf.is_empty() {
        tokens.push(buf);
    }
    tokens
}

impl DiffEngine {
    /// Compute word-level diff within a line
    fn compute_word_diff(
        &self,
        old: &str,
        new: &str,
        old_line: usize,
        new_line: usize,
    ) -> Vec<ChangeSpan> {
        let old_tokens = tokenize_code(old);
        let new_tokens = tokenize_code(new);
        let old_refs: Vec<&str> = old_tokens.iter().map(|s| s.as_str()).collect();
        let new_refs: Vec<&str> = new_tokens.iter().map(|s| s.as_str()).collect();
        let word_diff = TextDiff::from_slices(&old_refs, &new_refs);
        let mut spans = Vec::new();

        for change in word_diff.iter_all_changes() {
            let text = change.value().to_string();
            let span = match change.tag() {
                ChangeTag::Equal => ChangeSpan::equal(text),
                ChangeTag::Delete => ChangeSpan::delete(text),
                ChangeTag::Insert => ChangeSpan::insert(text),
            }
            .with_lines(Some(old_line), Some(new_line));
            spans.push(span);
        }

        spans
    }

    /// Compute diff between two files
    pub fn diff_files(&self, old_path: &Path, new_path: &Path) -> Result<FileDiff, DiffError> {
        let old_content = std::fs::read_to_string(old_path)?;
        let new_content = std::fs::read_to_string(new_path)?;

        let result = self.diff_strings(&old_content, &new_content);

        Ok(FileDiff {
            old_path: Some(old_path.to_string_lossy().to_string()),
            new_path: Some(new_path.to_string_lossy().to_string()),
            result,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_diff() {
        let engine = DiffEngine::new();
        let old = "foo\nbar\nbaz";
        let new = "foo\nqux\nbaz";

        let result = engine.diff_strings(old, new);

        assert_eq!(result.insertions, 1);
        assert_eq!(result.deletions, 1);
        assert!(!result.significant_changes.is_empty());
    }

    #[test]
    fn test_no_changes() {
        let engine = DiffEngine::new();
        let text = "foo\nbar\nbaz";

        let result = engine.diff_strings(text, text);

        assert_eq!(result.insertions, 0);
        assert_eq!(result.deletions, 0);
        assert!(result.significant_changes.is_empty());
    }

    #[test]
    fn test_word_level_diff() {
        let engine = DiffEngine::new().with_word_level(true);
        let old = "const foo = 4";
        let new = "const bar = 4";

        let result = engine.diff_strings(old, new);

        // Should have a single change with word-level spans
        assert_eq!(result.significant_changes.len(), 1);
    }

    #[test]
    fn test_tokenize_code_basic() {
        let tokens = tokenize_code("KeyModifiers, MouseEventKind}");
        assert_eq!(
            tokens,
            vec!["KeyModifiers", ",", " ", "MouseEventKind", "}"]
        );
    }

    #[test]
    fn test_tokenize_code_identifiers() {
        let tokens = tokenize_code("foo_bar baz123");
        assert_eq!(tokens, vec!["foo_bar", " ", "baz123"]);
    }

    #[test]
    fn test_tokenize_code_punctuation() {
        let tokens = tokenize_code("use foo::{A, B};");
        assert_eq!(
            tokens,
            vec!["use", " ", "foo", ":", ":", "{", "A", ",", " ", "B", "}", ";"]
        );
    }

    #[test]
    fn test_word_diff_punctuation_separation() {
        use crate::change::ChangeKind;

        // This is the exact bug case: adding MouseEventKind to an import list
        let engine = DiffEngine::new().with_word_level(true);
        let old = "use foo::{KeyModifiers};";
        let new = "use foo::{KeyModifiers, MouseEventKind};";

        let result = engine.diff_strings(old, new);

        // Should have one change
        assert_eq!(result.significant_changes.len(), 1);

        let change = &result.changes[result.significant_changes[0]];

        // Find spans by kind
        let equal_content: String = change
            .spans
            .iter()
            .filter(|s| s.kind == ChangeKind::Equal)
            .map(|s| s.text.as_str())
            .collect();
        let insert_content: String = change
            .spans
            .iter()
            .filter(|s| s.kind == ChangeKind::Insert)
            .map(|s| s.text.as_str())
            .collect();

        // KeyModifiers should be in equal spans (unchanged)
        assert!(
            equal_content.contains("KeyModifiers"),
            "KeyModifiers should be equal, got equal: '{}', insert: '{}'",
            equal_content,
            insert_content
        );

        // MouseEventKind should be in insert spans (new)
        assert!(
            insert_content.contains("MouseEventKind"),
            "MouseEventKind should be inserted, got equal: '{}', insert: '{}'",
            equal_content,
            insert_content
        );

        // KeyModifiers should NOT be in insert spans
        assert!(
            !insert_content.contains("KeyModifiers"),
            "KeyModifiers should not be inserted, got insert: '{}'",
            insert_content
        );
    }
}
