//! 构建期资源内嵌（D-015）：Windows 下用 MinGW binutils 的 windres 把
//! `assets/AppIcon.ico` 编译成 COFF 目标文件并链接进 exe，
//! 修复任务栏 / 资源管理器图标回退链。不引入任何构建依赖。
//!
//! 注意：windres 依赖 dev.bat PATH 中的 `D:\dev\mingw64\bin`；
//! 找不到时回退到该固定路径，再失败则 panic（编译失败，不让坏图标静默过）。

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));

    icons(&manifest_dir, &out_dir);

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let icon = manifest_dir.join("assets").join("AppIcon.ico");
    println!("cargo:rerun-if-changed={}", icon.display());

    let rc = out_dir.join("app_icon.rc");
    let obj = out_dir.join("app_icon.o");

    // windres 的 .rc 里路径必须用正斜杠。
    let icon_str = icon.to_string_lossy().replace('\\', "/");
    fs::write(&rc, format!("1 ICON \"{icon_str}\"\n")).expect("write app_icon.rc");

    let windres = find_windres();
    let status = Command::new(&windres)
        .arg(format!("--input={}", rc.display()))
        .arg(format!("--output={}", obj.display()))
        .status()
        .unwrap_or_else(|e| panic!("spawn {} failed: {e}", windres.display()));
    if !status.success() {
        panic!("windres failed to compile {icon_str}");
    }

    println!("cargo:rustc-link-arg-bins={}", obj.display());
}

/// 图标管线（D-014 + 密度桶）：`assets/icons.txt` 按需清单 → `assets/icons-src/`
/// 内随仓库提交的 Phosphor 256px PNG（源图进源码，换机 clone 后可直接构建，
/// 不依赖仓库外 devSource）→ ① CPU Lanczos3 生成密度桶 16/20/24/32/48 五档（运行时近 1:1 采样，
/// 避免 256 大图 GPU 大比例缩放发糊）→ ② 256px 原档经 pngquant 压缩作兜底 →
/// 生成 icons_gen.rs 内嵌双表。另含品牌 logo（assets/TitleBarLogo.png，多色，不重着色）。
fn icons(manifest_dir: &std::path::Path, out_dir: &std::path::Path) {
    let list = manifest_dir.join("assets").join("icons.txt");
    println!("cargo:rerun-if-changed={}", list.display());

    let phosphor = manifest_dir.join("assets").join("icons-src");
    let out_icons = out_dir.join("icons");
    fs::create_dir_all(&out_icons).expect("create OUT_DIR/icons");

    let mut scaled: Vec<(String, u32, PathBuf)> = Vec::new();
    let mut entries: Vec<(String, PathBuf)> = Vec::new();

    let logo = manifest_dir.join("assets").join("TitleBarLogo.png");
    println!("cargo:rerun-if-changed={}", logo.display());
    scaled.extend(buckets(&logo, &out_icons, "app-logo"));
    let logo_dst = out_icons.join("app-logo.png");
    quantize(&logo, &logo_dst);
    entries.push(("app-logo".to_owned(), logo_dst));

    let text = fs::read_to_string(&list).expect("read assets/icons.txt");
    for line in text.lines() {
        let name = line.split('#').next().unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }
        let src = phosphor.join(format!("{name}.png"));
        println!("cargo:rerun-if-changed={}", src.display());
        scaled.extend(buckets(&src, &out_icons, name));
        let dst = out_icons.join(format!("{name}.png"));
        quantize(&src, &dst);
        entries.push((name.to_owned(), dst));
    }

    let mut manifest_rs = String::from(
        "/// 构建期生成（build.rs icons()）：图标名 → 密度桶物理边长 → PNG 字节\n\
         /// （256px 源 CPU Lanczos3 下采样；运行时按 DPI 选 ≥ 需求的最小档，见 src/icons.rs）。\n",
    );
    manifest_rs.push_str("pub const SCALED: &[(&str, u32, &[u8])] = &[\n");
    for (name, side, path) in &scaled {
        manifest_rs.push_str(&format!(
            "    (\"{name}\", {side}, include_bytes!(r#\"{}\"#)),\n",
            path.display()
        ));
    }
    manifest_rs.push_str("];\n");
    manifest_rs.push_str(
        "/// 构建期生成（build.rs icons()）：图标名 → pngquant 压缩 256px PNG 字节（兜底档）。\n",
    );
    manifest_rs.push_str("pub const ENTRIES: &[(&str, &[u8])] = &[\n");
    for (name, path) in &entries {
        manifest_rs.push_str(&format!(
            "    (\"{name}\", include_bytes!(r#\"{}\"#)),\n",
            path.display()
        ));
    }
    manifest_rs.push_str("];\n");
    let gen_path = out_dir.join("icons_gen.rs");
    fs::write(&gen_path, manifest_rs).expect("write icons_gen.rs");
}

