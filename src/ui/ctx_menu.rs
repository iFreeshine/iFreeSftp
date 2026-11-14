//! 编辑框全局右键菜单（examples/ctx_menu_hook.rs 黑箱验证后的接入版）。
//!
//! 机制（egui 0.37 实测定案）：
//! ① 帧首拦截 Secondary press（raw 队列移除）→ 命中已注册编辑框 rect →
//!    快照 TextEditState 选区（此刻尚未被本帧 show 折叠）→ 开浮层菜单。
//!    raw 拦截同时让 TextEdit 的 pointer_interaction 收不到右键按下，
//!    不会把光标折叠到点击处。
//! ② 菜单开着：逐帧回写快照选区 + request_focus 锁焦——egui 0.37 默认
//!    SurrenderFocusOn::Clicks：click 完成帧（松开帧）any_click() 且本框
//!    非 hovered 时收焦（context.rs:1844-1853），收焦后的帧 show 开头会把
//!    选区折叠成单光标（builder.rs owns_ime_events=has_focus），锁焦 +
//!    回写双保险维持高亮。
//!    Escape / 菜单外左键 → 关菜单并恢复快照选区。
//! ③ 菜单项松开帧：frame_end 记 pending（松开帧 interact 已收焦，state
//!    选区或被折叠，快照在手）；下一帧指针完全无按压时「恢复快照选区 +
//!    注入事件」——
//!    Event::Copy/Cut/Paste 与键盘 Ctrl+C/X/V 同路径（egui-winit lib.rs
//!    :1131-1149 即此转换）；全选注入合成 Ctrl+A，modifiers.command 必须
//!    为 true（cursor_range.rs:122 判定读 command，Modifiers::CTRL 常量
//!    command 位为 false）。无按压帧注入是关键：按压帧 interact 抢先收焦
//!    丢事件，无按压帧不触发 surrender，动作后选区/焦点稳定。
//! 粘贴读剪贴板用 arboard（egui 核心只有写 API，egui-winit 已拉入同版本）。

/// 菜单动作。
#[derive(Clone, Copy, Debug, PartialEq)]
enum Act {
    Copy,
    Cut,
    Paste,
    SelectAll,
}

/// 菜单形态（按注册目标区分项集）。
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// 面包屑编辑框：复制/剪切/粘贴/全选。
    Crumb,
    /// 编辑器编辑区：复制/剪切/粘贴（无「全选」）。
    Editor,
}

/// 编辑框全局右键菜单。
///
/// 每帧调用序：main.rs `frame_start`（先于一切控件）→ 编辑框 show 后
/// [`CtxMenu::register`] → `frame_end`（画浮层菜单）。注册表帧间由
/// frame_start 清空、编辑帧重新填充——编辑态退出（回车提交等）后本帧
/// 无注册，frame_end 据此收掉残留菜单。
#[derive(Default)]
pub struct CtxMenu {
    /// 本帧注册的编辑框（id, rect, 形态）。frame_start 命中测试用的是上一帧
    /// 值（编辑框 rect 帧间稳定），命中后清空待本帧重填。
    registry: Vec<(egui::Id, egui::Rect, Kind)>,
    /// 开着的菜单（目标编辑框 id, 菜单位置, 形态）。
    menu: Option<(egui::Id, egui::Pos2, Kind)>,
    /// 菜单 Area 上一帧 rect（点外关闭判定）。
    menu_rect: Option<egui::Rect>,
    /// 菜单项松开帧（frame_end clicked）设立，下一帧无按压时注入。
    pending: Option<(egui::Id, Act)>,
    /// 提交豁免旗标：菜单项按下/松开帧与动作注入帧置位，供宿主的
    /// lost_focus/Enter 提交分支跳过。egui 0.37 默认 SurrenderFocusOn::
    /// Clicks（input_state/mod.rs:26-32）：收焦发生在 click 完成帧
    /// （松开帧，context.rs:1844-1853 any_click 分支）——插桩实测按下帧
    /// lost_focus=false、松开帧 true，且 pending 到 frame_end 才设立，
    /// 豁免必须提前在 frame_start（show 之前）就位；frame_start 开头
    /// 统一清旗标，只护设置它的那一帧。
    submit_guard: bool,
    /// 点击「粘贴」时同步读到的剪贴板文本。
    paste_text: Option<String>,
    /// 右键帧快照的选区（id, 选区）；动作注入前恢复。
    snapshot: Option<(egui::Id, egui::text_selection::CCursorRange)>,
}

/// 自动注册侧道：拿不到 `CtxMenu` 实例的输入框（dialogs::winui_text_box 等）
/// show 后经 [`register_auto`] 上报 (id, rect)，frame_start 排空并入注册表。
/// 生命周期与 `register()` 一致——命中测试用上一帧 rect（帧间稳定），每帧
/// 重新上报。
static AUTO_REG: std::sync::Mutex<Vec<(egui::Id, egui::Rect)>> = std::sync::Mutex::new(Vec::new());

