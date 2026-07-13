use super::{App, TopbarTabContent, ViewHistoryRecipe};

const VIEW_HISTORY_CAP: usize = 100;

impl ViewHistoryRecipe {
    fn same_landing(self, other: Self) -> bool {
        match (self, other) {
            (
                Self::File {
                    tab_id: left_tab,
                    file_index: left_file,
                    ..
                },
                Self::File {
                    tab_id: right_tab,
                    file_index: right_file,
                    ..
                },
            ) => left_tab == right_tab && left_file == right_file,
            (Self::Comment { comment_id: left }, Self::Comment { comment_id: right }) => {
                left == right
            }
            (
                Self::PrComments {
                    tab_id: left_tab,
                    focus_comment_id: left_focus,
                },
                Self::PrComments {
                    tab_id: right_tab,
                    focus_comment_id: right_focus,
                },
            )
            | (
                Self::OutdatedComments {
                    tab_id: left_tab,
                    focus_comment_id: left_focus,
                },
                Self::OutdatedComments {
                    tab_id: right_tab,
                    focus_comment_id: right_focus,
                },
            ) => left_tab == right_tab && left_focus == right_focus,
            (Self::Help { tab_id: left }, Self::Help { tab_id: right })
            | (Self::Settings { tab_id: left }, Self::Settings { tab_id: right }) => left == right,
            (Self::Tab { tab_id: left }, Self::Tab { tab_id: right }) => left == right,
            _ => false,
        }
    }
}

impl App {
    pub(crate) fn current_view_history_recipe(&self) -> Option<ViewHistoryRecipe> {
        if let Some(view) = &self.outdated_diff_view {
            return Some(ViewHistoryRecipe::Comment {
                comment_id: view.comment_id,
            });
        }
        match self.active_topbar_content() {
            Some(TopbarTabContent::File(file_index)) => Some(ViewHistoryRecipe::File {
                tab_id: self.active_topbar_tab,
                file_index,
                scroll_offset: self.scroll_offset,
            }),
            Some(TopbarTabContent::PrComments) => {
                self.active_topbar_tab
                    .map(|tab_id| ViewHistoryRecipe::PrComments {
                        tab_id,
                        focus_comment_id: self.pr_comment_focus,
                    })
            }
            Some(TopbarTabContent::OutdatedComments) => {
                self.active_topbar_tab
                    .map(|tab_id| ViewHistoryRecipe::OutdatedComments {
                        tab_id,
                        focus_comment_id: self.outdated_comment_focus,
                    })
            }
            Some(TopbarTabContent::Help) => self
                .active_topbar_tab
                .map(|tab_id| ViewHistoryRecipe::Help { tab_id }),
            Some(TopbarTabContent::Settings) => self
                .active_topbar_tab
                .map(|tab_id| ViewHistoryRecipe::Settings { tab_id }),
            None => None,
        }
    }

    pub(crate) fn view_history_origin(&self) -> Option<ViewHistoryRecipe> {
        self.view_history
            .get(self.view_history_cursor)
            .copied()
            .or_else(|| self.current_view_history_recipe())
    }

    pub(crate) fn record_view_landing(
        &mut self,
        origin: Option<ViewHistoryRecipe>,
        destination: ViewHistoryRecipe,
    ) {
        if self.view_history_replaying {
            return;
        }
        if self.view_history.is_empty() {
            if let Some(origin) = origin {
                self.view_history.push(origin);
            }
            self.view_history_cursor = self.view_history.len().saturating_sub(1);
        } else if let Some(origin) = origin {
            if self.view_history[self.view_history_cursor].same_landing(origin) {
                self.view_history[self.view_history_cursor] = origin;
            }
        }
        if self
            .view_history
            .get(self.view_history_cursor)
            .is_some_and(|current| current.same_landing(destination))
        {
            self.view_history[self.view_history_cursor] = destination;
            return;
        }
        self.view_history
            .truncate(self.view_history_cursor.saturating_add(1));
        self.view_history.push(destination);
        if self.view_history.len() > VIEW_HISTORY_CAP {
            let excess = self.view_history.len() - VIEW_HISTORY_CAP;
            self.view_history.drain(..excess);
        }
        self.view_history_cursor = self.view_history.len().saturating_sub(1);
    }

