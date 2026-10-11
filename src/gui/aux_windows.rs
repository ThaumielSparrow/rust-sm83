//! Secondary OS windows: keybindings and cheats.

use super::*;

/// Which tab of the Keybindings window is visible.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BindTab {
    Keyboard,
    Controller,
}

/// Uniform width for binding-value buttons so rows in the keybindings grids line up.
pub(super) const BIND_BUTTON_WIDTH: f32 = 130.0;

/// Fixed logical width of the keybindings window. Width is not auto-sized:
/// full-width widgets (separators, wrapped labels) track the available width,
/// so measuring it would chase the window size (see the height-only measure
/// in `draw_bind_window`).
pub(super) const BIND_WINDOW_WIDTH: f32 = 440.0;

/// Read-only reference of fixed keyboard system hotkeys, shown in the
/// keybindings window's Keyboard tab so they're discoverable in-app.
pub(super) const SYSTEM_HOTKEY_HELP: &[(&str, &str)] = &[
    ("F1–F4", "Save State 1–4"),
    ("F5–F8", "Load State 1–4"),
    ("F9", "FPS overlay"),
    ("F11", "Fullscreen"),
    ("Shift", "Turbo (hold)"),
    ("T", "Turbo (toggle)"),
    ("Backspace", "Rewind (hold)"),
    ("\\", "Rewind (step back)"),
    ("Y", "Linear interpolation"),
    ("P", "Pause"),
    ("M", "Mute"),
    ("Ctrl+R", "Reset"),
    ("Esc", "Exit (double-press)"),
];

/// A small auxiliary OS window (keybindings, cheats) with its own GL context
/// and egui instance, height-fit to its content so it never has to squeeze
/// into the game window at small scales.
pub(super) struct AuxWindow {
    pub(super) window: winit::window::Window,
    pub(super) display: glium::Display<glium::glutin::surface::WindowSurface>,
    pub(super) egui_glium: egui_glium::EguiGlium,
    /// Last height requested to fit content; avoids re-requesting every frame.
    pub(super) requested_height: Option<f32>,
}

impl AuxWindow {
    pub(super) fn open(event_loop: &ActiveEventLoop, title: &str, width: f32, initial_height: u32) -> AuxWindow {
        let (window, display) = glium::backend::glutin::SimpleWindowBuilder::new()
            .with_title(title)
            .with_inner_size(width as u32, initial_height)
            .build(event_loop);
        let egui_glium =
            egui_glium::EguiGlium::new(egui::ViewportId::ROOT, &display, &window, &event_loop);
        window.request_redraw();
        AuxWindow { window, display, egui_glium, requested_height: None }
    }

    /// Run one egui frame (`ui_fn` draws and returns the measured content
    /// height), paint, honor egui repaint requests, and fit the window height
    /// to the content. Width stays fixed. Only re-requests a size when the
    /// target changes so a denied/clamped request can't loop.
    pub(super) fn draw(&mut self, width: f32, mut ui_fn: impl FnMut(&egui::Context) -> f32) {
        let mut content_height: f32 = 0.0;
        self.egui_glium.run(&self.window, |ctx| {
            content_height = ui_fn(ctx);
        });
        let mut target = self.display.draw();
        target.clear_color(0.1, 0.1, 0.1, 1.0);
        self.egui_glium.paint(&self.display, &mut target);
        let _ = target.finish();
        // Nothing else drives redraws of this window, so honor egui's repaint
        // requests (collapsing-header animation, etc.) ourselves.
        if self.egui_glium.egui_ctx().has_requested_repaint() {
            self.window.request_redraw();
        }
        let desired = (content_height + 24.0).clamp(120.0, 800.0);
        if self.requested_height.is_none_or(|h| (h - desired).abs() > 2.0) {
            self.requested_height = Some(desired);
            let _ = self
                .window
                .request_inner_size(winit::dpi::LogicalSize::new(width, desired));
        }
    }
}

