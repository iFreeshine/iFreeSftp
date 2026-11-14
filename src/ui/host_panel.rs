//! 主机设置面板（C# HostPanel 920×640 1:1）：Header 主机钮打开的窗口内覆盖层。
//!
//! 左列主机卡片网格（顶部 新增/导入，卡片右键 连接/编辑/复制/删除，拖拽排序，
//! 单击卡片进编辑——2026-09-30 C# 定案：点击进编辑不连接），右列 常规/同步/传输
//! 三页签 + 底部 保存/测试连接/关闭 + InfoBar 提醒（保存成功 2.5s 自动关，
//! 测试结果手动关）。
//!
//! 显式保存语义：打开 = 盘上快照，编辑只改内存工作集，保存整表落盘
//! （hosts::replace_all），未保存关闭直接丢弃（C# HostStoreService.Reload 回滚
//! 在 Rust 等价于「从未写盘」，故无需 dirty 标记）。复制条目走 append_to_disk
//! 立即落盘（C# AppendHosts 即存语义，Reload 后仍在）。主机地址变更 → 指纹
//! 失效清空（C# SyncToProfile 同规则）。

use std::sync::mpsc::Receiver;

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2};

use crate::hosts::{AuthMode, HostProfile};
use crate::i18n::{t, t_ctx, tfmt};
use crate::theme;

use super::State;
use super::dialogs::{self, ShellOpts, connect_profile, shell, std_button_fs};
use super::{CursorLayer, cursor_intent};

/// 测试连接回执（G15）：结果 + 主机条目快照；成功带本次确认指纹
/// （None = 存档精确匹配未弹框）。
type TestReceipt = (Result<Option<String>, String>, HostProfile);

// ── 布局令牌（对照 HostPanel.xaml）──────────────────────────
/// 面板尺寸。
const SIZE: (f32, f32) = (920.0, 640.0);
/// 左右卡片间隔（C# 主体 Grid ColumnSpacing 5）。
const CARD_GAP: f32 = 5.0;
/// 左右卡片宽度比（6:4）。
const LEFT_RATIO: f32 = 0.6;
/// 卡片内边距。
const CARD_PAD: f32 = 8.0;
/// 顶部按钮高度（WinUI 控件默认 32；宽度按 icon+文字内容自适应）。
const ADD_BTN_H: f32 = 32.0;
/// 卡片网格单元 + 内缩（ItemsWrapGrid 258×64, 卡片 Margin 4,3）。
const CELL: (f32, f32) = (258.0, 64.0);
const CELL_MARGIN: Vec2 = Vec2::new(4.0, 3.0);
/// 页签条高（TabViewHeaderPadding 0,8,0,0 + 页签 MinHeight 32）。
const TAB_H: f32 = 40.0;
/// 表单控件高 / 行步进（WinUI 控件默认高 32 / RowSpacing 14）。
const ROW_H: f32 = 32.0;
const ROW_STEP: f32 = 46.0;
/// 标签列宽（右对齐）/ 列间距。
const LABEL_W: f32 = 88.0;
const COL_GAP: f32 = 12.0;
/// 页脚：1px 分隔线 + 上距 8 + 按钮高 32 + 底距 10。
const FOOT_H: f32 = 51.0;

/// 主机卡片配色（ThemeDictionaries HostCard*Brush）。
fn host_card_bg(dark: bool, active: bool) -> Color32 {
    match (dark, active) {
        (true, true) => Color32::from_rgb(0x22, 0x31, 0x4A),
        (true, false) => Color32::from_rgb(0x1B, 0x24, 0x30),
        (false, true) => Color32::from_rgb(0xF0, 0xF0, 0xF0),
        (false, false) => Color32::WHITE,
    }
}

/// 选中卡片强调色边框；未选中无边框（2026-09-30 定案）。
fn host_card_border(dark: bool, active: bool) -> Option<Color32> {
    match (dark, active) {
        (true, true) => Some(Color32::from_rgb(0x4C, 0xC2, 0xFF)),
        (false, true) => Some(Color32::from_rgb(0x00, 0x5F, 0xB8)),
        _ => None,
    }
}

/// 页签选中/悬停配色（TabViewItemHeader* 主题资源）。亮色取 Palette 侧
/// （F-11 C 类漂移收口：选中 #DADADA = nav_selected_bg、未选中透明 = nav_rest_bg
/// ——C# 未选中页签底实为 transparent，#F0F0F0/#DEDEDE 是历史漂移，记有意差异）；
/// 暗色 HostPanel 覆写资源专值保留。
fn tab_bg(p: &theme::Palette, dark: bool, selected: bool) -> Color32 {
    match (dark, selected) {
        (true, true) => Color32::from_rgb(0x0D, 0x14, 0x20),
        (true, false) => Color32::from_rgb(0x2C, 0x3B, 0x52),
        (false, true) => p.nav_selected_bg,
        (false, false) => p.nav_rest_bg,
    }
}

/// 页签按下底（HostPanel.xaml ThemeDictionaries
/// TabViewItemHeaderBackgroundPressed：亮 #E4E4E4 / 暗 #22314A）。亮色有意差异
/// 收口 Palette 侧 pressed_bg #E0E0E0（F-11 C 类漂移）；暗色专值保留。
fn tab_bg_pressed(p: &theme::Palette, dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(0x22, 0x31, 0x4A)
    } else {
        p.pressed_bg
    }
}

fn tab_fg(dark: bool) -> Color32 {
    if dark {
        Color32::WHITE
    } else {
        Color32::from_rgb(0x22, 0x22, 0x22)
    }
}

/// InfoBar 提醒底色/强调色（C# InfoBar Severity 主题资源，明暗由 Palette 承载）：
/// 底 = SystemFillColorSuccessBackground / SystemFillColorCriticalBackground，
/// 字 = SystemFillColorSuccess / SystemFillColorCritical。
fn info_colors(ok: bool, p: &theme::Palette) -> (Color32, Color32) {
    if ok {
        (p.success_bg, p.success_fg)
    } else {
        (p.critical_bg, p.critical)
    }
}

/// 卡片拖拽状态：src = 当前落位下标（扫过即换位，随指针实时更新），
/// grab = 按下点相对卡片左上角偏移（幽灵卡保持相对位置跟随），
/// press = 按下时指针位置，lifted = 越过起拖阈值（防按压抖动误换位）。
pub(crate) struct CardDrag {
    src: usize,
    grab: Vec2,
    press: Pos2,
    lifted: bool,
}

/// C 方案：本平台不保存密码（Linux 本期发版 / macOS 暂不承诺）。
/// 存在非空密码时拦在写盘前，避免 protect 恒 Err 触发「整批不落盘」让用户白点；
/// 给明确降级文案，引导密钥文件或连接时输入。
#[cfg(not(windows))]
fn unsavable_password_on_this_platform(list: &[HostProfile]) -> bool {
    list.iter()
        .any(|h| h.auth_mode == AuthMode::Password && !h.password.is_empty())
}

#[cfg(windows)]
fn unsavable_password_on_this_platform(_list: &[HostProfile]) -> bool {
    false
}

/// 打开面板（Header 主机钮）：盘上快照 + 默认选中第一项。
/// 存档损坏通知（S-10）→ 红色 InfoBar 提示备份路径（C# HostPanel ctor 同款，
/// 手动关，面板每次打开都提示直到下次 load 翻新）。
pub(crate) fn open(st: &mut State) {
    let list = crate::hosts::with(|hs| hs.to_vec());
    let sel = (!list.is_empty()).then_some(0);
    let (orig_host, port_text, ka_text) = sel
        .and_then(|i| list.get(i))
        .map(|h| {
            (
                h.host.clone(),
                h.port.to_string(),
                h.keep_alive_interval_seconds.to_string(),
            )
        })
        .unwrap_or_default();
    let alert = if let Some((backup, reason)) = crate::hosts::load_notice() {
        Some((
            false,
            t("主机配置读取失败").to_owned(),
            tfmt(
                "hosts.json 读取失败（{1}），原文件已备份到 {0}",
                &[&backup, &reason],
            ),
            0.0,
            false,
        ))
    } else if list.iter().any(|h| h.credential_unavailable) {
        Some((
            false,
            t("凭据需重新录入").to_owned(),
            t("部分主机的密码无法在本平台解密，请在密码框重新输入；本次输入仅保存在内存中，不会写入存档").to_owned(),
            0.0,
            false,
        ))
    } else {
        None
    };
    st.dialog = dialogs::Dialog::HostPanel {
        list,
        sel,
        orig_host,
        tab: 0,
        port_text,
        ka_text,
        menu: None,
        alert,
        test_rx: None,
        fp_rx: None,
        testing: false,
        drag: None,
        win_drag: Vec2::ZERO,
    };
}