/// 输入框 show 后自注册右键菜单（复制/剪切/粘贴/全选，Crumb 项集）。
pub(crate) fn register_auto(id: egui::Id, rect: egui::Rect) {
    if let Ok(mut q) = AUTO_REG.lock() {
        q.push((id, rect));
    }
}

impl CtxMenu {
    /// 编辑框 show 后注册真实 id 与占位 rect（response.id 含 id_salt 包装）。
    /// 面包屑形态（含「全选」）。
    pub fn register(&mut self, id: egui::Id, rect: egui::Rect) {
        self.registry.push((id, rect, Kind::Crumb));
    }

    /// 编辑器编辑区注册：无「全选」项。
    pub fn register_plain(&mut self, id: egui::Id, rect: egui::Rect) {
        self.registry.push((id, rect, Kind::Editor));
    }

    /// 注入帧是否抑制宿主的 lost_focus/Enter 提交（见 submit_guard 注释）。
    pub fn suppress_submit(&self) -> bool {
        self.pending.is_some() || self.submit_guard
    }

    /// 快照选区回写（幂等；快照为空或 state 已随编辑框销毁则空操作）。
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

    /// 帧首钩子（先于一切控件）：动作注入 / 右键拦截 / 菜单维持。
    pub fn frame_start(&mut self, ctx: &egui::Context) {
        // 上一帧的提交豁免到此为止：旗标只护设置它的那一帧（show 尚未执行）。
        self.submit_guard = false;

        // 自动注册侧道排空并入注册表（上一帧 register_auto 上报的输入框）：
        // 与 register() 同生命周期——保持到下方命中测试之后、末尾 clear 收走，
        // 本帧编辑帧重新上报/注册。rect 帧间稳定，命中测试用上一帧值同语义。
        if let Ok(mut q) = AUTO_REG.lock() {
            self.registry
                .extend(q.drain(..).map(|(id, r)| (id, r, Kind::Crumb)));
        }

        // pending 动作：推迟到指针完全无按压的帧执行（模块注释③）。
        let clear = !ctx.input(|i| i.pointer.any_down());
        if clear && let Some((id, act)) = self.pending.take() {
            self.submit_guard = true;
            // 焦点保证：builder.rs:654 无焦点不处理事件；request_focus 同帧生效。
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
                    // 点击菜单帧 TextEdit 已被 interact 收焦并折叠 state 里的
                    // 选区 → 注入前恢复快照，否则粘贴只插入不替换。
                    self.restore_snapshot(ctx, id);
                    let text = self.paste_text.take().unwrap_or_default();
                    ctx.input_mut(|i| i.events.push(egui::Event::Paste(text)));
                }
                Act::SelectAll => {
                    // 不恢复快照：全选由 egui 原生写入，恢复旧快照会毁掉它。
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

        // Secondary press 拦截：raw 队列移除 + 命中测试 → 快照 + 开菜单。
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
                // 帧首 state 尚未被本帧 show 折叠 → 快照 = 按下前选区。
                self.snapshot = egui::widgets::text_edit::TextEditState::load(ctx, id)
                    .and_then(|s| s.cursor.char_range())
                    .map(|r| (id, r));
                self.menu = Some((id, pos, kind));
                // 防御性豁免：收焦由 SurrenderFocusOn::Clicks 在 click 完成
                // 帧触发（见 submit_guard 注释），右键按下帧实测不丢焦，
                // 多护一帧无害。
                self.submit_guard = true;
            } else {
                self.menu = None;
            }
        }

        // 菜单开着：逐帧回写快照（高亮保持）+ 锁焦；Escape/点外关闭并恢复。
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
                // 菜单项按下/松开帧都要豁免（插桩实证）：收焦发生在 click
                // 完成帧——selectable_label 的 clicked() 在松开帧才生效，
                // 松开帧 lost_focus=true 而按下帧 false；pending 到
                // frame_end 才设立，两帧的豁免都必须在 frame_start 提前
                // 就位，否则宿主照 lost_focus 提交、编辑态即退。
                if primary || released {
                    self.submit_guard = true;
                }
                self.restore_snapshot(ctx, id);
                ctx.memory_mut(|m| m.request_focus(id));
            }
        }

        // 清注册表，留给本帧编辑框重新注册（编辑态退出 → 空表 → 帧尾收菜单）。
        self.registry.clear();
    }

    /// 帧尾钩子：画浮层菜单、处理点击、记录 menu_rect（供下帧点外判定）。
    pub fn frame_end(&mut self, ctx: &egui::Context) {
        let Some((id, pos, kind)) = self.menu else {
            self.menu_rect = None;
            return;
        };
        // 编辑框已退出（本帧无注册，如回车提交导航）→ 收掉残留菜单。
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
