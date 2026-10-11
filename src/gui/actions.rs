//! System hotkey dispatch and emulator-thread event handling.

use super::*;

/// Side effects of a system action that can't run while `self.phase` is
/// mutably borrowed; the caller applies them after the borrow ends.
#[derive(Default)]
pub(super) struct SysOutcome {
    pub(super) request_reset: bool,
    pub(super) apply_fullscreen: bool,
    /// Keyboard only: the caller has the ROM path and palette it needs.
    pub(super) screenshot: bool,
}

/// Borrowed `Running`-phase fields needed to execute a `SystemAction`.
/// Shared by the keyboard path (window_event) and the gamepad path
/// (poll_gamepad) so dispatch logic exists exactly once.
pub(super) struct SysCtx<'a> {
    pub(super) sender: &'a mpsc::Sender<GBEvent>,
    pub(super) save_slots: &'a mut SaveSlotCache,
    pub(super) latest_frame: &'a Option<Arc<Vec<u8>>>,
    pub(super) renderoptions: &'a mut RenderOptions,
    pub(super) turbo_toggle: &'a mut bool,
    pub(super) turbo_held: &'a mut bool,
    pub(super) volume: &'a mut u8,
    pub(super) pre_mute_volume: &'a mut Option<u8>,
    pub(super) paused: &'a mut bool,
    pub(super) fullscreen: &'a mut bool,
    pub(super) fps_overlay: &'a mut bool,
    pub(super) rewinding: &'a mut bool,
    pub(super) toasts: &'a mut ToastQueue,
}

pub(super) fn handle_system_action(action: crate::input::SystemAction, ctx: &mut SysCtx) -> SysOutcome {
    use crate::input::SystemAction;
    let mut outcome = SysOutcome::default();
    match action {
        SystemAction::SaveState(s) => {
            request_save_state(ctx.sender, ctx.save_slots, s, ctx.latest_frame);
        }
        SystemAction::LoadState(s) => {
            let _ = ctx.sender.send(GBEvent::LoadState(s));
        }
        SystemAction::TurboHold(press) => {
            if press {
                if !*ctx.turbo_toggle && !*ctx.turbo_held {
                    let _ = ctx.sender.send(GBEvent::SpeedUp);
                }
                *ctx.turbo_held = true;
            } else {
                *ctx.turbo_held = false;
                if !*ctx.turbo_toggle {
                    let _ = ctx.sender.send(GBEvent::SpeedDown);
                }
            }
        }
        SystemAction::TurboToggle => {
            *ctx.turbo_toggle = !*ctx.turbo_toggle;
            if *ctx.turbo_toggle {
                if !*ctx.turbo_held {
                    let _ = ctx.sender.send(GBEvent::SpeedUp);
                }
            } else if !*ctx.turbo_held {
                let _ = ctx.sender.send(GBEvent::SpeedDown);
            }
            ctx.toasts.push(
                if *ctx.turbo_toggle { "Turbo on" } else { "Turbo off" },
                ToastKind::Info,
            );
        }
        SystemAction::ToggleInterpolation => {
            let next = ctx.renderoptions.filter.next();
            ctx.renderoptions.filter = next;
            crate::config::update_config(|c| c.screen_filter = next);
            ctx.toasts.push(format!("Filter: {}", next.label()), ToastKind::Info);
        }
        SystemAction::TogglePause => {
            *ctx.paused = !*ctx.paused;
            let _ = ctx.sender.send(GBEvent::SetPaused(*ctx.paused));
        }
        SystemAction::Reset => {
            outcome.request_reset = true;
        }
        SystemAction::ToggleFullscreen => {
            *ctx.fullscreen = !*ctx.fullscreen;
            let fs = *ctx.fullscreen;
            crate::config::update_config(|c| c.fullscreen = fs);
            outcome.apply_fullscreen = true;
        }
        SystemAction::ToggleMute => {
            if let Some(restored) = ctx.pre_mute_volume.take() {
                *ctx.volume = restored;
            } else {
                *ctx.pre_mute_volume = Some(*ctx.volume);
                *ctx.volume = 0;
            }
            let _ = ctx.sender.send(GBEvent::UpdateVolume(perceptual_to_linear(*ctx.volume)));
            let v = *ctx.volume;
            crate::config::update_config(|c| c.volume = v);
            ctx.toasts.push(
                if *ctx.volume == 0 { "Muted" } else { "Volume restored" },
                ToastKind::Info,
            );
        }
        SystemAction::ToggleFpsOverlay => {
            *ctx.fps_overlay = !*ctx.fps_overlay;
            let on = *ctx.fps_overlay;
            crate::config::update_config(|c| c.fps_overlay = on);
        }
        SystemAction::RewindHold(press) => {
            *ctx.rewinding = press;
            let _ = ctx.sender.send(GBEvent::SetRewinding(press));
        }
        SystemAction::RewindStep => {
            let _ = ctx.sender.send(GBEvent::RewindStep);
        }
        SystemAction::Screenshot => {
            outcome.screenshot = true;
        }
    }
    outcome
}

