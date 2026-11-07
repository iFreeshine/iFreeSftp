//! 应用数据目录与原子落盘（H4：路径收敛 + 防半截存档）。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// 应用数据目录（全项目唯一平台分支点，其余模块一律经本函数取目录）：
/// - Windows：`%LOCALAPPDATA%\iFreeSftp`（与 C# 版共用存档；缺失回退 exe
///   同目录，C# AppSettingsService 同策略）；
/// - macOS：`~/Library/Application Support/iFreeSftp`；
/// - Linux/其他 Unix：`$XDG_CONFIG_HOME/iFreeSftp`（XDG 未设或空回退
///   `~/.config/iFreeSftp`）。
#[cfg(windows)]
pub(crate) fn data_dir() -> PathBuf {
    if let Some(v) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(v).join("iFreeSftp");
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(target_os = "macos")]
pub(crate) fn data_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Library")
        .join("Application Support")
        .join("iFreeSftp")
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn data_dir() -> PathBuf {
    // XDG 规范：空串视同未设置
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("iFreeSftp");
        }
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".config").join("iFreeSftp")
}

/// 临时文件序号：同进程内并发写同一目标（UI 线程 save 与连接线程
/// save_single_session 都写 hosts.json）临时名不得撞车 —— 仅 pid 后缀时
/// 两次写共用同一临时文件，一次 rename 与另一次 write 交错会把半截内容
/// 改名成正式存档。pid + 进程内递增序号保证唯一。
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 测试中断点：Some(目标路径) 时 write_atomic 对**该路径**在「tmp 已写完、
/// rename 前」返回 Err，模拟进程在改名前被杀（cfg(test) 分支，正式构建零
/// 痕迹）。按路径定向，进程级并行用例互不影响。
#[cfg(test)]
static TEST_ABORT_TARGET: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// 原子写盘：写临时文件（唯一名）→ sync_all 强制刷盘 → rename 覆盖
/// （防进程中断产生半截存档；rename 前 sync_all 保证 crash 后改名过的
/// 文件内容已落盘 —— NTFS/ext4 均可能延迟刷盘；KB 级低频写开销可忽略。
/// 覆盖已存在目标：Windows MoveFileEx(REPLACE_EXISTING) / Unix rename 同语义）。
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!(
        "tmp{}-{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let r = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        #[cfg(test)]
        if TEST_ABORT_TARGET.lock().unwrap().as_deref() == Some(path) {
            return Err(std::io::Error::other("测试桩：rename 前中断"));
        }
        std::fs::rename(&tmp, path)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp); // 清理残留临时文件
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_roundtrip_and_overwrite() {
        let dir = std::env::temp_dir().join("ifree-app-paths-test").join("rt");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("w.json");
        let _ = std::fs::remove_file(&p);
        write_atomic(&p, b"first").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"first");
        write_atomic(&p, b"second-longer").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"second-longer");
        // 无临时残留
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "残留临时文件：{leftovers:?}");
        let _ = std::fs::remove_file(&p);
    }

    fn no_tmp_leftovers(dir: &Path) {
        let leftovers: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "残留临时文件：{leftovers:?}");
    }

    /// rename 前中断（test-only 桩模拟进程被杀）：目标仍是完整旧内容，
    /// tmp 经错误清理路径清空，随后可正常重写。
    /// （真实 crash 不走清理路径会残留 tmp——名字带 pid+序号唯一，不会
    /// 撞名转正，此差异见函数注释。）
    #[test]
    fn write_atomic_interrupt_before_rename_keeps_target_intact() {
        let dir = std::env::temp_dir()
            .join("ifree-app-paths-test")
            .join("abort");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("abort.json");
        std::fs::write(&p, b"OLD-CONTENT").unwrap();
        // 定向中断桩：仅本测试的目标路径会被打断，并行用例零影响
        *TEST_ABORT_TARGET.lock().unwrap() = Some(p.clone());
        let r = write_atomic(&p, b"NEW-CONTENT");
        *TEST_ABORT_TARGET.lock().unwrap() = None;
        assert!(r.is_err());
        // 半截新内容未转正：目标仍是完整旧内容
        assert_eq!(std::fs::read(&p).unwrap(), b"OLD-CONTENT");
        no_tmp_leftovers(&dir);
        // 中断后可正常重写
        write_atomic(&p, b"NEW-CONTENT").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"NEW-CONTENT");
        let _ = std::fs::remove_file(&p);
    }

    /// 并发写：两线程各写同一路径 100 次，终值必是某一线程的完整一份
    /// （原子 rename 保证无交错半截），且无 tmp 残留。
    #[test]
    fn write_atomic_concurrent_writers_produce_intact_file() {
        let dir = std::env::temp_dir()
            .join("ifree-app-paths-test")
            .join("conc");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("conc.json");
        let _ = std::fs::remove_file(&p);
        let path = std::sync::Arc::new(p.clone());
        let tail = "x".repeat(64);
        let mut handles = Vec::new();
        for t in 0..2 {
            let path = std::sync::Arc::clone(&path);
            let tail = tail.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..100 {
                    let content = format!("writer-{t}-iter-{i:03}-{tail}");
                    write_atomic(&path, content.as_bytes()).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let final_text = std::fs::read_to_string(&p).unwrap();
        // 完整一份：已知前后缀（写者标识 + 定长尾巴），绝非交错半截
        assert!(
            final_text.starts_with("writer-0-iter-") || final_text.starts_with("writer-1-iter-"),
            "终值不是任一写者的完整内容：{final_text}"
        );
        assert!(final_text.ends_with(&tail), "终值被截断/交错：{final_text}");
        no_tmp_leftovers(&dir);
        let _ = std::fs::remove_file(&p);
    }
}
