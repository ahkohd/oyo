use super::review::ReviewSide;
use super::App;
use neo_frizbee::{
    match_list, match_list_indices, match_list_parallel, CaseMatching, Config, UnicodeMatching,
};
use oyo_core::{
    multi::{DiffStatus, FileEntry},
    ChangeKind, DiffEngine, FileStatus,
};
use ratatui::text::Span;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::{mpsc, Arc};
use unicode_segmentation::UnicodeSegmentation;

const EXACT_REVIEW_GREP_SCORE: u16 = u16::MAX;
const MAX_FUZZY_REVIEW_GREP_SCORE: u16 = u16::MAX - 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FuzzyFileMatch {
    pub file_index: usize,
    pub indices: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReviewGrepMatch {
    pub file_index: usize,
    pub score: u16,
    pub side: ReviewSide,
    pub line_number: usize,
    content: Arc<str>,
    range: Range<usize>,
    pub indices: Vec<usize>,
}

impl ReviewGrepMatch {
    pub(crate) fn text(&self) -> &str {
        &self.content[self.range.clone()]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ReviewGrepScope {
    Changes,
    #[default]
    Everything,
}

impl ReviewGrepScope {
    fn toggled(self) -> Self {
        match self {
            Self::Changes => Self::Everything,
            Self::Everything => Self::Changes,
        }
    }
}

#[derive(Clone)]
struct ReviewGrepSource {
    file_index: usize,
    status: FileStatus,
    diff_status: DiffStatus,
    old: Arc<str>,
    new: Arc<str>,
}

#[derive(Clone)]
struct ReviewGrepLine {
    file_index: usize,
    side: ReviewSide,
    line_number: usize,
    content: Arc<str>,
    range: Range<usize>,
    folded: String,
    changes: bool,
    everything: bool,
}

impl ReviewGrepLine {
    fn text(&self) -> &str {
        &self.content[self.range.clone()]
    }
}

struct ReviewGrepRequest {
    generation: u64,
    diff_revision: u64,
    query: String,
    scope: ReviewGrepScope,
    context_lines: usize,
    sources: Arc<Vec<ReviewGrepSource>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReviewGrepSyntaxIdentity {
    pub diff_revision: u64,
    pub ui_theme: Option<String>,
    pub syntax_theme: String,
    pub light: bool,
}

struct ReviewGrepResponse {
    generation: u64,
    diff_revision: u64,
    query: String,
    scope: ReviewGrepScope,
    results: Vec<ReviewGrepMatch>,
}

pub(crate) struct ReviewGrepState {
    active: bool,
    query: String,
    selection: usize,
    results: Vec<ReviewGrepMatch>,
    searching: bool,
    generation: u64,
    requested_revision: Option<u64>,
    source_revision: Option<u64>,
    source_context_lines: Option<usize>,
    sources: Arc<Vec<ReviewGrepSource>>,
    pending_files: usize,
    request_tx: Option<mpsc::Sender<ReviewGrepRequest>>,
    response_rx: Option<mpsc::Receiver<ReviewGrepResponse>>,
    list_area: Option<(u16, u16, u16, u16)>,
    list_start: usize,
    list_count: usize,
    item_height: u16,
    pending_jump: Option<(usize, ReviewSide, usize)>,
    scope: ReviewGrepScope,
    changes_hit: Option<(u16, u16, u16, u16)>,
    everything_hit: Option<(u16, u16, u16, u16)>,
    scope_hover: Option<ReviewGrepScope>,
    pub(crate) syntax_identity: Option<ReviewGrepSyntaxIdentity>,
    pub(crate) syntax_spans: HashMap<(usize, u8, usize), Option<Vec<Span<'static>>>>,
    #[cfg(test)]
    pub(crate) syntax_cache_misses: usize,
}

impl Default for ReviewGrepState {
    fn default() -> Self {
        Self {
            active: false,
            query: String::new(),
            selection: 0,
            results: Vec::new(),
            searching: false,
            generation: 0,
            requested_revision: None,
            source_revision: None,
            source_context_lines: None,
            sources: Arc::new(Vec::new()),
            pending_files: 0,
            request_tx: None,
            response_rx: None,
            list_area: None,
            list_start: 0,
            list_count: 0,
            item_height: 1,
            pending_jump: None,
            scope: ReviewGrepScope::Everything,
            changes_hit: None,
            everything_hit: None,
            scope_hover: None,
            syntax_identity: None,
            syntax_spans: HashMap::new(),
            #[cfg(test)]
            syntax_cache_misses: 0,
        }
    }
}

pub(crate) fn fuzzy_config(query: &str) -> Config {
    let length = query.chars().count();
    let max_typos = match length {
        0..=3 => 0,
        4..=7 => 1,
        _ => 2,
    };
    Config {
        max_typos: Some(max_typos),
        casing: CaseMatching::Ignore,
        unicode: UnicodeMatching::Smart,
        sort: true,
        ..Config::default()
    }
}

pub(crate) fn fuzzy_text_indices(query: &str, text: &str) -> Vec<usize> {
    let mut matches = match_list_indices(query, &[text], &fuzzy_config(query));
    let Some(mut matched) = matches.pop() else {
        return Vec::new();
    };
    matched.indices.sort_unstable();
    matched.indices.dedup();
    matched.indices
}

pub(crate) fn fuzzy_file_matches(files: &[FileEntry], query: &str) -> Vec<FuzzyFileMatch> {
    let query = query.trim();
    if query.is_empty() {
        return (0..files.len())
            .map(|file_index| FuzzyFileMatch {
                file_index,
                indices: Vec::new(),
            })
            .collect();
    }
    let paths = files
        .iter()
        .map(|file| file.display_name.as_str())
        .collect::<Vec<_>>();
    match_list_indices(query, &paths, &fuzzy_config(query))
        .into_iter()
        .map(|mut matched| {
            matched.indices.sort_unstable();
            matched.indices.dedup();
            FuzzyFileMatch {
                file_index: matched.index as usize,
                indices: matched.indices,
            }
        })
        .collect()
}

fn push_content_lines(
    lines: &mut Vec<ReviewGrepLine>,
    file_index: usize,
    side: ReviewSide,
    content: &Arc<str>,
    changes: &HashSet<usize>,
    everything: bool,
    only_changes: bool,
) {
    let mut offset = 0usize;
    for (line_index, segment) in content.split_inclusive('\n').enumerate() {
        let line_number = line_index + 1;
        let is_change = changes.contains(&line_number);
        let without_newline = segment.strip_suffix('\n').unwrap_or(segment);
        let text = without_newline
            .strip_suffix('\r')
            .unwrap_or(without_newline);
        let end = offset + text.len();
        if !only_changes || is_change {
            lines.push(ReviewGrepLine {
                file_index,
                side,
                line_number,
                content: Arc::clone(content),
                range: offset..end,
                folded: text.to_lowercase(),
                changes: is_change,
                everything,
            });
        }
        offset += segment.len();
    }
}

fn build_review_grep_corpus(
    sources: &[ReviewGrepSource],
    context_lines: usize,
) -> Arc<Vec<ReviewGrepLine>> {
    let mut lines = Vec::new();
    for source in sources {
        if source.status == FileStatus::Deleted {
            let changed = (1..=source.old.lines().count()).collect::<HashSet<_>>();
            push_content_lines(
                &mut lines,
                source.file_index,
                ReviewSide::Old,
                &source.old,
                &changed,
                true,
                false,
            );
            continue;
        }
        let diff = DiffEngine::new()
            .with_word_level(false)
            .with_context(context_lines)
            .diff_strings(&source.old, &source.new);
        let mut new_changes = HashSet::new();
        let mut old_deletions = HashSet::new();
        for change in &diff.changes {
            for span in &change.spans {
                match span.kind {
                    ChangeKind::Equal | ChangeKind::Insert => {
                        if let Some(line) = span.new_line {
                            new_changes.insert(line);
                        }
                    }
                    ChangeKind::Delete => {
                        if let Some(line) = span.old_line {
                            old_deletions.insert(line);
                        }
                    }
                    ChangeKind::Replace => {
                        if let Some(line) = span.new_line {
                            new_changes.insert(line);
                        }
                        if let Some(line) = span.old_line {
                            old_deletions.insert(line);
                        }
                    }
                }
            }
        }
        push_content_lines(
            &mut lines,
            source.file_index,
            ReviewSide::New,
            &source.new,
            &new_changes,
            true,
            false,
        );
        // Disabled placeholders show current content only, so old-side matches cannot land.
        if source.diff_status != DiffStatus::Disabled {
            push_content_lines(
                &mut lines,
                source.file_index,
                ReviewSide::Old,
                &source.old,
                &old_deletions,
                false,
                true,
            );
        }
    }
    Arc::new(lines)
}

fn case_insensitive_substring_range(
    text: &str,
    folded_text: &str,
    folded_query: &str,
) -> Option<Range<usize>> {
    let folded_start = folded_text.find(folded_query)?;
    if text.is_ascii() {
        return Some(folded_start..folded_start + folded_query.len());
    }

    let folded_end = folded_start + folded_query.len();
    let mut source_start = None;
    let mut folded_offset = 0usize;
    for (start, ch) in text.char_indices() {
        let source_end = start + ch.len_utf8();
        let folded_char_len = ch.to_lowercase().map(char::len_utf8).sum::<usize>();
        let next_folded_offset = folded_offset + folded_char_len;
        if source_start.is_none() && folded_start < next_folded_offset {
            source_start = Some(start);
        }
        if folded_end <= next_folded_offset {
            return source_start.map(|source_start| source_start..source_end);
        }
        folded_offset = next_folded_offset;
    }
    None
}

fn exact_line_ranges(
    corpus: &[ReviewGrepLine],
    line_indices: &[usize],
    folded_query: &str,
    threads: usize,
) -> Vec<(usize, Option<Range<usize>>)> {
    if line_indices.len() < 2_048 || threads <= 1 {
        return line_indices
            .iter()
            .map(|index| {
                let line = &corpus[*index];
                (
                    *index,
                    case_insensitive_substring_range(line.text(), &line.folded, folded_query),
                )
            })
            .collect();
    }

    let chunk_size = line_indices.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let handles = line_indices
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|index| {
                            let line = &corpus[*index];
                            (
                                *index,
                                case_insensitive_substring_range(
                                    line.text(),
                                    &line.folded,
                                    folded_query,
                                ),
                            )
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("exact search worker panicked"))
            .collect()
    })
}

fn run_review_grep(request: &ReviewGrepRequest, corpus: &[ReviewGrepLine]) -> ReviewGrepResponse {
    if request.query.is_empty() {
        return ReviewGrepResponse {
            generation: request.generation,
            diff_revision: request.diff_revision,
            query: request.query.clone(),
            scope: request.scope,
            results: Vec::new(),
        };
    }

    let config = fuzzy_config(&request.query);
    let line_indices = corpus
        .iter()
        .enumerate()
        .filter(|(_, line)| match request.scope {
            ReviewGrepScope::Changes => line.changes,
            ReviewGrepScope::Everything => line.everything,
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let folded_query = request.query.to_lowercase();
    let threads = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .min(8);
    let exact_ranges = exact_line_ranges(corpus, &line_indices, &folded_query, threads);
    let mut grouped: HashMap<usize, (u16, Vec<ReviewGrepMatch>)> = HashMap::new();
    let mut fuzzy_line_indices = Vec::new();
    for (line_index, exact_range) in exact_ranges {
        let line = &corpus[line_index];
        let Some(exact_range) = exact_range else {
            fuzzy_line_indices.push(line_index);
            continue;
        };
        let indices = line.text()[exact_range.clone()]
            .grapheme_indices(true)
            .map(|(offset, _)| exact_range.start + offset)
            .collect();
        let group = grouped
            .entry(line.file_index)
            .or_insert_with(|| (EXACT_REVIEW_GREP_SCORE, Vec::new()));
        group.0 = EXACT_REVIEW_GREP_SCORE;
        group.1.push(ReviewGrepMatch {
            file_index: line.file_index,
            score: EXACT_REVIEW_GREP_SCORE,
            side: line.side,
            line_number: line.line_number,
            content: Arc::clone(&line.content),
            range: line.range.clone(),
            indices,
        });
    }

    let haystacks = fuzzy_line_indices
        .iter()
        .map(|index| corpus[*index].text())
        .collect::<Vec<_>>();
    let fuzzy_matches = if haystacks.len() >= 2_048 && threads > 1 {
        match_list_parallel(&request.query, &haystacks, &config, threads)
    } else {
        match_list(&request.query, &haystacks, &config)
    };
    for matched in fuzzy_matches {
        let Some(line) = fuzzy_line_indices
            .get(matched.index as usize)
            .and_then(|index| corpus.get(*index))
        else {
            continue;
        };
        let score = matched.score.min(MAX_FUZZY_REVIEW_GREP_SCORE);
        let group = grouped
            .entry(line.file_index)
            .or_insert_with(|| (score, Vec::new()));
        group.0 = group.0.max(score);
        group.1.push(ReviewGrepMatch {
            file_index: line.file_index,
            score,
            side: line.side,
            line_number: line.line_number,
            content: Arc::clone(&line.content),
            range: line.range.clone(),
            indices: Vec::new(),
        });
    }

    let mut groups = grouped
        .into_iter()
        .map(|(file_index, (best_score, matches))| (file_index, best_score, matches))
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    let mut results = Vec::new();
    for (_, _, mut matches) in groups {
        matches.sort_by_key(|matched| {
            (
                matched.score != EXACT_REVIEW_GREP_SCORE,
                matched.line_number,
                match matched.side {
                    ReviewSide::Old => 0,
                    ReviewSide::New => 1,
                },
            )
        });
        results.extend(matches);
    }
    let fuzzy_result_indices = results
        .iter()
        .enumerate()
        .filter_map(|(index, matched)| (matched.score != EXACT_REVIEW_GREP_SCORE).then_some(index))
        .collect::<Vec<_>>();
    let matched_lines = fuzzy_result_indices
        .iter()
        .map(|index| results[*index].text())
        .collect::<Vec<_>>();
    for mut matched in match_list_indices(&request.query, &matched_lines, &config) {
        let Some(result) = fuzzy_result_indices
            .get(matched.index as usize)
            .and_then(|index| results.get_mut(*index))
        else {
            continue;
        };
        matched.indices.sort_unstable();
        matched.indices.dedup();
        result.indices = matched.indices;
    }

    ReviewGrepResponse {
        generation: request.generation,
        diff_revision: request.diff_revision,
        query: request.query.clone(),
        scope: request.scope,
        results,
    }
}

impl ReviewGrepState {
    fn ensure_worker(&mut self) {
        if self.request_tx.is_some() {
            return;
        }
        let (request_tx, request_rx) = mpsc::channel::<ReviewGrepRequest>();
        let (response_tx, response_rx) = mpsc::channel::<ReviewGrepResponse>();
        std::thread::spawn(move || {
            let mut corpus_key = None;
            let mut corpus = Arc::new(Vec::new());
            while let Ok(mut request) = request_rx.recv() {
                while let Ok(newer) = request_rx.try_recv() {
                    request = newer;
                }
                let key = (request.diff_revision, request.context_lines);
                if corpus_key != Some(key) {
                    corpus = build_review_grep_corpus(&request.sources, request.context_lines);
                    corpus_key = Some(key);
                }
                if response_tx
                    .send(run_review_grep(&request, &corpus))
                    .is_err()
                {
                    break;
                }
            }
        });
        self.request_tx = Some(request_tx);
        self.response_rx = Some(response_rx);
    }
}

impl App {
    pub(crate) fn fuzzy_file_matches_for_query(&self, query: &str) -> Vec<FuzzyFileMatch> {
        let files = self
            .outdated_live_files()
            .unwrap_or(self.multi_diff.files.as_slice());
        fuzzy_file_matches(files, query)
    }

    fn build_review_grep_sources(&self) -> (Arc<Vec<ReviewGrepSource>>, usize) {
        let mut sources = Vec::new();
        let mut pending_files = 0usize;
        for (file_index, file) in self.multi_diff.files.iter().enumerate() {
            if file.binary {
                continue;
            }
            let Some((old, new)) = self.multi_diff.file_contents_arc(file_index) else {
                pending_files += 1;
                continue;
            };
            sources.push(ReviewGrepSource {
                file_index,
                status: file.status,
                diff_status: self.multi_diff.diff_status(file_index),
                old,
                new,
            });
        }
        (Arc::new(sources), pending_files)
    }

    fn submit_review_grep(&mut self) {
        let query = self.review_grep.query.trim().to_string();
        self.review_grep.generation = self.review_grep.generation.wrapping_add(1);
        if query.is_empty() {
            self.review_grep.results.clear();
            self.review_grep.selection = 0;
            self.review_grep.searching = false;
            self.review_grep.requested_revision = Some(self.diff_revision());
            self.review_grep.source_context_lines = Some(self.fold_context_lines);
            return;
        }
        let revision = self.diff_revision();
        let context_lines = self.fold_context_lines;
        if self.review_grep.source_revision != Some(revision)
            || self.review_grep.source_context_lines != Some(context_lines)
        {
            let (sources, pending_files) = self.build_review_grep_sources();
            self.review_grep.sources = sources;
            self.review_grep.pending_files = pending_files;
            self.review_grep.source_revision = Some(revision);
            self.review_grep.source_context_lines = Some(context_lines);
        }
        self.review_grep.ensure_worker();
        self.review_grep.searching = true;
        self.review_grep.requested_revision = Some(revision);
        let request = ReviewGrepRequest {
            generation: self.review_grep.generation,
            diff_revision: revision,
            query,
            scope: self.review_grep.scope,
            context_lines,
            sources: Arc::clone(&self.review_grep.sources),
        };
        if self
            .review_grep
            .request_tx
            .as_ref()
            .is_none_or(|tx| tx.send(request).is_err())
        {
            self.review_grep.searching = false;
        }
    }

    pub fn start_review_grep(&mut self) {
        self.restore_live_diff_after_outdated_view();
        if self.multi_diff.file_count() == 0 {
            return;
        }
        self.review_grep.active = true;
        self.review_grep.query.clear();
        self.review_grep.selection = 0;
        self.review_grep.results.clear();
        self.review_grep.list_area = None;
        self.review_grep.pending_jump = None;
        self.file_filter_active = false;
        self.clear_search();
        self.clear_goto();
        self.stop_command_palette();
        self.stop_file_search();
        self.stop_comment_picker();
        self.stop_theme_picker();
        self.reset_picker_cursor();
        self.submit_review_grep();
    }

    pub fn stop_review_grep(&mut self) {
        self.review_grep.active = false;
        self.review_grep.generation = self.review_grep.generation.wrapping_add(1);
        self.review_grep.requested_revision = None;
        self.review_grep.source_revision = None;
        self.review_grep.source_context_lines = None;
        self.review_grep.sources = Arc::new(Vec::new());
        self.review_grep.pending_files = 0;
        self.review_grep.results.clear();
        self.review_grep.searching = false;
        self.review_grep.list_area = None;
        self.review_grep.changes_hit = None;
        self.review_grep.everything_hit = None;
        self.review_grep.scope_hover = None;
        self.review_grep.syntax_identity = None;
        self.review_grep.syntax_spans.clear();
    }

    pub fn review_grep_active(&self) -> bool {
        self.review_grep.active
    }

    pub(crate) fn review_grep_query(&self) -> &str {
        &self.review_grep.query
    }

    pub(crate) fn review_grep_selection(&self) -> usize {
        self.review_grep.selection
    }

    pub(crate) fn review_grep_results(&self) -> &[ReviewGrepMatch] {
        &self.review_grep.results
    }

    pub(crate) fn review_grep_searching(&self) -> bool {
        self.review_grep.searching
    }

    pub(crate) fn review_grep_pending_files(&self) -> usize {
        self.review_grep.pending_files
    }

    pub(crate) fn review_grep_scope(&self) -> ReviewGrepScope {
        self.review_grep.scope
    }

    pub fn select_review_grep_scope(&mut self, scope: ReviewGrepScope) {
        if self.review_grep.scope == scope {
            return;
        }
        self.review_grep.scope = scope;
        self.submit_review_grep();
    }

    pub fn toggle_review_grep_scope(&mut self) {
        self.select_review_grep_scope(self.review_grep.scope.toggled());
    }

    pub fn open_review_grep_scope(&mut self, scope: ReviewGrepScope) {
        if !self.review_grep.active {
            self.start_review_grep();
        }
        self.select_review_grep_scope(scope);
    }

    pub fn push_review_grep_char(&mut self, ch: char) {
        self.review_grep.query.push(ch);
        self.reset_picker_cursor();
        self.submit_review_grep();
    }

    pub fn pop_review_grep_char(&mut self) {
        self.review_grep.query.pop();
        self.reset_picker_cursor();
        self.submit_review_grep();
    }

    pub fn clear_review_grep_text(&mut self) {
        self.review_grep.query.clear();
        self.reset_picker_cursor();
        self.submit_review_grep();
    }

    pub fn move_review_grep_selection(&mut self, delta: isize) {
        let total = self.review_grep.results.len();
        if total == 0 {
            self.review_grep.selection = 0;
            return;
        }
        let current = self.review_grep.selection.min(total - 1) as isize;
        self.review_grep.selection =
            (current + delta).clamp(0, total.saturating_sub(1) as isize) as usize;
    }

    pub fn apply_review_grep_selection(&mut self) {
        let Some(result) = self
            .review_grep
            .results
            .get(self.review_grep.selection)
            .cloned()
        else {
            return;
        };
        self.stop_review_grep();
        self.restore_live_diff_after_outdated_view();
        self.select_file(result.file_index);
        if matches!(
            self.multi_diff.current_file_diff_status(),
            DiffStatus::Ready | DiffStatus::Disabled
        ) {
            self.goto_review_grep_line(result.side, result.line_number);
        } else {
            self.review_grep.pending_jump =
                Some((result.file_index, result.side, result.line_number));
        }
        self.file_list_focused = false;
    }

    pub(crate) fn poll_review_grep_jump(&mut self) -> bool {
        let Some((file_index, side, line_number)) = self.review_grep.pending_jump else {
            return false;
        };
        if self.multi_diff.selected_index != file_index {
            self.review_grep.pending_jump = None;
            return false;
        }
        match self.multi_diff.current_file_diff_status() {
            DiffStatus::Ready | DiffStatus::Disabled => {
                self.review_grep.pending_jump = None;
                self.goto_review_grep_line(side, line_number);
                true
            }
            DiffStatus::Failed => {
                self.review_grep.pending_jump = None;
                true
            }
            DiffStatus::Loading | DiffStatus::Deferred | DiffStatus::Computing => false,
        }
    }

    pub(crate) fn poll_review_grep(&mut self) -> bool {
        let mut changed = false;
        if self.review_grep.active
            && (self.review_grep.requested_revision != Some(self.diff_revision())
                || self.review_grep.source_context_lines != Some(self.fold_context_lines))
        {
            self.submit_review_grep();
            changed = true;
        }
        let Some(rx) = self.review_grep.response_rx.as_ref() else {
            return changed;
        };
        let mut latest = None;
        while let Ok(response) = rx.try_recv() {
            latest = Some(response);
        }
        let Some(response) = latest else {
            return changed;
        };
        if response.generation != self.review_grep.generation
            || response.diff_revision != self.diff_revision()
            || response.query != self.review_grep.query.trim()
            || response.scope != self.review_grep.scope
        {
            return changed;
        }
        self.review_grep.results = response.results;
        self.review_grep.selection = self
            .review_grep
            .selection
            .min(self.review_grep.results.len().saturating_sub(1));
        self.review_grep.searching = false;
        true
    }

    pub(crate) fn review_grep_list_start(&self) -> usize {
        self.review_grep.list_start
    }

    #[cfg(test)]
    pub(crate) fn review_grep_list_test_geometry(&self) -> Option<(u16, u16, u16, u16, usize)> {
        self.review_grep
            .list_area
            .map(|(x, y, width, height)| (x, y, width, height, self.review_grep.list_count))
    }

    pub(crate) fn set_review_grep_list_area(
        &mut self,
        area: Option<(u16, u16, u16, u16)>,
        start: usize,
        count: usize,
        item_height: u16,
    ) {
        self.review_grep.list_area = area;
        self.review_grep.list_start = start;
        self.review_grep.list_count = count;
        self.review_grep.item_height = item_height.max(1);
    }

    pub(crate) fn set_review_grep_scope_hits(
        &mut self,
        changes: (u16, u16, u16, u16),
        everything: (u16, u16, u16, u16),
    ) {
        self.review_grep.changes_hit = Some(changes);
        self.review_grep.everything_hit = Some(everything);
    }

    pub(crate) fn update_review_grep_scope_hover(&mut self, column: u16, row: u16) -> bool {
        let point_in = |rect: (u16, u16, u16, u16)| {
            column >= rect.0
                && column < rect.0.saturating_add(rect.2)
                && row >= rect.1
                && row < rect.1.saturating_add(rect.3)
        };
        let hover = if self.review_grep.changes_hit.is_some_and(point_in) {
            Some(ReviewGrepScope::Changes)
        } else if self.review_grep.everything_hit.is_some_and(point_in) {
            Some(ReviewGrepScope::Everything)
        } else {
            None
        };
        if hover == self.review_grep.scope_hover {
            return false;
        }
        self.review_grep.scope_hover = hover;
        true
    }

    pub(crate) fn review_grep_scope_hover(&self) -> Option<ReviewGrepScope> {
        self.review_grep.scope_hover
    }

    pub(crate) fn update_review_grep_list_hover(&mut self, column: u16, row: u16) -> bool {
        let Some((x, y, width, height)) = self.review_grep.list_area else {
            return false;
        };
        if column < x
            || column >= x.saturating_add(width)
            || row < y
            || row >= y.saturating_add(height)
        {
            return false;
        }
        let offset = row.saturating_sub(y) / self.review_grep.item_height.max(1);
        if offset as usize >= self.review_grep.list_count {
            return false;
        }
        let selection = self.review_grep.list_start.saturating_add(offset as usize);
        if selection == self.review_grep.selection {
            return false;
        }
        self.review_grep.selection = selection;
        true
    }

    pub(crate) fn handle_review_grep_click(&mut self, column: u16, row: u16) -> bool {
        let point_in = |rect: (u16, u16, u16, u16)| {
            column >= rect.0
                && column < rect.0.saturating_add(rect.2)
                && row >= rect.1
                && row < rect.1.saturating_add(rect.3)
        };
        if self.review_grep.changes_hit.is_some_and(point_in) {
            self.select_review_grep_scope(ReviewGrepScope::Changes);
            return true;
        }
        if self.review_grep.everything_hit.is_some_and(point_in) {
            self.select_review_grep_scope(ReviewGrepScope::Everything);
            return true;
        }
        let Some((x, y, width, height)) = self.review_grep.list_area else {
            return false;
        };
        if column < x
            || column >= x.saturating_add(width)
            || row < y
            || row >= y.saturating_add(height)
        {
            return false;
        }
        let offset = row.saturating_sub(y) / self.review_grep.item_height.max(1);
        if offset as usize >= self.review_grep.list_count {
            return false;
        }
        self.review_grep.selection = self.review_grep.list_start.saturating_add(offset as usize);
        self.apply_review_grep_selection();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ViewMode;
    use oyo_core::{
        multi::{ContentSource, RawFileDiff},
        MultiFileDiff,
    };
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn file(path: &str) -> FileEntry {
        FileEntry {
            path: PathBuf::from(path),
            old_path: None,
            old_source_path: None,
            new_source_path: None,
            display_name: path.to_string(),
            status: FileStatus::Modified,
            insertions: 1,
            deletions: 1,
            binary: false,
        }
    }

    fn wait_for_grep(app: &mut App) {
        for _ in 0..200 {
            app.poll_review_grep();
            if !app.review_grep_searching() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("review grep did not finish");
    }

    fn grep_line(file_index: usize, line_number: usize, text: &str) -> ReviewGrepLine {
        let content: Arc<str> = Arc::from(text);
        let length = content.len();
        ReviewGrepLine {
            file_index,
            side: ReviewSide::New,
            line_number,
            content,
            range: 0..length,
            folded: text.to_lowercase(),
            changes: true,
            everything: true,
        }
    }

    #[test]
    fn fuzzy_files_rank_typos_and_return_unicode_safe_indices() {
        let mut files = (0..100)
            .map(|index| file(&format!("src/generated/file_{index}.rs")))
            .collect::<Vec<_>>();
        files.push(file("src/services/user_service.rs"));
        let matches = fuzzy_file_matches(&files, "usrservce");
        assert_eq!(matches.first().map(|matched| matched.file_index), Some(100));
        assert!(matches[0].indices.windows(2).all(|pair| pair[0] < pair[1]));

        let indices = fuzzy_text_indices("mañna", "mañana.rs");
        assert!(!indices.is_empty());
        assert!(indices.iter().all(|index| *index < "mañana.rs".len()));
    }

    #[test]
    fn exact_grep_phrases_rank_first_and_keep_fuzzy_typos() {
        let corpus = vec![
            grep_line(0, 1, "struct Argz"),
            grep_line(1, 1, "struct Argz helper"),
            grep_line(1, 2, "pub STRUCT ARGS {"),
        ];
        let mut request = ReviewGrepRequest {
            generation: 1,
            diff_revision: 1,
            query: "struct Args".to_string(),
            scope: ReviewGrepScope::Everything,
            context_lines: 0,
            sources: Arc::new(Vec::new()),
        };

        let response = run_review_grep(&request, &corpus);
        assert_eq!(response.results[0].file_index, 1);
        assert_eq!(response.results[0].line_number, 2);
        assert_eq!(response.results[0].score, EXACT_REVIEW_GREP_SCORE);
        assert_eq!(response.results[0].indices, (4..15).collect::<Vec<_>>());
        assert_eq!(response.results[1].file_index, 1);
        assert_eq!(response.results[1].line_number, 1);
        assert!(response.results[1].score < EXACT_REVIEW_GREP_SCORE);

        request.query = "AR".to_string();
        let short_response = run_review_grep(&request, &[grep_line(0, 1, "target")]);
        assert_eq!(short_response.results[0].score, EXACT_REVIEW_GREP_SCORE);
        assert_eq!(short_response.results[0].indices, vec![1, 2]);

        request.query = "strct Args".to_string();
        let typo_response = run_review_grep(&request, &[grep_line(0, 1, "struct Args")]);
        assert_eq!(typo_response.results.len(), 1);
        assert!(typo_response.results[0].score < EXACT_REVIEW_GREP_SCORE);
        assert!(!typo_response.results[0].indices.is_empty());

        request.query.clear();
        assert!(run_review_grep(&request, &corpus).results.is_empty());

        let unicode = "pre CAFÉ post";
        let unicode_folded = unicode.to_lowercase();
        let range = case_insensitive_substring_range(unicode, &unicode_folded, "café").unwrap();
        assert_eq!(&unicode[range], "CAFÉ");
        request.query = "café".to_string();
        let unicode_response = run_review_grep(&request, &[grep_line(0, 1, unicode)]);
        assert_eq!(unicode_response.results[0].indices, vec![4, 5, 6, 7]);

        let expanding = "İx";
        let expanding_folded = expanding.to_lowercase();
        assert_eq!(
            case_insensitive_substring_range(expanding, &expanding_folded, "x"),
            Some(2..3)
        );
        assert_eq!(
            case_insensitive_substring_range(expanding, &expanding_folded, &"İ".to_lowercase()),
            Some(0..2)
        );
    }

    #[test]
    fn parallel_grep_scales_and_keeps_file_matches_contiguous() {
        let mut corpus = Vec::with_capacity(20_000);
        for file_index in 0..100 {
            for line_index in 0..200 {
                let text = if file_index == 7 && line_index == 10 {
                    "needlz".to_string()
                } else if file_index == 42 && line_index < 5 {
                    format!("needle result {line_index}")
                } else {
                    "zzzz".to_string()
                };
                let folded = text.to_lowercase();
                let content: Arc<str> = Arc::from(text);
                let length = content.len();
                corpus.push(ReviewGrepLine {
                    file_index,
                    side: ReviewSide::New,
                    line_number: line_index + 1,
                    content,
                    range: 0..length,
                    folded,
                    changes: true,
                    everything: true,
                });
            }
        }
        let request = ReviewGrepRequest {
            generation: 1,
            diff_revision: 1,
            query: "needlz".to_string(),
            scope: ReviewGrepScope::Everything,
            context_lines: 0,
            sources: Arc::new(Vec::new()),
        };
        let response = run_review_grep(&request, &corpus);
        assert_eq!(response.results.len(), 6);
        assert_eq!(response.results[0].file_index, 7);
        assert_eq!(response.results[0].score, EXACT_REVIEW_GREP_SCORE);
        assert!(response.results[1..]
            .iter()
            .all(|matched| matched.file_index == 42));
        assert_eq!(
            response.results[1..]
                .iter()
                .map(|matched| matched.line_number)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );
        assert!(response
            .results
            .iter()
            .all(|matched| !matched.indices.is_empty()));
        assert!(response.results[1..]
            .iter()
            .all(|matched| matched.score < EXACT_REVIEW_GREP_SCORE));
    }

    #[test]
    fn split_grep_landings_count_aligned_blank_rows() {
        let mut old_target = App::new(
            MultiFileDiff::from_file_pairs(vec![(
                "src/inserted.rs".into(),
                "one\ntarget\n".to_string(),
                "one\ninserted\ntarget\n".to_string(),
            )]),
            ViewMode::Split,
            0,
            false,
            None,
        );
        old_target.toggle_stepping();
        old_target.auto_center = false;
        old_target.split_align_lines = true;
        old_target.goto_review_grep_line(ReviewSide::Old, 2);
        assert_eq!(old_target.scroll_offset, 2);

        let mut new_target = App::new(
            MultiFileDiff::from_file_pairs(vec![(
                "src/deleted.rs".into(),
                "one\ndeleted\ntarget\n".to_string(),
                "one\ntarget\n".to_string(),
            )]),
            ViewMode::Split,
            0,
            false,
            None,
        );
        new_target.toggle_stepping();
        new_target.auto_center = false;
        new_target.split_align_lines = true;
        new_target.goto_review_grep_line(ReviewSide::New, 2);
        assert_eq!(new_target.scroll_offset, 2);
    }

    #[test]
    fn empty_query_poll_does_not_resubmit_forever() {
        let diff = MultiFileDiff::from_file_pairs(vec![(
            "src/one.rs".into(),
            "old\n".to_string(),
            "new\n".to_string(),
        )]);
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.start_review_grep();
        let generation = app.review_grep.generation;
        assert!(!app.poll_review_grep());
        assert!(!app.poll_review_grep());
        assert_eq!(app.review_grep.generation, generation);
    }

    #[test]
    fn grep_keeps_results_and_selection_while_next_query_is_pending() {
        let diff = MultiFileDiff::from_file_pairs(vec![
            (
                "src/one.rs".into(),
                "old\n".to_string(),
                "needle one\n".to_string(),
            ),
            (
                "src/two.rs".into(),
                "old\n".to_string(),
                "needle two\n".to_string(),
            ),
        ]);
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.start_review_grep();
        for ch in "needle".chars() {
            app.push_review_grep_char(ch);
        }
        wait_for_grep(&mut app);
        app.move_review_grep_selection(1);
        let old_results = app.review_grep_results().to_vec();
        assert_eq!(old_results.len(), 2);
        assert_eq!(app.review_grep_selection(), 1);

        for ch in "qqq".chars() {
            app.push_review_grep_char(ch);
        }
        assert!(app.review_grep_searching());
        assert_eq!(app.review_grep_results(), old_results);
        assert_eq!(app.review_grep_selection(), 1);

        wait_for_grep(&mut app);
        assert!(app.review_grep_results().is_empty());
        assert_eq!(app.review_grep_selection(), 0);
    }

    #[test]
    fn grep_refreshes_when_pending_review_content_arrives() {
        let entry = file("src/later.rs");
        let diff = MultiFileDiff::from_pending_files(
            None,
            vec![(entry, ContentSource::Empty, ContentSource::Empty)],
            true,
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.start_review_grep();
        for ch in "arrived".chars() {
            app.push_review_grep_char(ch);
        }
        wait_for_grep(&mut app);
        assert_eq!(app.review_grep_pending_files(), 1);
        assert!(app.review_grep_results().is_empty());

        let content = MultiFileDiff::prepare_file_content(
            Some(Vec::new()),
            Some(b"content arrived later\n".to_vec()),
        );
        assert!(app.multi_diff.apply_prepared_content(0, content));
        app.mark_diff_changed();
        app.poll_review_grep();
        wait_for_grep(&mut app);
        assert_eq!(app.review_grep_pending_files(), 0);
        assert_eq!(app.review_grep_results()[0].file_index, 0);
    }

    #[test]
    fn grep_scopes_changes_and_everything_without_losing_session_scope() {
        let old = (1..=30)
            .map(|line| match line {
                10 => "old needle".to_string(),
                25 => "unchanged faraway".to_string(),
                _ => format!("stable {line}"),
            })
            .collect::<Vec<_>>()
            .join("\n");
        let new = (1..=30)
            .map(|line| match line {
                10 => "new needle".to_string(),
                25 => "unchanged faraway".to_string(),
                _ => format!("stable {line}"),
            })
            .collect::<Vec<_>>()
            .join("\n");
        let diff = MultiFileDiff::from_raw_files(
            None,
            vec![
                RawFileDiff {
                    path: PathBuf::from("src/changed.rs"),
                    old_path: None,
                    old_source_path: None,
                    new_source_path: None,
                    status: FileStatus::Modified,
                    old_content: old,
                    new_content: new,
                    binary: false,
                },
                RawFileDiff {
                    path: PathBuf::from("src/deleted.rs"),
                    old_path: None,
                    old_source_path: None,
                    new_source_path: None,
                    status: FileStatus::Deleted,
                    old_content: "legacyword\n".to_string(),
                    new_content: String::new(),
                    binary: false,
                },
            ],
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.start_review_grep();
        for ch in "needle".chars() {
            app.push_review_grep_char(ch);
        }
        wait_for_grep(&mut app);
        assert_eq!(app.review_grep_results().len(), 1);

        app.select_review_grep_scope(ReviewGrepScope::Changes);
        wait_for_grep(&mut app);
        assert_eq!(app.review_grep_results().len(), 2);

        app.clear_review_grep_text();
        for ch in "stable 12".chars() {
            app.push_review_grep_char(ch);
        }
        wait_for_grep(&mut app);
        assert!(app
            .review_grep_results()
            .iter()
            .any(|matched| matched.line_number == 12));

        app.clear_review_grep_text();
        for ch in "faraway".chars() {
            app.push_review_grep_char(ch);
        }
        wait_for_grep(&mut app);
        assert!(app.review_grep_results().is_empty());
        app.select_review_grep_scope(ReviewGrepScope::Everything);
        wait_for_grep(&mut app);
        assert_eq!(app.review_grep_results()[0].line_number, 25);

        app.clear_review_grep_text();
        for ch in "legacyword".chars() {
            app.push_review_grep_char(ch);
        }
        wait_for_grep(&mut app);
        assert_eq!(app.review_grep_results()[0].side, ReviewSide::Old);
        app.select_review_grep_scope(ReviewGrepScope::Changes);
        wait_for_grep(&mut app);
        assert_eq!(app.review_grep_results()[0].side, ReviewSide::Old);

        app.stop_review_grep();
        app.start_review_grep();
        assert_eq!(app.review_grep_scope(), ReviewGrepScope::Changes);
    }

    #[test]
    fn review_grep_scope_segments_hover_and_select() {
        let diff = MultiFileDiff::from_file_pairs(vec![(
            "src/one.rs".into(),
            "old\n".to_string(),
            "new\n".to_string(),
        )]);
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.start_review_grep();
        app.set_review_grep_scope_hits((10, 5, 12, 1), (25, 5, 16, 1));
        assert!(app.update_review_grep_scope_hover(11, 5));
        assert_eq!(
            app.review_grep_scope_hover(),
            Some(ReviewGrepScope::Changes)
        );
        assert!(app.handle_review_grep_click(11, 5));
        assert_eq!(app.review_grep_scope(), ReviewGrepScope::Changes);
        assert!(app.handle_review_grep_click(26, 5));
        assert_eq!(app.review_grep_scope(), ReviewGrepScope::Everything);
    }

    #[test]
    fn grep_searches_full_review_content_and_rejects_stale_queries() {
        let repo = std::env::temp_dir().join(format!(
            "oyo-review-grep-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("outside.txt"), "outsideonly\n").unwrap();
        let unchanged = (2..=80)
            .map(|line| {
                if line == 70 {
                    "unchanged secretword".to_string()
                } else {
                    format!("stable line {line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let diff = MultiFileDiff::from_raw_files(
            Some(repo.clone()),
            vec![
                RawFileDiff {
                    path: PathBuf::from("src/current.rs"),
                    old_path: None,
                    old_source_path: None,
                    new_source_path: None,
                    status: FileStatus::Modified,
                    old_content: format!("changed before\n{unchanged}\n"),
                    new_content: format!("changed after\n{unchanged}\n"),
                    binary: false,
                },
                RawFileDiff {
                    path: PathBuf::from("src/deleted.rs"),
                    old_path: None,
                    old_source_path: None,
                    new_source_path: None,
                    status: FileStatus::Deleted,
                    old_content: "legacyword\n".to_string(),
                    new_content: String::new(),
                    binary: false,
                },
            ],
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.start_review_grep();
        for ch in "legacyword".chars() {
            app.push_review_grep_char(ch);
        }
        app.clear_review_grep_text();
        for ch in "secrtword".chars() {
            app.push_review_grep_char(ch);
        }
        wait_for_grep(&mut app);

        assert_eq!(app.review_grep_results().len(), 1);
        let result = &app.review_grep_results()[0];
        assert_eq!(result.file_index, 0);
        assert_eq!(result.side, ReviewSide::New);
        assert_eq!(result.line_number, 70);
        assert!(result.text().contains("secretword"));
        assert!(!result.text().contains("legacyword"));
        app.apply_review_grep_selection();
        assert_eq!(app.multi_diff.selected_index, 0);
        let view = app.current_view_with_frame(oyo_core::AnimationFrame::Idle);
        assert_eq!(view[app.scroll_offset].new_line, Some(70));

        app.start_review_grep();
        app.clear_review_grep_text();
        for ch in "legacyword".chars() {
            app.push_review_grep_char(ch);
        }
        wait_for_grep(&mut app);
        let result = &app.review_grep_results()[0];
        assert_eq!(result.file_index, 1);
        assert_eq!(result.side, ReviewSide::Old);
        assert_eq!(result.line_number, 1);
        app.apply_review_grep_selection();
        assert_eq!(app.multi_diff.selected_index, 1);
        let view = app.current_view_with_frame(oyo_core::AnimationFrame::Idle);
        assert_eq!(view[app.scroll_offset].old_line, Some(1));

        app.start_review_grep();
        for ch in "outsideonly".chars() {
            app.push_review_grep_char(ch);
        }
        wait_for_grep(&mut app);
        assert!(app.review_grep_results().is_empty());
        let _ = std::fs::remove_dir_all(repo);
    }
}
