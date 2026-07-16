//! On-screen toast notifications. Pure queue logic (testable) plus an egui
//! draw pass. Release Windows builds have no console, so this is the only
//! user-visible channel for save/load feedback and similar events.
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

pub struct Toast {
    text: String,
    kind: ToastKind,
    created: Instant,
}

/// Max simultaneously visible toasts; pushing beyond evicts the oldest.
const MAX_TOASTS: usize = 4;
/// Fade-out window at the end of a toast's life.
const FADE: Duration = Duration::from_millis(400);

fn lifetime(kind: ToastKind) -> Duration {
    match kind {
        ToastKind::Info | ToastKind::Success => Duration::from_millis(2500),
        ToastKind::Error => Duration::from_millis(4000),
    }
}

#[derive(Default)]
pub struct ToastQueue {
    toasts: VecDeque<Toast>,
}

impl ToastQueue {
    pub fn push(&mut self, text: impl Into<String>, kind: ToastKind) {
        self.push_at(text, kind, Instant::now());
    }

    fn push_at(&mut self, text: impl Into<String>, kind: ToastKind, now: Instant) {
        if self.toasts.len() == MAX_TOASTS {
            self.toasts.pop_front();
        }
        self.toasts.push_back(Toast { text: text.into(), kind, created: now });
    }

    fn retain_active_at(&mut self, now: Instant) {
        self.toasts
            .retain(|t| now.duration_since(t.created) < lifetime(t.kind));
    }

    pub fn is_empty(&self) -> bool {
        self.toasts.is_empty()
    }

    /// 1.0 for most of a toast's life, linear fade to 0.0 over the final FADE.
    fn opacity(toast: &Toast, now: Instant) -> f32 {
        let remaining = lifetime(toast.kind).saturating_sub(now.duration_since(toast.created));
        if remaining >= FADE {
            1.0
        } else {
            remaining.as_secs_f32() / FADE.as_secs_f32()
        }
    }

    /// Draw active toasts anchored bottom-left, oldest on top, newest nearest
    /// the anchor. Drops expired toasts first.
    pub fn draw(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        self.retain_active_at(now);
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("toast_stack"))
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(8.0, -8.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                for toast in self.toasts.iter() {
                    let color = match toast.kind {
                        ToastKind::Info => egui::Color32::from_rgb(220, 220, 220),
                        ToastKind::Success => egui::Color32::from_rgb(140, 220, 140),
                        ToastKind::Error => egui::Color32::from_rgb(240, 120, 110),
                    };
                    ui.scope(|ui| {
                        ui.set_opacity(Self::opacity(toast, now));
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.colored_label(color, &toast.text);
                        });
                    });
                    ui.add_space(4.0);
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_toasts_are_dropped() {
        let t0 = Instant::now();
        let mut q = ToastQueue::default();
        q.push_at("hi", ToastKind::Info, t0);
        q.retain_active_at(t0 + Duration::from_millis(2400));
        assert!(!q.is_empty(), "still alive at 2.4s");
        q.retain_active_at(t0 + Duration::from_millis(2600));
        assert!(q.is_empty(), "expired after 2.5s");
    }

    #[test]
    fn error_toasts_outlive_info_lifetime() {
        let t0 = Instant::now();
        let mut q = ToastQueue::default();
        q.push_at("bad", ToastKind::Error, t0);
        q.retain_active_at(t0 + Duration::from_millis(3000));
        assert!(!q.is_empty(), "error alive at 3s");
        q.retain_active_at(t0 + Duration::from_millis(4100));
        assert!(q.is_empty());
    }

    #[test]
    fn capacity_evicts_oldest() {
        let t0 = Instant::now();
        let mut q = ToastQueue::default();
        for i in 0..5 {
            q.push_at(format!("t{}", i), ToastKind::Info, t0);
        }
        assert_eq!(q.toasts.len(), 4);
        assert_eq!(q.toasts.front().unwrap().text, "t1", "t0 evicted");
    }

    #[test]
    fn opacity_full_then_fades_to_zero() {
        let t0 = Instant::now();
        let toast = Toast { text: "x".into(), kind: ToastKind::Info, created: t0 };
        assert_eq!(ToastQueue::opacity(&toast, t0 + Duration::from_millis(2000)), 1.0);
        let mid = ToastQueue::opacity(&toast, t0 + Duration::from_millis(2300));
        assert!(mid > 0.0 && mid < 1.0, "fading at 2.3s, got {}", mid);
        assert_eq!(ToastQueue::opacity(&toast, t0 + Duration::from_millis(2500)), 0.0);
    }
}
