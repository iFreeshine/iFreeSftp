# MIGRATION — C# → Rust 公开仓库迁移与发版规则

> 本文档是 `github/ifreeSftp`（公开 AGPL 仓库）的**唯一更新规则**。每次更新本仓前先读这里，
> 机械步骤交给 `..\sync-to-github.ps1`，判断步骤（版本号、README、CHANGELOG）按本文执行。
> 规则变更先改本文再动手——文档与实际不一致视为 bug。

## 1. 仓库边界

```
E:\aiProject\iFreeSftpRust\            主开发仓（私有，含闭源与素材）
├── ifreeSftp\                         Rust workspace 根 = 公开源码本体
│   ├── src\                           全部公开（official 门控代码带 #[cfg]，无 ifree-core 也能编译）
│   ├── build.rs / Cargo.toml / Cargo.lock / rust-toolchain.toml / .cargo\  公开
│   ├── README.md / DECISIONS.md / docs\ / templates\ / dev.bat             主仓自用，【不公开】
├── ifree-core\                        闭源注册验签核心，【永不公开】
├── tools\worker\                      Cloudflare Worker 运营服务端（吊销/统计），【永不公开】
├── devSource\ / assets\               设计素材，【不公开】
└── github\
    ├── sync-to-github.ps1             主仓 → 公开仓 同步脚本（唯一同步入口）
    └── ifreeSftp\                     ← 本目录 = 公开仓库工作副本（独立 git 仓库）
```

**排除规则（白名单制，不做黑名单）**：`sync-to-github.ps1` 只拷贝白名单里的东西，
主仓新增目录默认不进公开仓；想公开某个新目录，先改本文第 2 节再改脚本。

## 2. 同步白名单（sync 脚本的拷贝清单）

