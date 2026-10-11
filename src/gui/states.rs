//! Save-state slot menu, previews, and timestamp formatting.

use super::*;

pub(super) struct SaveSlotUi {
    pub(super) slot: u8,
    pub(super) preview: Option<SaveStatePreview>,
    pub(super) texture: Option<egui::TextureHandle>,
    pub(super) saving: bool,
    pub(super) save_failed: bool,
}

pub(super) struct SaveSlotCache {
    pub(super) slots: Vec<SaveSlotUi>,
}

impl SaveSlotCache {
    pub(super) fn from_paths(paths: Vec<(u8, PathBuf)>) -> Self {
        let slots = paths
            .into_iter()
            .map(|(slot, path)| SaveSlotUi {
                slot,
                preview: read_save_state_preview(&path),
                texture: None,
                saving: false,
                save_failed: false,
            })
            .collect();
        SaveSlotCache { slots }
    }

    pub(super) fn mark_saving(&mut self, slot: u8) {
        if let Some(slot) = self.slot_mut(slot) {
            slot.saving = true;
            slot.save_failed = false;
        }
    }

    pub(super) fn mark_saved(&mut self, slot: u8, preview: SaveStatePreview) {
        if let Some(slot) = self.slot_mut(slot) {
            slot.preview = Some(preview);
            slot.texture = None;
            slot.saving = false;
            slot.save_failed = false;
        }
    }

    pub(super) fn mark_failed(&mut self, slot: u8) {
        if let Some(slot) = self.slot_mut(slot) {
            slot.saving = false;
            slot.save_failed = true;
        }
    }

    pub(super) fn slot_mut(&mut self, slot: u8) -> Option<&mut SaveSlotUi> {
        self.slots.iter_mut().find(|entry| entry.slot == slot)
    }
}

pub(super) fn request_save_state(
    sender: &mpsc::Sender<GBEvent>,
    save_slots: &mut SaveSlotCache,
    slot: u8,
    latest_frame: &Option<Arc<Vec<u8>>>,
) {
    save_slots.mark_saving(slot);
    let thumbnail = latest_frame.as_ref().map(Arc::clone);
    if sender.send(GBEvent::SaveState { slot, thumbnail }).is_err() {
        save_slots.mark_failed(slot);
    }
}

pub(super) fn show_states_menu(
    ui: &mut egui::Ui,
    sender: &mpsc::Sender<GBEvent>,
    save_slots: &mut SaveSlotCache,
    latest_frame: &Option<Arc<Vec<u8>>>,
) {
    ui.set_min_width(320.0);

    let mut load_requested = None;
    let mut save_requested = None;

    for slot in &mut save_slots.slots {
        let load_enabled = slot.preview.is_some() && !slot.saving;
        let status = slot_status_text(slot);
        let color = slot_status_color(slot);
        let slot_number = slot.slot;

        let row = ui.horizontal(|ui| {
            let slot_response = ui.add_sized(
                [58.0, 22.0],
                egui::Label::new(egui::RichText::new(format!("Slot {}", slot_number)).strong()),
            );
            let status_response = ui.add_sized(
                [138.0, 22.0],
                egui::Label::new(egui::RichText::new(status).color(color)),
            );

            let load_response = ui.add_enabled(
                load_enabled,
                egui::Button::new("Load").min_size(egui::vec2(52.0, 22.0)),
            );
            let save_response = ui.add(egui::Button::new("Save").min_size(egui::vec2(52.0, 22.0)));
            let hovered = slot_response.hovered()
                || status_response.hovered()
                || load_response.hovered()
                || save_response.hovered();
            let row_rect = slot_response
                .rect
                .union(status_response.rect)
                .union(load_response.rect)
                .union(save_response.rect);

            (
                load_response.clicked(),
                save_response.clicked(),
                hovered,
                row_rect,
            )
        });

        let (load_clicked, save_clicked, row_hovered, row_rect) = row.inner;
        if slot.preview.is_some() && (row_hovered || row.response.hovered()) {
            show_slot_preview_area(ui, slot, row_rect);
        }

        if load_clicked {
            load_requested = Some(slot_number);
        }
        if save_clicked {
            save_requested = Some(slot_number);
        }
    }

    if let Some(slot) = load_requested {
        let _ = sender.send(GBEvent::LoadState(slot));
        ui.close();
    }
    if let Some(slot) = save_requested {
        request_save_state(sender, save_slots, slot, latest_frame);
        ui.close();
    }
}

pub(super) fn slot_status_text(slot: &SaveSlotUi) -> String {
    if slot.saving {
        "Saving...".to_string()
    } else if slot.save_failed {
        "Save failed".to_string()
    } else if let Some(preview) = &slot.preview {
        format_short_timestamp(preview.saved_at_unix_secs)
    } else {
        "Empty".to_string()
    }
}

pub(super) fn slot_status_color(slot: &SaveSlotUi) -> egui::Color32 {
    if slot.save_failed {
        egui::Color32::from_rgb(210, 72, 64)
    } else if slot.preview.is_some() {
        egui::Color32::from_rgb(190, 220, 190)
    } else {
        egui::Color32::GRAY
    }
}

