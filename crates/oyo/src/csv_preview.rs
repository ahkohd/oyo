use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CsvPreviewSignature {
    file_name: String,
    len: usize,
    hash: u64,
}

impl CsvPreviewSignature {
    pub(crate) fn new(file_name: &str, text: &str) -> Self {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        Self {
            file_name: file_name.to_string(),
            len: text.len(),
            hash: hasher.finish(),
        }
    }
}

pub(crate) struct CsvPreviewState {
    signature: CsvPreviewSignature,
    rows: Vec<Vec<String>>,
    selected_row: usize,
    selected_col: usize,
    col_offset: usize,
}

impl CsvPreviewState {
    pub(crate) fn new(signature: CsvPreviewSignature, text: &str) -> Result<Self, String> {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(text.as_bytes());
        let rows = reader
            .records()
            .map(|record| {
                record
                    .map(|record| record.iter().map(clean_csv_cell).collect::<Vec<_>>())
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            signature,
            rows,
            selected_row: 0,
            selected_col: 0,
            col_offset: 0,
        })
    }

    pub(crate) fn signature(&self) -> &CsvPreviewSignature {
        &self.signature
    }

    pub(crate) fn rows(&self) -> &[Vec<String>] {
        &self.rows
    }

    #[allow(dead_code)]
    pub(crate) fn selected_row(&self) -> usize {
        self.selected_row
    }

    pub(crate) fn selected_col(&self) -> usize {
        self.selected_col
    }

    pub(crate) fn col_offset(&self) -> usize {
        self.col_offset
    }

    pub(crate) fn set_col_offset(&mut self, offset: usize) {
        self.col_offset = offset.min(self.max_cols().saturating_sub(1));
    }

    /// Scroll offset into the data rows (the header and separator are pinned,
    /// so this is 0-based over data rows: data row `r` is at offset `r - 1`).
    pub(crate) fn selected_visual_line(&self) -> usize {
        self.selected_row.saturating_sub(1)
    }

    pub(crate) fn move_up(&mut self, count: usize) {
        self.selected_row = self.selected_row.saturating_sub(count);
    }

    pub(crate) fn move_down(&mut self, count: usize) {
        self.selected_row = self
            .selected_row
            .saturating_add(count)
            .min(self.rows.len().saturating_sub(1));
    }

    pub(crate) fn move_left(&mut self, count: usize) {
        self.selected_col = self.selected_col.saturating_sub(count);
        if self.selected_col < self.col_offset {
            self.col_offset = self.selected_col;
        }
    }

    pub(crate) fn move_right(&mut self, count: usize) {
        self.selected_col = self
            .selected_col
            .saturating_add(count)
            .min(self.max_cols().saturating_sub(1));
    }

    pub(crate) fn focus_top(&mut self) {
        self.selected_row = 0;
    }

    pub(crate) fn focus_bottom(&mut self) {
        self.selected_row = self.rows.len().saturating_sub(1);
    }

    fn max_cols(&self) -> usize {
        self.rows.iter().map(Vec::len).max().unwrap_or(0)
    }
}

fn clean_csv_cell(cell: &str) -> String {
    cell.chars()
        .map(|ch| match ch {
            '\r' | '\n' | '\t' => ' ',
            _ => ch,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_moves_cells() {
        let sig = CsvPreviewSignature::new("data.csv", "a,b\n1,2\n");
        let mut state = CsvPreviewState::new(sig, "a,b\n1,2\n").unwrap();
        state.move_down(1);
        state.move_right(1);
        assert_eq!(state.selected_row(), 1);
        assert_eq!(state.selected_col(), 1);
        // Data-row offset: the first data row sits at the top of the body.
        assert_eq!(state.selected_visual_line(), 0);
    }
}