| 主仓来源（ifreeSftp\ 下） | 公开仓去处 | 说明 |
|---|---|---|
| `src\` | `ifreeSftp\src\` | 全量镜像（official 门控零引用，见 §6） |
| `build.rs` `Cargo.toml` `Cargo.lock` | 同名 | Cargo.toml 原样（workspace 根与 member 同文件，公开仓自足） |
| `rust-toolchain.toml` `.cargo\config.toml` `.gitattributes` `.gitignore` | 同名 | 工具链锁定 + windows-gnu 链接配置（D-019）；同步改写：config.toml 剥离主仓 `[build] target-dir`（私有产物目录策略），rust-toolchain.toml 的 channel 由 host 三元组形式改写为 `1.99` + gnu targets（CI / fork 侧 rustup host 是 msvc，会报 "target tuple in channel name"） |
| `assets\` | 同名 | build.rs 内嵌图标/Logo 资源（缺目录编译失败） |
| `docs\zh-CN.png` 等图标/静态资源 | 同名 | 若后续新增 |
| `E:\aiProject\iFreeSftp\github\LICENSE` | `LICENSE` | AGPL-3.0 沿用 C# 版 |
| `E:\aiProject\iFreeSftpRust\devSource\logo-sftp.png` | `logo.png` | 品牌沿用（2026-11 新版 SFTP 意象 logo） |

**不拷（公开仓自有/独占）**：`README.md`、`README.zh-CN.md`、`CHANGELOG.md`、
`CHANGELOG.zh-CN.md`、`build.ps1`、`build.cmd`（dev loop，Rust 版重写）、`tools\`、
`.github\`、`docs\screenshots\`、本文件 `MIGRATION.md`——这些在公开仓独立维护，
sync 永不覆盖。主仓 `.gitignore` 已忽略 `/github/`（本目录是独立 git 仓库）。

**不拷（永不公开）**：主仓 `README.md`（开发向）、`DECISIONS.md`、`docs\`（IMPL-*/HANDOVER/
发版清单等内部文档）、`templates\`、`scripts\`、`dev.bat`（内含本机路径）、`ifree-core\`、
`tools\`（含 worker 运营服务端）、keygen。

## 3. README / CHANGELOG 改写规则（C# 文字 → Rust 事实）

继承 C# 版（`E:\aiProject\iFreeSftp\github\`）的文档骨架与文案风格，但**逐条事实替换**，
不许照抄 C# 能力宣称——「文案承诺了代码没做的事」是本项目红线：

| C# 表述 | Rust 版表述 | 依据 |
|---|---|---|
| WinUI 3 / Windows App SDK | Rust + egui（eframe 0.37） | Cargo.toml |
| .NET 10 SDK 构建 | Rust 1.99（stable-x86_64-pc-windows-gnu），`.\build.ps1` | rust-toolchain.toml |
| SSH.NET 2026.0.0 | russh 0.65 + russh-sftp 3.0（ring 后端） | Cargo.toml |
| zip 内 `iFreeSftp.exe + iFreeSftp.Core.dll + wpf\ + locales\` | **单 exe**（0 依赖定案 D-019，导入表仅系统 DLL） | D-019 |
| tar 加速打包/解包（本机 bsdtar 子进程 + 服务端 tar） | 纯 Rust 内嵌（`tar` crate + `flate2` gzip，零子进程零 C 依赖）；**有意优于 C#**：tar 头按 PAX/UTF-8 写，中文文件名不依赖系统代码页——C# 的 bsdtar 在 Windows 按系统代码页解释文件名，非 UTF-8 环境有乱码风险；远端打包/解包命令仍走服务端 tar | src/transfer/tar.rs |
| **服务端直达远程→远程（一条 cp）** | **不得宣称已实现**——当前同主机经本机中转（tar 打包单流加速）；服务端直达在路线图（core `ServerCopyAsync` 移植项） | ifree-core ITransferCore 未移植 |
| AvalonEdit 语法高亮 | 内置编辑器（egui）+ 远程编辑回传 + mtime 冲突检测；语法高亮未实现不写 | src/ui/editor_window.rs |
| DPAPI 加密存储 | 密码加密存储（不写具体算法，按实现更新） | hosts/settings |
| Mica 材质 | 不写（未实现） | — |
| 服务端校验码 SHA-256/512 + SM3 | 服务端 SHA-256/512（sha256sum/openssl）；SM3 仅当服务端有 openssl；Windows 服务器不支持；本地 SHA-256/512 | 能力矩阵 |
| 注册验签在 Core.dll | 正式发布构建内置离线验签（ECDsa P-256）；公开仓收紧版源码自编译的构建无激活入口（`IsFullImplementation=false` 口径） | registration/core.rs |

**能力矩阵（公开口径）必须整表随 README 发布**——「不支持什么」与「支持什么」同等重要，
来源：主仓 `ifreeSftp\README.md` 的能力矩阵节，更新时同步。

版本徽章数字、对比表、功能表在每次发版随 CHANGELOG 一起核对（见 §5）。

### 3.1 有意安全差异：远程状态缓存剔除购买/更新链接（2026-11-09 定案）

- **C# 事实**：`RegistrationService.RefreshAsync` 把 `/v1/state` 应答**原文全量**落缓存
  `remote.json`（RegistrationService.cs:135-137）；24h 节流命中时把缓存整体
  `ApplyRemoteState`（:118-127）——购买链接与更新 URL 随缓存恢复。
- **Rust 差异（有意为之）**：`registration/service.rs` 的 `write_cache` 落盘前剔除
  `purchase` 整块与 `update.url`（保留公告/勋章/更新版本号/吊销名单）；缓存命中的会话
  这些字段维持默认，**不随缓存恢复**。仅 fresh（联网拉取）应答才应用链接，且
  `update.url` 入口过 `guard::is_safe_web_url`、消费点另有 UrlGuard 拦截（双重防御）。
- **理由**：缓存文件位于用户目录，可被篡改；「缓存恢复的购买/更新链接」是钓鱼页注入面。
  剔除后篡改者只能删信息不能注信息，与 guard.rs 仅 http(s) 协议白名单自洽。
- **将来若恢复缓存链接**，必须同时满足：缓存 URL 同样过 guard + UI 显示目标域名
  （让用户看见即将跳往何处），缺一不可。

### 3.2 有意差异补记（T19-5，2026-11-09 拍板定稿，均维持 Rust 现状）

- **远程工具栏「新建」**：C# `FileView.xaml.cs:2088-2093` 远程工具栏 New 按钮禁用
  → Rust **启用**（远程目录下新建目录/文件可用）→ 维持理由：远程新建是正当需求，
  C# 禁用属历史局限而非能力边界，公开 README 不作远程不支持新建的声明。
- **本地「编辑」≤10MB 上限**：C# `FileView.xaml.cs:2443`（入口）与 `:2736-2756`
  （编辑判定仅 8KB 文本探测、无总体积限制）→ Rust 加 **10MB** 上限，超限
  「编辑」置灰，仍可经「打开」用系统默认关联程序处理 → 维持理由：防大文件
  整读卡死内置编辑器（egui 全量驻内存），体验换稳定；「打开」为既有通用入口
  （dialogs.rs:4241 恒可用），对超限内容同样可用。
- **远程右键「新建」门槛**：C# `FileView.xaml.cs:2467` 仅判 `IsConnected` 即可
  → Rust 须**已浏览到目录**（本次会话列出过目录项）→ 维持理由：未列目录时无
  落点上下文（新建目标路径未知），放宽只会制造报错路径；已列目录后功能等同。
- **列显隐状态作用域**（T20-1）：C# 左右窗格列状态独立（`FileView.xaml.cs:134`
  FileColumnState per-view）→ Rust **全局一份**（左右窗格与新建 tab 全部继承；
  列宽/拖拽几何/权限列仅远程显示的本地态仍 per-pane）→ 维持理由：跨窗格一致的
  列视图符合双窗格对比浏览场景，C# 分侧记忆在双窗格同览时反成噪音。
- **设置面板长描述**（F-6，2026-11-11）：设置卡说明不再逐字对齐 C# Resources.resw
  ——以 Rust 当前实现为事实基线做语义纠偏与精简（如 tar 本地内嵌打包、并发独立
  连接、校验算法可用性顺位），C# 原句仅作历史参考；主题下拉补「跟随系统」第三项
  （store theme = "" 同语义，对齐 C# ThemeService 空值）。

## 4. 发版规则（继承 C# 项目区）

1. **版本基准唯一**：`ifreeSftp\Cargo.toml` 的 `[workspace.package] version`（当前 0.9.0）。
   其余声明点全部向它对齐：README 双语徽章 `badge/version-x.y.z-`、CHANGELOG 双语 `## [x.y.z]` 节。
