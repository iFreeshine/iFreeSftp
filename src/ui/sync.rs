//! 同步浏览 + 文件比对状态机（对照 C# SyncBrowseService / FileCompareService /
//! FolderContainer.HandleSyncNavigation / FileView.ApplyCompareLayout）。
//!
//! 同步浏览规则（用户口述口径，C# 同）：
//! ①主机配置了同步浏览且连接落位（联对面基准根定位）→ 自动点亮；
//! ②联动面未就绪（两侧非「一本地一远程」配对 / 远程侧无活动视图）→ 按钮禁用；
//! ③点击建立 = 手动以两侧当前打开目录为基准根直接联动（不依赖主机预设
//!   同步目录）；本地侧未打开具体目录 → 弹窗提示；
//! ④联动中失配（镜像导航失败 / 越出基准根）→ 弹窗并熄灭。
//! 联对面 = 一侧本地 tab + 另一侧远程 tab（对称窗格，任一侧均可为本地/远程，
//! C# 左恒本地右恒远程的对称泛化）。匹配判定 = 两侧各有同步基准根（连接时由
//! 主机同步目录设置）且当前目录相对各自基准根的相对路径一致（本地侧大小写不敏感）。
//!
//! 文件比对规则：只有同步浏览点亮才允许；激活时两侧按合并名清单重排（目录
//! 优先 + 名称序，两侧行序一致逐行配对），单侧独有文件行 Caution 高亮（缺侧
//! 占位空行），两侧都有但大小或时间不同行 Critical 高亮；目录参与配对不染色。

use std::collections::HashMap;
use std::path::Path;

use crate::i18n::t;

use super::State;
use super::dialogs::Dialog;
use super::panes::{Entry, FilePane};

// ── 相对路径判定（C# RelToBase） ──────────────────────────────

/// 本地当前目录相对基准根的相对路径（'/' 归一；None = 无路径/越出基准根）。
fn rel_local(base: &str, path: &str) -> Option<String> {
    if path.is_empty() {
        return None; // 根视图（此电脑）
    }
    if path.eq_ignore_ascii_case(base) {
        return Some(String::new());
    }
    let prefix = if base.ends_with('\\') {
        base.to_owned()
    } else {
        format!("{base}\\")
    };
    if path.len() >= prefix.len() && path[..prefix.len()].eq_ignore_ascii_case(&prefix) {
        Some(
            path[prefix.len()..]
                .replace('\\', "/")
                .trim_end_matches('/')
                .to_owned(),
        )
    } else {
        None
    }
}

/// 远程当前目录相对基准根的相对路径（None = 越出基准根）。
fn rel_remote(base: &str, path: &str) -> Option<String> {
    if base == "/" {
        return Some(if path.len() <= 1 {
            String::new()
        } else {
            path.trim_start_matches('/').to_owned()
        });
    }
    if path == base {
        return Some(String::new());
    }
    path.strip_prefix(&format!("{base}/"))
        .map(|rest| rest.trim_end_matches('/').to_owned())
}

/// 侧 → 活动窗格视图（本地 tab = 该侧本地 pane；远程 tab = 该侧远程面板）。
fn view(st: &State, side: usize) -> &FilePane {
    super::pane_view(st, side)
}

/// 联动配对面（C# 对称窗格）：两侧活动窗格一本地一远程才成对。
/// 返回 (本地侧, 远程侧)；None = 同为本地/同为远程（无法联动）。
fn sides(st: &State) -> Option<(usize, usize)> {
    match (super::pane_is_local(st, 0), super::pane_is_local(st, 1)) {
        (true, false) => Some((0, 1)),
        (false, true) => Some((1, 0)),
        _ => None,
    }
}

/// 本地侧相对路径（取该侧活动窗格 path）。
fn rel_local_of(st: &State, base: &str, ls: usize) -> Option<String> {
    rel_local(
        base,
        &view(st, ls)
            .path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    )
}

/// 远程侧相对路径（取该侧活动窗格 remote_path）。
fn rel_remote_of(st: &State, base: &str, rs: usize) -> Option<String> {
    rel_remote(base, view(st, rs).remote_path.as_deref().unwrap_or(""))
}