/// Renders the keybindings UI into its own OS window and resizes the window
/// to fit the active tab's content.
pub(super) fn draw_bind_window(bw: &mut AuxWindow, phase: &mut RootPhase, gilrs: Option<&Gilrs>) {
    let RootPhase::Running(running) = phase
    else {
        return;
    };
    let Running { keybindings,
        capturing,
        gamepad_bindings,
        resolved_gamepad,
        gamepad_capturing,
        bind_tab,
        .. } = &mut **running;
    let gilrs_available = gilrs.is_some();
    let pad_name: Option<String> =
        gilrs.and_then(|g| g.gamepads().next().map(|(_, p)| p.name().to_string()));
    bw.draw(BIND_WINDOW_WIDTH, |ctx| {
        let mut content_height: f32 = 0.0;
        egui::CentralPanel::default().show(ctx, |ui| {
            // CentralPanel expands to fill the window; measure a child scope
            // (like the loader screen) so the resize below tracks content.
            let content = ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(bind_tab, BindTab::Keyboard, "Keyboard");
                    ui.selectable_value(bind_tab, BindTab::Controller, "Controller");
                });
                ui.separator();
                let keys = [rust_gbe::KeypadKey::A, rust_gbe::KeypadKey::B, rust_gbe::KeypadKey::Start, rust_gbe::KeypadKey::Select,
                    rust_gbe::KeypadKey::Up, rust_gbe::KeypadKey::Down, rust_gbe::KeypadKey::Left, rust_gbe::KeypadKey::Right];
                let key_label = |k: rust_gbe::KeypadKey| match k { rust_gbe::KeypadKey::A=>"A", rust_gbe::KeypadKey::B=>"B", rust_gbe::KeypadKey::Start=>"Start", rust_gbe::KeypadKey::Select=>"Select", rust_gbe::KeypadKey::Up=>"Up", rust_gbe::KeypadKey::Down=>"Down", rust_gbe::KeypadKey::Left=>"Left", rust_gbe::KeypadKey::Right=>"Right" };
                match *bind_tab {
                    BindTab::Keyboard => {
                        ui.label("Click a binding, then press a key (Esc to cancel capture). Reserved keys can't be used.");
                        ui.add_space(4.0);
                        egui::Grid::new("kb_bind_grid").num_columns(3).spacing([12.0, 4.0]).show(ui, |ui| {
                            for k in keys {
                                ui.label(key_label(k));
                                let active = matches_capturing(*capturing, k);
                                let val = binding_value(keybindings, k);
                                let conflict = is_reserved_key_name(&val);
                                let label = if active { "(press key)".to_string() } else { val.clone() };
                                let mut button = egui::Button::new(label).min_size(egui::vec2(BIND_BUTTON_WIDTH, 0.0));
                                if conflict {
                                    button = button.fill(egui::Color32::from_rgb(100,0,0));
                                }
                                if ui.add(button).clicked() {
                                    *capturing = Some(k);
                                    *gamepad_capturing = None; // keyboard capture replaces any gamepad capture
                                }
                                if conflict {
                                    ui.colored_label(egui::Color32::RED, "Conflicts with system keybind");
                                }
                                ui.end_row();
                            }
                        });
                        if capturing.is_some() && ui.button("Cancel Capture").clicked() { *capturing=None; }
                        ui.add_space(4.0);
                        egui::CollapsingHeader::new("System hotkeys (fixed)")
                            .default_open(false)
                            .show(ui, |ui| {
                                egui::Grid::new("kb_hotkey_grid")
                                    .num_columns(2)
                                    .spacing([12.0, 4.0])
                                    .show(ui, |ui| {
                                        for (keys, action) in SYSTEM_HOTKEY_HELP {
                                            ui.label(*keys);
                                            ui.label(*action);
                                            ui.end_row();
                                        }
                                    });
                            });
                    }
                    BindTab::Controller => {
                        match (&pad_name, gilrs_available) {
                            (Some(n), _) => { ui.label(format!("Connected: {}", n)); }
                            (None, true) => { ui.colored_label(egui::Color32::GRAY, "No controller detected"); }
                            (None, false) => { ui.colored_label(egui::Color32::GRAY, "Controller unavailable"); }
                        }
                        ui.label("Click a binding, then press a controller button (Esc to cancel capture).");
                        ui.add_space(4.0);
                        egui::Grid::new("pad_bind_grid").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                            for k in keys {
                                ui.label(key_label(k));
                                let name = crate::gamepad::keypad_key_name(k);
                                let active = matches!(gamepad_capturing, Some(BindTarget::Gb(g)) if *g == k);
                                let bound = gamepad_bindings.buttons.get(name).cloned().unwrap_or_else(|| "Unbound".to_string());
                                let label = if active { "(press button)".to_string() } else { bound };
                                if ui.add(egui::Button::new(label).min_size(egui::vec2(BIND_BUTTON_WIDTH, 0.0))).clicked() {
                                    *gamepad_capturing = Some(BindTarget::Gb(k));
                                    *capturing = None; // gamepad capture replaces any keyboard capture
                                }
                                ui.end_row();
                            }
                        });
                        ui.add_space(4.0);
                        egui::CollapsingHeader::new("System hotkeys").default_open(false).show(ui, |ui| {
                            egui::Grid::new("pad_hotkey_grid").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                                for action in crate::gamepad::HotkeyAction::all() {
                                    ui.label(action.label());
                                    let active = matches!(gamepad_capturing, Some(BindTarget::Hotkey(a)) if *a == action);
                                    let bound = gamepad_bindings.hotkeys.get(action.name()).cloned().unwrap_or_else(|| "Unbound".to_string());
                                    let label = if active { "(press button)".to_string() } else { bound };
                                    if ui.add(egui::Button::new(label).min_size(egui::vec2(BIND_BUTTON_WIDTH, 0.0))).clicked() {
                                        *gamepad_capturing = Some(BindTarget::Hotkey(action));
                                        *capturing = None;
                                    }
                                    ui.end_row();
                                }
                            });
                        });
                        ui.horizontal(|ui| {
                            if gamepad_capturing.is_some() && ui.button("Cancel Controller Capture").clicked() {
                                *gamepad_capturing = None;
                            }
                            if ui.button("Reset Controller Defaults").clicked() {
                                *gamepad_bindings = GamepadBindings::default();
                                *resolved_gamepad = crate::gamepad::resolve(gamepad_bindings);
                                *gamepad_capturing = None;
                                let gb = gamepad_bindings.clone();
                                crate::config::update_config(|c| c.gamepad = gb);
                            }
                        });
                    }
                }
            });
            content_height = content.response.rect.height();
        });
        content_height
    });
}