/// 每帧入口。打开时把 st.dialog 取出为本地值，末尾写回（shell 关闭路径置 None）。
pub(crate) fn draw(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let dialogs::Dialog::HostPanel {
        mut list,
        mut sel,
        mut orig_host,
        mut tab,
        mut port_text,
        mut ka_text,
        mut menu,
        mut alert,
        mut test_rx,
        mut fp_rx,
        mut testing,
        mut drag,
        mut win_drag,
    } = std::mem::replace(&mut st.dialog, dialogs::Dialog::None)
    else {
        return;
    };
    let dark = ui.ctx().theme() == egui::Theme::Dark;
    let now = ui.ctx().input(|i| i.time);

    // G15：测试连接 Interactive 指纹门事件（弹框复用 st.fp_ask_test 覆盖层、日志全局无前缀）。
    if let Some(rx) = &mut fp_rx {
        match rx.try_recv() {
            Ok(crate::ssh::Ev::AskFingerprint {
                first,
                host,
                stored,
                presented,
                reply,
            }) => {
                st.fp_ask_test = Some(dialogs::FpAsk {
                    first,
                    host,
                    stored,
                    presented,
                    reply,
                });
            }
            Ok(crate::ssh::Ev::Log { msg }) => {
                st.terminal.push((crate::local::now_hms(), msg));
                if st.terminal.len() > 400 {
                    st.terminal.drain(0..st.terminal.len() - 400);
                }
            }
            Ok(_) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => fp_rx = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }

    // 测试连接回执（G15：成功带本次确认指纹 → 写回存档 + 工作集同步，同主连接 S1.9）。
    if testing
        && let Some(rx) = &mut test_rx
        && let Ok((res, hp)) = rx.try_recv()
    {
        testing = false;
        fp_rx = None;
        let (ok, title, msg) = match res {
            Ok(confirmed) => {
                // S9：测试连接首次确认的指纹同样写回存档（C# 与主连接同一套落盘时机）。
                if let Some(fp) = &confirmed
                    && hp.host_key_fingerprint != *fp
                {
                    let was_empty = hp.host_key_fingerprint.trim().is_empty();
                    let saved = crate::hosts::save_single_session(
                        &hp.host,
                        hp.port,
                        &hp.username,
                        fp,
                        &hp.last_remote_path,
                    );
                    if let Some(h) = list.iter_mut().find(|h| {
                        h.host == hp.host && h.port == hp.port && h.username == hp.username
                    }) {
                        h.host_key_fingerprint = fp.clone();
                    }
                    let text = match saved {
                        Ok(()) => {
                            if was_empty {
                                t("已记住主机指纹，下次连接自动校验").to_owned()
                            } else {
                                t("已更新主机指纹存档，下次连接自动校验").to_owned()
                            }
                        }
                        // 盘上无此记录（新增未保存）：不谎称「下次自动」，如实提示。
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            t("主机尚未保存进存档，指纹仅本会话生效").to_owned()
                        }
                        Err(e) => tfmt("指纹已在本会话生效，但写入存档失败：{0}", &[&e]),
                    };
                    st.terminal.push((crate::local::now_hms(), text));
                    if st.terminal.len() > 400 {
                        st.terminal.drain(0..st.terminal.len() - 400);
                    }
                }
                (true, t("连接成功").to_owned(), String::new())
            }
            Err(e) => (false, t("连接失败").to_owned(), e),
        };
        alert = Some((ok, title, msg, now, true));
    }

    // 保存成功提醒 2.5s 自动关（测试结果手动关）。
    if let Some((_, _, _, t, auto)) = &alert {
        if *auto && now - *t > 2.5 {
            alert = None;
        } else if *auto {
            let left = (2.5 - (now - *t)).max(0.0);
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(left + 0.05));
        }
    }
    if testing {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(120));
    }

    // 表单数字框每帧过滤非数字（C# PortBox_BeforeTextChanging 拒绝进入）。
    port_text.retain(|c| c.is_ascii_digit());
    ka_text.retain(|c| c.is_ascii_digit());

    // C# HostPanel 无统一内容容器（左右卡片 Border 直接挂 Margin 5 主体 Grid 上），
    // content_card = false，body 即左右卡片直接布局区（不再二次缩边）。
    let mut connect = false;
    let mut close_requested = false;
    let alive = shell(
        ui,
        st,
        p,
        SIZE.0,
        SIZE.1,
        t("主机设置"),
        true,
        4.0,
        false,
        Some(&mut win_drag),
        // C1/F-9：显式保存语义（打开快照、保存整表落盘），未保存工作集不允许
        // 遮罩一次误点丢弃——F19 遮罩不关（Esc/× 仍关，关闭即无提示丢弃，
        // 用户拍板③保持不变）。
        ShellOpts {
            mask_close: false,
            ..Default::default()
        },
        |ui, content, st| {
            host_panel_body(
                ui,
                st,
                p,
                content,
                dark,
                now,
                &mut list,
                &mut sel,
                &mut orig_host,
                &mut tab,
                &mut port_text,
                &mut ka_text,
                &mut menu,
                &mut alert,
                &mut test_rx,
                &mut fp_rx,
                &mut testing,
                &mut drag,
                &mut connect,
                &mut close_requested,
            );
        },
    );
    if !alive {
        return; // 关闭：丢弃工作集（C# Reload 同效，盘上从未被写）
    }
    if connect {
        // 卡片右键「连接」：菜单内已同步 sel，关闭面板并发起连接（st.dialog 已 None）。
        // HostPanel 无侧归属 → 恒落右窗格（侧 1，C# 右侧 HostPicker 语义）。
        let h = list[sel.unwrap_or(0)].clone();
        connect_profile(st, &h, 1, false);
        return;
    }
    if close_requested {
        return; // 不写回 = 关闭
    }
    // 导入钮已在本帧把 st.dialog 切为 Import 向导（draw_left 内 open(st)），
    // 不得用 HostPanel 写回覆盖（同 import_dialog done 先例：换弹窗即不写回）。
    if matches!(st.dialog, dialogs::Dialog::None) {
        st.dialog = dialogs::Dialog::HostPanel {
            list,
            sel,
            orig_host,
            tab,
            port_text,
            ka_text,
            menu,
            alert,
            test_rx,
            fp_rx,
            testing,
            drag,
            win_drag,
        };
    }
}

/// HostPanel 正文（shell 回调，见 shell 文档①②③）。connect / close_requested
/// 由回调桥回 draw 调用点（连接 / 不写回关闭均在 shell 返回后处置）。
#[expect(clippy::too_many_arguments)]
fn host_panel_body(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    content: Rect,
    dark: bool,
    now: f64,
    list: &mut Vec<HostProfile>,
    sel: &mut Option<usize>,
    orig_host: &mut String,
    tab: &mut usize,
    port_text: &mut String,
    ka_text: &mut String,
    menu: &mut Option<(usize, Pos2)>,
    alert: &mut Option<(bool, String, String, f64, bool)>,
    test_rx: &mut Option<Receiver<TestReceipt>>,
    fp_rx: &mut Option<Receiver<crate::ssh::Ev>>,
    testing: &mut bool,
    drag: &mut Option<CardDrag>,
    connect: &mut bool,
    close_requested: &mut bool,
) {
    let body = content;
    let left_w = (body.width() - CARD_GAP) * LEFT_RATIO;
    let left = Rect::from_min_size(body.min, Vec2::new(left_w, body.height()));
    let right = Rect::from_min_max(Pos2::new(left.right() + CARD_GAP, body.top()), body.max);

    draw_left(
        ui, st, p, dark, left, list, sel, orig_host, port_text, ka_text, menu, drag,
    );
    draw_right(
        ui,
        p,
        dark,
        now,
        right,
        list,
        sel,
        orig_host,
        tab,
        port_text,
        ka_text,
        alert,
        test_rx,
        fp_rx,
        testing,
        close_requested,
        st.dialog_fresh,
    );
    if let Some((mi, mpos)) = *menu {
        *connect = draw_card_menu(
            ui, p, now, mpos, mi, list, sel, orig_host, port_text, ka_text, alert, menu,
        );
    }
    draw_alert(ui, st, p, right, alert);
}