pub(super) fn show_slot_preview_area(ui: &mut egui::Ui, slot: &mut SaveSlotUi, row_rect: egui::Rect) {
    let preview_pos = save_slot_preview_position(ui.ctx(), row_rect);
    egui::Area::new(egui::Id::new(("save-slot-preview", slot.slot)))
        .order(egui::Order::Tooltip)
        .fixed_pos(preview_pos)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                show_slot_preview_contents(ui, slot);
            });
        });
}

pub(super) fn save_slot_preview_position(ctx: &egui::Context, row_rect: egui::Rect) -> egui::Pos2 {
    const PREVIEW_WIDTH: f32 = 260.0;
    const PREVIEW_HEIGHT: f32 = 270.0;
    const GAP: f32 = 10.0;

    let content_rect = ctx.content_rect();
    let fits_left = row_rect.left() - GAP - PREVIEW_WIDTH >= content_rect.left();
    let fits_right = row_rect.right() + GAP + PREVIEW_WIDTH <= content_rect.right();
    let clamp_x = |x: f32| {
        x.clamp(
            content_rect.left(),
            (content_rect.right() - PREVIEW_WIDTH).max(content_rect.left()),
        )
    };
    let clamp_y = |y: f32| {
        y.clamp(
            content_rect.top(),
            (content_rect.bottom() - PREVIEW_HEIGHT).max(content_rect.top()),
        )
    };

    if fits_left {
        egui::pos2(
            row_rect.left() - GAP - PREVIEW_WIDTH,
            clamp_y(row_rect.top()),
        )
    } else if fits_right {
        egui::pos2(row_rect.right() + GAP, clamp_y(row_rect.top()))
    } else {
        let below = row_rect.bottom() + GAP + PREVIEW_HEIGHT <= content_rect.bottom();
        let y = if below {
            row_rect.bottom() + GAP
        } else {
            row_rect.top() - GAP - PREVIEW_HEIGHT
        };
        egui::pos2(clamp_x(row_rect.left()), clamp_y(y))
    }
}

pub(super) fn show_slot_preview_contents(ui: &mut egui::Ui, slot: &mut SaveSlotUi) {
    if let Some(preview) = &slot.preview {
        ui.label(format!("Slot {}", slot.slot));
        ui.label(format_full_timestamp(preview.saved_at_unix_secs));

        if let Some(thumbnail_rgb) = preview.thumbnail_rgb.as_deref() {
            if slot.texture.is_none() {
                let image = egui::ColorImage::from_rgb(
                    [
                        preview.thumbnail_width as usize,
                        preview.thumbnail_height as usize,
                    ],
                    thumbnail_rgb,
                );
                slot.texture = Some(ui.ctx().load_texture(
                    format!("save-slot-{}-thumbnail", slot.slot),
                    image,
                    egui::TextureOptions::NEAREST,
                ));
            }

            if let Some(texture) = &slot.texture {
                ui.add(
                    egui::Image::from_texture(texture)
                        .max_width(240.0)
                        .max_height(216.0),
                );
            }
        } else {
            ui.label("No preview available");
        }
    }
}

pub(super) fn format_short_timestamp(saved_at_unix_secs: u64) -> String {
    let datetime = local_datetime(saved_at_unix_secs);
    format!(
        "{} {} {:02}:{:02} {}",
        month_abbrev(datetime.month()),
        datetime.day(),
        hour12(datetime.hour()),
        datetime.minute(),
        period(datetime.hour())
    )
}

pub(super) fn format_full_timestamp(saved_at_unix_secs: u64) -> String {
    let datetime = local_datetime(saved_at_unix_secs);
    format!(
        "{} {}, {} {:02}:{:02}:{:02} {}",
        month_abbrev(datetime.month()),
        datetime.day(),
        datetime.year(),
        hour12(datetime.hour()),
        datetime.minute(),
        datetime.second(),
        period(datetime.hour())
    )
}

pub(super) fn local_datetime(unix_secs: u64) -> OffsetDateTime {
    let unix_secs = i64::try_from(unix_secs).unwrap_or(i64::MAX);
    let datetime =
        OffsetDateTime::from_unix_timestamp(unix_secs).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    datetime.to_offset(offset)
}

pub(super) fn month_abbrev(month: Month) -> &'static str {
    match month {
        Month::January => "Jan",
        Month::February => "Feb",
        Month::March => "Mar",
        Month::April => "Apr",
        Month::May => "May",
        Month::June => "Jun",
        Month::July => "Jul",
        Month::August => "Aug",
        Month::September => "Sep",
        Month::October => "Oct",
        Month::November => "Nov",
        Month::December => "Dec",
    }
}

pub(super) fn hour12(hour: u8) -> u8 {
    match hour % 12 {
        0 => 12,
        hour => hour,
    }
}

pub(super) fn period(hour: u8) -> &'static str {
    if hour < 12 {
        "AM"
    } else {
        "PM"
    }
}
