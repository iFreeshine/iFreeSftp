use std::sync::mpsc::channel;

use egui::{Align2, Pos2, Rect, Sense, Stroke, Vec2};

use crate::hosts::AuthMode;
use crate::i18n::{t, tfmt};
use crate::import::SourceOptions;
use crate::theme::{self, BTN_H};

use super::State;
use super::dialogs::{
    self, ShellOpts, TextMode, accent_button, btn_w, checkbox, dlg_text, shell, std_button,
};

const W: f32 = 780.0;

const H_CONSENT: f32 = 330.0;

const H_PREVIEW: f32 = 525.0;

const PAD: f32 = 16.0;

const PAD_V: f32 = 12.0;

const ROW: f32 = 32.0;

const CB: f32 = 18.0;

const CB_STEP: f32 = 36.0;

pub(crate) struct ImportRow {
    checked: bool,

    enabled: bool,
    name: String,
    host: String,
    port: String,
    user: String,
    auth: String,
    status: String,
}

pub(crate) fn open(st: &mut State) {
    st.dialog = dialogs::Dialog::Import {
        stage: 0,
        consent: false,
        sources: [true; 6],
        scanning: false,
        scan_rx: None,
        sessions: Vec::new(),
        stats: Vec::new(),
        rows: Vec::new(),
        summary: String::new(),
        missing: Vec::new(),
        errors: Vec::new(),
        truncated: 0,
        imported: 0,
    };
}

fn warn_text(code: &str) -> &'static str {
    match code {
        "keyNotFound" => "密钥文件不存在",
        "uncPath" => "UNC 密钥路径，已回退密码",
        _ => "",
    }
}