    fn replay_view_history_recipe(&mut self, recipe: ViewHistoryRecipe) -> bool {
        match recipe {
            ViewHistoryRecipe::File {
                tab_id,
                file_index,
                scroll_offset,
            } => {
                if file_index >= self.multi_diff.file_count() {
                    return false;
                }
                if let Some(tab_id) = tab_id {
                    if !self.topbar_tabs.iter().any(|tab| tab.id == tab_id) {
                        return false;
                    }
                    self.select_topbar_tab(tab_id);
                }
                self.select_file(file_index);
                self.scroll_offset = scroll_offset;
                true
            }
            ViewHistoryRecipe::Comment { comment_id } => {
                let Some(index) = self
                    .review_comments
                    .iter()
                    .position(|comment| comment.id == comment_id && !comment.deleted)
                else {
                    return false;
                };
                self.open_review_comment(index)
            }
            ViewHistoryRecipe::PrComments {
                tab_id,
                focus_comment_id,
            } => {
                if !self.topbar_tabs.iter().any(|tab| tab.id == tab_id) {
                    return false;
                }
                self.select_topbar_tab(tab_id);
                self.open_pr_comments_in_current_tab(focus_comment_id);
                true
            }
            ViewHistoryRecipe::OutdatedComments {
                tab_id,
                focus_comment_id,
            } => {
                if !self.topbar_tabs.iter().any(|tab| tab.id == tab_id) {
                    return false;
                }
                self.select_topbar_tab(tab_id);
                self.open_outdated_comments_in_current_tab(focus_comment_id);
                true
            }
            ViewHistoryRecipe::Help { tab_id } => {
                if !self.topbar_tabs.iter().any(|tab| tab.id == tab_id) {
                    return false;
                }
                self.select_topbar_tab(tab_id);
                self.open_help_in_current_tab();
                true
            }
            ViewHistoryRecipe::Settings { tab_id } => {
                if !self.topbar_tabs.iter().any(|tab| tab.id == tab_id) {
                    return false;
                }
                self.select_topbar_tab(tab_id);
                self.open_settings_in_current_tab();
                true
            }
            ViewHistoryRecipe::Tab { tab_id } => {
                if !self.topbar_tabs.iter().any(|tab| tab.id == tab_id) {
                    return false;
                }
                self.select_topbar_tab(tab_id);
                true
            }
        }
    }

    pub(super) fn navigate_view_history(&mut self, backward: bool) -> bool {
        if self.request_settings_leave(super::settings::SettingsLeaveTarget::History(backward)) {
            return true;
        }
        if self.view_history.is_empty() {
            return false;
        }
        let mut index = self.view_history_cursor;
        loop {
            index = if backward {
                let Some(previous) = index.checked_sub(1) else {
                    return false;
                };
                previous
            } else {
                let next = index.saturating_add(1);
                if next >= self.view_history.len() {
                    return false;
                }
                next
            };
            let recipe = self.view_history[index];
            self.view_history_replaying = true;
            let replayed = self.replay_view_history_recipe(recipe);
            self.view_history_replaying = false;
            if replayed {
                self.view_history_cursor = index;
                return true;
            }
        }
    }

    pub(crate) fn navigate_view_back(&mut self) -> bool {
        self.navigate_view_history(true)
    }

