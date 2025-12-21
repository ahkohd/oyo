//! Change representation for diff operations

use serde::{Deserialize, Serialize};

/// The kind of change in a diff
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeKind {
    /// Content was added
    Insert,
    /// Content was removed
    Delete,
    /// Content was modified (for word-level changes within a line)
    Replace,
    /// Content is unchanged (context)
    Equal,
}

/// A span of text that represents a change
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSpan {
    /// The kind of change
    pub kind: ChangeKind,
    /// The text content (old content for Delete/Replace, new for Insert)
    pub text: String,
    /// For Replace: the new text that replaces the old
    pub new_text: Option<String>,
    /// Line number in the old file (if applicable)
    pub old_line: Option<usize>,
    /// Line number in the new file (if applicable)
    pub new_line: Option<usize>,
}

impl ChangeSpan {
    pub fn new(kind: ChangeKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            new_text: None,
            old_line: None,
            new_line: None,
        }
    }

    pub fn insert(text: impl Into<String>) -> Self {
        Self::new(ChangeKind::Insert, text)
    }

    pub fn delete(text: impl Into<String>) -> Self {
        Self::new(ChangeKind::Delete, text)
    }

    pub fn equal(text: impl Into<String>) -> Self {
        Self::new(ChangeKind::Equal, text)
    }

    pub fn replace(old: impl Into<String>, new: impl Into<String>) -> Self {
        Self {
            kind: ChangeKind::Replace,
            text: old.into(),
            new_text: Some(new.into()),
            old_line: None,
            new_line: None,
        }
    }

    pub fn with_lines(mut self, old_line: Option<usize>, new_line: Option<usize>) -> Self {
        self.old_line = old_line;
        self.new_line = new_line;
        self
    }

    /// Check if this is an actual change (not just context)
    pub fn is_change(&self) -> bool {
        self.kind != ChangeKind::Equal
    }
}

/// A complete change unit (may contain multiple spans for word-level diffs)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// Unique ID for this change
    pub id: usize,
    /// The spans that make up this change
    pub spans: Vec<ChangeSpan>,
    /// Description of the change (e.g., "modified function call")
    pub description: Option<String>,
}

impl Change {
    pub fn new(id: usize, spans: Vec<ChangeSpan>) -> Self {
        Self {
            id,
            spans,
            description: None,
        }
    }

    pub fn single(id: usize, span: ChangeSpan) -> Self {
        Self::new(id, vec![span])
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Get all the changes (non-equal spans)
    pub fn changes(&self) -> impl Iterator<Item = &ChangeSpan> {
        self.spans.iter().filter(|s| s.is_change())
    }

    /// Check if this change contains any actual modifications
    pub fn has_changes(&self) -> bool {
        self.spans.iter().any(|s| s.is_change())
    }
}