/// 选中卡片后同步表单派生值（主机指纹比对基准 / 数字文本框）。
fn sync_sel(
    list: &[HostProfile],
    sel: Option<usize>,
    orig_host: &mut String,
    port_text: &mut String,
    ka_text: &mut String,
) {
    if let Some(h) = sel.and_then(|i| list.get(i)) {
        *orig_host = h.host.clone();
        *port_text = h.port.to_string();
        *ka_text = h.keep_alive_interval_seconds.to_string();
    }
}

// ── 左列：主机卡片网格 ──────────────────────────────────────
#[expect(clippy::too_many_arguments)]
fn draw_left(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    dark: bool,
    area: Rect,
    list: &mut Vec<HostProfile>,
    sel: &mut Option<usize>,
    orig_host: &mut String,
    port_text: &mut String,
    ka_text: &mut String,
    menu: &mut Option<(usize, Pos2)>,
    drag: &mut Option<CardDrag>,
) {
    let painter = ui.painter().clone();
    // C# 卡片容器亮暗主题均无描边（HostPanel Card 未设 BorderBrush）。
    painter.rect_filled(area, 6.0, theme::palette(ui.ctx()).card_bg);

    // 顶部按钮行（居中）：新增 / 导入（icon+文字；C# 为纯图标，按用户要求加文字）。
    let btn_gap = 8.0;
    let lbl_add = t("新增主机");
    let lbl_imp = t("导入主机");
    let text_w = |s: &str| {
        painter
            .layout_no_wrap(s.to_owned(), theme::font_small(), Color32::WHITE)
            .size()
            .x
    };
    // 内容 = 16px 图标 + 6px 间距 + 12px 文字，左右垫 12。
    let w_add = 12.0 + 16.0 + 6.0 + text_w(lbl_add) + 12.0;
    let w_imp = 12.0 + 16.0 + 6.0 + text_w(lbl_imp) + 12.0;
    let bx = area.center().x - (w_add + w_imp + btn_gap) / 2.0;
    let add_r = Rect::from_min_size(
        Pos2::new(bx, area.top() + CARD_PAD),
        Vec2::new(w_add, ADD_BTN_H),
    );
    let imp_r = Rect::from_min_size(
        Pos2::new(bx + w_add + btn_gap, area.top() + CARD_PAD),
        Vec2::new(w_imp, ADD_BTN_H),
    );
    if accent_icon_button(ui, st, p, add_r, "plus", lbl_add) {
        list.push(HostProfile {
            name: t("新主机").to_owned(),
            ..Default::default()
        });
        *sel = Some(list.len() - 1);
        sync_sel(list, *sel, orig_host, port_text, ka_text);
    }
    if accent_icon_button(ui, st, p, imp_r, "arrow-line-right", lbl_imp) {
        // 会话导入向导（WinSCP/FileZilla/PuTTY/OpenSSH/自有存档）。
        super::import_dialog::open(st);
    }

    // 卡片网格（两列，ItemHeight 64 / ItemWidth 258）。
    let grid_l = area.left() + CARD_PAD;
    let grid_t = area.top() + CARD_PAD + ADD_BTN_H + CARD_PAD;
    for (i, h) in list.iter().enumerate() {
        let col = i % 2;
        let row = i / 2;
        let cell = Rect::from_min_size(
            Pos2::new(grid_l + col as f32 * CELL.0, grid_t + row as f32 * CELL.1),
            Vec2::new(CELL.0, CELL.1),
        );
        let card = cell.shrink2(CELL_MARGIN);
        let active = *sel == Some(i);
        // 被拖起的卡片原槽只留淡底占位（实体内容随幽灵卡移动）。
        let lifted_here = matches!(drag.as_ref(), Some(d) if d.lifted && d.src == i);
        if lifted_here {
            painter.rect_filled(card, 6.0, host_card_bg(dark, active).gamma_multiply(0.3));
        } else {
            painter.rect_filled(card, 6.0, host_card_bg(dark, active));
            if let Some(b) = host_card_border(dark, active) {
                painter.rect_stroke(
                    card,
                    6.0,
                    theme::stroke(ui.ctx().pixels_per_point(), 1.0, b),
                    egui::StrokeKind::Inside,
                );
            }
        }
        let resp = ui.allocate_rect(cell, Sense::click_and_drag());
        // 常规箭头手势（用户反馈：抓手/十字手势怪异）；拖拽排序能力保留
        // （按下后仍可拖动重排），pencil 编辑钮上是 PointingHand。
        if resp.hovered() {
            cursor_intent(ui.ctx(), CursorLayer::Hover, egui::CursorIcon::Default);
        }
        if resp.clicked() {
            *sel = Some(i);
            sync_sel(list, *sel, orig_host, port_text, ka_text);
        }
        if resp.secondary_clicked() {
            *menu = Some((i, ui.ctx().pointer_interact_pos().unwrap_or(cell.center())));
        }
        if resp.drag_started() {
            let pos = resp.interact_pointer_pos().unwrap_or(card.min);
            *drag = Some(CardDrag {
                src: i,
                grab: pos - card.min,
                press: pos,
                lifted: false,
            });
        }
        // 被拖起的卡片不画内容与编辑钮（原槽淡出，实体见下方幽灵卡）。
        if lifted_here {
            continue;
        }

        // 内容：系统图标 18 + 名称（SemiBold）/ user@host 副标题 12。
        let icon_c = Pos2::new(card.left() + 10.0 + 9.0, card.center().y);
        let icon_col = if active {
            p.text_primary
        } else {
            p.text_secondary
        };
        if let Some(img) = st.icons.get(ui.ctx(), "monitor", icon_col, 18.0) {
            img.paint_at(ui, Rect::from_center_size(icon_c, Vec2::splat(18.0)));
        }
        let tx = icon_c.x + 9.0 + 10.0;
        painter.text(
            Pos2::new(tx, card.center().y - 9.0),
            Align2::LEFT_CENTER,
            &h.name,
            theme::font_title(false),
            p.text_primary,
        );
        painter.text(
            Pos2::new(tx, card.center().y + 9.0),
            Align2::LEFT_CENTER,
            h.detail(),
            theme::font_small(),
            p.text_secondary,
        );

        // hover 编辑钮（右侧 23×23 pencil = Padding 5 + 图标 13）：点击 = 选中进编辑。
        // 命中区无条件注册（曾仅 resp.hovered 时分配：auto id 随 hover 翻转漂移、
        // hit-test 与卡片主体 click_and_drag 耦合，按下/释放两帧 hover 抖动时
        // 点击会配错相邻卡片）；视觉仍按卡片 hover 显示（C# hover 才现编辑钮）。
        let er = Rect::from_center_size(
            Pos2::new(card.right() - 10.0 - 11.5, card.center().y),
            Vec2::splat(23.0),
        );
        let eresp = ui.allocate_rect(er, Sense::click());
        if resp.hovered() {
            if eresp.hovered() {
                painter.rect_filled(er, 4.0, p.hover_bg);
                // pencil 上是点击进编辑手势，不继承整卡拖拽 Grab。
                cursor_intent(ui.ctx(), CursorLayer::Hover, egui::CursorIcon::PointingHand);
            }
            if let Some(img) = st.icons.get(ui.ctx(), "pencil", p.text_primary, 13.0) {
                img.paint_at(ui, Rect::from_center_size(er.center(), Vec2::splat(13.0)));
            }
        }
        if eresp.clicked() {
            *sel = Some(i);
            sync_sel(list, *sel, orig_host, port_text, ka_text);
        }
    }

    // 拖拽排序（ListView CanReorderItems：拖过即落位 + 工作集同步）。
    // 越过起拖阈值才换位/起视觉（防按压抖动误换位）。
    let released = ui.ctx().input(|i| i.pointer.any_released());
    if released {
        *drag = None;
    } else if let Some(d) = drag.as_mut()
        && let Some(pos) = ui.ctx().pointer_interact_pos()
    {
        if !d.lifted {
            d.lifted = pos.distance(d.press) > 6.0;
        }
        if d.lifted {
            let cols = 2usize;
            let max_row = list.len().saturating_sub(1) / cols;
            let col = (((pos.x - grid_l) / CELL.0).floor() as usize).min(cols - 1);
            let row =
                (((pos.y - grid_t) / CELL.1).floor() as i64).clamp(0, max_row as i64) as usize;
            let t = (row * cols + col).min(list.len() - 1);
            if t != d.src {
                let it = list.remove(d.src);
                list.insert(t, it);
                *sel = Some(t);
                sync_sel(list, *sel, orig_host, port_text, ka_text);
                d.src = t;
            }
            cursor_intent(ui.ctx(), CursorLayer::Drag, egui::CursorIcon::Grabbing);
        }
    }

    // 幽灵卡置顶绘制：卡片被「抓起」的实体——投影 + 强调描边 + 完整内容，
    // 保持按下时相对位置跟随指针；落点槽描边高亮（扫过即落位，src 即落点）。
    if let Some(d) = drag.as_ref()
        && d.lifted
        && let Some(pos) = ui.ctx().pointer_interact_pos()
        && let Some(h) = list.get(d.src)
    {
        let cell = Rect::from_min_size(
            Pos2::new(
                grid_l + (d.src % 2) as f32 * CELL.0,
                grid_t + (d.src / 2) as f32 * CELL.1,
            ),
            Vec2::new(CELL.0, CELL.1),
        );
        painter.rect_stroke(
            cell,
            8.0,
            theme::stroke(ui.ctx().pixels_per_point(), 1.5, theme::ACCENT),
            egui::StrokeKind::Outside,
        );
        let card = Rect::from_min_size(pos - d.grab, cell.shrink2(CELL_MARGIN).size());
        painter.rect_filled(
            card.translate(Vec2::new(0.0, 4.0)),
            6.0,
            Color32::from_black_alpha(60),
        );
        painter.rect_filled(card, 6.0, host_card_bg(dark, true));
        painter.rect_stroke(
            card,
            6.0,
            theme::stroke(ui.ctx().pixels_per_point(), 1.5, theme::ACCENT),
            egui::StrokeKind::Inside,
        );
        // 内容与卡片循环同款：图标 18 + 名称（SemiBold）/ user@host 副标题 12。
        let icon_c = Pos2::new(card.left() + 10.0 + 9.0, card.center().y);
        if let Some(img) = st.icons.get(ui.ctx(), "monitor", p.text_primary, 18.0) {
            img.paint_at(ui, Rect::from_center_size(icon_c, Vec2::splat(18.0)));
        }
        let tx = icon_c.x + 9.0 + 10.0;
        painter.text(
            Pos2::new(tx, card.center().y - 9.0),
            Align2::LEFT_CENTER,
            &h.name,
            theme::font_title(false),
            p.text_primary,
        );
        painter.text(
            Pos2::new(tx, card.center().y + 9.0),
            Align2::LEFT_CENTER,
            h.detail(),
            theme::font_small(),
            p.text_secondary,
        );
    }

    // 空态提示。
    if list.is_empty() {
        painter.text(
            Pos2::new(area.center().x, grid_t + 24.0),
            Align2::CENTER_CENTER,
            t("暂无主机，点 + 新增"),
            theme::font_small(),
            p.text_tertiary,
        );
    }
}

