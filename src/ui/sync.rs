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

/// 同步挂接对（P4，SPEC §5）：带 tab 维度的双侧基准根，本地端 (l_side, l_tab)
/// 与远程端 (r_side, r_tab) 及各自基准路径。活动 tab 切走 = 挂起（base 保留），
/// 切回原对 = evaluate 天然重新成立；切到别的 tab = 该对未挂接，不联动。
#[derive(Clone)]
pub(crate) struct SyncBase {
    pub l_side: u8,
    pub l_tab: u32,
    pub l_path: String,
    pub r_side: u8,
    pub r_tab: u32,
    pub r_path: String,
}

/// 导航来源归属（纯函数）：Some(true) = 本地端 tab / Some(false) = 远程端 tab /
/// None = 不在挂接对内（handle_nav 据此过滤，挂起态不镜像不重排）。
fn nav_side_of(b: &SyncBase, side: usize, tab: u32) -> Option<bool> {
    if side == b.l_side as usize && tab == b.l_tab {
        Some(true)
    } else if side == b.r_side as usize && tab == b.r_tab {
        Some(false)
    } else {
        None
    }
}

/// 挂接对是否处于活动位（两端 tab 分别仍是各自侧的活动 tab）。
/// 活动性判定 = 侧级活动 tab id 比对（本地/远程 tab 同一 id 空间）。
fn pair_active(st: &State, b: &SyncBase) -> bool {
    super::active_tab_id(st, b.l_side as usize) == Some(b.l_tab)
        && super::active_tab_id(st, b.r_side as usize) == Some(b.r_tab)
}

/// 挂接状态 + 活动位合一门（Header 胶囊亮度）：已挂接且挂接对处于活动位。
/// 活动对就绪 → 亮；切走（挂起，base 保留）→ 灭（可再点回 / 切回自动复亮）。
pub(crate) fn pair_is_active(st: &State) -> bool {
    st.sync_base.as_ref().is_some_and(|b| pair_active(st, b))
}

// P4 挂接对过滤纯逻辑：handle_nav 的 tab 门（导航只接受挂接对端 tab）。
#[cfg(test)]
mod nav_pair_tests {
    use super::{SyncBase, nav_side_of};

    fn base() -> SyncBase {
        SyncBase {
            l_side: 0,
            l_tab: 7,
            l_path: "C:/work".into(),
            r_side: 1,
            r_tab: 9,
            r_path: "/srv".into(),
        }
    }

    #[test]
    fn local_end_and_remote_end_hit() {
        let b = base();
        assert_eq!(nav_side_of(&b, 0, 7), Some(true)); // 本地端
        assert_eq!(nav_side_of(&b, 1, 9), Some(false)); // 远程端
    }

    #[test]
    fn other_side_or_tab_or_swapped_ends_miss() {
        let b = base();
        assert_eq!(nav_side_of(&b, 0, 9), None); // 本地侧的另一 tab
        assert_eq!(nav_side_of(&b, 1, 7), None); // 远程侧的另一 tab
        assert_eq!(nav_side_of(&b, 0, 99), None);
        // 端对调（侧与 tab 各自错位）不算命中。
        assert_eq!(nav_side_of(&b, 1, 7), None);
    }
}

/// 挂接对两端窗格是否可用（pane 存在且远程端非空态）。挂接对 pane 缺失
/// （tab 已关）= 联动面失效 → 熄灭口径；单纯切走（挂起）不算。
fn pair_panels_ready(st: &State, b: &SyncBase) -> bool {
    let Some(lp) = super::tab_pane(st, b.l_side as usize, b.l_tab) else {
        return false;
    };
    let Some(rp) = super::tab_pane(st, b.r_side as usize, b.r_tab) else {
        return false;
    };
    !lp.empty && !rp.empty
}

/// 联动就绪（C# 规则②对称口径）：两侧活动视图一本地一远程配对且远程侧有
/// 活动视图。Header 同步/比对胶囊的启用门（能否建立/已有联动可点灭）。
pub(crate) fn pair_ready(st: &State) -> bool {
    match sides(st) {
        Some((_, rs)) => !view(st, rs).empty,
        None => false,
    }
}

