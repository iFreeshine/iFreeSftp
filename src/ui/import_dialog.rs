//! 会话导入向导弹窗（C# ImportDialog 1:1，780 宽）：三态 = 同意页 → 预览表
//! （判重）→ 结果页。解析/判重走 `crate::import`（三条红线：口令不导、命令
//! 字段不导、只导连接元数据），勾选条目经 `hosts::append_to_disk` 即存写回
//! （C# ImportRequested → HostStoreService 同语义），完成后关闭并刷新宿主
//! HostPanel。样式遵循弹框统一原则：shell 外壳 + 标题栏拖拽 + ESC/遮罩/关闭。

use std::sync::mpsc::channel;

use egui::{Align2, Color32, FontFamily, FontId, Pos2, Rect, Sense, Stroke, Vec2};

use crate::hosts::AuthMode;
use crate::i18n::{t, tfmt};
use crate::import::SourceOptions;
use crate::theme;

use super::State;
use super::dialogs::{self, shell, std_button};

/// 卡片宽（预览表 7 列需要横向空间）。
const W: f32 = 780.0;
/// 各态卡总高（C# 卡片 Auto 高；= 标题栏 35 + 内容 StackPanel 自然高 + 底 margin 5 + 边框 2）。
/// 同意页内容（Padding 16,12 / Spacing 12，xaml:90）：12 + 说明 2 行 32 + 12 + 区头 16
/// + 12 + 九宫格 104（3×32 + 2×4 RowSpacing）+ 12 + 同意框 32 + 12 + 按钮行 32 + 12 = 288 → 330。
const H_CONSENT: f32 = 330.0;
/// 预览页内容（Spacing 10，xaml:132-136）：12 + 概要 16 + 10 + 表头 19（15 + 底 4）
/// + 10 + 列表 320（MaxHeight）+ 10 + 警告 2 行 11pt 30 + 10 + 按钮行 32 + 12 = 481 → 525。
const H_PREVIEW: f32 = 525.0;
/// 内容容器左右边距（StackPanel Padding "16,12"：xaml:90/134/282）。
const PAD: f32 = 16.0;
/// 内容容器上下边距（同上 Padding 的上下 12）。
const PAD_V: f32 = 12.0;
/// 行 / CheckBox 控件高（WinUI 控件 MinHeight 32；预览行在 ListViewItem MinHeight=0
/// 之下行高由 32 高的 CheckBox 决定，xaml:193/220-224）。
const ROW: f32 = 32.0;
/// 复选框视觉方块 18×18（WinUI CheckBox 模板；32 高控件内垂直居中 → 上偏 7）。
const CB: f32 = 18.0;
/// 来源九宫格行步进（Grid RowSpacing 4 + 控件 32，xaml:100）。
const CB_STEP: f32 = 36.0;
/// 按钮高（WinUI Button 默认 MinHeight 32；内容 padding 左右 11 → 宽 = 文字 + 22）。
const BTN_H: f32 = 32.0;

/// 预览行（列文本预构建）。
pub(crate) struct ImportRow {
    checked: bool,
    /// 已存在 / 批内重复行不可勾选（不可重复导入）。
    enabled: bool,
    name: String,
    host: String,
    port: String,
    user: String,
    auth: String,
    status: String,
}

/// 打开向导（HostPanel 导入钮）：态 0 同意页，全部来源默认勾选。
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

/// 警告码 → zh-CN 文案。
fn warn_text(code: &str) -> &'static str {
    match code {
        "keyNotFound" => "密钥文件不存在",
        "uncPath" => "UNC 密钥路径，已回退密码",
        _ => "",
    }
}

/// 错误码 → zh-CN 文案。
fn err_text(code: &str) -> &'static str {
    match code {
        "fileTooLarge" => "文件超过 8MB，已跳过",
        "fileReadFailed" => "文件读取失败",
        "registryReadFailed" => "注册表读取失败",
        _ => "解析失败",
    }
}

