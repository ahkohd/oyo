use super::{AnimationPhase, App};
use crate::toasts::ToastEvent;
use std::time::Instant;

impl App {
    pub fn toggle_autoplay(&mut self) {
        if self.autoplay && !self.autoplay_reverse {
            self.autoplay = false;
        } else {
            self.autoplay = true;
            self.autoplay_reverse = false;
        }
        self.last_autoplay_tick = Instant::now();
    }

    pub fn toggle_autoplay_reverse(&mut self) {
        if self.autoplay && self.autoplay_reverse {
            self.autoplay = false;
        } else {
            self.autoplay = true;
            self.autoplay_reverse = true;
            self.autoplay_remaining = None;
        }
        self.last_autoplay_tick = Instant::now();
    }

    pub fn toggle_animation(&mut self) {
        self.animation_enabled = !self.animation_enabled;
        self.notify(ToastEvent::Animation(self.animation_enabled));
        if !self.animation_enabled {
            self.stop_animation_now();
        }
    }

    pub(crate) fn stop_autoplay(&mut self) {
        self.autoplay = false;
        self.autoplay_reverse = false;
        self.autoplay_remaining = None;
    }

    pub(crate) fn stop_animation_now(&mut self) {
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.snap_frame = None;
        self.snap_frame_started_at = None;
        self.clear_active_on_next_render = false;
    }

    pub(crate) fn stop_control_motion(&mut self) {
        self.stop_autoplay();
        self.stop_animation_now();
    }

    pub fn increase_speed(&mut self) {
        self.animation_speed = (self.animation_speed + 50).min(2000);
    }

    pub fn decrease_speed(&mut self) {
        self.animation_speed = self.animation_speed.saturating_sub(50).max(50);
    }
}