// ── 状态机 ────────────────────────────────────────────────────

/// 联动就绪（C# 规则②对称口径）：两侧一本地一远程配对且远程侧有活动视图。
/// Header 同步/比对胶囊的启用门。
pub(crate) fn pair_ready(st: &State) -> bool {
    match sides(st) {
        Some((_, rs)) => !view(st, rs).empty,
        None => false,
    }
}

/// 同步匹配评估（C# Evaluate）。Ok = 可点亮；Err = 失配原因键。
pub(crate) fn evaluate(st: &State) -> Result<(), &'static str> {
    let Some((ls, rs)) = sides(st) else {
        // 非一本地一远程配对（两侧同为本地/同为远程）→ 无法联动。
        return Err("SyncBrowseMismatch");
    };
    if view(st, rs).empty {
        // 远程侧无活动视图（连接中 / 空态）。
        return Err("SyncBrowseMismatch");
    }
    let Some((lb, rb)) = &st.sync_base else {
        // 一侧未建立同步基准（主机未配置同步浏览）。
        return Err("SyncMismatchNotConfigured");
    };
    let l = rel_local_of(st, lb, ls).ok_or("SyncMismatchMsg")?;
    let r = rel_remote_of(st, rb, rs).ok_or("SyncMismatchMsg")?;
    if l.eq_ignore_ascii_case(&r) {
        Ok(())
    } else {
        Err("SyncMismatchMsg")
    }
}

/// 手动建立联动（用户口述口径：分别打开本地/远程目录后点同步即联动）：
/// 以两侧当前打开目录为同步基准根（相对路径清零，天然匹配），不依赖主机
/// 预设同步目录。要求联对面就绪 + 本地侧已打开具体目录（非「此电脑」根
/// 视图）。成功仅设基准根（调用方点亮）；失败返回失配键弹窗。
/// evaluate 保留：SyncMismatch 弹窗重试按钮仍按基准校验。
pub(crate) fn attach_manual(st: &mut State) -> Result<(), &'static str> {
    let Some((ls, rs)) = sides(st) else {
        return Err("SyncBrowseMismatch");
    };
    if view(st, rs).empty {
        return Err("SyncBrowseMismatch");
    }
    let rp = view(st, rs)
        .remote_path
        .clone()
        .ok_or("SyncBrowseMismatch")?;
    let lp = view(st, ls)
        .path
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|p| !p.is_empty())
        .ok_or("SyncManualNoLocalDir")?;
    // 以两侧当前目录为基准根；旧镜像请求作废（新基准下重新镜像）。
    st.sync_base = Some((normalize_local_base(&lp), rp));
    st.sync_mirror = None;
    Ok(())
}

/// 失配处置（C# ReportMismatch + OnSyncMismatch）：熄灭联动（联动熄灭比对）、
/// 终端日志、弹窗提示（已有弹窗时不覆盖，防连续导航叠弹窗）。
pub(crate) fn report_mismatch(st: &mut State, key: &str) {
    st.sync_browse = false;
    if st.compare {
        st.compare = false;
        compare_restore_both(st);
    }
    let msg = match key {
        "SyncMismatchNotConfigured" => t("主机未设置同步浏览目录，或一侧未建立同步基准"),
        "SyncManualNoLocalDir" => t("本地侧未打开具体目录，无法建立同步"),
        "SyncMismatchMsg" => t("两侧目录不一致，无法建立同步浏览"),
        _ => t("同步浏览: 两侧目录不一致, 已关闭联动"),
    };
    st.terminal.push((crate::local::now_hms(), msg.to_owned()));
    if st.terminal.len() > 400 {
        st.terminal.drain(0..st.terminal.len() - 400);
    }
    if matches!(st.dialog, Dialog::None) {
        st.dialog = Dialog::SyncMismatch {
            msg: msg.to_owned(),
        };
    }
}

