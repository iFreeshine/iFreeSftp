// GUI 子系统（对齐 C# 版双击行为）：不声明时链接为控制台子系统，
// 每次启动会额外弹一个黑命令窗。无条件声明——命令窗不再出现。
#![windows_subsystem = "windows"]

use std::sync::Arc;

mod app_paths;
mod editor;
mod hosts;
mod i18n;
mod icons;
mod icons_sys;
mod import;
mod local;
mod persistence;
mod registration;
mod settings;
mod sm3;
mod ssh;
mod theme;
mod transfer;
mod ui;

/// 闭源 ifree-core 唯一并入点（正式发布构建专用，registration/transfer 共用；
/// 目录缺席时绝不编译到本模块——不带 official 的源码零引用完整可编译）。
#[cfg(feature = "official")]
#[path = "../../ifree-core/src/lib.rs"]
pub(crate) mod ifree_core;

const WINDOW_TITLE: &str = "iFreeSftp";
const DEFAULT_SIZE: [f32; 2] = [1280.0, 800.0];
const MIN_SIZE: [f32; 2] = [900.0, 600.0];

/// 拉丁字体：按需从 devSource 落地项目内（src/assets/fonts/），编译期嵌入
/// （D-022：devSource 不给项目提供依赖，运行时零路径引用）。
/// 只装正体一款：egui 0.37 不按 weight 选字体（`strong()` 仅变色），
/// 且 C# 版全程 Regular 无 Bold 用量；SemiBold 仅品牌名用，走独立 Name family。
const LATIN_FONT: &[u8] = include_bytes!("assets/fonts/Inter-Regular.ttf");
/// 品牌名字体（C# 应用名 SemiBold）：注册为独立 family，需要处显式指定。
const BRAND_FONT: &[u8] = include_bytes!("assets/fonts/Inter-SemiBold.ttf");

/// 等宽字体：JetBrains Mono Regular（SIL OFL 1.1，编译期嵌入 D-022，取代
/// 运行时读盘 CascadiaMono→Consolas 探测——Win10/Win11 观感漂移与失配分支
/// 一并消除，与 Inter 同口径）。现有等宽消费点（终端 / 大小列 / 校验码）无
/// 加粗语义，Regular 一款足够；不含 CJK，中文经 CJK_FONT_CANDIDATES 追加
/// Monospace 队尾回退。
const MONO_FONT: &[u8] = include_bytes!("assets/fonts/JetBrainsMono-Regular.ttf");

const CJK_FONT_CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\msyh.ttc",
    r"C:\Windows\Fonts\msyhbd.ttc",
    r"C:\Windows\Fonts\simhei.ttf",
    r"C:\Windows\Fonts\simsun.ttc",
];

fn main() -> eframe::Result {
    // SM3 实现自检（C# Debug 启动 RunSelfTest 同语义）：实现被误改启动即 panic。
    #[cfg(debug_assertions)]
    sm3::debug_self_check();
    // 上次快照：几何 + 主题。无效 / 缺失走默认。
    let saved = persistence::load();
    // 应用设置（settings.json，C# AppSettingsService 同款；先于 State 默认值）。
    settings::load();
    // 主机档案（hosts.json，C# HostStoreService 同款；结构损坏时原文备份，
    // 备份路径存全局通知由主机面板提示，S-10）。
    hosts::load();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(WINDOW_TITLE)
        .with_inner_size(saved.map(|s| [s.w, s.h]).unwrap_or(DEFAULT_SIZE))
        .with_min_inner_size(MIN_SIZE)
        // 1:1 自绘标题栏（C# ExtendsContentIntoTitleBar）：去系统装饰，
        // 最小化/最大化/关闭与拖拽在 ui::header 实现；边缘缩放由 ui::edge_resize
        // 命中区接手（无边框后 OS 缩放边消失），resizable 保持 true 是
        // ViewportCommand::StartResize 的前置条件。
        .with_resizable(true)
        .with_decorations(false);
    if let Some(s) = saved {
        viewport = viewport.with_position([s.x as f32, s.y as f32]);
        if s.maximized {
            viewport = viewport.with_maximized(true);
        }
    }
    // 窗口/任务栏图标（C# AppWindow.SetIcon 对齐）：eframe 默认塞 egui logo，
    // 不覆盖任务栏图标就是错的；exe 资源图标（windres 内嵌）只管资源管理器。
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        WINDOW_TITLE,
        options,
        Box::new(move |cc| {
            let app = App::new(cc, saved);
            Ok(Box::new(app))
        }),
    )
}

