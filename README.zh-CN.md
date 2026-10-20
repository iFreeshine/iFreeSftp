# iFreeSftp

[English](README.md) | 简体中文

基于 WinUI 3 的桌面 SFTP 客户端——双窗格、多标签，快速、安全、完全免费。

![Version](https://img.shields.io/badge/version-0.8.5-4C8BF5) ![License](https://img.shields.io/badge/license-AGPL--3.0-4C8BF5) ![Platform](https://img.shields.io/badge/platform-Windows-4C8BF5)

官网 <https://ifrees.top> · 发布页 <https://github.com/iFreeshine/iFreeSftp/releases> · 赞助 <https://afdian.com/a/ifree>

> **免费承诺**：全部功能对所有用户永久免费开放。注册勋章纯属自愿赞助，不解锁、不限制任何功能。
>
> **许可证**：本应用以 [GNU AGPL-3.0](https://www.gnu.org/licenses/agpl-3.0.html) 许可证开源发布，克隆仓库即可完整编译。

---

## 为什么选 iFreeSftp

- **左右双窗格 + IDE 式多标签**——两侧各自独立多开 tab（本地或远程），多主机并行操作不必来回切窗口
- **服务端直达传输**——同主机远程→远程复制由服务器一条 `cp` 直接完成：客户端零流量，大目录整树一次到位，没有逐文件往返
- **主机一键导入**——换工具？主机一键带走：从 WinSCP、FileZilla、PuTTY、OpenSSH config 先授权后扫描，预览去重、勾选即入；只搬连接信息，密码绝不读取
- **默认安全**——首次连接确认主机指纹（SHA256）并记住，服务器密钥变化直接拒绝（防中间人）；凭据与注册数据 DPAPI 加密存储
- **内置编辑器 + 远程编辑回传**——下载临时副本、编辑、保存自动上传；远端文件被他人改过会弹 mtime 冲突确认
- **真正的文件管理器手感**——四方向拖拽传输、橡皮筋框选、同步浏览文件比对、带逐文件进度与失败重试的上传队列
- **服务端工具随取随用**——九宫格 chmod 实时拉取、SHA-256 / SHA-512 / SM3 校验码服务器端即算、主机命令窗完整会话日志
- **连接细节讲究**——连接/操作超时、重试与延迟、keep-alive；报错同步显示在状态栏与命令行面板，绝不无限挂起
- **自包含单文件 exe**——无需安装任何运行时；深浅主题、中英文运行时切换

## 与同类软件对比

对照 Windows 主流 SFTP 客户端（依据各软件官方公开文档整理，2026 年 10 月——如有出入欢迎 issue 指正）：

| | iFreeSftp | WinSCP | FileZilla | MobaXterm |
|---|---|---|---|---|
| 许可 | AGPL-3.0，免费 | GPL，免费 | GPL，免费 | 免费增值（Home 版有会话数限制，Pro 收费） |
| 双窗格 + **两侧**独立多标签 | ✓ | 会话有标签、窗格固定 | 窗格固定、无标签 | 终端多标签 + 侧栏 SFTP 浏览器 |
| **服务端直达远程→远程** | ✓ 同主机一条 `cp` 复制整棵树——零流量经过本机 | 经本机逐文件重传 | ✗ 不支持 | ✗ 不支持 |
| 远程编辑 + 保存自动回传 | ✓ | ✓ | △ 保存时提示上传 | △ 经 SFTP 浏览器编辑 |
| 同步浏览文件夹比对 | ✓ | ✓（同步浏览） | ✗ | ✗ |
| 服务端校验码（SHA-256/512、SM3） | ✓ | ✓ | ✗ | ✗ |
| 免安装单文件 exe | ✓ | ✓ | △ | ✓ |

差异化的关键一行是服务端直达传输：当两侧窗格指向同一主机时，iFreeSftp 通过服务端命令通道下发一条 `cp` 命令，上万文件的目录树以服务器磁盘速度完成复制，而不是在你的电脑上绕一圈逐文件往返。

## 下载

从 [GitHub Releases](https://github.com/iFreeshine/iFreeSftp/releases) 或[官网](https://ifrees.top)获取最新 `iFreeSftp-vX.Y.Z-win-x64.zip`：

```
iFreeSftp.exe          # 单文件主程序（自包含, 免装运行时）
iFreeSftp.Core.dll     # 核心组件（传输与注册）
wpf\                   # 内置编辑器 WPF 集群
locales\               # en-US / zh-CN 语言词典——可直接编辑翻译新语言
```

解压到任意目录运行 `iFreeSftp.exe`；或从源码构建（见下）。

> 请仅从上述官方渠道下载：单文件 exe 可被重打包篡改，第三方转载无完整性保证（与本应用组件的混淆保护无关）；解压目录内的 `iFreeSftp.exe` 与 `iFreeSftp.Core.dll` 请勿单独替换。

## 截图

![主界面](docs/screenshots/app-main-dark.png)

![浅色主题](docs/screenshots/app-main-light.png)

![设置](docs/screenshots/app-settings.png)

## 从源码构建

依赖：Windows 10 1809+ / Windows 11，[.NET 10 SDK](https://dotnet.microsoft.com/download/dotnet/10.0)。

```powershell
.\build.ps1 build      # Debug 编译（克隆即可编译）
.\build.ps1 run        # 编译并运行
.\build.ps1 release    # 单文件发布 -> release\win-x64\iFreeSftp.exe
.\build.ps1 clean      # 清理
```

## 仓库结构

```
src/
├── iFreeSftp/                    # WinUI 3 主程序
├── iFreeSftp.Core.Abstractions/  # 核心模块契约
├── iFreeSftp.Core.Stub/          # 核心模块实现
└── iFreeSftp.WpfEditor/          # 内置编辑器（WPF AvalonEdit）
tools/
└── export-locales.ps1            # resw -> 语言词典 JSON（发布管线用）
docs/screenshots/                 # 截图
build.ps1 / build.cmd             # 构建脚本
```

## 功能

| 模块 | 功能 |
|---|---|
| 主界面 | **左右双窗口 + 多标签页**：左右两个文件夹窗口各自独立多开 tab（IDE 式标签条），每个 tab 都可浏览本地或连接远程；上传队列、主机命令窗（连接日志输出） |
| 连接 | SSH.NET 真实连接，密码或私钥文件认证；支持 keep-alive |
| 主机管理 | 主机卡片面板：新增/编辑/复制/删除、拖拽排序、右键菜单、单击卡片即连接、hover 编辑钮 |
| 会话导入 | 一键导入 WinSCP（INI/注册表）、FileZilla 站点管理器、PuTTY、OpenSSH `~/.ssh/config` 与 iFreeSftp 自有存档；先同意后扫描、预览判重；口令与登录后命令绝不导入——只导连接元数据 |
| 连接安全 | 首次连接弹窗确认主机指纹（SHA256），确认后记住并自动校验；服务器密钥变化直接拒绝，防中间人 |
| 连接参数 | 连接超时、操作超时（0=禁用；报错同步显示在状态栏与命令行面板）、最大重试次数、登录重试延迟、keep-alive 开关与间隔 |
| 连接打开方式 | 点击主机连接时询问在右窗口当前 tab 还是新 tab 打开（可"总是"）；**先连接、成功才落位** |
| 传输 | 四方向拖拽/右键传输；同主机远程→远程走服务端直达，失败自动回退；目录传输逐文件展开入队（独立进度/失败记账）；传输队列含等待/失败页签与整批重跑 |
| 设置面板 | 左侧大类（外观/连接/文件编辑/传输/注册）+ 右侧表单；修改即时生效并落盘 |
| 注册勋章 | 设置 → 注册：注册码激活（离线 ECDsa P-256 验签 + DPAPI 存储）、金色"支持者"勋章、远程公告；**注册与免费版功能无差别** |
| 本地化 | 中文 / English 运行时切换（MRT Core + resw），全部文案双语 |
| 主题 | 深 / 浅主题切换；首次未配置时跟随 Windows 系统主题 |
| 文件编辑 | 内置编辑器（WPF AvalonEdit）：内容探测启用"编辑"菜单（头部 8KB，不看扩展名）、语法高亮、行号、查找、撤消、保留原编码写回；也可改用外部编辑器 |
| 远程文件编辑 | 下载临时副本 + 保存自动回传；mtime 冲突检测确认；按主机清理临时目录 |
| 右键菜单 | 本地全量菜单（四方向传输/编辑/重命名/删除/新建/刷新）；远程模式追加九宫格 chmod（实时拉取）与 SHA-256/SHA-512/SM3 校验码 |
| 文件比对 | 同步浏览联动；按名称对齐两侧列表，单侧存在（Caution）与大小/mtime 差异（Critical）行高亮 |
| 框选 | 两侧列表橡皮筋框选多选，Ctrl 增选 |
| 其他 | 主机命令窗实时日志、窗口尺寸记忆、ESC/遮罩关闭面板、Windows 系统真实图标（Vanara SHGetFileInfo） |

## 技术框架

桌面应用，无服务器后端；分层即"前后端"：

- **UI 层**：WinUI 3 / Windows App SDK——`Views/`（Header、MainContainer、FileView、HostPanel、SettingsPanel、TerminalContainer）+ `Controls/`
- **内置编辑器**：独立 WPF 类库（`UseWPF` + AvalonEdit 6.3），专用 STA 线程承载（WinUI 3 窗口无法内嵌 WPF 内容）
- **服务层**：静态服务类（无 DI、单例语义）+ `Models/`（HostProfile）+ JSON 持久化
- **运行时**：.NET 10（`net10.0-windows10.0.26100.0`），非打包自包含发布（单 exe）
- **SSH**：SSH.NET `2026.0.0`（不支持 PuTTY `.ppk` 私钥）
- **核心模块**：`iFreeSftp.Core.Abstractions` 定义模块契约；`iFreeSftp.Core.Stub` 为默认实现；发布版附带 `iFreeSftp.Core.dll`

## 注册 / 支持者勋章

设置 → 注册：离线验签注册码、金色"支持者"勋章、远程公告。**注册与免费版功能无差别**——勋章纯荣誉。验签在核心模块内离线完成（ECDsa P-256）。注册码由作者经赞助渠道发放。

## 赞助

如果 iFreeSftp 帮你省了时间，欢迎赞助支持开发——附带应用内金色**支持者**勋章：

- 中国大陆：[爱发电 · 注册码商品页](https://afdian.com/item/3a69f926c4ac11f1878052540025c377)（微信 / 支付宝；未登录可能显示不全，也可从[爱发电主页](https://afdian.com/a/ifree)进入）
- 国际：[Ko-fi Commissions · 注册码商品页](https://ko-fi.com/ifrees/commissions?commissionAlias=61b90e1115)

赞助后在 Ko-fi Commissions 或爱发电留言期望的勋章名，作者回发签名注册码（设置 → 注册 激活）。

## 许可证

[GNU AGPL-3.0](LICENSE) — Copyright © 2026 [ifrees.top](https://ifrees.top)