/// 密度桶档位（物理像素边长，业界经验：Android mipmap / Windows ico 多分辨率）：
/// 16 = 1x 屏 13/14 逻辑px（WinUI caption 字形即 16 物理 px）；20 = 125%/150% DPI；
/// 24 = 200% DPI；32 / 48 = 列表大图标与 4K 250% 兜底。
const BUCKETS: [u32; 5] = [16, 20, 24, 32, 48];

/// 256px 源 CPU Lanczos3 一次性下采样到 BUCKETS 各档，写 `{name}-{side}.png`。
/// （构建期一次重采样，信息富余、质量远高于运行时 GPU 双线性。）
fn buckets(
    src: &std::path::Path,
    out_icons: &std::path::Path,
    name: &str,
) -> Vec<(String, u32, PathBuf)> {
    let img = image::open(src)
        .unwrap_or_else(|e| panic!("decode {} failed: {e}", src.display()))
        .to_rgba8();
    BUCKETS
        .iter()
        .map(|&side| {
            let dst = out_icons.join(format!("{name}-{side}.png"));
            image::imageops::resize(&img, side, side, image::imageops::FilterType::Lanczos3)
                .save_with_format(&dst, image::ImageFormat::Png)
                .unwrap_or_else(|e| panic!("save bucket {name}x{side} failed: {e}"));
            (name.to_owned(), side, dst)
        })
        .collect()
}

/// pngquant 压缩；失败（质量不达标 / 工具缺失）回退原样拷贝，不让构建挂死。
fn quantize(src: &std::path::Path, dst: &std::path::Path) {
    let pngquant = [
        PathBuf::from(r"D:\dev\pngquant\pngquant.exe"),
        PathBuf::from("pngquant"),
    ]
    .into_iter()
    .find(|c| {
        Command::new(c)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    });

    if let Some(tool) = pngquant {
        let status = Command::new(&tool)
            .args(["--quality=40-95", "--speed", "3", "--force", "--output"])
            .arg(dst)
            .arg("--")
            .arg(src)
            .status();
        if let Ok(s) = status
            && s.success()
            && dst.exists()
        {
            return;
        }
    }
    fs::copy(src, dst)
        .unwrap_or_else(|e| panic!("icon fallback copy {} failed: {e}", src.display()));
}

/// 依次探测：PATH（dev.bat 已注入）→ 固定绿色路径。
fn find_windres() -> PathBuf {
    let candidates = [
        PathBuf::from("windres.exe"),
        PathBuf::from(r"D:\dev\mingw64\bin\windres.exe"),
    ];
    for c in &candidates {
        if let Ok(out) = Command::new(c).arg("--version").output()
            && out.status.success()
        {
            return c.clone();
        }
    }
    panic!(
        "windres.exe not found; expected in PATH or D:\\dev\\mingw64\\bin (see dev.bat / DECISIONS.md D-009)"
    );
}