/// 每帧入口。
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

    // 扫描回执（扫描在后台线程：文件 + 注册表 IO 不卡帧）。
    if scanning
        && let Some(rx) = &mut scan_rx
        && let Ok(res) = rx.try_recv()
    {
        scanning = false;
        // 判重口径：Host:Port:Username，先对存档再批内去重。
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
            let status = if !batch_keys.insert(key) {
                existing += 1;
                1 // 已存在（存档或批内重复，不可重复导入）
            } else {
                0
            };
            let wt = warn_text(s.warning);
            // 状态列（C# StatusTextFor）：已存在优先；警告独立显示（无「新条目；」拼接）。
            rows.push(ImportRow {
                checked: status == 0,
                enabled: status == 0,
                name: s.name.clone(),
                host: s.host.clone(),
                port: s.port.to_string(),
                user: s.username.clone(),
                auth: match s.auth_mode {
                    AuthMode::KeyFile => t("密钥文件").to_owned(),
                    AuthMode::Password => t("密码").to_owned(),
                },
                status: if status == 1 {
                    t("已存在").to_owned()
                } else if !wt.is_empty() {
                    t(wt).to_owned()
                } else {
                    t("新条目").to_owned()
                },
            });
        }
        // C# ScanBtn_Click：0 条会话不进预览页，直接 ShowResult（结果页）。
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

    // 结果页明细行数（BuildDetailText：截断行 + 错误行；C# ResultDetailText 空则
    // Collapsed，卡片 Auto 高 → 明细区随行数伸缩）。
    let detail_n = usize::from(truncated > 0) + errors.len();
    let h = match stage {
        0 => H_CONSENT,
        1 => H_PREVIEW,
        // 结果页（Spacing 14，xaml:280-310）：12 + 图标行 28 + 14 + 明细 n×16
        // + 14 + 按钮行 32 + 12 + 边框 2（C# Auto 高）。
        _ => 114.0 + detail_n as f32 * 16.0,
    };
    // 壳层 Grid 无 RowSpacing（C# ImportDialog.xaml:36-40 仅两行无间距；RowSpacing=4
    // 只在同意页来源九宫格内部）。
    let Some(content) = shell(ui, st, p, W, h, t("导入会话"), true, 0.0, true, None) else {
        return;
    };
    let area = content.shrink2(Vec2::new(PAD, PAD_V));
    let painter = ui.painter().clone();
    let x = area.left();
    let mut y = area.top();
    let mut done = false;

    match stage {
        0 => {
            // ── 态 1：同意页（ConsentView Padding 16,12 / Spacing 12，xaml:90-129）──
            let note = t(
                "仅在本机读取您所选来源中的连接元数据（名称、主机、端口、用户名、认证方式、密钥路径）；口令与登录后命令一律不导入。扫描结果只在本弹窗内展示，关闭即丢弃。单文件不超过 8MB，单次最多 5000 条。",
            );
            wrap_text(
                ui,
                &painter,
                x,
                y,
                area.width(),
                note,
                12.0,
                p.text_secondary,
            );
            y += 32.0 + 12.0; // 说明 2 行 12pt + StackPanel Spacing
            // 区头（12pt SemiBold，xaml:96-99；未设 Foreground = 默认一级）。
            painter.text(
                Pos2::new(x, y + 8.0),
                Align2::LEFT_CENTER,
                t("选择要扫描的来源"),
                FontId::new(12.0, FontFamily::Name("brand".into())),
                p.text_primary,
            );
            y += 16.0 + 12.0; // 区头行高 + Spacing
            // 来源九宫格（Grid ColumnSpacing 16 / RowSpacing 4，xaml:100-116）；
            // 两列 * 均分 → 第二列起点 = (w-16)/2 + 16 = w/2 + 8。
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
                    Rect::from_min_size(Pos2::new(cx, cy + (ROW - CB) / 2.0), Vec2::splat(CB)),
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
                    FontId::proportional(14.0),
                    p.text_primary,
                );
            }
            y += 3.0 * CB_STEP - 4.0 + 12.0; // 九宫格高 3×32+2×4=104 + Spacing
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
                FontId::proportional(14.0),
                p.text_primary,
            );
            // 页脚（按钮行 Spacing 8，xaml:121-128）：开始扫描（同意后可用）/ 取消；
            // 底距 = StackPanel Padding 下 12。
            let by = area.bottom() - PAD_V - BTN_H;
            let scan_lb = if scanning {
                t("正在扫描…")
            } else {
                t("开始扫描")
            };
            let scan_w = text_w(&painter, scan_lb, 14.0) + 22.0;
            let cancel_w = text_w(&painter, t("取消"), 14.0) + 22.0;
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
            // 扫描中禁用（C# ScanBtn_Click 置 IsEnabled=false，走禁用三件套）。
            if accent_button(ui, scan_r, scan_lb, consent && !scanning, p) {
                // 来源白名单：未勾选的来源不探测（隐私红线）。
                let opts = SourceOptions { kinds: sources };
                let (tx, rx) = channel();
                std::thread::spawn(move || {
                    let _ = tx.send(crate::import::scan_sources(&opts));
                });
                scan_rx = Some(rx);
                scanning = true;
            }
        }
        1 => {
            // ── 态 2：预览页（PreviewView Padding 16,12 / Spacing 10，xaml:132-136）──
            wrap_text(
                ui,
                &painter,
                x,
                y,
                area.width(),
                &summary,
                12.0,
                p.text_primary,
            );
            y += 16.0 + 10.0; // 概要 1 行 12pt + Spacing
            // 列头 Grid（ColumnSpacing 8 / 底线 1px DividerStroke / Padding 底 4，
            // xaml:141-145）；列 32/*/​*/52/88/64/96，表头 11pt secondary（xaml:146-184）。
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
                    FontId::proportional(11.0),
                    p.text_secondary,
                );
            }
            y += 15.0 + 4.0; // 表头行高 11pt + Padding 底 4 → 底线位置
            painter.line_segment(
                [Pos2::new(x, y), Pos2::new(area.right(), y)],
                Stroke::new(1.0, p.divider_stroke),
            );
            y += 10.0; // Spacing
            // 行（ListView MaxHeight 320，xaml:186-190；行高 32 由 MinHeight=0 之下
            // CheckBox 控件高决定，xaml:193）。
            let list_h = 320.0f32.min(rows.len() as f32 * ROW);
            for (shown, r) in rows.iter_mut().enumerate() {
                let row_y = y + shown as f32 * ROW;
                if row_y + ROW > y + list_h + 8.0 {
                    break;
                }
                let row_r =
                    Rect::from_min_max(Pos2::new(x, row_y), Pos2::new(area.right(), row_y + ROW));
                let resp = ui.allocate_rect(row_r, Sense::hover());
                if resp.hovered() {
                    painter.rect_filled(row_r, 0.0, p.hover_bg);
                }
                // CheckBox 列 0 宽 32 + HorizontalAlignment=Center → 方块中心 = 列起点 + 16。
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
                    let tx = trim_text(&painter, txt, 12.0, cw - 2.0);
                    painter.text(
                        Pos2::new(*cl, row_y + ROW / 2.0),
                        Align2::LEFT_CENTER,
                        tx,
                        FontId::proportional(12.0),
                        p.text_primary,
                    );
                }
                // 行底线：DividerStroke 再降透明度（Opacity 0.5，xaml:201-208）。
                painter.line_segment(
                    [
                        Pos2::new(x, row_y + ROW),
                        Pos2::new(area.right(), row_y + ROW),
                    ],
                    Stroke::new(1.0, theme::dim50(p.divider_stroke)),
                );
            }
            y += list_h + 10.0; // Spacing
            // 警告区（WarnText 11pt secondary，xaml:264-269）：单来源错误 / 超限截断
            // （C# MissingSources 不上 UI，未检测到的来源静默）。
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
            // 部分损坏显式告警（T6）：无效条目不能静默，跳过条数必须可见。
            let skipped: usize = stats.iter().map(|s| s.skipped_invalid).sum();
            if skipped > 0 {
                warns.push(tfmt(
                    "{0} 条会话无效已跳过（缺少主机名或端口无效）。",
                    &[&skipped],
                ));
            }
            if !warns.is_empty() {
                wrap_text(
                    ui,
                    &painter,
                    x,
                    y,
                    area.width(),
                    &warns.join("\n"),
                    11.0,
                    p.text_secondary,
                );
            }
            // 页脚（按钮行 Spacing 8，xaml:270-276）：导入 (n) / 返回。
            let by = area.bottom() - PAD_V - BTN_H;
            let n = rows.iter().filter(|r| r.checked && r.enabled).count();
            let imp_lb = tfmt("导入 ({0})", &[&n]);
            let imp_w = text_w(&painter, &imp_lb, 14.0) + 22.0;
            let back_w = text_w(&painter, t("返回"), 14.0) + 22.0;
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
            if accent_button(ui, imp_r, &imp_lb, n > 0, p) && n > 0 {
                // 即存写回：勾选新条目逐条 append（C# AppendHosts 同语义）。
                // rows 与 sessions 平行同序：先存档判重键集，再逐条即存。
                let mut have: std::collections::HashSet<String> = crate::hosts::with(|hs| {
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
                            // C# ImportRequested：Hosts.Add（内存工作集）+ AppendHosts
                            //（盘）双写，HostPanel 重开立即可见。落盘失败不阻断
                            //（H4：条目已在工作集，显式计数上报，「保存」可重试）。
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
                // C# HostPanel ImportRequested → TerminalLogService.Log(LogImportDone, count)。
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
        }
        _ => {
            // ── 态 3：结果页（ResultView Padding 16,12 / Spacing 14，xaml:280-310）──
            let text = if imported > 0 {
                tfmt("已导入 {0} 条会话", &[&imported])
            } else {
                t("未找到可导入的新会话").to_owned()
            };
            // 图标行（水平 Spacing 10，xaml:285-297）：C# ShowResult 双态 —— 成功
            // E73E 对勾 / SystemFillColorSuccess；无结果 E946 信息 / TextFillColorSecondary
            //（egui 无 Segoe Fluent Icons 字形，用 ✓/ⓘ 近似；行高取 28）。
            let (glyph, icon_c) = if imported > 0 {
                ("✓", p.success_fg)
            } else {
                ("ⓘ", p.text_secondary)
            };
            painter.text(
                Pos2::new(x + 10.0, y + 14.0),
                Align2::CENTER_CENTER,
                glyph,
                FontId::proportional(20.0),
                icon_c,
            );
            painter.text(
                Pos2::new(x + 20.0 + 10.0, y + 14.0),
                Align2::LEFT_CENTER,
                text,
                FontId::new(14.0, FontFamily::Name("brand".into())),
                p.text_primary,
            );
            // 明细（ResultDetailText 12pt secondary，xaml:298-303）：BuildDetailText =
            // 超限截断行 + 单来源错误行（"{来源}: {code 文案}"，半角冒号，C# 原格式）；
            // 空则 Collapsed（高度已在 detail_n 里伸缩）。
            let mut dy = y + 28.0 + 14.0; // 图标行高 + Spacing
            if truncated > 0 {
                painter.text(
                    Pos2::new(x, dy),
                    Align2::LEFT_CENTER,
                    tfmt("会话数量超过 5000 条上限，已截断 {0} 条。", &[&truncated]),
                    FontId::proportional(12.0),
                    p.text_secondary,
                );
                dy += 16.0; // 12pt 行高
            }
            for (k, code) in &errors {
                painter.text(
                    Pos2::new(x, dy),
                    Align2::LEFT_CENTER,
                    format!("{}: {}", k.label(), t(err_text(code))),
                    FontId::proportional(12.0),
                    p.text_secondary,
                );
                dy += 16.0;
            }
            // 页脚（xaml:304-309）：仅完成钮（C# 结果页无返回）。
            let by = area.bottom() - PAD_V - BTN_H;
            let ok_lb = t("完成");
            let ok_w = text_w(&painter, ok_lb, 14.0) + 22.0;
            let ok_r =
                Rect::from_min_size(Pos2::new(area.right() - ok_w, by), Vec2::new(ok_w, BTN_H));
            if accent_button(ui, ok_r, ok_lb, true, p) {
                done = true;
                // 刷新宿主 HostPanel（重开 = 从内存工作集重取快照，含刚导入条目）。
                super::host_panel::open(st);
            }
        }
    }

    if done {
        // 关闭路径：HostPanel 已在「完成」里重开；其余直接丢弃（结果不入存档）。
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

/// 预览表 7 列 x 坐标与宽（32 + 弹性×2 + 52 + 88 + 64 + 96）。
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

/// 方形复选框（视觉方块 18×18，由调用处传入；返回 true = 本次点击翻转）。
/// 态口径（WinUI CheckBox 模板默认资源，C# 未覆写）：选中底/勾边 = Accent；
/// 未选中底 = ControlFillColorDefault、边 = ControlStrokeColorSecondary；
/// 禁用 = ControlFillColorDisabled + ControlStrokeColorDisabled（不可点）。
fn checkbox(ui: &mut egui::Ui, r: Rect, on: bool, enabled: bool, p: &theme::Palette) -> bool {
    let resp = ui.allocate_rect(r, Sense::click());
    let painter = ui.painter().clone();
    if !enabled {
        painter.rect_filled(r, theme::CONTROL_RADIUS, p.control_disabled);
        painter.rect_stroke(
            r,
            theme::CONTROL_RADIUS,
            Stroke::new(1.0, p.control_stroke_disabled),
            egui::StrokeKind::Inside,
        );
        return false;
    }
    if resp.hovered() {
        painter.rect_filled(r.expand(2.0), 3.0, p.control_hover);
    }
    let border = if on { theme::ACCENT } else { p.control_stroke };
    let bg = if on { theme::ACCENT } else { p.control_bg };
    painter.rect_filled(r, theme::CONTROL_RADIUS, bg);
    painter.rect_stroke(
        r,
        theme::CONTROL_RADIUS,
        Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );
    if on {
        painter.text(
            r.center(),
            Align2::CENTER_CENTER,
            "✓",
            FontId::proportional(12.0),
            Color32::WHITE,
        );
    }
    resp.clicked()
}

/// 自动换行多行文本（空格断词；中文逐字符断行）。size：字号（12 正文 / 11 警告）。
fn wrap_text(
    ui: &egui::Ui,
    painter: &egui::Painter,
    x: f32,
    y: f32,
    w: f32,
    text: &str,
    size: f32,
    color: Color32,
) {
    let font = FontId::proportional(size);
    let galley = painter.layout(text.to_owned(), font, color, w);
    painter.galley(Pos2::new(x, y), galley, color);
    let _ = ui;
}

/// 按宽度截断加省略号。
fn trim_text(painter: &egui::Painter, s: &str, size: f32, max_w: f32) -> String {
    if painter
        .layout_no_wrap(s.to_owned(), FontId::proportional(size), Color32::WHITE)
        .size()
        .x
        <= max_w
    {
        return s.to_owned();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut n = chars.len();
    while n > 0 {
        let t: String = chars[..n].iter().collect::<String>() + "…";
        let w = painter
            .layout_no_wrap(t.clone(), FontId::proportional(size), Color32::WHITE)
            .size()
            .x;
        if w <= max_w {
            return t;
        }
        n -= 1;
    }
    "…".to_owned()
}

fn text_w(painter: &egui::Painter, s: &str, size: f32) -> f32 {
    painter
        .layout_no_wrap(s.to_owned(), FontId::proportional(size), Color32::WHITE)
        .size()
        .x
}

/// 主色文字钮（C# AccentButtonStyle：同意页开始扫描 / 导入 / 完成）。
/// 底 AccentFillColorDefault = 品牌蓝三态（与 header/host_panel 同口径）；禁用 =
/// AccentFillColorDisabled 底 + TextOnAccentFillColorDisabled 字（点击不响应）。
/// 字号 14（Button 默认）、高 32、宽 = 文字 + 22（内容左右 padding 11，调用处）。
fn accent_button(
    ui: &mut egui::Ui,
    r: Rect,
    label: &str,
    enabled: bool,
    p: &theme::Palette,
) -> bool {
    let resp = ui.allocate_rect(r, Sense::click());
    let (bg, text_c) = if !enabled {
        (p.accent_disabled, theme::TEXT_ON_ACCENT_DISABLED)
    } else if resp.is_pointer_button_down_on() {
        (theme::ACCENT_PRESSED, Color32::WHITE)
    } else if resp.hovered() {
        (theme::ACCENT_HOVER, Color32::WHITE)
    } else {
        (theme::ACCENT, Color32::WHITE)
    };
    ui.painter().rect_filled(r, 4.0, bg);
    ui.painter().text(
        r.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(14.0),
        text_c,
    );
    enabled && resp.clicked()
}