/// 主色图标钮（#0078D4 / hover #106EBE / pressed #005A9E，白色前景，圆角 4）。
/// icon+文字模式（C# AddBtn/ImportBtn 纯图标，用户要求加文字）：内容 =
/// 16px 图标 + 6px 间距 + 12px 文字，水平居中、垂直对齐中线。
fn accent_icon_button(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    r: Rect,
    icon: &str,
    label: &str,
) -> bool {
    let resp = ui.allocate_rect(r, Sense::click());
    // M-12：自绘钮无控件语义，补 widget_info（C# AutomationProperties 同位，
    // 屏幕阅读器 / AccessKit 检视可见「{label}」钮）。
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let bg = dialogs::accent_bg(resp.is_pointer_button_down_on(), resp.hovered(), true, p);
    let painter = ui.painter().clone();
    painter.rect_filled(r, 4.0, bg);
    let tw = painter
        .layout_no_wrap(label.to_owned(), theme::font_small(), Color32::WHITE)
        .size()
        .x;
    let left = r.center().x - (16.0 + 6.0 + tw) / 2.0;
    if let Some(img) = st.icons.get(ui.ctx(), icon, theme::ON_ACCENT_FG, 16.0) {
        img.paint_at(
            ui,
            Rect::from_min_size(Pos2::new(left, r.center().y - 8.0), Vec2::splat(16.0)),
        );
    }
    painter.text(
        Pos2::new(left + 16.0 + 6.0, r.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::font_small(),
        theme::ON_ACCENT_FG,
    );
    resp.clicked()
}

// ── 右列：三页签 + 页脚 ─────────────────────────────────────
#[expect(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
fn draw_right(
    ui: &mut egui::Ui,
    p: &theme::Palette,
    dark: bool,
    now: f64,
    area: Rect,
    list: &mut Vec<HostProfile>,
    sel: &mut Option<usize>,
    orig_host: &mut String,
    tab: &mut usize,
    port_text: &mut String,
    ka_text: &mut String,
    alert: &mut Option<(bool, String, String, f64, bool)>,
    test_rx: &mut Option<Receiver<TestReceipt>>,
    fp_rx: &mut Option<Receiver<crate::ssh::Ev>>,
    testing: &mut bool,
    close: &mut bool,
    // C3②（F-9）：弹窗打开首帧旗标（draw_right 无 State 入参，由 draw 传入）。
    fresh: bool,
) {
    let painter = ui.painter().clone();
    // C# 卡片容器亮暗主题均无描边（HostPanel Card 未设 BorderBrush）。
    painter.rect_filled(area, 6.0, theme::palette(ui.ctx()).card_bg);
    let enabled = sel.is_some();

    // 页签条：常规 / 同步 / 传输（未选中主机时整体禁用）。
    let strip = Rect::from_min_max(area.min, Pos2::new(area.right(), area.top() + TAB_H));
    // 页签起点：TabView 模板合成左 stub 宽 9（TabView col0 2 + ScrollViewer col0 2 +
    // ScrollContentPresenter Padding 左 1 + ItemsPresenter Header stub 4）。
    let mut x = strip.left() + 9.0;
    let mut spans = [(0.0, 0.0); 3];
    for (i, lb) in [t("常规"), t("同步"), t("传输")].iter().enumerate() {
        // 页签顶部偏移 8（TabViewHeaderPadding 0,8,0,0）、高 32（MinHeight）、
        // 宽 = 12px 文字 + 左右 8（TabViewItemHeaderPadding WithoutCloseButton 8,3,8,3）。
        let tw = painter
            .layout_no_wrap((*lb).to_owned(), theme::font_small(), Color32::WHITE)
            .size()
            .x
            + 16.0;
        let tr = Rect::from_min_size(Pos2::new(x, strip.top() + 8.0), Vec2::new(tw, TAB_H - 8.0));
        spans[i] = (tr.left(), tr.right());
        let tresp = ui.allocate_rect(tr, Sense::click());
        if enabled && tresp.clicked() {
            *tab = i;
        }
        let is_sel = *tab == i;
        if is_sel && enabled {
            painter.rect_filled(tr, 4.0, tab_bg(p, dark, true));
            // 底部两角补直（WinUI TabViewItem 上圆下方）。
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(tr.left(), tr.bottom() - 4.0),
                    Pos2::new(tr.right(), tr.bottom()),
                ),
                0.0,
                tab_bg(p, dark, true),
            );
        } else if enabled && tresp.is_pointer_button_down_on() {
            // 按下（CommonStates Pressed：TabViewItemHeaderBackgroundPressed）。
            painter.rect_filled(tr, 4.0, tab_bg_pressed(p, dark));
        } else if enabled && tresp.hovered() {
            painter.rect_filled(tr, 4.0, tab_bg(p, dark, false));
        }
        // 前景（TabViewItemHeaderForeground* 系统值）：禁用 Disabled / 选中 =
        // HostPanel 覆写 …ForegroundSelected / 按下 = …Pressed（Tertiary）/
        // 其余（常态·悬停同值）= …Foreground（Secondary）。
        let fg = if !enabled {
            p.text_disabled
        } else if is_sel {
            tab_fg(dark)
        } else if tresp.is_pointer_button_down_on() {
            p.text_tertiary
        } else {
            p.text_secondary
        };
        painter.text(
            tr.center(),
            Align2::CENTER_CENTER,
            *lb,
            theme::font_small(),
            fg,
        );
        x += tw;
    }
    // 底线：WinUI 合成 = 模板 stubs（左 9px：TabView col0 2 + ScrollViewer col0 2 +
    // ScrollContentPresenter 1 + ItemsPresenter Header stub 4；右：Footer stub 4 +
    // TabView RightBottomBorderLine 到条右缘）+ 非选中页签各自的 BottomBorderLine
    // （TabViewItem 模板，Fill=TabViewBorderBrush=CardStrokeColorDefault，选中系
    // 组合态收起）→ 净效果 = 选中页签段断开的贯穿线。线占条末行 [39,40]。
    let y = strip.bottom() - 0.5;
    let (l0, r0) = spans[*tab];
    for (a, b) in [(strip.left(), l0), (r0, strip.right())] {
        if b > a {
            painter.line_segment(
                [Pos2::new(a, y), Pos2::new(b, y)],
                theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.tab_border),
            );
        }
    }

    // 内容区（ScrollViewer padding 10）。
    let area = Rect::from_min_max(
        Pos2::new(area.left(), strip.bottom()),
        Pos2::new(area.right(), area.bottom() - FOOT_H),
    );
    let pad = 10.0;
    // label 列宽（C# HostPanel.xaml 各表单行 Grid label 列 Width="Auto"，列宽随内容
    // 自适应）：Rust 侧原固定 88，英文长标签（"Connect timeout (s)" 等）右对齐绘制
    // 无裁剪，向左溢出卡片。对齐 Auto 语义 = 取当前页签全部标签的最大文本宽，
    // 下限 88 保中文「密钥文件路径」既有视觉。
    let tab_labels: &[&str] = match *tab {
        0 => &[
            t("项目名称"),
            t("IP 地址"),
            t("端口"),
            t("登录方式"),
            t("用户名"),
            t("密码"),
            t("密钥文件路径"),
        ],
        1 => &[
            t("开启同步"),
            t("开启比较"),
            t("本地同步路径"),
            t("远程同步路径"),
        ],
        _ => &[
            "keep-alive",
            t("间隔秒数"),
            t("连接超时（秒）"),
            t("最大重试次数"),
            t("登录重试延迟（秒）"),
        ],
    };
    let label_w = tab_labels
        .iter()
        .map(|s| {
            painter
                .layout_no_wrap((*s).to_owned(), theme::font_small(), p.text_primary)
                .size()
                .x
        })
        .fold(LABEL_W, f32::max);
    let in_x = area.left() + pad + label_w + COL_GAP;
    let in_w = area.right() - pad - in_x;
    let mut y = area.top() + pad;
    // 标签裁剪区（自适应后一般不触发；超宽兜底 = C# TextBlock 列内裁剪）。
    let label_clip = Rect::from_min_max(
        Pos2::new(in_x - COL_GAP - label_w, area.top()),
        Pos2::new(in_x - COL_GAP, area.bottom()),
    );

    // 表单行：标签（12 右对齐、列内裁剪）+ 单行输入。
    let label = |y: f32, text: &str| {
        painter.with_clip_rect(label_clip).text(
            Pos2::new(in_x - COL_GAP, y + ROW_H / 2.0),
            Align2::RIGHT_CENTER,
            text,
            theme::font_small(),
            p.text_primary,
        );
    };

    if let Some(i) = *sel
        && let Some(h) = list.get_mut(i)
    {
        match *tab {
            0 => {
                // ── 常规 ──
                label(y, t("项目名称"));
                text_row(ui, in_x, y, in_w, &mut h.name, false, true, "hp-name");
                y += ROW_STEP;

                label(y, t("IP 地址"));
                text_row(ui, in_x, y, in_w, &mut h.host, false, true, "hp-host");
                y += ROW_STEP;

                label(y, t("端口"));
                text_row(ui, in_x, y, 150.0, port_text, false, true, "hp-port");
                let port = port_text.parse::<i32>().unwrap_or(h.port);
                h.port = port.clamp(1, 65535);
                y += ROW_STEP;

                label(y, t("登录方式"));
                let mut auth_i = usize::from(h.auth_mode == AuthMode::KeyFile);
                dialogs::card_combo(
                    ui,
                    Rect::from_min_size(Pos2::new(in_x, y), Vec2::new(150.0, ROW_H)),
                    "host-auth",
                    &mut auth_i,
                    &[t("密码"), t("密钥文件")],
                    150.0,
                    theme::FONT_SMALL,
                    true,
                );
                h.auth_mode = if auth_i == 1 {
                    AuthMode::KeyFile
                } else {
                    AuthMode::Password
                };
                y += ROW_STEP;

                label(y, t("用户名"));
                text_row(ui, in_x, y, in_w, &mut h.username, false, true, "hp-user");
                y += ROW_STEP;

                if h.auth_mode == AuthMode::Password {
                    // 密钥方式隐藏密码行（C# UpdateAuthRows）。
                    label(y, t("密码"));
                    text_row(ui, in_x, y, in_w, &mut h.password, true, true, "hp-pass");
                    y += ROW_STEP;
                }

                // 密钥文件路径（只读 + 浏览；行常显）。浏览钮 FontSize 12 / 高 32，
                // 宽 = 文字 + Padding 12,5 左右（22）；框宽 = 输入列 - 钮宽 - ColumnSpacing 8。
                label(y, t("密钥文件路径"));
                let old = h.key_file_path.clone();
                let bw_key = painter
                    .layout_no_wrap(t("浏览").to_owned(), theme::font_small(), Color32::WHITE)
                    .size()
                    .x
                    + 22.0;
                text_row(
                    ui,
                    in_x,
                    y,
                    in_w - bw_key - 8.0,
                    &mut h.key_file_path,
                    false,
                    true,
                    "hp-key",
                );
                h.key_file_path = old; // 只读
                let br = Rect::from_min_size(
                    Pos2::new(in_x + in_w - bw_key, y),
                    Vec2::new(bw_key, ROW_H),
                );
                if std_button_fs(ui, br, t("浏览"), p.text_primary, p, theme::font_small())
                    && let Some(path) = pick_key_file()
                    && h.key_file_path != path
                {
                    h.key_file_path = path;
                }
            }
            1 => {
                // ── 同步 ──
                label(y, t("开启同步"));
                if dialogs::toggle(ui, toggle_rect(in_x, y), h.sync_enabled, true, p) {
                    h.sync_enabled = !h.sync_enabled;
                }
                y += ROW_STEP;

                label(y, t("开启比较"));
                // 文件比对依赖同步浏览（基准根定位），同步关闭时一并禁用。
                if dialogs::toggle(
                    ui,
                    toggle_rect(in_x, y),
                    h.compare_enabled,
                    h.sync_enabled,
                    p,
                ) {
                    h.compare_enabled = !h.compare_enabled;
                }
                y += ROW_STEP;

                label(y, t("本地同步路径"));
                text_row(
                    ui,
                    in_x,
                    y,
                    in_w,
                    &mut h.local_sync_path,
                    false,
                    h.sync_enabled,
                    "hp-sync-local",
                );
                y += ROW_STEP;

                label(y, t("远程同步路径"));
                text_row(
                    ui,
                    in_x,
                    y,
                    in_w,
                    &mut h.remote_sync_path,
                    false,
                    h.sync_enabled,
                    "hp-sync-remote",
                );
            }
            _ => {
                // ── 传输 ──
                label(y, "keep-alive");
                if dialogs::toggle(ui, toggle_rect(in_x, y), h.keep_alive_enabled, true, p) {
                    h.keep_alive_enabled = !h.keep_alive_enabled;
                }
                y += ROW_STEP;

                label(y, t("间隔秒数"));
                text_row(
                    ui,
                    in_x,
                    y,
                    150.0,
                    ka_text,
                    false,
                    h.keep_alive_enabled,
                    "hp-ka",
                );
                let ka = ka_text
                    .parse::<i32>()
                    .unwrap_or(h.keep_alive_interval_seconds);
                h.keep_alive_interval_seconds = ka.clamp(1, 3600);
                y += ROW_STEP;

                y = slider_row(
                    ui,
                    &painter,
                    y,
                    label_clip,
                    in_x,
                    t("连接超时（秒）"),
                    "{0} 秒",
                    &mut h.timeout_seconds,
                    15.0,
                    30.0,
                    5.0,
                    p,
                );
                y = slider_row(
                    ui,
                    &painter,
                    y,
                    label_clip,
                    in_x,
                    t("最大重试次数"),
                    "{0} 次",
                    &mut h.max_retries,
                    1.0,
                    3.0,
                    1.0,
                    p,
                );
                slider_row(
                    ui,
                    &painter,
                    y,
                    label_clip,
                    in_x,
                    t("登录重试延迟（秒）"),
                    "{0} 秒",
                    &mut h.retry_delay_seconds,
                    3.0,
                    10.0,
                    1.0,
                    p,
                );
            }
        }
    } else {
        painter.text(
            area.center(),
            Align2::CENTER_CENTER,
            t("选择左侧主机查看与编辑"),
            theme::font_small(),
            p.text_tertiary,
        );
    }

    // 页脚：顶部分隔线 + 保存（主色）/ 测试连接 / 关闭（居左）。
    let foot_top = area.bottom();
    painter.line_segment(
        [
            Pos2::new(area.left(), foot_top),
            Pos2::new(area.right(), foot_top),
        ],
        theme::stroke(ui.ctx().pixels_per_point(), 1.0, p.separator),
    );
    let by = foot_top + 9.0;
    let text_w = |s: &str| {
        painter
            .layout_no_wrap(s.to_owned(), theme::font_small(), Color32::WHITE)
            .size()
            .x
            + 28.0
    };
    let mut x = area.left() + pad;
    let save_label = t("保存");
    let test_label = if *testing {
        t("连接中…")
    } else {
        t("测试连接")
    };
    let save_r = Rect::from_min_size(Pos2::new(x, by), Vec2::new(text_w(save_label), ROW_H));
    x += save_r.width() + 8.0;
    let test_r = Rect::from_min_size(Pos2::new(x, by), Vec2::new(text_w(test_label), ROW_H));
    x += test_r.width() + 8.0;
    let close_r = Rect::from_min_size(Pos2::new(x, by), Vec2::new(text_w(t("关闭")), ROW_H));

    // C3②（F-9）：打开首帧初焦点 → 页脚主钮「保存」。请求必须在创建之后：
    // egui 新 Area 首帧 = sizing pass（AreaState 未持久化 → ui invisible/disabled），
    // 创建时 create_widget 走 interested_in_focus=false → ignore_focus 会清掉
    // 同帧先置的焦点。
    let save_id = ui.next_auto_id();
    if accent_text_button(ui, p, save_r, save_label) {
        // 保存：补空名 / 去空白 → 钳制 → 指纹失效规则 → 整表落盘。
        for h in &mut *list {
            h.name = h.name.trim().to_owned();
            if h.name.is_empty() {
                h.name = t("新主机").to_owned();
            }
            h.host = h.host.trim().to_owned();
            h.username = h.username.trim().to_owned();
            h.key_file_path = h.key_file_path.trim().to_owned();
            h.local_sync_path = h.local_sync_path.trim().to_owned();
            h.remote_sync_path = h.remote_sync_path.trim().to_owned();
            h.port = h.port.clamp(1, 65535);
            h.keep_alive_interval_seconds = h.keep_alive_interval_seconds.clamp(1, 3600);
            h.timeout_seconds = h.timeout_seconds.clamp(15, 30);
            h.max_retries = h.max_retries.clamp(1, 3);
            h.retry_delay_seconds = h.retry_delay_seconds.clamp(3, 10);
        }
        if let Some(h) = sel.and_then(|i| list.get_mut(i)) {
            if h.host != *orig_host {
                // 主机地址变更 → 旧服务器指纹失效（C# SyncToProfile 同规则）。
                h.host_key_fingerprint.clear();
            }
            *orig_host = h.host.clone();
            *port_text = h.port.to_string();
            *ka_text = h.keep_alive_interval_seconds.to_string();
        }
        // C 方案：非 Windows 写盘前拦截非空密码（protect 恒 Err 会让整批不落盘），
        // 给明确降级文案，不静默、不假成功；H4 的整批不落盘语义一行不动。
        if unsavable_password_on_this_platform(list) {
            // 只同步内存工作集：本次输入的密码可供连接使用，但一个字节都不写盘。
            crate::hosts::replace_working_set(list.clone());
            *alert = Some((
                false,
                t("本平台不保存密码").to_owned(),
                t("请改用密钥文件，或清空密码后在连接时输入；未清空前本平台不会写入存档")
                    .to_owned(),
                now,
                true,
            ));
        } else if let Err(e) = crate::hosts::replace_all(list.clone()) {
            // H4：落盘失败显式报错（任何加密/写盘失败整批不落盘，盘上存档原封不动；
            // 改动仍在工作集，修复后「保存」重试）。
            *alert = Some((
                false,
                tfmt("保存失败：{0}（存档未改动，修复后重试）", &[&e]),
                String::new(),
                now,
                true,
            ));
        } else {
            *alert = Some((true, t("保存成功").to_owned(), String::new(), now, false));
        }
    }
    if fresh {
        ui.ctx().memory_mut(|m| m.request_focus(save_id));
    }
    if std_button_fs(
        ui,
        test_r,
        test_label,
        p.text_primary,
        p,
        theme::font_small(),
    ) && !*testing
        && let Some(h) = sel.and_then(|i| list.get(i).cloned())
    {
        if h.host.trim().is_empty() || h.username.trim().is_empty() {
            *alert = Some((
                false,
                t("请先填写 IP 地址和登录名").to_owned(),
                String::new(),
                now,
                true,
            ));
        } else if h.auth_mode == AuthMode::KeyFile && h.key_file_path.trim().is_empty() {
            *alert = Some((
                false,
                t("密钥登录方式需要先选择密钥文件").to_owned(),
                String::new(),
                now,
                true,
            ));
        } else if h.credential_unavailable && h.password.is_empty() {
            // C 方案：载入时密文已清空，不能拿空密码去撞认证（会被误报成
            // 「用户名或密码错误」）。先在密码框输入本次连接密码即可。
            *alert = Some((
                false,
                t("凭据需重新录入").to_owned(),
                t("请先在密码框输入本次连接密码；该密码仅保存在内存中").to_owned(),
                now,
                true,
            ));
        } else {
            // G15：测试连接走 Interactive 指纹门（C# S9 同一套弹窗/存档语义），不重试；
            // 指纹弹框/日志经 fp_rx 回 UI，最终回执带主机条目快照供落盘同步。
            let (tx, rx) = std::sync::mpsc::channel();
            let (etx, erx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let auth = match h.auth_mode {
                    AuthMode::KeyFile => crate::ssh::Auth::KeyFile(h.key_file_path.clone()),
                    AuthMode::Password => crate::ssh::Auth::Password(h.password.clone()),
                };
                let port = h.port.clamp(1, 65535) as u16;
                let trusted = (!h.host_key_fingerprint.trim().is_empty())
                    .then(|| h.host_key_fingerprint.clone());
                let res = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt.block_on(async {
                        crate::ssh::connect_sftp_interactive(
                            &h.host,
                            port,
                            &h.username,
                            &auth,
                            trusted,
                            etx,
                        )
                        .await
                    }),
                    Err(e) => Err(e.to_string()),
                };
                let _ = tx.send((res, h));
            });
            *fp_rx = Some(erx);
            *test_rx = Some(rx);
            *testing = true;
        }
    }
    if std_button_fs(
        ui,
        close_r,
        t("关闭"),
        p.text_primary,
        p,
        theme::font_small(),
    ) {
        *close = true;
    }
}

