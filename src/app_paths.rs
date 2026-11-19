use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
static TEST_ABORT_TARGET: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

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
        let _ = std::fs::remove_file(&tmp);
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

    fn clear_tmp_leftovers(dir: &Path) {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().contains(".tmp") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }

    #[test]
    fn write_atomic_interrupt_before_rename_keeps_target_intact() {
        let dir = std::env::temp_dir()
            .join("ifree-app-paths-test")
            .join("abort");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("abort.json");
        std::fs::write(&p, b"OLD-CONTENT").unwrap();

        *TEST_ABORT_TARGET.lock().unwrap() = Some(p.clone());
        let r = write_atomic(&p, b"NEW-CONTENT");
        *TEST_ABORT_TARGET.lock().unwrap() = None;
        assert!(r.is_err());

        assert_eq!(std::fs::read(&p).unwrap(), b"OLD-CONTENT");
        no_tmp_leftovers(&dir);

        write_atomic(&p, b"NEW-CONTENT").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"NEW-CONTENT");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn write_atomic_concurrent_writers_produce_intact_file() {
        let dir = std::env::temp_dir()
            .join("ifree-app-paths-test")
            .join("conc");
        std::fs::create_dir_all(&dir).unwrap();
        clear_tmp_leftovers(&dir);
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

        assert!(
            final_text.starts_with("writer-0-iter-") || final_text.starts_with("writer-1-iter-"),
            "终值不是任一写者的完整内容：{final_text}"
        );
        assert!(final_text.ends_with(&tail), "终值被截断/交错：{final_text}");
        no_tmp_leftovers(&dir);
        let _ = std::fs::remove_file(&p);
    }
}