pub(super) const CHEAT_WINDOW_WIDTH: f32 = 420.0;

/// Persist this game's cheat list and push the enabled pokes to the emulator.
pub(super) fn sync_cheats(sender: &mpsc::Sender<GBEvent>, key: &str, cheats: &[crate::cheats::Cheat]) {
    let key_owned = key.to_string();
    let list = cheats.to_vec();
    crate::config::update_config(move |c| {
        if list.is_empty() {
            c.cheats.remove(&key_owned);
        } else {
            c.cheats.insert(key_owned, list);
        }
    });
    let _ = sender.send(GBEvent::SetCheats(crate::cheats::enabled_pokes(cheats)));
}

/// Renders the cheat editor into its own OS window (AuxWindow shell).
pub(super) fn draw_cheat_window(cw: &mut AuxWindow, phase: &mut RootPhase) {
    let RootPhase::Running(running) = phase
    else {
        return;
    };
    let Running { cheats,
        cheat_key,
        cheat_entry,
        cheat_entry_label,
        cheat_error,
        sender,
        .. } = &mut **running;
    cw.draw(CHEAT_WINDOW_WIDTH, |ctx| {
        let mut height: f32 = 0.0;
        egui::CentralPanel::default().show(ctx, |ui| {
            let content = ui.vertical(|ui| {
                ui.label("GameShark codes (XXYYZZAA). Types 01 and 90–97 supported.");
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Code:");
                    ui.add(egui::TextEdit::singleline(cheat_entry).desired_width(90.0));
                    ui.label("Label:");
                    ui.add(egui::TextEdit::singleline(cheat_entry_label).desired_width(120.0));
                    if ui.button("Add").clicked() {
                        match crate::cheats::parse_code(cheat_entry) {
                            Ok(_) => {
                                cheats.push(crate::cheats::Cheat {
                                    code: crate::cheats::normalize_code(cheat_entry),
                                    label: cheat_entry_label.clone(),
                                    enabled: true,
                                });
                                cheat_entry.clear();
                                cheat_entry_label.clear();
                                *cheat_error = None;
                                sync_cheats(sender, cheat_key, cheats);
                            }
                            Err(e) => *cheat_error = Some(e),
                        }
                    }
                });
                if let Some(err) = *cheat_error {
                    ui.colored_label(egui::Color32::RED, err);
                }
                ui.separator();
                let mut changed = false;
                let mut remove: Option<usize> = None;
                for (i, cheat) in cheats.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut cheat.enabled, "").changed() {
                            changed = true;
                        }
                        ui.monospace(&cheat.code);
                        ui.label(&cheat.label);
                        if ui.button("Delete").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    cheats.remove(i);
                    changed = true;
                }
                if changed {
                    sync_cheats(sender, cheat_key, cheats);
                }
                if cheats.is_empty() {
                    ui.colored_label(egui::Color32::GRAY, "No cheats yet.");
                }
            });
            height = content.response.rect.height();
        });
        height
    });
}

pub(super) fn key_to_string(key: &winit::keyboard::Key<&str>) -> String {
    use winit::keyboard::{Key, NamedKey};
    match key {
        Key::Character(c) => c.to_uppercase(),
        Key::Named(NamedKey::ArrowUp) => "ArrowUp".into(),
        Key::Named(NamedKey::ArrowDown) => "ArrowDown".into(),
        Key::Named(NamedKey::ArrowLeft) => "ArrowLeft".into(),
        Key::Named(NamedKey::ArrowRight) => "ArrowRight".into(),
        Key::Named(NamedKey::Enter) => "Enter".into(),
        Key::Named(NamedKey::Space) => "Space".into(),
        Key::Named(other) => format!("{other:?}"), // fallback to debug name
        _ => "Unknown".into(),
    }
}

pub(super) fn matches_capturing(capturing: Option<rust_gbe::KeypadKey>, k: rust_gbe::KeypadKey) -> bool {
    use rust_gbe::KeypadKey::*;
    match (capturing, k) {
        (Some(A), A)
        | (Some(B), B)
        | (Some(Start), Start)
        | (Some(Select), Select)
        | (Some(Up), Up)
        | (Some(Down), Down)
        | (Some(Left), Left)
        | (Some(Right), Right) => true,
        _ => false,
    }
}