/// 开关在输入列的位置（轨道 40×20 垂直居中）。
fn toggle_rect(in_x: f32, y: f32) -> Rect {
    Rect::from_center_size(
        Pos2::new(in_x + 20.0, y + ROW_H / 2.0),
        Vec2::new(40.0, 20.0),
    )
}

/// 表单单行输入（WinUI TextBox 自绘容器，委托 dialogs::winui_text_box：
/// 1px ControlStrokeColorDefault 淡边 / 聚焦 accent 下划线；HostPanel 表单 FontSize 12）。
fn text_row(
    ui: &mut egui::Ui,
    in_x: f32,
    y: f32,
    w: f32,
    val: &mut String,
    secret: bool,
    on: bool,
    key: &'static str,
) {
    dialogs::winui_text_box(
        ui,
        Rect::from_min_size(Pos2::new(in_x, y), Vec2::new(w, ROW_H)),
        egui::Id::unique(key),
        val,
        secret,
        theme::FONT_SMALL,
        on,
        &theme::palette(ui.ctx()),
    );
}

/// 连接滑条行（宽 140 + 当前值文字），返回下一行 y。label_r = 标签裁剪区。
#[expect(clippy::too_many_arguments)]
fn slider_row(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    y: f32,
    label_r: Rect,
    in_x: f32,
    label: &str,
    suffix: &'static str,
    val: &mut i32,
    min: f32,
    max: f32,
    step: f64,
    p: &theme::Palette,
) -> f32 {
    // C# 打开面板直接 TimeoutSlider.Value = TimeoutSeconds（HostPanel.xaml.cs L873），
    // hosts.json 里 6000 等 ms 语义历史脏值被 WPF clamp 收敛到 [min,max]（打开即显示
    // 30 而非 6000）；对齐同行为：读值即收敛（保存侧另有 clamp(15,30) 落盘）。
    *val = (*val as f32).clamp(min, max) as i32;
    let mut v = *val as f32;
    painter.with_clip_rect(label_r).text(
        Pos2::new(in_x - COL_GAP, y + ROW_H / 2.0),
        Align2::RIGHT_CENTER,
        label,
        theme::font_small(),
        p.text_primary,
    );
    // 高 22.5 容纳 18 拇指（SliderHorizontalThumbHeight），WinUI Slider 自绘。
    let rect = Rect::from_center_size(
        Pos2::new(in_x + 70.0, y + ROW_H / 2.0),
        Vec2::new(140.0, 22.5),
    );
    dialogs::winui_slider(ui, rect, &mut v, min, max, step, true, p);
    *val = v as i32;
    painter.text(
        Pos2::new(in_x + 150.0, y + ROW_H / 2.0),
        Align2::LEFT_CENTER,
        tfmt(suffix, &[&v]),
        theme::font_small(),
        p.text_primary,
    );
    y + ROW_STEP
}