struct App {
    st: ui::State,
    /// 已落盘快照（变更检测基线；None = 尚未存过）。
    saved: Option<persistence::WinState>,
    /// 上次落盘时刻（节流）。
    last_save: f64,
}

/// 解 assets/AppIcon.ico 中最大的 PNG 条目为窗口图标数据（生成规则：256 档为 PNG
/// 压缩、其余档为 BMP 32bpp；手写容器解析即可，无需 image 的 ico feature）。
/// None = 解析失败（保持默认图标）。
pub fn window_icon() -> Option<egui::IconData> {
    const ICO: &[u8] = include_bytes!("../assets/AppIcon.ico");
    // ICONDIR 6 字节 + 每条目 16 字节：宽(0=256)、色数、保留、位平面、位深、数据长、偏移。
    let count = u16::from_le_bytes([ICO[4], ICO[5]]) as usize;
    let (mut off, mut len, mut best) = (0usize, 0usize, 0u32);
    for i in 0..count {
        let e = 6 + 16 * i;
        // 宽度字节 0 = 256（ICO 规范），比大小时必须还原；只认 PNG 条目——
        // 条目内 BMP 无 BITMAPFILEHEADER，image::load_from_memory 解不动。
        let w = match *ICO.get(e)? {
            0 => 256,
            w => u32::from(w),
        };
        let entry_len = u32::from_le_bytes(ICO.get(e + 8..e + 12)?.try_into().ok()?) as usize;
        let entry_off = u32::from_le_bytes(ICO.get(e + 12..e + 16)?.try_into().ok()?) as usize;
        if !ICO.get(entry_off..entry_off + 4)?.starts_with(b"\x89PNG") {
            continue;
        }
        if w >= best {
            best = w;
            len = entry_len;
            off = entry_off;
        }
    }
    let img = image::load_from_memory(ICO.get(off..off + len)?)
        .ok()?
        .to_rgba8();
    Some(egui::IconData {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    })
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, saved: Option<persistence::WinState>) -> Self {
        // 明暗两套都写入令牌，末次调用为当前主题：
        // 主题来源 settings.json theme 键（T19-1，C# ThemeService.Initialize 同
        // 语义、与 C# 共用存档互认）："Dark"/"Light" 显式；空/未知 = 跟随系统
        // （theme::system_prefers_light 注册表探测）。window.ini dark 不再作
        // 主题依据（几何快照仍含 dark 字段仅作落盘结构保持）。
        theme::install(&cc.egui_ctx, egui::Theme::Light);
        theme::install(&cc.egui_ctx, egui::Theme::Dark);
        if !settings::startup_theme_dark() {
            theme::install(&cc.egui_ctx, egui::Theme::Light);
        }

        install_fonts(&cc.egui_ctx);

        // 列显隐偏好回灌（T20-1 契约 A：读取独立于窗口几何完整性判据——
        // 几何非法也生效；文件缺失保持全开默认）。
        if let Some(c) = persistence::load_cols() {
            ui::panes::set_col_vis(ui::panes::ColVisibility {
                size: c[0],
                ty: c[1],
                modified: c[2],
                perms: c[3],
            });
        }

        // 注册服务装配（C# App 启动链：Load 已存注册码 + 启动静默拉远程，
        // 24h 节流内走缓存不联网；结果供 Header 更新徽标/设置页消费）。
        crate::registration::service::init(cc.egui_ctx.clone());

        Self {
            st: ui::State::default(),
            saved,
            last_save: 0.0,
        }
    }

    /// 窗口状态变更检测 + 节流落盘（拖动 / 缩放 / 最大化切换 / 主题切换）。
    fn persist_window(&mut self, ctx: &egui::Context) {
        let (inner, maximized, minimized) = ctx.input(|i| {
            let v = i.viewport();
            (
                v.inner_rect,
                v.maximized.unwrap_or(false),
                v.minimized.unwrap_or(false),
            )
        });
        let Some(rect) = inner else {
            return;
        };
        if minimized {
            return; // 最小化边界不是有效几何
        }

        let dark = ctx.theme() == egui::Theme::Dark;
        // 列显隐并入快照（T20-1 契约 A）：PartEq 变更检测 + 0.3s 节流写 window.ini
        // （不写 settings.json——C# AppSettingsService 整体覆写会丢未知键）；
        // 最大化分支同样直读当前全局值（折叠 ..base 的几何延续）。
        let cv = ui::panes::col_vis();
        let base = self.saved.unwrap_or_default();
        let cand = if maximized {
            persistence::WinState {
                maximized: true,
                dark,
                col_size: cv.size,
                col_type: cv.ty,
                col_modified: cv.modified,
                col_perms: cv.perms,
                ..base
            }
        } else {
            persistence::WinState {
                x: rect.left() as i32,
                y: rect.top() as i32,
                w: rect.width(),
                h: rect.height(),
                maximized: false,
                dark,
                col_size: cv.size,
                col_type: cv.ty,
                col_modified: cv.modified,
                col_perms: cv.perms,
            }
        };
        if self.saved == Some(cand) {
            return;
        }
        let now = ctx.time();
        if now - self.last_save < 0.3 {
            // 节流窗口内：预约一次重绘，保证待存快照最终落盘
            //（否则无事件时不再重绘，这帧的变更会一直悬着）。
            ctx.request_repaint_after(std::time::Duration::from_millis(350));
            return;
        }
        persistence::save(&cand);
        self.saved = Some(cand);
        self.last_save = now;
    }
}