2. **发版前必跑** `.\tools\check-version.ps1`（build.ps1 release 已自动挂钩，不一致即 FAIL 阻塞）；
   官网 website\ 目录缺失时对应项自动 SKIP，README/CHANGELOG 项缺失 FAIL。
3. **发布物（2026-11-08 口径更新，DECISIONS.md D-018/D-021）**：产品唯一、按平台分 3 个发布版本，
   **3 版全部为 official 构建（含注册功能）**，全部在主仓本机编译后**手动上传**——公开 CI
   不持有 ifree-core，不出正式发布构建（与 C#「发布 exe 不走 CI」同规则）：
   - `iFreeSftp-vX.Y.Z-win-x64.zip`（内含单个 official `ifreeSftp.exe`）→ GitHub Release；
   - 微软商店包 `iFreeSftp-X.Y.Z.0-x64.msix`（**主仓** `tools\make-msix.ps1 -Official` 产出——
     打包工具与 worker 同属运营工具，永不进公开仓；未签名，Store
     提交时 Partner Center 重签）；
   - Linux 版 `iFreeSftp-vX.Y.Z-linux-x64.tar.gz`（official 构建；主仓本机 zigbuild 交叉编译，
     glibc 目标 2.39 = Ubuntu 24.04，见主仓 DECISIONS.md D-018/D-021）。
   公开仓 `.\build.ps1 release` 的产物是**公示源码验证构建**（无 official feature），
   不作为发布物上传。商店包身份沿用 C# 版 Partner Center 真实值
   （`iFrees.iFreeSftp` / `CN=029857DD-…`），提交时无需改。
4. **Tag 规则**：`vX.Y.Z`，与版本基准一致；Release 页文案从 CHANGELOG 对应节复制；
   Partner Center 提交用同一版本号（make-msix 自动把 `0.9.0` 归一为 `0.9.0.0`）。
5. **CHANGELOG 遵循 Keep a Changelog / SemVer**；用户可见变更逐条记录，Rust 版从 0.9.0 起
   （C# 历史见 C# 版仓库 CHANGELOG，不合并）。
6. **微软商店流程**（包全自动，提交人工）：主仓 `.\tools\make-msix.ps1 -Official` 一条命令产出
   可提交的 MSIX（构建 → 图标齐套校验 → AppxManifest 注入 → makepri 生成 resources.pri →
   MakeAppx 打包）。Partner Center 手动步骤：登录 → 提交新版本 → 上传 msix →
   Store listing 沿用 C# 版商店文案/截图（截图仍需 Rust 界面重截）→ 提交审核。
   本地冒烟装包：主仓 `.\tools\make-msix.ps1 -Sign` 后按脚本输出两行命令导入证书安装。

## 5. 每次更新操作流程（checklist）