/// 主色文字钮（保存）。
fn accent_text_button(ui: &mut egui::Ui, p: &theme::Palette, r: Rect, label: &str) -> bool {
    let resp = ui.allocate_rect(r, Sense::click());
    let bg = dialogs::accent_bg(resp.is_pointer_button_down_on(), resp.hovered(), true, p);
    ui.painter().rect_filled(r, 4.0, bg);
    // C3④（F18/F-9）：键盘焦点框（accent 底恒品牌蓝，只加 2px 焦点框）。
    if resp.has_focus() {
        ui.painter().rect_stroke(
            r,
            4.0,
            theme::stroke(ui.ctx().pixels_per_point(), 2.0, p.focus_stroke),
            egui::StrokeKind::Inside,
        );
    }
    ui.painter().text(
        r.center(),
        Align2::CENTER_CENTER,
        label,
        theme::font_small(),
        theme::ON_ACCENT_FG,
    );
    resp.clicked()
}

/// 卡片右键菜单：连接 / 编辑 / 复制 / 删除（危险红）。
/// 返回 true = 「连接」通过校验（调用方关闭面板并发起连接）。
#[expect(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
fn draw_card_menu(
    ui: &mut egui::Ui,
    p: &theme::Palette,
    now: f64,
    pos: Pos2,
    mi: usize,
    list: &mut Vec<HostProfile>,
    sel: &mut Option<usize>,
    orig_host: &mut String,
    port_text: &mut String,
    ka_text: &mut String,
    alert: &mut Option<(bool, String, String, f64, bool)>,
    menu: &mut Option<(usize, Pos2)>,
) -> bool {
    let full = ui.max_rect();
    let painter = ui.painter().clone();
    let ppp = ui.ctx().pixels_per_point();
    // 菜单几何与文件右键菜单同一套（C# HostPanel CardMenuFlyout 复用 MenuFlyout
    // 默认模板 + PaneMenuPresenterStyle）：宽 200、项高 36、体上下留白 2，无分隔线。
    let mw = dialogs::MENU_W;
    let mh = dialogs::MENU_ITEM_H;
    let mut mr = Rect::from_min_size(pos, Vec2::new(mw, mh * 4.0 + dialogs::MENU_BODY_PAD * 2.0));
    // 屏内钳制。
    let dx = (mr.right() - full.right()).max(0.0) + (full.left() - mr.left()).max(0.0);
    let dy = (mr.bottom() - full.bottom()).max(0.0) + (full.top() - mr.top()).max(0.0);
    mr = mr.translate(egui::vec2(-dx, -dy));

    // 菜单体：软投影 + 不透明底（PaneMenuPresenterStyle 覆写色）+ SurfaceStrokeColorFlyout
    // 描边（1px 设备对齐），圆角 OverlayCornerRadius 8。
    theme::draw_shadow(ui, mr, 8.0, theme::ShadowKind::Flyout);
    painter.rect_filled(mr, 8.0, p.menu_bg);
    painter.rect_stroke(
        theme::snap_rect(mr, ppp),
        8.0,
        Stroke::new(theme::border_width(ppp), p.combo_popup_stroke),
        egui::StrokeKind::Inside,
    );
    // 菜单外点击关闭（先挂，条目后挂盖在其上）。
    if ui.allocate_rect(full, Sense::click()).clicked() {
        *menu = None;
    }
    let items = [
        (t_ctx("连接", "Connect"), false),
        (t("编辑"), false),
        (t("复制"), false),
        (t("删除"), true),
    ];
    for (idx, (text, danger)) in items.iter().enumerate() {
        let ir = Rect::from_min_size(
            // 项外边距 4（分隔线 Margin -4 所抵消的同款），体顶留白 2。
            Pos2::new(
                mr.left() + 4.0,
                mr.top() + dialogs::MENU_BODY_PAD + idx as f32 * mh,
            ),
            Vec2::new(mw - 8.0, mh),
        );
        let resp = ui.allocate_rect(ir, Sense::click());
        // hover 底 = SubtleFillColorSecondary；按下底 = SubtleFillColorTertiary（优先）。
        if resp.is_pointer_button_down_on() {
            painter.rect_filled(ir, 4.0, p.row_pressed);
        } else if resp.hovered() {
            painter.rect_filled(ir, 4.0, p.row_hover);
        }
        // 危险红 = HostPanel.xaml `SystemFillColorCriticalBrush`（亮 #C42B1C / 暗 #FF99A4，
        // 非 FileView MenuItem 工厂的硬编码 #E81123）；hover/按下翻回正文色（模板 VSM 同款）。
        let fg = if *danger && !resp.hovered() {
            p.critical
        } else {
            p.text_primary
        };
        painter.text(
            // 左缩进 11（MenuFlyoutItemThemePadding 左 11）；Padding 上 8 下 9 →
            // 文字中心较几何中心偏上 0.5px。
            Pos2::new(ir.left() + 11.0, ir.center().y - 0.5),
            Align2::LEFT_CENTER,
            *text,
            theme::font_body(),
            fg,
        );
        if resp.clicked() {
            *menu = None;
            match idx {
                0 => {
                    // 连接：校验失败弹提醒，通过则关闭面板发起连接。
                    let h = &list[mi];
                    if h.host.trim().is_empty() || h.username.trim().is_empty() {
                        *alert = Some((
                            false,
                            t("请先填写 IP 地址和登录名").to_owned(),
                            String::new(),
                            now,
                            true,
                        ));
                        *sel = Some(mi);
                    } else if h.auth_mode == AuthMode::KeyFile && h.key_file_path.trim().is_empty()
                    {
                        *alert = Some((
                            false,
                            t("密钥登录方式需要先选择密钥文件").to_owned(),
                            String::new(),
                            now,
                            true,
                        ));
                        *sel = Some(mi);
                    } else {
                        *sel = Some(mi);
                        sync_sel(list, *sel, orig_host, port_text, ka_text);
                        return true;
                    }
                }
                1 => {
                    *sel = Some(mi);
                    sync_sel(list, *sel, orig_host, port_text, ka_text);
                }
                2 => {
                    // 复制：克隆插入其后 + 立即落盘（C# AppendHosts 即存语义；
                    // H4：落盘失败显式报错，副本仍在工作集，「保存」可重试）。
                    let mut c = list[mi].clone();
                    c.name = format!("{} {}", c.name, t("副本"));
                    list.insert(mi + 1, c.clone());
                    *sel = Some(mi + 1);
                    sync_sel(list, *sel, orig_host, port_text, ka_text);
                    if let Err(e) = crate::hosts::append_to_disk(c) {
                        *alert = Some((
                            false,
                            tfmt("复制落盘失败：{0}（副本已加入列表，保存可重试）", &[&e]),
                            String::new(),
                            now,
                            true,
                        ));
                    }
                }
                _ => {
                    // 删除：从工作集移除（不落盘，保存才生效；C# 同语义）。
                    list.remove(mi);
                    *sel = if list.is_empty() {
                        None
                    } else {
                        Some(mi.min(list.len() - 1))
                    };
                    sync_sel(list, *sel, orig_host, port_text, ka_text);
                }
            }
        }
    }
    false
}