/// Inter 正体置 Proportional 队首、品牌 SemiBold 独立 family，
/// JetBrains Mono 置 Monospace 队首（编译期嵌入），中文追加队尾作回退。
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    // 正体插入 Proportional 队首；品牌 SemiBold 注册独立 family（显式选用才生效）。
    // 编译期嵌入（D-022），无运行时文件读取与失配分支。
    fonts.font_data.insert(
        "inter".to_owned(),
        Arc::new(egui::FontData::from_owned(LATIN_FONT.to_vec())),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".to_owned());
    fonts.font_data.insert(
        "inter-semibold".to_owned(),
        Arc::new(egui::FontData::from_owned(BRAND_FONT.to_vec())),
    );
    fonts.families.insert(
        egui::FontFamily::Name("brand".into()),
        vec!["inter-semibold".to_owned()],
    );

    // 等宽编译期嵌入置队首（D-022，JetBrains Mono OFL 1.1）：不再探测系统
    // CascadiaMono/Consolas，跨机一致；中文回退由 CJK 段追加队尾。
    fonts.font_data.insert(
        "mono".to_owned(),
        Arc::new(egui::FontData::from_owned(MONO_FONT.to_vec())),
    );
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, "mono".to_owned());

    for path in CJK_FONT_CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };

        fonts.font_data.insert(
            "cjk".to_owned(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );

        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("cjk".to_owned());
        }

        ctx.set_fonts(fonts);
        return;
    }

    eprintln!(
        "warning: no CJK font found in {}",
        CJK_FONT_CANDIDATES.join(", ")
    );
    ctx.set_fonts(fonts);
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // S-6：DPAPI 解密失败日志投递（load/导入期入全局队列，这里每帧排空）。
        self.st.terminal.extend(crate::hosts::drain_pending_logs());
        // 编辑框全局右键菜单：帧首钩子必须先于 root 里的一切控件
        // （拦截 Secondary press / 注入菜单动作），帧尾画浮层菜单。
        self.st.ctx_menu.frame_start(ui.ctx());
        ui::root(ui, &mut self.st);
        self.st.ctx_menu.frame_end(ui.ctx());
        let modal = ui::frame_modal(&self.st);
        ui::edge_resize(ui, modal);
        // F-8：主视口帧尾唯一一次光标写入（须在 edge_resize 之后）。
        ui::cursor_decide(ui.ctx());
        self.persist_window(ui.ctx());
    }
}
