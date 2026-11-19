#[derive(Clone, Copy, Debug, PartialEq)]
enum Act {
    Copy,
    Cut,
    Paste,
    SelectAll,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {

    Crumb,

    Editor,
}

#[derive(Default)]
pub struct CtxMenu {

    registry: Vec<(egui::Id, egui::Rect, Kind)>,

    menu: Option<(egui::Id, egui::Pos2, Kind)>,

    menu_rect: Option<egui::Rect>,

    pending: Option<(egui::Id, Act)>,

    submit_guard: bool,

    paste_text: Option<String>,

    snapshot: Option<(egui::Id, egui::text_selection::CCursorRange)>,
}

static AUTO_REG: std::sync::Mutex<Vec<(egui::Id, egui::Rect)>> = std::sync::Mutex::new(Vec::new());

pub(crate) fn register_auto(id: egui::Id, rect: egui::Rect) {
    if let Ok(mut q) = AUTO_REG.lock() {
        q.push((id, rect));
    }
}

impl CtxMenu {

    pub fn register(&mut self, id: egui::Id, rect: egui::Rect) {
        self.registry.push((id, rect, Kind::Crumb));
    }

    pub fn register_plain(&mut self, id: egui::Id, rect: egui::Rect) {
        self.registry.push((id, rect, Kind::Editor));
    }

    pub fn suppress_submit(&self) -> bool {
        self.pending.is_some() || self.submit_guard
    }

    fn restore_snapshot(&mut self, ctx: &egui::Context, id: egui::Id) {
        let Some((sid, snap)) = self.snapshot else {
            return;
        };
        if sid != id {
            return;
        }
        if let Some(mut s) = egui::widgets::text_edit::TextEditState::load(ctx, id) {
            s.cursor.set_char_range(Some(snap));
            s.store(ctx, id);
        }
    }

    pub fn frame_start(&mut self, ctx: &egui::Context) {

        self.submit_guard = false;

        if let Ok(mut q) = AUTO_REG.lock() {
            self.registry
                .extend(q.drain(..).map(|(id, r)| (id, r, Kind::Crumb)));
        }

        let clear = !ctx.input(|i| i.pointer.any_down());
        if clear && let Some((id, act)) = self.pending.take() {
            self.submit_guard = true;

            ctx.memory_mut(|m| m.request_focus(id));
            match act {
                Act::Copy => {
                    self.restore_snapshot(ctx, id);
                    ctx.input_mut(|i| i.events.push(egui::Event::Copy));
                }
                Act::Cut => {
                    self.restore_snapshot(ctx, id);
                    ctx.input_mut(|i| i.events.push(egui::Event::Cut));
                }
                Act::Paste => {

                    self.restore_snapshot(ctx, id);
                    let text = self.paste_text.take().unwrap_or_default();
                    ctx.input_mut(|i| i.events.push(egui::Event::Paste(text)));
                }
                Act::SelectAll => {

                    ctx.input_mut(|i| {
                        i.events.push(egui::Event::Key {
                            key: egui::Key::A,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers {
                                ctrl: true,
                                command: true,
                                ..egui::Modifiers::default()
                            },
                        })
                    });
                }
            }
        }

        let press = ctx.input_mut(|i| {
            let mut pos = None;
            i.events.retain(|e| match e {
                egui::Event::PointerButton {
                    button: egui::PointerButton::Secondary,
                    pressed: true,
                    pos: p,
                    ..
                } => {
                    pos = Some(*p);
                    false
                }
                _ => true,
            });
            pos
        });
        if let Some(pos) = press {
            let hit = self
                .registry
                .iter()
                .find(|(_, r, _)| r.contains(pos))
                .map(|(id, _, kind)| (*id, *kind));
            if let Some((id, kind)) = hit {

                self.snapshot = egui::widgets::text_edit::TextEditState::load(ctx, id)
                    .and_then(|s| s.cursor.char_range())
                    .map(|r| (id, r));
                self.menu = Some((id, pos, kind));

                self.submit_guard = true;
            } else {
                self.menu = None;
            }
        }

        if let Some((id, ..)) = self.menu {
            let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
            let p = ctx.input(|i| i.pointer.latest_pos()).unwrap_or_default();
            let primary = ctx.input(|i| i.pointer.primary_pressed());
            let released = ctx.input(|i| i.pointer.primary_released());
            let inside = self.menu_rect.is_none_or(|r| r.contains(p));
            if esc || (primary && !inside) {
                self.menu = None;
                self.restore_snapshot(ctx, id);
            } else {

                if primary || released {
                    self.submit_guard = true;
                }
                self.restore_snapshot(ctx, id);
                ctx.memory_mut(|m| m.request_focus(id));
            }
        }

        self.registry.clear();
    }

    pub fn frame_end(&mut self, ctx: &egui::Context) {
        let Some((id, pos, kind)) = self.menu else {
            self.menu_rect = None;
            return;
        };

        if !self.registry.iter().any(|(rid, _, _)| *rid == id) {
            self.menu = None;
            self.menu_rect = None;
            self.snapshot = None;
            return;
        }
        let mut clicked: Option<Act> = None;
        let mut rect: Option<egui::Rect> = None;
        egui::Area::new(egui::Id::unique("ifree_ctx_menu"))
            .fixed_pos(pos)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(120.0);
                    let items: &[(&str, Act)] = match kind {
                        Kind::Crumb => &[
                            (crate::i18n::t("复制选中"), Act::Copy),
                            (crate::i18n::t("剪切"), Act::Cut),
                            (crate::i18n::t("粘贴"), Act::Paste),
                            (crate::i18n::t("全选"), Act::SelectAll),
                        ],
                        Kind::Editor => &[
                            (crate::i18n::t_ctx("复制", "Copy"), Act::Copy),
                            (crate::i18n::t_ctx("剪切", "Cut"), Act::Cut),
                            (crate::i18n::t_ctx("粘贴", "Paste"), Act::Paste),
                        ],
                    };
                    for &(label, act) in items {
                        let r = ui.selectable_label(false, label);
                        if r.clicked() {
                            if act == Act::Paste {
                                self.paste_text = arboard::Clipboard::new()
                                    .and_then(|mut c| c.get_text())
                                    .ok();
                            }
                            clicked = Some(act);
                            ui.close();
                        }
                    }
                    rect = Some(ui.min_rect());
                });
            });
        if let Some(act) = clicked {
            self.pending = Some((id, act));
            self.menu = None;
        }
        self.menu_rect = rect;
    }
}
