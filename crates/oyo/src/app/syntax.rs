use super::types::SyntaxScopeCache;
use super::{display_metrics, AnimationPhase, App, ViewMode};
use crate::syntax::{SyntaxCache, SyntaxEngine, SyntaxSide};
use oyo_core::{LineKind, ViewLine};
use ratatui::text::Span;

impl App {
    pub fn syntax_enabled(&self) -> bool {
        match self.syntax_mode {
            crate::config::SyntaxMode::On => true,
            crate::config::SyntaxMode::Off => false,
        }
    }

    pub fn syntax_spans_for_line(
        &mut self,
        side: SyntaxSide,
        line_num: Option<usize>,
    ) -> Option<Vec<Span<'static>>> {
        if !self.syntax_enabled() {
            return None;
        }
        let line_num = line_num?;
        if line_num == 0 {
            return None;
        }
        let cache = self.ensure_syntax_cache()?;
        let spans = cache.spans(side, line_num - 1)?;
        Some(
            spans
                .iter()
                .map(|span| Span::styled(span.text.clone(), span.style))
                .collect(),
        )
    }

    pub fn syntax_scope_target(&mut self, view: &[ViewLine]) -> Option<(usize, String)> {
        if !self.show_syntax_scopes {
            return None;
        }
        let step_direction = self.multi_diff.current_step_direction();
        let (display_len, _) = display_metrics(
            view,
            self.view_mode,
            self.animation_phase,
            self.scroll_offset,
            step_direction,
            self.split_align_lines,
        );
        if display_len == 0 {
            return None;
        }
        let viewport_height = self.last_viewport_height.max(1);
        let target_idx = self.scroll_offset.saturating_add(viewport_height / 2);
        let display_idx = target_idx.min(display_len.saturating_sub(1));

        let (side, line_num) = self.syntax_line_for_display(view, display_idx)?;
        let file_index = self.multi_diff.selected_index;
        if let Some(cache) = &self.syntax_scope_cache {
            if cache.file_index == file_index && cache.side == side && cache.line_num == line_num {
                return Some((display_idx, cache.label.clone()));
            }
        }
        let file_name = self.current_file_path();
        let nav = self.multi_diff.current_navigator();
        let content = match side {
            SyntaxSide::Old => nav.old_content(),
            SyntaxSide::New => nav.new_content(),
        };
        if self.syntax_engine.is_none() {
            self.syntax_engine = Some(SyntaxEngine::new(&self.syntax_theme, self.theme_is_light));
        }
        let engine = self.syntax_engine.as_ref()?;
        let scopes = engine.scopes_for_line(content, &file_name, line_num - 1);
        let label = if scopes.is_empty() {
            "scopes: (none)".to_string()
        } else {
            format!("scopes: {}", scopes.join(" | "))
        };
        self.syntax_scope_cache = Some(SyntaxScopeCache {
            file_index,
            side,
            line_num,
            label: label.clone(),
        });
        Some((display_idx, label))
    }

    fn ensure_syntax_cache(&mut self) -> Option<&SyntaxCache> {
        if !self.syntax_enabled() {
            return None;
        }
        let idx = self.multi_diff.selected_index;
        if idx >= self.syntax_caches.len() {
            self.syntax_caches = vec![None; self.multi_diff.file_count()];
        }
        if self.syntax_caches[idx].is_none() {
            let file_name = self.current_file_path();
            let (old_content, new_content) = {
                let nav = self.multi_diff.current_navigator();
                (nav.old_content().to_string(), nav.new_content().to_string())
            };
            if self.syntax_engine.is_none() {
                self.syntax_engine =
                    Some(SyntaxEngine::new(&self.syntax_theme, self.theme_is_light));
            }
            let engine = self.syntax_engine.as_ref()?;
            self.syntax_caches[idx] = Some(SyntaxCache::new(
                engine,
                &old_content,
                &new_content,
                &file_name,
            ));
        }
        self.syntax_caches[idx].as_ref()
    }

    fn syntax_line_for_display(
        &self,
        view: &[ViewLine],
        display_idx: usize,
    ) -> Option<(SyntaxSide, usize)> {
        match self.view_mode {
            ViewMode::UnifiedPane | ViewMode::Blame => view.get(display_idx).and_then(|line| {
                line.new_line.or(line.old_line).map(|line_num| {
                    let side = if line.new_line.is_some() {
                        SyntaxSide::New
                    } else {
                        SyntaxSide::Old
                    };
                    (side, line_num)
                })
            }),
            ViewMode::Evolution => {
                let mut display_count = 0usize;
                for line in view {
                    let visible = match line.kind {
                        LineKind::Deleted => false,
                        LineKind::PendingDelete => {
                            line.is_active && self.animation_phase != AnimationPhase::Idle
                        }
                        _ => true,
                    };
                    if visible {
                        if display_count == display_idx {
                            let line_num = line.new_line.or(line.old_line)?;
                            let side = if line.new_line.is_some() {
                                SyntaxSide::New
                            } else {
                                SyntaxSide::Old
                            };
                            return Some((side, line_num));
                        }
                        display_count += 1;
                    }
                }
                None
            }
            ViewMode::Split => {
                let align_lines = self.split_align_lines;
                let mut old_count = 0usize;
                let mut new_count = 0usize;
                let mut old_line = None;
                let mut new_line = None;

                for line in view {
                    let old_present = line.old_line.is_some();
                    let new_present = line.new_line.is_some()
                        && !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete);
                    if old_present || (align_lines && new_present) {
                        if old_present && old_count == display_idx {
                            old_line = line.old_line;
                        }
                        old_count += 1;
                    }
                    if new_present || (align_lines && old_present) {
                        if new_present && new_count == display_idx {
                            new_line = line.new_line;
                        }
                        new_count += 1;
                    }
                    if new_line.is_some() || (old_count > display_idx && new_count > display_idx) {
                        break;
                    }
                }

                if let Some(line_num) = new_line {
                    Some((SyntaxSide::New, line_num))
                } else {
                    old_line.map(|line_num| (SyntaxSide::Old, line_num))
                }
            }
        }
    }
}