    pub(crate) fn navigate_view_forward(&mut self) -> bool {
        self.navigate_view_history(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ViewMode;
    use oyo_core::git::FileStatus;
    use oyo_core::multi::RawFileDiff;
    use oyo_core::MultiFileDiff;

    fn app_with_files(count: usize) -> App {
        let files = (0..count)
            .map(|index| RawFileDiff {
                path: format!("{index}.txt").into(),
                old_path: None,
                old_source_path: None,
                new_source_path: None,
                status: FileStatus::Modified,
                old_content: "old\n".to_string(),
                new_content: format!("new {index}\n"),
                binary: false,
            })
            .collect();
        App::new(
            MultiFileDiff::from_raw_files(None, files),
            ViewMode::UnifiedPane,
            0,
            false,
            None,
        )
    }

    #[test]
    fn first_navigation_seeds_origin_and_replay_does_not_reenter() {
        let mut app = app_with_files(2);
        app.scroll_offset = 7;
        app.select_file(1);
        assert_eq!(app.view_history.len(), 2);

        assert!(app.navigate_view_back());
        assert_eq!(app.multi_diff.selected_index, 0);
        assert_eq!(app.scroll_offset, 7);
        assert_eq!(app.view_history.len(), 2);
        assert!(app.navigate_view_forward());
        assert_eq!(app.multi_diff.selected_index, 1);
        assert_eq!(app.view_history.len(), 2);
    }

    #[test]
    fn new_landing_after_back_truncates_forward_and_deduplicates() {
        let mut app = app_with_files(3);
        app.select_file(1);
        app.select_file(2);
        assert_eq!(app.view_history.len(), 3);
        assert!(app.navigate_view_back());
        app.select_file(0);
        assert!(!app.navigate_view_forward());
        let len = app.view_history.len();
        app.select_file(0);
        assert_eq!(app.view_history.len(), len);
    }

    #[test]
    fn special_views_capture_their_replay_recipe() {
        let mut app = app_with_files(2);
        app.open_help_tab();
        let help_tab = app.active_topbar_tab.unwrap();
        let tab_count = app.topbar_tabs.len();
        assert!(matches!(
            app.view_history.last(),
            Some(ViewHistoryRecipe::Help { tab_id }) if *tab_id == help_tab
        ));
        assert_eq!(app.view_history.len(), 2);
        app.select_file(1);
        assert!(app.navigate_view_back());
        assert_eq!(app.active_topbar_tab, Some(help_tab));
        assert_eq!(app.topbar_tabs.len(), tab_count);
        assert_eq!(app.active_topbar_content(), Some(TopbarTabContent::Help));
        assert!(app.navigate_view_forward());
        assert_eq!(app.multi_diff.selected_index, 1);

        let mut app = app_with_files(2);
        app.open_outdated_comments_tab(Some(41));
        let outdated_tab = app.active_topbar_tab.unwrap();
        let tab_count = app.topbar_tabs.len();
        assert!(matches!(
            app.view_history.last(),
            Some(ViewHistoryRecipe::OutdatedComments {
                tab_id,
                focus_comment_id: Some(41)
            }) if *tab_id == outdated_tab
        ));
        assert_eq!(app.view_history.len(), 2);
        app.select_file(1);
        assert!(app.navigate_view_back());
        assert_eq!(app.active_topbar_tab, Some(outdated_tab));
        assert_eq!(app.topbar_tabs.len(), tab_count);
        assert_eq!(
            app.active_topbar_content(),
            Some(TopbarTabContent::OutdatedComments)
        );
        assert_eq!(app.outdated_comment_focus, Some(41));
        assert!(app.navigate_view_forward());
        assert_eq!(app.multi_diff.selected_index, 1);

        let mut app = app_with_files(2);
        app.open_pr_comments_tab(Some(42));
        let pr_tab = app.active_topbar_tab.unwrap();
        let tab_count = app.topbar_tabs.len();
        assert!(matches!(
            app.view_history.last(),
            Some(ViewHistoryRecipe::PrComments {
                tab_id,
                focus_comment_id: Some(42)
            }) if *tab_id == pr_tab
        ));
        assert_eq!(app.view_history.len(), 2);
        app.select_file(1);
        assert!(app.navigate_view_back());
        assert_eq!(app.active_topbar_tab, Some(pr_tab));
        assert_eq!(app.topbar_tabs.len(), tab_count);
        assert_eq!(
            app.active_topbar_content(),
            Some(TopbarTabContent::PrComments)
        );
        assert_eq!(app.pr_comment_focus, Some(42));
        assert!(app.navigate_view_forward());
        assert_eq!(app.multi_diff.selected_index, 1);
    }

    #[test]
    fn invalid_recipes_are_skipped_and_history_is_capped() {
        let mut app = app_with_files(2);
        let tab_id = app.active_topbar_tab;
        app.view_history = vec![
            ViewHistoryRecipe::File {
                tab_id,
                file_index: 0,
                scroll_offset: 0,
            },
            ViewHistoryRecipe::Help { tab_id: 999 },
            ViewHistoryRecipe::File {
                tab_id,
                file_index: 99,
                scroll_offset: 0,
            },
            ViewHistoryRecipe::File {
                tab_id,
                file_index: 1,
                scroll_offset: 0,
            },
        ];
        app.view_history_cursor = 3;
        app.multi_diff.select_file(1);
        assert!(app.navigate_view_back());
        assert_eq!(app.multi_diff.selected_index, 0);

        for index in 0..110 {
            app.select_file(index % 2);
        }
        assert_eq!(app.view_history.len(), VIEW_HISTORY_CAP);
    }
}