**发版人工项提醒**：版本号变更时必须提醒用户同步 Cloudflare Worker `ifree-reg` 的
`UPDATE_VERSION` 环境变量（Dashboard 操作，仓库外配置；漏设则更新提醒静默失效）——
详见主仓 `docs\发版清单.md` §8。

**本机构建前置**：windows-gnu 工具链编译含 raw-dylib 的依赖（windows-*、getrandom 等）
需要 mingw-w64 binutils——`dlltool.exe` 必须在 PATH（本机 `D:\dev\mingw64\bin`，与链接器
配置同源）；GitHub runner（windows-latest）自带，公开仓 CI 无需额外配置。首次从零编译
报 `error calling dlltool 'dlltool.exe': program not found` 即此因。

1. 主仓开发、验收（cargo test 双模式绿 + fmt 0 diff）。
2. 若涉及能力/文案变化 → 按本文 §3 修改公开仓 README 双语；用户可见变更 → 补 CHANGELOG
   双语（新节或 Unreleased 节）。
3. 改版本号时：Cargo.toml（主仓）→ 同步 → README 徽章 ×2 → CHANGELOG 节 ×2，跑 check-version。
4. 跑 `..\sync-to-github.ps1`（源码镜像，幂等）。
5. 公开仓内验证：`.\build.ps1 build`（源码模式克隆可编译）+ `cargo test --offline`。
6. 公开仓 git commit；发版时打 tag、上传发布物（Win64 zip / MSIX / Linux tar.gz，均为
   主仓 official 构建产物，见 §4-3）、贴 CHANGELOG。

## 6. 闭源隔离约束（official 门控）

- `src\main.rs` 顶层的 `#[cfg(feature = "official")] #[path = "../../ifree-core/src/lib.rs"]`
  （`pub(crate) mod ifree_core`）是**唯一**闭源并入点，registration 与 transfer 两层共用；
  源码模式不启用该 feature，`#[path]` 指向的目录在公开仓不存在也零编译。
- 公开仓 CI 只跑源码模式（`cargo build && cargo test`）；任何把 ifree-core 源码、私钥、
  keygen 引入本仓的改动都是违规，直接拒绝。
- 服务端直达复制（core `ServerCopyAsync`，C# FullTransferCore）已移植：决策逻辑
  （命令组装/CP_OK 判定/600s 超时）在 `ifree-core\`（闭源），trait/Stub/调度在主仓
  （公开，`src\transfer\core.rs`）；源码编译版显式提示激活组件未随源码分发、改走两步
  传输，保底路径 = 本地中转。公开仓 README 对应能力行在正式发布版**发布后**才可改为「已支持」。

## 7. 未尽事宜（open items，处理一条勾一条）

- [ ] **截图**：`docs\screenshots\` 目前空缺——C# 截图是 WinUI3 界面不适用，需 Rust 界面重截
      （主窗口深/浅主题、设置页、导入向导、传输队列，PNG 存本目录后 README 引用生效）。
- [ ] **仓库归宿**：沿用 `github.com/iFreeshine/iFreeSftp` 覆盖 C# 版，还是新开仓库？
      未定前 README 的 Releases/官网链接按沿用填写，定案后如变更需改 README ×2 处链接。
- [ ] **首个 Release 实测**：tag → 手动上传 zip → 下载验签（objdump 导入表核对 D-019）全流程未走过一遍。
- [x] ~~MSIX 打包~~ **已继承**（脚本与模板/图标资产住**主仓** `tools\{make-msix.ps1, msix\}`——
      与 worker 同为运营工具不公开；0.9.0.0 商店包已产出并解包验证）；
      商店提交为人工步骤（见 §4.6）。Store listing 的截图仍依赖本清单第 1 条。
- [ ] **官网联动**：ifrees.top 下载链接/changelog 页是否同步 Rust 版版本号（check-version 的
      website 项现为 SKIP，接通后从 SKIP 变 FAIL 校验）。
- [ ] **隐私政策页**：C# README/导入向导提及隐私政策链接，Rust 公开仓需对应页面或链接。
- [ ] **对比表更新**：对比表「服务端直达」行标为「规划中」，core ServerCopy 移植发布后改回「✓」
      （§6），并同步官网对比区（如有）。
- [ ] **源码编译版激活口径**：README 已写明激活模块仅含于 ifrees.top 发布版，需在注册页
      UI 文案再核对一遍（源码编译版打开注册页的提示与 README 一致）。
- [ ] **同步脚本回归**：sync-to-github.ps1 首跑已验证；主仓新增公开目录时更新本文 §2 白名单并重跑。
