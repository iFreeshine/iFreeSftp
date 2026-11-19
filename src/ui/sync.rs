use std::collections::HashMap;
use std::path::Path;

use crate::i18n::t;

use super::State;
use super::dialogs::Dialog;
use super::panes::{Entry, FilePane};

fn rel_local(base: &str, path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
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

fn view(st: &State, side: usize) -> &FilePane {
    super::pane_view(st, side)
}

fn sides(st: &State) -> Option<(usize, usize)> {
    match (super::pane_is_local(st, 0), super::pane_is_local(st, 1)) {
        (true, false) => Some((0, 1)),
        (false, true) => Some((1, 0)),
        _ => None,
    }
}

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

fn rel_remote_of(st: &State, base: &str, rs: usize) -> Option<String> {
    rel_remote(base, view(st, rs).remote_path.as_deref().unwrap_or(""))
}

#[derive(Clone)]
pub(crate) struct SyncBase {
    pub l_side: u8,
    pub l_tab: u32,
    pub l_path: String,
    pub r_side: u8,
    pub r_tab: u32,
    pub r_path: String,
}

fn nav_side_of(b: &SyncBase, side: usize, tab: u32) -> Option<bool> {
    if side == b.l_side as usize && tab == b.l_tab {
        Some(true)
    } else if side == b.r_side as usize && tab == b.r_tab {
        Some(false)
    } else {
        None
    }
}

fn pair_active(st: &State, b: &SyncBase) -> bool {
    super::active_tab_id(st, b.l_side as usize) == Some(b.l_tab)
        && super::active_tab_id(st, b.r_side as usize) == Some(b.r_tab)
}

pub(crate) fn pair_is_active(st: &State) -> bool {
    st.sync_base.as_ref().is_some_and(|b| pair_active(st, b))
}

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
        assert_eq!(nav_side_of(&b, 0, 7), Some(true));
        assert_eq!(nav_side_of(&b, 1, 9), Some(false));
    }

    #[test]
    fn other_side_or_tab_or_swapped_ends_miss() {
        let b = base();
        assert_eq!(nav_side_of(&b, 0, 9), None);
        assert_eq!(nav_side_of(&b, 1, 7), None);
        assert_eq!(nav_side_of(&b, 0, 99), None);

        assert_eq!(nav_side_of(&b, 1, 7), None);
    }
}

fn pair_panels_ready(st: &State, b: &SyncBase) -> bool {
    let Some(lp) = super::tab_pane(st, b.l_side as usize, b.l_tab) else {
        return false;
    };
    let Some(rp) = super::tab_pane(st, b.r_side as usize, b.r_tab) else {
        return false;
    };
    !lp.empty && !rp.empty
}

pub(crate) fn pair_ready(st: &State) -> bool {
    match sides(st) {
        Some((_, rs)) => !view(st, rs).empty,
        None => false,
    }
}

pub(crate) fn evaluate(st: &State) -> Result<(), &'static str> {
    let Some(b) = st.sync_base.as_ref() else {

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

pub(crate) fn handle_nav(st: &mut State, side: usize, tab: u32, path: String) {
    let _ = path;

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

    if !pair_active(st, &b) {
        return;
    }
    let ls = b.l_side as usize;
    let rs = b.r_side as usize;
    if side == ls {

        let Some(rel) = rel_local_of(st, &b.l_path, ls) else {

            report_mismatch(st, "SyncMismatchMsg");
            return;
        };

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

        let mtag = super::next_seq(st);
        if let Some(rt) = st.remotes[rs].tabs.iter_mut().find(|t| t.id == b.r_tab) {
            rt.wait = Some((mtag, target.clone()));
            rt.pane.loading = true;
        }
        if let Some(s) = super::session_of_tab(st, rs, b.r_tab) {
            s.ssh.list(mtag, target);
        }
    } else if side == rs {

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

fn snapshot(p: &FilePane) -> HashMap<String, Entry> {
    p.entries
        .iter()
        .filter(|e| !e.name.is_empty())
        .map(|e| (e.name.to_lowercase(), e.clone()))
        .collect()
}

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

    names.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let mut rows: Vec<Entry> = Vec::with_capacity(names.len());
    let mut marks: Vec<u8> = Vec::with_capacity(names.len());
    for (name, is_dir) in names {
        match mine.get(&name) {
            Some(e) => {
                let state = if !is_dir {
                    match other.get(&name) {
                        None => 1,
                        Some(o) if o.size_bytes != e.size_bytes || o.mtime_secs != e.mtime_secs => {
                            2
                        }
                        Some(_) => 0,
                    }
                } else {
                    0
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

        st.compare = false;
        compare_restore_both(st);
        return;
    }

    if !pair_active(st, &b) {
        return;
    }

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

pub(crate) fn compare_restore_both(st: &mut State) {
    for s in 0..2 {
        let pane = super::pane_view_mut(st, s);
        if pane.compare_marks.take().is_some() {
            pane.entries.retain(|e| !e.name.is_empty());
            crate::local::apply_sort(pane);
        }
    }
}

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

    let opposite = 1 - connect_side;
    if !super::pane_is_local(st, opposite) {
        return;
    }

    crate::local::navigate(super::pane_view_mut(st, opposite), Path::new(&local));

    st.sync_base = Some(SyncBase {
        l_side: opposite as u8,
        l_tab: super::active_tab_id(st, opposite).unwrap_or(0),
        l_path: local,
        r_side: connect_side as u8,
        r_tab: super::active_tab_id(st, connect_side).unwrap_or(0),
        r_path: remote_base,
    });

    st.sync_browse = true;
    if h.compare_enabled {
        st.compare = true;
    }
}

fn normalize_local_base(p: &str) -> String {
    let t = p.trim().trim_end_matches('\\');
    if t.len() <= 2 {
        format!("{t}\\")
    } else {
        t.to_owned()
    }
}