/// 同步匹配评估（C# Evaluate）。Ok = 可点亮；Err = 失配原因键。
/// P4 活动对口径：挂接对不活动（切走）= 挂起 → 失配口径返回（base 保留，
/// 切回原对天然重新成立）；挂接对 pane 缺失（tab 已关）= 失配。
pub(crate) fn evaluate(st: &State) -> Result<(), &'static str> {
    let Some(b) = st.sync_base.as_ref() else {
        // 一侧未建立同步基准（主机未配置同步浏览）。
        return Err("SyncMismatchNotConfigured");
    };
    if !pair_active(st, b) || !pair_panels_ready(st, b) {
        return Err("SyncBrowseMismatch");
    }
    let ls = b.l_side as usize;
    let rs = b.r_side as usize;
    let l = rel_local(
        &b.l_path,
        &super::tab_pane(st, ls, b.l_tab)
            .and_then(|p| p.path.as_ref().map(|p| p.to_string_lossy().into_owned()))
            .unwrap_or_default(),
    )
    .ok_or("SyncMismatchMsg")?;
    let r = rel_remote(
        &b.r_path,
        super::tab_pane(st, rs, b.r_tab)
            .and_then(|p| p.remote_path.as_deref())
            .unwrap_or(""),
    )
    .ok_or("SyncMismatchMsg")?;
    if l.eq_ignore_ascii_case(&r) {
        Ok(())
    } else {
        Err("SyncMismatchMsg")
    }
}