/// InfoBar 提醒（footer 上方浮层；成功绿/失败红；手动关 + 保存 2.5s 自动关）。
fn draw_alert(
    ui: &mut egui::Ui,
    st: &mut State,
    p: &theme::Palette,
    right: Rect,
    alert: &mut Option<(bool, String, String, f64, bool)>,
) {
    let Some((ok, title, msg, _, _)) = alert.as_ref() else {
        return;
    };
    let h = if msg.is_empty() { 40.0 } else { 56.0 };
    let r = Rect::from_min_max(
        Pos2::new(right.left() + 20.0, right.bottom() - 61.0 - h),
        Pos2::new(right.right() - 20.0, right.bottom() - 61.0),
    );
    let (bg, fg) = info_colors(*ok, p);
    let painter = ui.painter().clone();
    painter.rect_filled(r, 4.0, bg);
    // 图标：圆点 + 对勾/叹号。
    let ic = Pos2::new(r.left() + 20.0, r.center().y);
    painter.circle_filled(ic, 8.0, fg);
    painter.text(
        ic,
        Align2::CENTER_CENTER,
        if *ok { "✓" } else { "!" },
        theme::font(theme::TextRole::Caption),
        Color32::WHITE,
    );
    painter.text(
        Pos2::new(
            r.left() + 38.0,
            r.top() + if msg.is_empty() { h / 2.0 } else { 14.0 },
        ),
        Align2::LEFT_CENTER,
        title,
        theme::font_body(),
        p.text_primary,
    );
    if !msg.is_empty() {
        let line: String = msg.chars().take(72).collect();
        painter.text(
            Pos2::new(r.left() + 38.0, r.top() + 32.0),
            Align2::LEFT_CENTER,
            line,
            theme::font_small(),
            p.text_secondary,
        );
    }
    // 关闭钮。
    let xr = Rect::from_center_size(Pos2::new(r.right() - 18.0, r.center().y), Vec2::splat(24.0));
    let xresp = ui.allocate_rect(xr, Sense::click());
    if xresp.hovered() {
        ui.painter().rect_filled(xr, 4.0, p.hover_bg);
    }
    if let Some(img) = st.icons.get(ui.ctx(), "x", p.text_primary, 10.0) {
        img.paint_at(ui, Rect::from_center_size(xr.center(), Vec2::splat(10.0)));
    }
    if xresp.clicked() {
        *alert = None;
    }
}

