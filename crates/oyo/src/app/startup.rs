use super::App;

pub(crate) type RestoreRemoteReview = Box<dyn FnOnce(&mut App) + Send>;

impl App {
    pub(crate) fn into_remote_startup(self) -> (oyo_core::MultiFileDiff, RestoreRemoteReview) {
        let diff = self.multi_diff;
        let restore = Box::new(move |app: &mut App| {
            app.review_mode = self.review_mode;
            app.review_comments = self.review_comments;
            app.reviewed_files = self.reviewed_files;
            app.review_session_baseline = self.review_session_baseline;
            app.review_editor = self.review_editor;
            app.review_dir_path = self.review_dir_path;
            app.review_db_path = self.review_db_path;
            app.review_db_stamp = self.review_db_stamp;
            app.last_review_db_check = self.last_review_db_check;
            app.review_base_dir_override = self.review_base_dir_override;
            app.review_workspace_root_override = self.review_workspace_root_override;
            app.review_persist_enabled = self.review_persist_enabled;
            app.review_filter_to_current_diff = self.review_filter_to_current_diff;
            app.review_diff_fingerprint = self.review_diff_fingerprint;
            app.review_storage_key = self.review_storage_key;
            app.review_repo_root = self.review_repo_root;
            app.review_target_metadata = self.review_target_metadata;
            app.review_pull_request_target = self.review_pull_request_target;
            app.review_author = self.review_author;
            app.review_session_created_at = self.review_session_created_at;
            app.review_next_comment_id = self.review_next_comment_id;
            app.review_mention_picker = self.review_mention_picker;
            app.remote_stream = self.remote_stream;
            app.remote_validation = self.remote_validation;
            app.pending_remote_review = self.pending_remote_review;
            app.content_worker_rx = self.content_worker_rx;
            app.content_session = self.content_session;
            app.content_generation = self.content_generation;
            app.content_loading = self.content_loading;
            app.review_revision = self.review_revision;
        });
        (diff, restore)
    }
}