/// 手动建立联动（用户口述口径：分别打开本地/远程目录后点同步即联动）：
/// 以两侧当前打开目录为同步基准根（相对路径清零，天然匹配），不依赖主机
/// 预设同步目录。要求联对面就绪 + 本地侧已打开具体目录（非「此电脑」根
/// 视图）。落位到「当时活动的一对 tab」（P4）。成功仅设基准根（调用方点亮）；
/// 失败返回失配键弹窗。evaluate 保留：SyncMismatch 弹窗重试按钮仍按基准校验。
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
    // 以两侧当前目录为基准根，落位当时活动的一对 tab；旧镜像请求作废（新基准
    // 下重新镜像）。
    st.sync_base = Some(SyncBase {
        l_side: ls as u8,
        l_tab: super::active_tab_id(st, ls).unwrap_or(0),
        l_path: normalize_local_base(&lp),
        r_side: rs as u8,
        r_tab: super::active_tab_id(st, rs).unwrap_or(0),
        r_path: rp,
    });
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
/// 比对激活即挂接对两侧重排；联动激活即镜像对侧。`side`/`tab` = 导航发起
/// 窗格（P4：只接受挂接对端 tab 的导航——切到别的 tab = 挂起态，不镜像不
/// 重排；镜像方向由该端在挂接对中的角色决定：本地端 → 远程镜像，反之亦然）。
pub(crate) fn handle_nav(st: &mut State, side: usize, tab: u32, path: String) {
    let _ = path; // 相对路径取自窗格状态（含根视图语义），入参仅表导航发生
    // 挂接对过滤：导航不来自挂接对端 tab → 不重排不镜像（挂起态；C# 切走
    // 即停联动的等价口径）。未挂接（无 base）→ 保持比对重排的全局旧路径。
    let in_pair = st
        .sync_base
        .as_ref()
        .is_none_or(|b| nav_side_of(b, side, tab).is_some());
    if !in_pair {
        return;
    }
    if st.compare {
        compare_apply_both(st);
    }
    if !st.sync_browse {
        return;
    }
    let Some(b) = st.sync_base.clone() else {
        return;
    };
    // 挂接对不活动（活动 tab 切走）= 挂起，不镜像（base 保留，切回恢复）。
    if !pair_active(st, &b) {
        return;
    }
    let ls = b.l_side as usize;
    let rs = b.r_side as usize;
    if side == ls {
        // 本地端 → 远程端镜像：异步列目录；失败在 Ev::Failed 按 (侧, tab) 匹配报失配。
        let Some(rel) = rel_local_of(st, &b.l_path, ls) else {
            // 越出基准根（回驱动器列表 / 基准根之上）→ 两侧无法对应，关闭联动。
            report_mismatch(st, "SyncMismatchMsg");
            return;
        };
        // 已同步（幂等：镜像回环 / 刷新不重发）。
        if rel_remote_of(st, &b.r_path, rs).as_deref() == Some(rel.as_str()) {
            return;
        }
        let rb = b.r_path.clone();
        let target = if rel.is_empty() {
            rb
        } else if rb == "/" {
            format!("/{rel}")
        } else {
            format!("{rb}/{rel}")
        };
        st.sync_mirror = Some((rs, b.r_tab, target.clone()));
        // 镜像请求落在挂接对远程端 tab（活动性已由 pair_active 保证）；
        // 发号进 wait（RemoteTab 槽），Ev::Listing/Failed 按 (侧, tab) + tag 定位。
        let mtag = super::next_seq(st);
        if let Some(rt) = st.remotes[rs].tabs.iter_mut().find(|t| t.id == b.r_tab) {
            rt.wait = Some((mtag, target.clone()));
            rt.pane.loading = true;
        }
        if let Some(s) = super::session_of_tab(st, rs, b.r_tab) {
            s.ssh.list(mtag, target);
        }
    } else if side == rs {
        // 远程端 → 本地端镜像：同步导航，失败（目录不存在）即失配。
        let Some(rel) = rel_remote_of(st, &b.r_path, rs) else {
            report_mismatch(st, "SyncMismatchMsg");
            return;
        };
        if rel_local_of(st, &b.l_path, ls).as_deref() == Some(rel.as_str()) {
            return;
        }
        let lb = b.l_path.clone();
        let mut t = if lb.ends_with('\\') {
            lb
        } else {
            format!("{lb}\\")
        };
        t.push_str(&rel.replace('/', "\\"));
        if let Some(lp) = super::tab_pane_mut(st, ls, b.l_tab)
            && !crate::local::navigate(lp, Path::new(&t))
        {
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

/// 挂接对两侧重排比对（C# ApplyBoth 的 tab 化）：只作用于挂接对两端 pane；
/// 挂起（活动 tab 切走）→ 不动（marks 保留，切回幂等重排）；联动面失效
/// （无基准 / 挂接对 pane 缺失 / 空态）→ 熄灭恢复。
pub(crate) fn compare_apply_both(st: &mut State) {
    if !st.compare {
        return;
    }
    let Some(b) = st.sync_base.clone() else {
        st.compare = false;
        compare_restore_both(st);
        return;
    };
    let ls = b.l_side as usize;
    let rs = b.r_side as usize;
    if !pair_panels_ready(st, &b) {
        // 挂接对窗格失效（tab 已关 / 空态）→ 联动面失效，熄灭恢复。
        // 注意 pair_panels_ready 不含活动性：挂起 ≠ 失效，marks 保留。
        st.compare = false;
        compare_restore_both(st);
        return;
    }
    // 挂起（活动 tab 切走）→ 不重排（切回后由导航/compare_req 幂等重排）。
    if !pair_active(st, &b) {
        return;
    }
    // 快照与排版顺序借用：先取远程端快照排本地端，再取本地端快照排远程端。
    let other = {
        let rp = super::tab_pane(st, rs, b.r_tab).expect("pair_panels_ready checked");
        snapshot(rp)
    };
    let lp = super::tab_pane_mut(st, ls, b.l_tab).expect("pair_panels_ready checked");
    apply_layout(lp, &other);
    let other = {
        let lp = super::tab_pane(st, ls, b.l_tab).expect("pair_panels_ready checked");
        snapshot(lp)
    };
    let rp = super::tab_pane_mut(st, rs, b.r_tab).expect("pair_panels_ready checked");
    apply_layout(rp, &other);
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
/// `sid` = 落位会话（P2 会话复用后同侧多会话，连接档案按会话定位而非活动 tab）。
pub(crate) fn attach_connect(
    st: &mut State,
    connect_side: usize,
    sid: u64,
    remote_base: Option<String>,
) {
    st.sync_base = None;
    st.sync_browse = false;
    st.sync_mirror = None;
    st.compare = false;
    // 联动全复位：两侧全部 tab 窗格的比对标记一并清（多 tab 后标记可能残存
    // 任一侧任一 tab）。
    for ls in &mut st.locals {
        for lt in &mut ls.tabs {
            lt.pane.compare_marks = None;
        }
    }
    for rs in &mut st.remotes {
        for rt in &mut rs.tabs {
            rt.pane.compare_marks = None;
        }
    }
    let Some(h) = st.remotes[connect_side]
        .sessions
        .iter()
        .find(|s| s.sid == sid)
        .and_then(|s| s.host.clone())
    else {
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
    // 本地端定位于基准根（C# OpenLocalSyncView；目录失效保持原目录，状态栏提示语义）。
    crate::local::navigate(super::pane_view_mut(st, opposite), Path::new(&local));
    // 落位「当时活动的一对 tab」（P4）：对侧活动本地 tab + 连接侧活动远程 tab。
    st.sync_base = Some(SyncBase {
        l_side: opposite as u8,
        l_tab: super::active_tab_id(st, opposite).unwrap_or(0),
        l_path: local,
        r_side: connect_side as u8,
        r_tab: super::active_tab_id(st, connect_side).unwrap_or(0),
        r_path: remote_base,
    });
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