// ── 密钥文件选择（comdlg32 GetOpenFileNameW，零依赖）────────
// 非 Windows 无 comdlg32：入口可见但恒不弹窗（对齐 dialogs.rs pick_exe_file 降级）。

#[cfg(windows)]
#[repr(C)]
struct OpenFileNameW {
    lstruct_size: u32,
    owner: *mut core::ffi::c_void,
    instance: *mut core::ffi::c_void,
    filter: *const u16,
    custom_filter: *mut u16,
    max_custom_filter: u32,
    filter_index: u32,
    file: *mut u16,
    max_file: u32,
    file_title: *mut u16,
    max_file_title: u32,
    initial_dir: *const u16,
    title: *const u16,
    flags: u32,
    file_offset: u16,
    file_extension: u16,
    def_ext: *const u16,
    cust_data: usize,
    hook: *mut core::ffi::c_void,
    template_name: *const u16,
    reserved: *mut core::ffi::c_void,
    reserved_flags: u32,
    flags_ex: u32,
}

#[cfg(windows)]
#[link(name = "comdlg32")]
unsafe extern "system" {
    fn GetOpenFileNameW(ofn: *mut OpenFileNameW) -> i32;
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 密钥文件选择框（.ppk/.pem/.key）；取消或失败返回 None。
#[cfg(windows)]
fn pick_key_file() -> Option<String> {
    let filter = wide(&format!(
        "{}\0*.ppk;*.pem;*.key\0{}\0*.*\0",
        t("密钥文件 (*.ppk;*.pem;*.key)"),
        t("所有文件 (*.*)")
    ));
    let title = wide(t("选择密钥文件"));
    let mut buf = [0u16; 1024];
    let mut ofn = OpenFileNameW {
        lstruct_size: std::mem::size_of::<OpenFileNameW>() as u32,
        owner: std::ptr::null_mut(),
        instance: std::ptr::null_mut(),
        filter: filter.as_ptr(),
        custom_filter: std::ptr::null_mut(),
        max_custom_filter: 0,
        filter_index: 1,
        file: buf.as_mut_ptr(),
        max_file: buf.len() as u32,
        file_title: std::ptr::null_mut(),
        max_file_title: 0,
        initial_dir: std::ptr::null(),
        title: title.as_ptr(),
        flags: 0x1804, // OFN_PATHMUSTEXIST | OFN_FILEMUSTEXIST | OFN_HIDEREADONLY
        file_offset: 0,
        file_extension: 0,
        def_ext: std::ptr::null(),
        cust_data: 0,
        hook: std::ptr::null_mut(),
        template_name: std::ptr::null(),
        reserved: std::ptr::null_mut(),
        reserved_flags: 0,
        flags_ex: 0,
    };
    let ok = unsafe { GetOpenFileNameW(&mut ofn) };
    if ok == 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&buf[..end]))
}

/// 非 Windows 无 comdlg32：恒 None（按钮可见但不弹窗）。
#[cfg(not(windows))]
fn pick_key_file() -> Option<String> {
    None
}