pub(super) fn drain_gui_events(
    receiver: &Receiver<GuiEvent>,
    save_slots: &mut SaveSlotCache,
    toasts: &mut ToastQueue,
    rumble: &mut Option<bool>,
) {
    loop {
        match receiver.try_recv() {
            Ok(GuiEvent::SaveStateSaved { slot, preview }) => {
                save_slots.mark_saved(slot, preview);
                toasts.push(format!("State saved to slot {}", slot), ToastKind::Success);
            }
            Ok(GuiEvent::SaveStateFailed { slot }) => {
                save_slots.mark_failed(slot);
                toasts.push(format!("Save to slot {} failed", slot), ToastKind::Error);
            }
            Ok(GuiEvent::LoadStateResult { slot, error: None }) => {
                toasts.push(format!("State {} loaded", slot), ToastKind::Success);
            }
            Ok(GuiEvent::LoadStateResult { slot, error: Some(e) }) => {
                toasts.push(format!("Couldn't load state {slot}: {e}"), ToastKind::Error);
            }
            Ok(GuiEvent::Rumble(on)) => *rumble = Some(on),
            Ok(GuiEvent::UndoLoadResult { ok: true }) => {
                toasts.push("Load undone", ToastKind::Success);
            }
            Ok(GuiEvent::UndoLoadResult { ok: false }) => {
                toasts.push("Nothing to undo", ToastKind::Info);
            }
            Ok(GuiEvent::BatterySaveFailed) => {
                toasts.push(
                    "Battery save failed — check disk space and permissions",
                    ToastKind::Error,
                );
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => break,
        }
    }
}

/// Release everything an input source may be holding: all Game Boy buttons,
/// momentary turbo and rewind. Used when key-up / button-up events can't
/// arrive (window lost focus, gamepad unplugged).
pub(super) fn release_held_inputs(
    sender: &mpsc::Sender<GBEvent>,
    turbo_toggle: bool,
    turbo_held: &mut bool,
    rewinding: &mut bool,
) {
    use rust_gbe::KeypadKey::*;
    for k in [A, B, Start, Select, Up, Down, Left, Right] {
        let _ = sender.send(GBEvent::KeyUp(k));
    }
    if *turbo_held {
        *turbo_held = false;
        if !turbo_toggle {
            let _ = sender.send(GBEvent::SpeedDown);
        }
    }
    if *rewinding {
        *rewinding = false;
        let _ = sender.send(GBEvent::SetRewinding(false));
    }
}

// Convert 0-100 slider value to linear gain (0.0-1.0) using a perceptual (log-like) curve.
// 50 -> ~0.5 perceived loudness; we map percentage p to gain = (p/100)^(gamma) with gamma ~ 1.5
// This softens high-end changes and gives finer control at low volumes.
pub(super) fn perceptual_to_linear(v: u8) -> f32 {
    let p = (v as f32) / 100.0;
    if p <= 0.0 {
        0.0
    } else {
        p.powf(1.5)
    } // simple gamma curve
}

// Dynamic mapping using current keybindings
pub(super) fn dynamic_winit_to_keypad(
    key: winit::keyboard::Key<&str>,
    physical: &winit::keyboard::PhysicalKey,
    bindings: &KeyBindings,
) -> Option<rust_gbe::KeypadKey> {
    use rust_gbe::KeypadKey::*;
    // Bindings are either physical positions ("Code:KeyX") or, from older
    // configs, logical key names; match either form.
    let logical = key_to_string(&key);
    let physical = crate::input::physical_binding(physical);
    [A, B, Start, Select, Up, Down, Left, Right].into_iter().find(|&k| {
        crate::input::binding_matches(&binding_value(bindings, k), &logical, physical.as_deref())
    })
}