/// 窗格导航驱动（C# HandleSyncNavigation + ApplySort 挂钩合并入口）：
/// 比对激活即两侧重排；联动激活即镜像对侧。`side` = 导航发起侧（对称窗格，
/// 镜像方向由该侧在配对面中的角色决定：本地侧 → 远程镜像，远程侧 → 本地镜像）。
pub(crate) fn handle_nav(st: &mut State, side: usize, path: String) {
    if st.compare {
        compare_apply_both(st);
    }
    if !st.sync_browse {
        return;
    }
    let Some((ls, rs)) = sides(st) else {
        return;
    };
    let Some((lb, rb)) = st.sync_base.clone() else {
        return;
    };
    let _ = path; // 相对路径取自窗格状态（含根视图语义），入参仅表导航发生
    if side == ls {
        // 本地 → 远程镜像：异步列目录；失败在 Ev::Failed 按 sync_mirror 侧匹配报失配。
        let Some(rel) = rel_local_of(st, &lb, ls) else {
            // 越出基准根（回驱动器列表 / 基准根之上）→ 两侧无法对应，关闭联动。
            report_mismatch(st, "SyncMismatchMsg");
            return;
        };
        // 已同步（幂等：镜像回环 / 刷新不重发）。
        if rel_remote_of(st, &rb, rs).as_deref() == Some(rel.as_str()) {
            return;
        }
        let target = if rel.is_empty() {
            rb.clone()
        } else if rb == "/" {
            format!("/{rel}")
        } else {
            format!("{rb}/{rel}")
        };
        st.remotes[rs].wait = Some(target.clone());
        st.sync_mirror = Some((rs, target.clone()));
        st.remotes[rs].pane.loading = true;
        if let Some(ssh) = st.remotes[rs].ssh.as_ref() {
            ssh.list(target);
        }
    } else if side == rs {
        // 远程 → 本地镜像：同步导航，失败（目录不存在）即失配。
        let Some(rel) = rel_remote_of(st, &rb, rs) else {
            report_mismatch(st, "SyncMismatchMsg");
            return;
        };
        if rel_local_of(st, &lb, ls).as_deref() == Some(rel.as_str()) {
            return;
        }
        let mut t = if lb.ends_with('\\') {
            lb.clone()
        } else {
            format!("{lb}\\")
        };
        t.push_str(&rel.replace('/', "\\"));
        if !crate::local::navigate(super::pane_view_mut(st, ls), Path::new(&t)) {
            report_mismatch(st, "SyncMismatchMsg");
        }
    }
}

// ── 文件比对（C# FileCompareService + ApplyCompareLayout） ────

/// 比对快照：小写名 → 条目（占位行不参与）。
fn snapshot(p: &FilePane) -> HashMap<String, Entry> {
    p.entries
        .iter()
        .filter(|e| !e.name.is_empty())
        .map(|e| (e.name.to_lowercase(), e.clone()))
        .collect()
}

/// 一侧重排（C# ApplyCompareLayout）：合并名清单（并集 + 目录优先 + 名称序，
/// 目录标记取两侧任一侧），本侧有 → 条目行，缺 → 占位空行；高亮只对文件。
fn apply_layout(pane: &mut FilePane, other: &HashMap<String, Entry>) {
    let mine = snapshot(pane);
    let mut names: Vec<(String, bool)> = mine
        .keys()
        .chain(other.keys().filter(|k| !mine.contains_key(*k)))
        .map(|k| {
            (
                k.clone(),
                mine.get(k).map(|e| e.is_dir).unwrap_or(false)
                    || other.get(k).map(|e| e.is_dir).unwrap_or(false),
            )
        })
        .collect();
    // 目录优先 + 名称序（小写键序 = C# OrdinalIgnoreCase 的近似口径）。
    names.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let mut rows: Vec<Entry> = Vec::with_capacity(names.len());
    let mut marks: Vec<u8> = Vec::with_capacity(names.len());
    for (name, is_dir) in names {
        match mine.get(&name) {
            Some(e) => {
                let state = if !is_dir {
                    match other.get(&name) {
                        None => 1, // 仅本侧有（对侧为占位空行）
                        Some(o) if o.size_bytes != e.size_bytes || o.mtime_secs != e.mtime_secs => {
                            2 // 大小或修改时间不同（C# FilesDiffer）
                        }
                        Some(_) => 0,
                    }
                } else {
                    0 // 目录参与配对但不染色
                };
                rows.push(e.clone());
                marks.push(state);
            }
            None => {
                rows.push(Entry::placeholder());
                marks.push(0);
            }
        }
    }
    pane.entries = rows;
    pane.compare_marks = Some(marks);
    pane.selected.clear();
}

