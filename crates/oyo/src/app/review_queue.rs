use super::{App, TopbarTabContent};
use rustc_hash::FxHashSet;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum IncrementalReview {
    #[default]
    Off,
    On(ReviewedFileVisibility),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReviewedFileVisibility {
    Hidden,
    Shown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileStep {
    Next,
    Prev,
    NextWrapping,
    PrevWrapping,
}

impl FileStep {
    fn forward(self) -> bool {
        matches!(self, FileStep::Next | FileStep::NextWrapping)
    }

    fn wraps(self) -> bool {
        matches!(self, FileStep::NextWrapping | FileStep::PrevWrapping)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReviewedFilesExhausted {
    EveryFile,
    FilterMatches,
    NoFilterMatches,
}

pub(crate) fn neighbour_file_index(
    order: &[usize],
    navigable: &[usize],
    current: usize,
    step: FileStep,
) -> Option<usize> {
    let (&first, &last) = (navigable.first()?, navigable.last()?);
    let Some(start) = order.iter().position(|&index| index == current) else {
        return Some(if step.forward() { first } else { last }).filter(|&index| index != current);
    };
    let after = &order[start + 1..];
    let before = &order[..start];
    let candidates: Vec<usize> = match (step.forward(), step.wraps()) {
        (true, false) => after.to_vec(),
        (true, true) => after.iter().chain(before).copied().collect(),
        (false, false) => before.iter().rev().copied().collect(),
        (false, true) => before
            .iter()
            .rev()
            .chain(after.iter().rev())
            .copied()
            .collect(),
    };
    let navigable: FxHashSet<usize> = navigable.iter().copied().collect();
    candidates
        .into_iter()
        .find(|index| navigable.contains(index))
        .filter(|&index| index != current)
}

impl App {
    pub(crate) fn set_incremental_review_enabled(&mut self, enabled: bool) {
        self.incremental_review = if enabled {
            IncrementalReview::On(ReviewedFileVisibility::Hidden)
        } else {
            IncrementalReview::Off
        };
    }

    fn reviewed_file_visibility(&self) -> Option<ReviewedFileVisibility> {
        let IncrementalReview::On(visibility) = self.incremental_review else {
            return None;
        };
        (self.review_mode && self.review_persist_enabled).then_some(visibility)
    }

    pub(crate) fn hides_reviewed_files(&self) -> bool {
        self.reviewed_file_visibility() == Some(ReviewedFileVisibility::Hidden)
    }

    pub(crate) fn file_hidden_as_reviewed(&self, file_index: usize) -> bool {
        self.hides_reviewed_files() && self.file_is_reviewed(file_index)
    }

    pub(crate) fn hidden_reviewed_file_count(&self) -> usize {
        if !self.hides_reviewed_files() {
            return 0;
        }
        (0..self.review_live_diff().file_count())
            .filter(|&index| self.file_is_reviewed(index))
            .count()
    }

    pub(crate) fn reviewed_files_visibility_label(&self) -> Option<String> {
        Some(match self.reviewed_file_visibility()? {
            ReviewedFileVisibility::Hidden => match self.hidden_reviewed_file_count() {
                0 => "Show reviewed files".to_string(),
                count => format!("Show reviewed files ({count} hidden)"),
            },
            ReviewedFileVisibility::Shown => "Hide reviewed files".to_string(),
        })
    }

    pub(crate) fn toggle_reviewed_files_visibility(&mut self) -> bool {
        let Some(visibility) = self.reviewed_file_visibility() else {
            return false;
        };
        self.incremental_review = IncrementalReview::On(match visibility {
            ReviewedFileVisibility::Hidden => ReviewedFileVisibility::Shown,
            ReviewedFileVisibility::Shown => ReviewedFileVisibility::Hidden,
        });
        self.reviewed_file_opened_on_purpose = None;
        self.file_list_scroll = 0;
        self.move_selection_off_hidden_reviewed_file();
        self.update_file_list_scroll();
        true
    }

    pub(crate) fn navigable_file_indices(&self) -> Vec<usize> {
        self.filtered_file_indices()
    }

    pub(crate) fn file_step_target(&self, step: FileStep) -> Option<usize> {
        let order = self.file_order_for_query(&self.file_filter);
        let navigable = self.navigable_file_indices();
        neighbour_file_index(&order, &navigable, self.multi_diff.selected_index, step)
    }

    pub(crate) fn remember_selection_opened_on_purpose(&mut self) {
        let selected = self.multi_diff.selected_index;
        self.reviewed_file_opened_on_purpose = self
            .file_hidden_as_reviewed(selected)
            .then(|| self.multi_diff.files.get(selected))
            .flatten()
            .map(|file| file.display_name.clone());
    }

    pub(crate) fn forget_selection_opened_on_purpose(&mut self) {
        self.reviewed_file_opened_on_purpose = None;
    }

    fn selection_shows_hidden_reviewed_file(&self) -> bool {
        let selected = self.multi_diff.selected_index;
        let opened_on_purpose = self.multi_diff.files.get(selected).is_some_and(|file| {
            self.reviewed_file_opened_on_purpose.as_deref() == Some(&file.display_name)
        });
        self.outdated_diff_view.is_none()
            && matches!(
                self.active_topbar_content(),
                Some(TopbarTabContent::File(_)) | None
            )
            && !opened_on_purpose
            && self.file_hidden_as_reviewed(selected)
    }

    pub(crate) fn move_selection_off_hidden_reviewed_file(&mut self) {
        if !self.selection_shows_hidden_reviewed_file() {
            return;
        }
        let order = self.file_order_for_query(&self.file_filter);
        let live_diff = self.review_live_diff();
        let known_unreviewed: Vec<usize> = self
            .navigable_file_indices()
            .into_iter()
            .filter(|&index| live_diff.file_contents(index).is_some())
            .collect();
        let selected = self.multi_diff.selected_index;
        if let Some(next) =
            neighbour_file_index(&order, &known_unreviewed, selected, FileStep::NextWrapping)
        {
            self.select_file(next);
        }
    }

    pub(crate) fn reviewed_files_exhausted(&self) -> Option<ReviewedFilesExhausted> {
        if !self.selection_shows_hidden_reviewed_file() || !self.navigable_file_indices().is_empty()
        {
            return None;
        }
        if self.file_filter.is_empty() {
            return Some(ReviewedFilesExhausted::EveryFile);
        }
        Some(if self.file_order_for_query(&self.file_filter).is_empty() {
            ReviewedFilesExhausted::NoFilterMatches
        } else {
            ReviewedFilesExhausted::FilterMatches
        })
    }

    pub(crate) fn handle_all_reviewed_show_click(&mut self, column: u16, row: u16) -> bool {
        let hit = self
            .all_reviewed_show_hit
            .is_some_and(|(x, y, width, height)| {
                column >= x
                    && column < x.saturating_add(width)
                    && row >= y
                    && row < y.saturating_add(height)
            });
        hit && self.toggle_reviewed_files_visibility()
    }
}

#[cfg(test)]
mod tests {
    use super::{neighbour_file_index, FileStep, ReviewedFilesExhausted};
    use crate::app::palette::PaletteAction;
    use crate::app::{App, TopbarTabContent, ViewMode};
    use oyo_core::MultiFileDiff;
    use std::path::{Path, PathBuf};

    const FILES: [(&str, &str, &str); 4] = [
        ("src/a.rs", "a\n", "a2\n"),
        ("src/b.rs", "b\n", "b2\n"),
        ("src/c.rs", "c\n", "c2\n"),
        ("tests/d.rs", "d\n", "d2\n"),
    ];

    fn review_base(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "oyo-incremental-review-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn diff_of(files: &[(&str, &str, &str)]) -> MultiFileDiff {
        MultiFileDiff::from_file_pairs(
            files
                .iter()
                .map(|(path, old, new)| (PathBuf::from(path), old.to_string(), new.to_string()))
                .collect(),
        )
    }

    fn review_app(base: &Path, files: &[(&str, &str, &str)], incremental: bool) -> App {
        let mut app = App::new(
            diff_of(files),
            ViewMode::UnifiedPane,
            0,
            false,
            Some("branch".to_string()),
        );
        app.set_review_base_dir(Some(base.to_path_buf()));
        app.set_incremental_review_enabled(incremental);
        app.enable_review_mode();
        app
    }

    fn mark_file(app: &mut App, index: usize) {
        app.select_file(index);
        assert!(app.toggle_current_file_reviewed());
    }

    fn palette_labels(app: &App) -> Vec<String> {
        app.command_palette_entries()
            .into_iter()
            .map(|entry| entry.label)
            .collect()
    }

    #[test]
    fn default_off_keeps_marked_files_listed_and_selected() {
        let base = review_base("off");
        let mut app = review_app(&base, &FILES, false);
        mark_file(&mut app, 1);

        assert_eq!(app.multi_diff.selected_index, 1);
        assert!(app.file_is_reviewed(1));
        assert_eq!(app.filtered_file_indices(), vec![0, 1, 2, 3]);
        assert_eq!(app.reviewed_files_exhausted(), None);
        assert!(!palette_labels(&app)
            .iter()
            .any(|label| label.contains("reviewed files")));
        app.prev_file();
        assert_eq!(app.multi_diff.selected_index, 0);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn opt_in_needs_persistent_review_mode() {
        let mut app = App::new(
            diff_of(&FILES),
            ViewMode::UnifiedPane,
            0,
            false,
            Some("branch".to_string()),
        );
        app.set_incremental_review_enabled(true);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        assert!(app.toggle_current_file_reviewed());

        assert!(!app.hides_reviewed_files());
        assert_eq!(app.multi_diff.selected_index, 0);
        assert_eq!(app.filtered_file_indices(), vec![0, 1, 2, 3]);
    }

    #[test]
    fn marking_hides_the_file_and_advances_to_the_next_remaining_file() {
        let base = review_base("advance");
        let mut app = review_app(&base, &FILES, true);
        mark_file(&mut app, 1);
        assert_eq!(app.multi_diff.selected_index, 2);
        assert_eq!(app.filtered_file_indices(), vec![0, 2, 3]);

        mark_file(&mut app, 3);
        assert_eq!(app.multi_diff.selected_index, 0);
        assert_eq!(app.filtered_file_indices(), vec![0, 2]);

        app.next_file();
        assert_eq!(app.multi_diff.selected_index, 2);
        app.next_file();
        assert_eq!(app.multi_diff.selected_index, 2);
        assert!(app.next_file_wrapped());
        assert_eq!(app.multi_diff.selected_index, 0);
        assert!(app.prev_file_wrapped());
        assert_eq!(app.multi_diff.selected_index, 2);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn reviewing_every_file_shows_all_reviewed_and_reveal_restores_access() {
        let base = review_base("all");
        let mut app = review_app(&base, &FILES, true);
        for _ in 0..FILES.len() {
            assert!(app.toggle_current_file_reviewed());
        }
        assert_eq!(app.hidden_reviewed_file_count(), 4);
        assert!(app.filtered_file_indices().is_empty());
        assert_eq!(
            app.reviewed_files_exhausted(),
            Some(ReviewedFilesExhausted::EveryFile)
        );
        assert!(app.multi_diff.selected_index < FILES.len());
        assert!(palette_labels(&app).contains(&"Show reviewed files (4 hidden)".to_string()));

        let selected = app.multi_diff.selected_index;
        app.execute_palette_action(PaletteAction::ToggleReviewedFilesVisibility);
        assert_eq!(app.reviewed_files_exhausted(), None);
        assert_eq!(app.filtered_file_indices(), vec![0, 1, 2, 3]);
        assert_eq!(app.multi_diff.selected_index, selected);
        assert!(palette_labels(&app).contains(&"Hide reviewed files".to_string()));

        assert!(app.toggle_current_file_reviewed());
        assert!(!app.file_is_reviewed(selected));
        app.execute_palette_action(PaletteAction::ToggleReviewedFilesVisibility);
        assert_eq!(app.filtered_file_indices(), vec![selected]);
        assert_eq!(app.multi_diff.selected_index, selected);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn unmarking_from_all_reviewed_brings_the_file_back() {
        let base = review_base("undo");
        let mut app = review_app(&base, &FILES[..2], true);
        assert!(app.toggle_current_file_reviewed());
        assert!(app.toggle_current_file_reviewed());
        let selected = app.multi_diff.selected_index;
        assert!(app.reviewed_files_exhausted().is_some());

        assert!(app.toggle_current_file_reviewed());
        assert_eq!(app.reviewed_files_exhausted(), None);
        assert_eq!(app.filtered_file_indices(), vec![selected]);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn hidden_files_stay_hidden_after_restart_until_their_diff_changes() {
        let base = review_base("restart");
        let mut first = review_app(&base, &FILES, true);
        mark_file(&mut first, 0);
        mark_file(&mut first, 2);

        let reopened = review_app(&base, &FILES, true);
        assert_eq!(reopened.filtered_file_indices(), vec![1, 3]);
        assert_eq!(reopened.multi_diff.selected_index, 1);

        let mut changed = FILES;
        changed[2].2 = "c3\n";
        let changed = review_app(&base, &changed, true);
        assert_eq!(changed.filtered_file_indices(), vec![1, 2, 3]);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn refresh_with_reordered_files_keeps_indices_and_selection_valid() {
        let base = review_base("refresh");
        let mut app = review_app(&base, &FILES, true);
        mark_file(&mut app, 1);
        assert_eq!(app.multi_diff.selected_index, 2);

        app.replace_multi_diff(diff_of(&[FILES[3], FILES[1], FILES[0]]));
        assert_eq!(app.filtered_file_indices(), vec![0, 2]);
        assert!(app.multi_diff.selected_index < app.multi_diff.file_count());
        assert!(!app.file_hidden_as_reviewed(app.multi_diff.selected_index));

        let mut changed_b = FILES[1];
        changed_b.2 = "b3\n";
        app.replace_multi_diff(diff_of(&[FILES[0], changed_b]));
        assert_eq!(app.filtered_file_indices(), vec![0, 1]);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn hiding_reviewed_files_moves_off_a_reviewed_selection() {
        let base = review_base("hide-selected");
        let mut app = review_app(&base, &FILES, true);
        mark_file(&mut app, 3);
        app.execute_palette_action(PaletteAction::ToggleReviewedFilesVisibility);
        app.select_file(3);
        assert_eq!(app.multi_diff.selected_index, 3);

        app.execute_palette_action(PaletteAction::ToggleReviewedFilesVisibility);
        assert_eq!(app.multi_diff.selected_index, 0);
        assert_eq!(app.filtered_file_indices(), vec![0, 1, 2]);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn a_reviewed_file_opened_on_purpose_stays_open() {
        let base = review_base("on-purpose");
        let mut app = review_app(&base, &FILES[..2], true);
        assert!(app.toggle_current_file_reviewed());
        assert!(app.toggle_current_file_reviewed());
        assert!(app.reviewed_files_exhausted().is_some());

        app.select_file(0);
        assert_eq!(app.multi_diff.selected_index, 0);
        assert_eq!(app.reviewed_files_exhausted(), None);
        app.move_selection_off_hidden_reviewed_file();
        assert_eq!(app.multi_diff.selected_index, 0);

        app.replace_multi_diff(diff_of(&[FILES[1], FILES[0]]));
        app.move_selection_off_hidden_reviewed_file();
        assert_eq!(
            app.multi_diff.files[app.multi_diff.selected_index].display_name,
            "src/a.rs"
        );
        assert_eq!(app.reviewed_files_exhausted(), None);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn a_filter_without_matches_never_shows_a_hidden_diff() {
        let base = review_base("no-match");
        let mut app = review_app(&base, &FILES[..2], true);
        assert!(app.toggle_current_file_reviewed());
        assert!(app.toggle_current_file_reviewed());
        for ch in "zzz".chars() {
            app.push_file_filter_char(ch);
        }
        assert_eq!(
            app.reviewed_files_exhausted(),
            Some(ReviewedFilesExhausted::NoFilterMatches)
        );
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn comment_sidebar_query_does_not_move_the_selected_file() {
        let base = review_base("comment-query");
        let mut app = review_app(&base, &FILES, true);
        mark_file(&mut app, 1);
        app.select_file(1);
        app.show_comments_sidebar();
        for ch in "d.rs".chars() {
            app.push_file_filter_char(ch);
        }
        assert_eq!(app.multi_diff.selected_index, 1);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn text_filter_combines_with_hiding_for_navigation_and_empty_state() {
        let base = review_base("filter");
        let mut app = review_app(&base, &FILES, true);
        for ch in "src".chars() {
            app.push_file_filter_char(ch);
        }
        assert_eq!(app.filtered_file_indices(), vec![0, 1, 2]);

        mark_file(&mut app, 1);
        assert_eq!(app.multi_diff.selected_index, 2);
        app.prev_file();
        assert_eq!(app.multi_diff.selected_index, 0);
        assert!(app.prev_file_wrapped());
        assert_eq!(app.multi_diff.selected_index, 2);

        assert!(app.toggle_current_file_reviewed());
        assert_eq!(app.multi_diff.selected_index, 0);
        assert!(app.toggle_current_file_reviewed());
        assert!(app.filtered_file_indices().is_empty());
        assert_eq!(
            app.reviewed_files_exhausted(),
            Some(ReviewedFilesExhausted::FilterMatches)
        );

        app.clear_file_filter();
        assert_eq!(app.filtered_file_indices(), vec![3]);
        assert_eq!(app.reviewed_files_exhausted(), None);
        assert_eq!(app.multi_diff.selected_index, 3);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn quick_file_search_lists_only_visible_files() {
        let base = review_base("search");
        let mut app = review_app(&base, &FILES, true);
        mark_file(&mut app, 0);
        assert_eq!(app.file_indices_for_query("rs"), vec![1, 2, 3]);
        assert_eq!(app.file_order_for_query("rs"), vec![0, 1, 2, 3]);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn hunk_navigation_crosses_into_the_next_visible_file() {
        let base = review_base("hunks");
        let mut app = review_app(&base, &FILES, true);
        app.hunk_wrap = crate::config::HunkWrapMode::File;
        app.toggle_stepping();
        mark_file(&mut app, 1);
        app.select_file(0);
        app.next_hunk();
        assert_eq!(app.multi_diff.selected_index, 2);
        app.next_hunk();
        assert_eq!(app.multi_diff.selected_index, 3);
        let mut visited_backwards = Vec::new();
        while app.multi_diff.selected_index != 0 && visited_backwards.len() < 6 {
            app.prev_hunk();
            visited_backwards.push(app.multi_diff.selected_index);
        }
        assert_eq!(visited_backwards.last(), Some(&0));
        assert!(!visited_backwards.contains(&1));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn hidden_files_keep_their_comments() {
        let base = review_base("comments");
        let mut app = review_app(&base, &FILES, true);
        app.add_review_comment_from_cli(
            "src/a.rs",
            crate::app::review::ReviewTargetKind::File,
            None,
            None,
            None,
            "keep me".to_string(),
        )
        .unwrap();
        mark_file(&mut app, 0);

        assert!(app.file_hidden_as_reviewed(0));
        assert!(app
            .review_comments
            .iter()
            .any(|comment| comment.body == "keep me"));
        let reopened = review_app(&base, &FILES, true);
        assert!(reopened
            .review_comments
            .iter()
            .any(|comment| comment.body == "keep me"));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn neighbour_matches_plain_navigation_when_every_file_is_navigable() {
        let all = [0, 1, 2];
        assert_eq!(neighbour_file_index(&all, &all, 0, FileStep::Next), Some(1));
        assert_eq!(neighbour_file_index(&all, &all, 2, FileStep::Next), None);
        assert_eq!(neighbour_file_index(&all, &all, 0, FileStep::Prev), None);
        assert_eq!(neighbour_file_index(&all, &all, 2, FileStep::Prev), Some(1));
        assert_eq!(
            neighbour_file_index(&all, &all, 2, FileStep::NextWrapping),
            Some(0)
        );
        assert_eq!(
            neighbour_file_index(&all, &all, 0, FileStep::PrevWrapping),
            Some(2)
        );
        assert_eq!(
            neighbour_file_index(&[0], &[0], 0, FileStep::NextWrapping),
            None
        );
    }

    #[test]
    fn neighbour_skips_hidden_files_from_a_hidden_current_file() {
        let order = [0, 1, 2, 3, 4];
        let navigable = [0, 3];
        assert_eq!(
            neighbour_file_index(&order, &navigable, 1, FileStep::Next),
            Some(3)
        );
        assert_eq!(
            neighbour_file_index(&order, &navigable, 4, FileStep::Next),
            None
        );
        assert_eq!(
            neighbour_file_index(&order, &navigable, 4, FileStep::NextWrapping),
            Some(0)
        );
        assert_eq!(
            neighbour_file_index(&order, &navigable, 2, FileStep::Prev),
            Some(0)
        );
        assert_eq!(
            neighbour_file_index(&order, &navigable, 0, FileStep::PrevWrapping),
            Some(3)
        );
        assert_eq!(neighbour_file_index(&order, &[], 1, FileStep::Next), None);
    }

    #[test]
    fn neighbour_follows_filter_order_and_enters_from_outside_it() {
        let order = [4, 1, 3];
        assert_eq!(
            neighbour_file_index(&order, &order, 1, FileStep::Next),
            Some(3)
        );
        assert_eq!(
            neighbour_file_index(&order, &order, 0, FileStep::Next),
            Some(4)
        );
        assert_eq!(
            neighbour_file_index(&order, &order, 0, FileStep::PrevWrapping),
            Some(3)
        );
    }

    fn pending_diff(root: &Path, names: &[&str]) -> MultiFileDiff {
        MultiFileDiff::from_pending_files(
            Some(root.to_path_buf()),
            names
                .iter()
                .map(|name| {
                    let file = oyo_core::multi::FileEntry {
                        path: name.into(),
                        old_path: None,
                        old_source_path: None,
                        new_source_path: None,
                        display_name: name.to_string(),
                        status: oyo_core::git::FileStatus::Modified,
                        insertions: 1,
                        deletions: 1,
                        binary: false,
                    };
                    (
                        file,
                        oyo_core::multi::ContentSource::File(root.join(format!("old-{name}"))),
                        oyo_core::multi::ContentSource::File(root.join(format!("new-{name}"))),
                    )
                })
                .collect(),
            true,
        )
    }

    fn finish_content_loading(app: &mut App) {
        for _ in 0..500 {
            app.poll_content_responses();
            if app.content_loading_count() == 0 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("content did not finish loading");
    }

    const PENDING_NAMES: [&str; 3] = ["one.txt", "two.txt", "three.txt"];

    fn pending_root(name: &str) -> PathBuf {
        let root = review_base(name);
        std::fs::create_dir_all(&root).unwrap();
        for name in PENDING_NAMES {
            std::fs::write(root.join(format!("old-{name}")), format!("old {name}\n")).unwrap();
            std::fs::write(root.join(format!("new-{name}")), format!("new {name}\n")).unwrap();
        }
        root
    }

    fn loading_review_app(base: &Path, root: &Path, names: &[&str]) -> App {
        let mut app = App::new(
            pending_diff(root, names),
            ViewMode::UnifiedPane,
            0,
            false,
            Some("branch".to_string()),
        );
        app.set_review_base_dir(Some(base.to_path_buf()));
        app.set_incremental_review_enabled(true);
        app.enable_review_mode();
        assert!(app.start_content_loading());
        app
    }

    fn tick_until_content_loaded(app: &mut App) {
        for _ in 0..500 {
            app.tick();
            if app.content_loading_count() == 0 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("content did not finish loading");
    }

    fn selected_name(app: &App) -> &str {
        &app.multi_diff.files[app.multi_diff.selected_index].display_name
    }

    #[test]
    fn files_still_loading_are_never_treated_as_reviewed() {
        let base = review_base("loading");
        let root = pending_root("loading-root");
        let names = &PENDING_NAMES[..2];
        let mut writer = loading_review_app(&base, &root, names);
        finish_content_loading(&mut writer);
        mark_file(&mut writer, 1);
        assert_eq!(writer.filtered_file_indices(), vec![0]);

        let mut reopened = loading_review_app(&base, &root, names);
        assert_eq!(reopened.multi_diff.file_contents(1), None);
        assert!(!reopened.file_is_reviewed(1));
        assert_eq!(reopened.filtered_file_indices(), vec![0, 1]);
        finish_content_loading(&mut reopened);
        assert_eq!(reopened.filtered_file_indices(), vec![0]);
        let _ = std::fs::remove_dir_all(base);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn content_arrival_keeps_a_reviewed_file_opened_on_purpose() {
        let base = review_base("arrival-on-purpose");
        let root = pending_root("arrival-on-purpose-root");
        let mut app = loading_review_app(&base, &root, &PENDING_NAMES);
        tick_until_content_loaded(&mut app);
        mark_file(&mut app, 0);
        assert_eq!(selected_name(&app), "two.txt");

        app.select_file(0);
        app.replace_multi_diff(pending_diff(&root, &PENDING_NAMES));
        assert!(app.content_loading_count() > 0);
        tick_until_content_loaded(&mut app);
        assert_eq!(selected_name(&app), "one.txt");
        assert_eq!(app.reviewed_files_exhausted(), None);
        let _ = std::fs::remove_dir_all(base);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn content_arrival_moves_off_a_startup_selection_that_turns_out_reviewed() {
        let base = review_base("arrival-startup");
        let root = pending_root("arrival-startup-root");
        let mut writer = loading_review_app(&base, &root, &PENDING_NAMES);
        tick_until_content_loaded(&mut writer);
        mark_file(&mut writer, 0);

        let mut reopened = loading_review_app(&base, &root, &PENDING_NAMES);
        assert_eq!(selected_name(&reopened), "one.txt");
        assert_eq!(reopened.multi_diff.file_contents(0), None);
        tick_until_content_loaded(&mut reopened);
        assert_eq!(selected_name(&reopened), "two.txt");
        let _ = std::fs::remove_dir_all(base);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn content_arrival_never_replaces_help_settings_or_pr_comments() {
        let openers = [
            (App::open_help_tab as fn(&mut App), TopbarTabContent::Help),
            (App::open_settings_tab, TopbarTabContent::Settings),
            (
                |app: &mut App| app.open_pr_comments_tab(None),
                TopbarTabContent::PrComments,
            ),
        ];
        for (open, content) in openers {
            let base = review_base("arrival-tabs");
            let root = pending_root("arrival-tabs-root");
            let mut writer = loading_review_app(&base, &root, &PENDING_NAMES);
            tick_until_content_loaded(&mut writer);
            mark_file(&mut writer, 0);

            let mut reopened = loading_review_app(&base, &root, &PENDING_NAMES);
            open(&mut reopened);
            assert_eq!(reopened.active_topbar_content(), Some(content));
            let history = reopened.view_history.len();
            tick_until_content_loaded(&mut reopened);
            assert!(reopened.file_hidden_as_reviewed(0));
            assert_eq!(reopened.active_topbar_content(), Some(content));
            assert_eq!(selected_name(&reopened), "one.txt");
            assert_eq!(reopened.view_history.len(), history);
            assert!(reopened.settings_leave_confirmation.is_none());
            let _ = std::fs::remove_dir_all(base);
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn all_reviewed_screen_replaces_the_diff_and_offers_the_way_back() {
        let base = review_base("screen");
        let mut app = review_app(&base, &FILES[..2], true);
        assert!(app.toggle_current_file_reviewed());
        assert!(app.toggle_current_file_reviewed());
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 24)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let screen = format!("{:?}", terminal.backend().buffer());

        assert!(screen.contains("All files reviewed."));
        assert!(screen.contains("Show reviewed files"));
        assert!(screen.contains("unmark src/b.rs"));
        assert!(!screen.contains("b2"));
        let (x, y, _, _) = app.all_reviewed_show_hit.unwrap();
        assert!(app.handle_all_reviewed_show_click(x, y));
        assert_eq!(app.filtered_file_indices(), vec![0, 1]);

        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let screen = format!("{:?}", terminal.backend().buffer());
        assert!(!screen.contains("All files reviewed."));
        assert!(screen.contains("b2"));
        let _ = std::fs::remove_dir_all(base);
    }
}