fn err_text(code: &str) -> &'static str {
    match code {
        "fileTooLarge" => "文件超过 8MB，已跳过",
        "fileReadFailed" => "文件读取失败",
        "registryReadFailed" => "注册表读取失败",
        _ => "解析失败",
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) fn draw(ui: &mut egui::Ui, st: &mut State, p: &theme::Palette) {
    let dialogs::Dialog::Import {
        mut stage,
        mut consent,
        mut sources,
        mut scanning,
        mut scan_rx,
        mut sessions,
        mut stats,
        mut rows,
        mut summary,
        mut missing,
        mut errors,
        mut truncated,
        mut imported,
    } = std::mem::replace(&mut st.dialog, dialogs::Dialog::None)
    else {
        return;
    };

    if scanning
        && let Some(rx) = &mut scan_rx
        && let Ok(res) = rx.try_recv()
    {
        scanning = false;

        let mut store_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
        crate::hosts::with(|hs| {
            for h in hs {
                store_keys.insert(crate::import::dedup_key(&h.host, h.port, &h.username));
            }
        });
        let mut batch_keys = store_keys.clone();
        rows = Vec::new();
        let mut existing = 0usize;
        for s in &res.sessions {
            let key = crate::import::dedup_key(&s.host, s.port, &s.username);

            let in_store = store_keys.contains(&key);
            let dup_batch = !batch_keys.insert(key) && !in_store;
            let exists = in_store || dup_batch;
            if in_store {
                existing += 1;
            }
            let wt = warn_text(s.warning);

            rows.push(ImportRow {
                checked: !exists,
                enabled: !exists,
                name: s.name.clone(),
                host: s.host.clone(),
                port: s.port.to_string(),
                user: s.username.clone(),
                auth: match s.auth_mode {
                    AuthMode::KeyFile => t("密钥文件").to_owned(),
                    AuthMode::Password => t("密码").to_owned(),
                },
                status: if in_store {
                    t("已存在").to_owned()
                } else if dup_batch {
                    t("本次扫描中重复").to_owned()
                } else if !wt.is_empty() {
                    t(wt).to_owned()
                } else {
                    t("新条目").to_owned()
                },
            });
        }

        let no_sessions = res.sessions.is_empty();
        sessions = res.sessions;
        stats = res.stats;

        summary = tfmt(
            "共找到 {0} 条会话，其中 {1} 条已存在（不可重复导入）。",
            &[&sessions.len(), &existing],
        );
        missing = res.missing;
        errors = res.errors;
        truncated = res.total_truncated;
        stage = if no_sessions { 2 } else { 1 };
    }
    if scanning {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(120));
    }

    let detail_n = usize::from(truncated > 0) + errors.len();
    let h = match stage {
        0 => H_CONSENT,
        1 => H_PREVIEW,

        _ => 114.0 + detail_n as f32 * 16.0,
    };

    let mut done = false;
    let alive = shell(
        ui,
        st,
        p,
        W,
        h,
        t("导入会话"),
        true,
        0.0,
        true,
        None,
        ShellOpts::default(),
        |ui, content, st| {
            let area = content.shrink2(Vec2::new(PAD, PAD_V));
            let painter = ui.painter().clone();
            let x = area.left();
            let mut y = area.top();

            match stage {
                0 => {

                    let note = t(
                        "仅在本机读取您所选来源中的连接元数据（名称、主机、端口、用户名、认证方式、密钥路径）；口令与登录后命令一律不导入。扫描结果只在本弹窗内展示，关闭即丢弃。单文件不超过 8MB，单次最多 5000 条。",
                    );
                    dlg_text(
                        &painter,
                        Pos2::new(x, y),
                        note,
                        theme::font_small(),
                        TextMode::Wrap,
                        area.width(),
                        p.text_secondary,
                    );
                    y += 32.0 + 12.0;

                    painter.text(
                        Pos2::new(x, y + 8.0),
                        Align2::LEFT_CENTER,
                        t("选择要扫描的来源"),
                        theme::font_brand_small(),
                        p.text_primary,
                    );
                    y += 16.0 + 12.0;

                    for (i, lb) in [
                        t("WinSCP (INI 文件)"),
                        t("WinSCP (注册表)"),
                        t("FileZilla 站点管理器"),
                        t("PuTTY 会话"),
                        t("OpenSSH (~/.ssh/config)"),
                        t("iFreeSftp 存档 (JSON)"),
                    ]
                    .iter()
                    .enumerate()
                    {
                        let col = i % 2;
                        let row = i / 2;
                        let cx = x + col as f32 * (area.width() / 2.0 + 8.0);
                        let cy = y + row as f32 * CB_STEP;
                        if checkbox(
                            ui,
                            Rect::from_min_size(
                                Pos2::new(cx, cy + (ROW - CB) / 2.0),
                                Vec2::splat(CB),
                            ),
                            sources[i],
                            true,
                            p,
                        ) {
                            sources[i] = !sources[i];
                        }
                        painter.text(
                            Pos2::new(cx + CB + 8.0, cy + ROW / 2.0),
                            Align2::LEFT_CENTER,
                            *lb,
                            theme::font_body(),
                            p.text_primary,
                        );
                    }
                    y += 3.0 * CB_STEP - 4.0 + 12.0;
                    if checkbox(
                        ui,
                        Rect::from_min_size(Pos2::new(x, y + (ROW - CB) / 2.0), Vec2::splat(CB)),
                        consent,
                        true,
                        p,
                    ) {
                        consent = !consent;
                    }
                    painter.text(
                        Pos2::new(x + CB + 8.0, y + ROW / 2.0),
                        Align2::LEFT_CENTER,
                        t("我已知晓并同意按上述方式仅在本机读取所选来源"),
                        theme::font_body(),
                        p.text_primary,
                    );

                    let by = area.bottom() - PAD_V - BTN_H;
                    let scan_lb = if scanning {
                        t("正在扫描…")
                    } else {
                        t("开始扫描")
                    };
                    let scan_w = btn_w(ui, scan_lb, &theme::font_body());
                    let cancel_w = btn_w(ui, t("取消"), &theme::font_body());
                    let cancel_r = Rect::from_min_size(
                        Pos2::new(area.right() - cancel_w, by),
                        Vec2::new(cancel_w, BTN_H),
                    );
                    let scan_r = Rect::from_min_size(
                        Pos2::new(cancel_r.left() - 8.0 - scan_w, by),
                        Vec2::new(scan_w, BTN_H),
                    );
                    if std_button(ui, cancel_r, t("取消"), p.text_primary, p) {
                        done = true;
                    }

                    let scan_id = ui.next_auto_id();
                    if accent_button(
                        ui,
                        scan_r,
                        scan_lb,
                        consent && !scanning,
                        p,
                        theme::font_body(),
                    ) {

                        let opts = SourceOptions { kinds: sources };
                        let (tx, rx) = channel();
                        std::thread::spawn(move || {
                            let _ = tx.send(crate::import::scan_sources(&opts));
                        });
                        scan_rx = Some(rx);
                        scanning = true;
                    }
                    if st.dialog_fresh {
                        ui.ctx().memory_mut(|m| m.request_focus(scan_id));
                    }
                }
                1 => {

                    dlg_text(
                        &painter,
                        Pos2::new(x, y),
                        &summary,
                        theme::font_small(),
                        TextMode::Wrap,
                        area.width(),
                        p.text_primary,
                    );
                    y += 16.0 + 10.0;

                    let cols = preview_cols(x, area.width());
                    for (lb, (cl, _cw)) in [
                        t("名称"),
                        t("主机"),
                        t("端口"),
                        t("用户名"),
                        t("认证"),
                        t("状态"),
                    ]
                    .iter()
                    .zip(cols.iter().skip(1))
                    {
                        painter.text(
                            Pos2::new(*cl, y + 7.5),
                            Align2::LEFT_CENTER,
                            *lb,
                            theme::font(theme::TextRole::Caption),
                            p.text_secondary,
                        );
                    }
                    y += 15.0 + 4.0;
                    painter.line_segment(
                        [Pos2::new(x, y), Pos2::new(area.right(), y)],
                        Stroke::new(1.0, p.divider_stroke),
                    );
                    y += 10.0;

                    let list_h = 320.0f32.min(rows.len() as f32 * ROW);
                    for (shown, r) in rows.iter_mut().enumerate() {
                        let row_y = y + shown as f32 * ROW;
                        if row_y + ROW > y + list_h + 8.0 {
                            break;
                        }
                        let row_r = Rect::from_min_max(
                            Pos2::new(x, row_y),
                            Pos2::new(area.right(), row_y + ROW),
                        );
                        let resp = ui.allocate_rect(row_r, Sense::hover());
                        if resp.hovered() {
                            painter.rect_filled(row_r, 0.0, p.hover_bg);
                        }

                        if checkbox(
                            ui,
                            Rect::from_min_size(
                                Pos2::new(cols[0].0 + 16.0 - CB / 2.0, row_y + (ROW - CB) / 2.0),
                                Vec2::splat(CB),
                            ),
                            r.checked,
                            r.enabled,
                            p,
                        ) && r.enabled
                        {
                            r.checked = !r.checked;
                        }
                        let texts = [&r.name, &r.host, &r.port, &r.user, &r.auth, &r.status];
                        for (txt, (cl, cw)) in texts.iter().zip(cols.iter().skip(1)) {

                            dlg_text(
                                &painter,
                                Pos2::new(*cl, row_y + (ROW - 16.0) / 2.0),
                                txt,
                                theme::font_small(),
                                TextMode::Truncate,
                                cw - 2.0,
                                p.text_primary,
                            );
                        }

                        painter.line_segment(
                            [
                                Pos2::new(x, row_y + ROW),
                                Pos2::new(area.right(), row_y + ROW),
                            ],
                            Stroke::new(1.0, theme::dim50(p.divider_stroke)),
                        );
                    }
                    y += list_h + 10.0;

                    let mut warns: Vec<String> = Vec::new();
                    for (k, code) in &errors {
                        warns.push(format!("{}：{}", k.label(), t(err_text(code))));
                    }
                    if truncated > 0 {
                        warns.push(tfmt(
                            "会话数量超过 5000 条上限，已截断 {0} 条。",
                            &[&truncated],
                        ));
                    }

                    let skipped: usize = stats.iter().map(|s| s.skipped_invalid).sum();
                    if skipped > 0 {
                        warns.push(tfmt(
                            "{0} 条会话无效已跳过（缺少主机名或端口无效）。",
                            &[&skipped],
                        ));
                    }
                    if !warns.is_empty() {
                        dlg_text(
                            &painter,
                            Pos2::new(x, y),
                            &warns.join("\n"),
                            theme::font(theme::TextRole::Caption),
                            TextMode::Wrap,
                            area.width(),
                            p.text_secondary,
                        );
                    }

                    let by = area.bottom() - PAD_V - BTN_H;
                    let n = rows.iter().filter(|r| r.checked && r.enabled).count();
                    let imp_lb = tfmt("导入 ({0})", &[&n]);
                    let imp_w = btn_w(ui, &imp_lb, &theme::font_body());
                    let back_w = btn_w(ui, t("返回"), &theme::font_body());
                    let back_r = Rect::from_min_size(
                        Pos2::new(area.right() - back_w, by),
                        Vec2::new(back_w, BTN_H),
                    );
                    let imp_r = Rect::from_min_size(
                        Pos2::new(back_r.left() - 8.0 - imp_w, by),
                        Vec2::new(imp_w, BTN_H),
                    );
                    if std_button(ui, back_r, t("返回"), p.text_primary, p) {
                        stage = 0;
                    }

                    let imp_id = ui.next_auto_id();
                    let imp = accent_button(ui, imp_r, &imp_lb, n > 0, p, theme::font_body());
                    if imp && n > 0 {

                        let mut have: std::collections::HashSet<String> =
                            crate::hosts::with(|hs| {
                                hs.iter()
                                    .map(|h| crate::import::dedup_key(&h.host, h.port, &h.username))
                                    .collect()
                            });
                        let mut count = 0usize;
                        let mut fail = 0usize;
                        for (r, s) in rows.iter().zip(sessions.iter()) {
                            if r.checked && r.enabled {
                                let key = crate::import::dedup_key(&s.host, s.port, &s.username);
                                if have.insert(key) {

                                    let prof = crate::import::to_profile(s);
                                    crate::hosts::push(prof.clone());
                                    if crate::hosts::append_to_disk(prof).is_err() {
                                        fail += 1;
                                    }
                                    count += 1;
                                }
                            }
                        }
                        imported = count;

                        if count > 0 {
                            st.terminal.push((
                                crate::local::now_hms(),
                                tfmt("已从本机来源导入 {0} 条主机", &[&count]),
                            ));
                        }
                        if fail > 0 {
                            st.terminal.push((
                                crate::local::now_hms(),
                                tfmt(
                                    "导入落盘失败 {0} 条（已加入列表，主机面板「保存」可重试）",
                                    &[&fail],
                                ),
                            ));
                        }
                        stage = 2;
                    }
                    if st.dialog_fresh {
                        ui.ctx().memory_mut(|m| m.request_focus(imp_id));
                    }
                }
                _ => {

                    let text = if imported > 0 {
                        tfmt("已导入 {0} 条会话", &[&imported])
                    } else {
                        t("未找到可导入的新会话").to_owned()
                    };

                    let (glyph, icon_c) = if imported > 0 {
                        ("✓", p.success_fg)
                    } else {
                        ("ⓘ", p.text_secondary)
                    };
                    painter.text(
                        Pos2::new(x + 10.0, y + 14.0),
                        Align2::CENTER_CENTER,
                        glyph,
                        theme::font_result_glyph(),
                        icon_c,
                    );
                    painter.text(
                        Pos2::new(x + 20.0 + 10.0, y + 14.0),
                        Align2::LEFT_CENTER,
                        text,
                        theme::font_title(false),
                        p.text_primary,
                    );

                    let mut dy = y + 28.0 + 14.0;
                    if truncated > 0 {
                        painter.text(
                            Pos2::new(x, dy),
                            Align2::LEFT_CENTER,
                            tfmt("会话数量超过 5000 条上限，已截断 {0} 条。", &[&truncated]),
                            theme::font_small(),
                            p.text_secondary,
                        );
                        dy += 16.0;
                    }
                    for (k, code) in &errors {
                        painter.text(
                            Pos2::new(x, dy),
                            Align2::LEFT_CENTER,
                            format!("{}: {}", k.label(), t(err_text(code))),
                            theme::font_small(),
                            p.text_secondary,
                        );
                        dy += 16.0;
                    }

                    let by = area.bottom() - PAD_V - BTN_H;
                    let ok_lb = t("完成");
                    let ok_w = btn_w(ui, ok_lb, &theme::font_body());
                    let ok_r = Rect::from_min_size(
                        Pos2::new(area.right() - ok_w, by),
                        Vec2::new(ok_w, BTN_H),
                    );

                    let ok_id = ui.next_auto_id();
                    if accent_button(ui, ok_r, ok_lb, true, p, theme::font_body()) {
                        done = true;

                        super::host_panel::open(st);
                    }
                    if st.dialog_fresh {
                        ui.ctx().memory_mut(|m| m.request_focus(ok_id));
                    }
                }
            }
        },
    );

    if !alive || done {
        return;
    }
    st.dialog = dialogs::Dialog::Import {
        stage,
        consent,
        sources,
        scanning,
        scan_rx,
        sessions,
        stats,
        rows,
        summary,
        missing,
        errors,
        truncated,
        imported,
    };
}

fn preview_cols(x: f32, w: f32) -> [(f32, f32); 7] {
    let fixed = [32.0, 52.0, 88.0, 64.0, 96.0];
    let gap = 8.0;
    let flex = w - fixed.iter().sum::<f32>() - gap * 6.0;
    let flex = flex / 2.0;
    let mut out = [(x, 32.0); 7];
    out[1] = (x + 32.0 + gap, flex);
    out[2] = (out[1].0 + flex + gap, flex);
    out[3] = (out[2].0 + flex + gap, 52.0);
    out[4] = (out[3].0 + 52.0 + gap, 88.0);
    out[5] = (out[4].0 + 88.0 + gap, 64.0);
    out[6] = (out[5].0 + 64.0 + gap, 96.0);
    out
}