/// 两侧重排比对（C# ApplyBoth）；联动面失效（非一本地一远程配对 / 任一侧
/// 失去可配对视图）→ 熄灭恢复。
pub(crate) fn compare_apply_both(st: &mut State) {
    if !st.compare {
        return;
    }
    let Some((ls, rs)) = sides(st) else {
        st.compare = false;
        compare_restore_both(st);
        return;
    };
    if view(st, ls).empty || view(st, rs).empty {
        st.compare = false;
        compare_restore_both(st);
        return;
    }
    let other = snapshot(view(st, rs));
    apply_layout(super::pane_view_mut(st, ls), &other);
    let other = snapshot(super::pane_view(st, ls));
    apply_layout(super::pane_view_mut(st, rs), &other);
}

/// 就地恢复常规布局（C# RestoreNormalLayout）：剔除占位行 + 清高亮 + 常规排序，
/// 不重新加载（真实条目数据仍在）。对称窗格：两侧活动视图一并恢复。
pub(crate) fn compare_restore_both(st: &mut State) {
    for s in 0..2 {
        let pane = super::pane_view_mut(st, s);
        if pane.compare_marks.take().is_some() {
            pane.entries.retain(|e| !e.name.is_empty());
            crate::local::apply_sort(pane);
        }
    }
}

/// 连接落位的同步/比对自动激活（C# FolderContainer.ConnectAndOpenAsync 规则①②）。
/// `connect_side` = 连接落位侧；对侧须为本地活动视图才能组建联对面
/// （对称窗格后任一侧连接均可能重组联对面，C# 对侧恒本地的按需判定版）。
/// `remote_base`：远程基准根（同步目录或上次浏览目录；无则 None = 未配置口径）。
pub(crate) fn attach_connect(st: &mut State, connect_side: usize, remote_base: Option<String>) {
    st.sync_base = None;
    st.sync_browse = false;
    st.sync_mirror = None;
    st.compare = false;
    // 联动全复位：四个窗格的比对标记一并清（对称窗格后标记可能残存任一侧）。
    st.left.compare_marks = None;
    st.right.compare_marks = None;
    st.remotes[0].pane.compare_marks = None;
    st.remotes[1].pane.compare_marks = None;
    let Some(h) = st.remotes[connect_side].host.clone() else {
        return;
    };
    let local = if h.local_sync_path.trim().is_empty() {
        None
    } else {
        Some(normalize_local_base(&h.local_sync_path))
    };
    let (Some(remote_base), Some(local)) = (remote_base, local) else {
        return;
    };
    if !h.sync_enabled {
        return;
    }
    // 对侧须为本地活动视图（联对面 = 连接侧远程 + 对侧本地）。
    let opposite = 1 - connect_side;
    if !super::pane_is_local(st, opposite) {
        return;
    }
    // 本地侧定位于基准根（C# OpenLocalSyncView；目录失效保持原目录，状态栏提示语义）。
    crate::local::navigate(super::pane_view_mut(st, opposite), Path::new(&local));
    st.sync_base = Some((local, remote_base));
    // 两侧均定位于基准根，天然匹配 → 自动点亮；主机配置了文件比对 → 一并自动激活
    //（比对重排在首个远程列表加载完成时执行）。
    st.sync_browse = true;
    if h.compare_enabled {
        st.compare = true;
    }
}

/// 本地基准根归一（C# Path.GetFullPath + 去尾 '\'，盘根 "E:\" 保留）。
fn normalize_local_base(p: &str) -> String {
    let t = p.trim().trim_end_matches('\\');
    if t.len() <= 2 {
        format!("{t}\\")
    } else {
        t.to_owned()
    }
}
