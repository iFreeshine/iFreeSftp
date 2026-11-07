# iFreeSftp

[English](README.md) | 简体中文

桌面 SFTP 客户端——双窗格、多标签，快速、安全、完全免费。Rust + egui 原生实现，单文件 exe、零随包运行时。

![Version](https://img.shields.io/badge/version-0.9.0-4C8BF5) ![License](https://img.shields.io/badge/license-AGPL--3.0-4C8BF5) ![Platform](https://img.shields.io/badge/platform-Windows-4C8BF5)

官网 <https://ifrees.top> · 发布页 <https://github.com/iFreeshine/iFreeSftp/releases> · 赞助 <https://afdian.com/a/ifree>

> **免费承诺**：全部功能对所有用户永久免费开放。注册勋章纯属自愿赞助，不解锁、不限制任何功能。
>
> **许可证**：本应用以 [GNU AGPL-3.0](https://www.gnu.org/licenses/agpl-3.0.html) 许可证开源发布，克隆仓库即可完整编译（社区构建）。

---

## 为什么选 iFreeSftp

- **左右双窗格 + IDE 式多标签**——两侧各自独立多开 tab（本地或远程），多主机并行操作不必来回切窗口
- **tar 打包加速目录传输**——大目录在服务端打成单一流、本地解包（上传反向同理）：上万文件的目录树一次往返到位，不再逐文件 chatter；文件数不达标或失败自动回退逐文件传输
- **断点续传 + 传输后校验**——中断的传输从字节断点继续；可选的传输后哈希比对（服务端 `sha256sum`/`sha512sum`/`openssl` SM3，走 exec 通道）尽早暴露损坏文件
- **主机一键导入**——换工具？主机一键带走：从 WinSCP、FileZilla、PuTTY、OpenSSH config 先授权后扫描，预览去重、勾选即入；只搬连接信息，密码绝不读取
- **默认安全**——首次连接确认主机指纹（SHA256）并记住，服务器密钥变化直接拒绝（防中间人）；密码加密存储
- **内置编辑器 + 远程编辑回传**——下载临时副本、编辑、保存自动上传；远端文件被他人改过会弹 mtime 冲突确认
- **真正的文件管理器手感**——拖拽传输、橡皮筋框选、同步浏览文件比对、带逐文件进度与失败重试的传输队列
- **服务端工具随取随用**——九宫格 chmod 实时拉取、服务端校验码即算、主机命令窗完整会话日志；目录删除优先服务端 `rm -rf`（秒级），不可用时回退 SFTP 递归
- **连接细节讲究**——连接/操作超时、重试与延迟、keep-alive；报错同步显示在状态栏与命令行面板，绝不无限挂起
- **单文件 exe 免安装**——一个文件跑起来（导入表实测仅 Windows 系统 DLL）；深浅主题、中英文运行时切换

## 与同类软件对比

对照 Windows 主流 SFTP 客户端（依据各软件官方公开文档整理，2026 年 10 月——如有出入欢迎 issue 指正）：

| | iFreeSftp | WinSCP | FileZilla | MobaXterm |
|---|---|---|---|---|
| 许可 | AGPL-3.0，免费 | GPL，免费 | GPL，免费 | 免费增值（Home 版有会话数限制，Pro 收费） |
| 双窗格 + **两侧**独立多标签 | ✓ | 会话有标签、窗格固定 | 窗格固定、无标签 | 终端多标签 + 侧栏 SFTP 浏览器 |
| 服务端直达远程→远程 | △ 规划中——同主机复制暂经本机中转（tar 打包单流） | 经本机逐文件重传 | ✗ 不支持 | ✗ 不支持 |
| 远程编辑 + 保存自动回传 | ✓ | ✓ | △ 保存时提示上传 | △ 经 SFTP 浏览器编辑 |
| 同步浏览文件夹比对 | ✓ | ✓（同步浏览） | ✗ | ✗ |
| 服务端校验码（SHA-256/512、SM3） | ✓（SM3 仅服务端 openssl） | ✓ | ✗ | ✗ |
| 免安装单文件 exe | ✓ | ✓ | △ | ✓ |

## 下载

从 [GitHub Releases](https://github.com/iFreeshine/iFreeSftp/releases) 或[官网](https://ifrees.top)获取最新 `iFreeSftp-vX.Y.Z-win-x64.zip`：

```
ifreeSftp.exe           # 单文件 exe——就这一个（无运行时、无随包 DLL）
```

解压到任意目录运行 `ifreeSftp.exe`；或从源码构建（见下）。

> 请仅从上述官方渠道下载：第三方重打包无完整性保证。

## 截图

<!-- TODO：Rust 界面截图待补——见 MIGRATION.md §7；勿复用 C#/WinUI 3 截图。 -->

## 从源码构建

依赖：Windows 与 [Rust](https://rustup.rs) 1.99（通道 `stable-x86_64-pc-windows-gnu`，由 `rust-toolchain.toml` 锁定）。

```powershell
.\build.ps1 build      # Debug 编译（克隆即可编译）
.\build.ps1 test       # cargo test
.\build.ps1 run        # 编译并运行
.\build.ps1 release    # Release 编译 -> release\ifreeSftp-<版本>-win64.exe（先跑版本防漏校验）
.\build.ps1 clean      # 清理
```

说明：`official` cargo feature（离线注册验签）依赖私有闭源模块，**不在本仓库内**；社区构建
不带该 feature 完整可编译，仅无激活入口。

## 仓库结构

```
ifreeSftp/                     # Rust workspace（src/ + build.rs + Cargo.toml + 工具链锁定）
tools/
└── check-version.ps1          # 版本一致性防漏校验（Cargo.toml <-> README 徽章 <-> CHANGELOG）
.github/workflows/build.yml    # CI：push/PR 社区模式编译 + 测试
docs/screenshots/              # 截图
build.ps1                      # 构建脚本
MIGRATION.md                   # 本仓库更新与发版规则（动手前先读）
```

## 功能

| 模块 | 功能 |
|---|---|
| 主界面 | **左右双窗口 + 多标签页**：左右两个文件夹窗口各自独立多开 tab（IDE 式标签条），每个 tab 都可浏览本地或连接远程；传输队列、主机命令窗（连接日志输出） |
| 连接 | russh（ring 后端）真实连接，密码或私钥文件认证（OpenSSH/PEM）；支持 keep-alive |
| 主机管理 | 主机卡片面板：新增/编辑/复制/删除、单击卡片即连接、活动高亮 |
| 会话导入 | 一键导入 WinSCP（INI/注册表）、FileZilla 站点管理器、PuTTY、OpenSSH `~/.ssh/config` 与 iFreeSftp 自有存档；先同意后扫描、预览判重；口令与登录后命令绝不导入——只导连接元数据 |
| 连接安全 | 首次连接弹窗确认主机指纹（SHA256），确认后记住并自动校验；服务器密钥变化直接拒绝，防中间人 |
| 连接参数 | 连接超时、操作超时（0=禁用；报错同步显示在状态栏与命令行面板）、最大重试次数、登录重试延迟、keep-alive 开关与间隔 |
| 传输 | 拖拽/右键传输；目录传输文件数达阈值自动走 tar 打包单流（失败/不达标回退逐文件）；断点续传；传输队列含等待/失败页签与整批重跑；目录环与 128 层深度防护 |
| 传输后校验 | 可选：服务端哈希走 exec 通道（SHA-256/512，SM3 经 openssl）；不一致任务标记失败；命令通道不可用时跳过（见能力矩阵） |
| 设置面板 | 左侧大类（外观/连接/文件编辑/传输/注册）+ 右侧表单；修改即时生效并落盘 |
| 注册勋章 | 官方构建：设置 → 注册，注册码激活（离线 ECDsa P-256 验签）、金色"支持者"勋章、远程公告；**注册与免费版功能无差别** |
| 本地化 | 中文 / English 运行时切换，全部文案双语 |
| 主题 | 深 / 浅主题切换 |
| 文件编辑 | 内置编辑器：内容探测启用"编辑"菜单（头部 8KB，不看扩展名）、行号、查找、撤消；也可改用外部编辑器 |
| 远程文件编辑 | 下载临时副本 + 保存自动回传；mtime 冲突检测确认；按主机清理临时目录 |
| 右键菜单 | 本地全量菜单（传输/编辑/重命名/删除/新建/刷新）；远程模式追加九宫格 chmod（实时拉取）与 SHA-256/SHA-512/SM3 校验码 |
| 文件比对 | 同步浏览联动；按名称对齐两侧列表，单侧存在（Caution）与大小/mtime 差异（Critical）行高亮 |
| 框选 | 两侧列表橡皮筋框选多选，Ctrl 增选 |

## 技术框架

桌面应用，无服务器后端；单一 workspace crate：

- **UI 层**：egui 0.37 / eframe——`src/ui/`（窗格、弹窗、主机面板、设置、编辑器窗口、导入向导）
- **SSH/SFTP**：russh 0.65（ring 后端）+ russh-sftp 3.0；独立工作线程跑 current-thread tokio
  runtime，UI 线程经 channel 收发命令/事件，绝不在帧内阻塞
- **传输**：队列工作线程，tar 打包加速目录模式、断点续传、传输后校验、显式栈目录遍历（环/深度防护）
- **注册**：ECDsa P-256 离线验签置于闭源 `official` feature 门之后（见上文说明）
- **运行时**：Rust 1.99、`x86_64-pc-windows-gnu`、自包含 CRT 链接——导入表仅 Windows 系统 DLL

## 能力矩阵（公开口径）

与 C# 版或直觉预期不一致的能力边界，以本表为准：

| 能力 | 现状 | 说明 |
|---|---|---|
| 逐文件传输保留修改时间 | **不支持** | 设置项已从 UI 摘除（实现需 `filetime` 依赖，未获批准）；`settings.json` 的 `preserveMtimeEnabled` 键保留仅作 C# 存档互读，无任何行为效果 |
| tar 打包传输保留修改时间 | 支持 | tar 归档天然携带 mtime/权限，上传下载均保留 |
| 传输冲突决策 | 仅批级 | 覆盖确认弹窗为「全部覆盖 / 跳过冲突项 / 取消」三选一，**无逐项勾选决策**（与 C# 一致） |
| 传输后校验回退 | 无本机回退 | 命令通道不可用或服务端无哈希命令时**校验跳过**，不做本机拉流+本地计算回退 |
| SM3 | 仅服务端 openssl | 本地校验码与传输校验本机侧明确不支持 SM3（`ring` 无 SM3，新依赖未批准）；服务端有 `openssl dgst -sm3` 时可用 |
| Windows 服务器服务端哈希 | 不支持 | 非 POSIX shell，校验码菜单禁用 |
| `.ppk` 私钥 | 不支持 | russh 只认 OpenSSH/PEM 格式密钥 |
| hosts.json 损坏存档 | 逐行容错 | 坏记录跳过、可解析部分照常载入；**不生成备份文件** |

## 注册 / 支持者勋章

设置 → 注册（官方构建）：离线验签注册码、金色"支持者"勋章、远程公告。**注册与免费版功能无差别**——勋章纯荣誉。验签全程离线完成（ECDsa P-256）。从本仓库自行编译的社区构建无激活入口。

## 赞助

如果 iFreeSftp 帮你省了时间，欢迎赞助支持开发——附带应用内金色**支持者**勋章：

- 中国大陆：[爱发电 · 注册码商品页](https://afdian.com/item/3a69f926c4ac11f1878052540025c377)（微信 / 支付宝；未登录可能显示不全，也可从[爱发电主页](https://afdian.com/a/ifree)进入）
- 国际：[Ko-fi Commissions · 注册码商品页](https://ko-fi.com/ifrees/commissions?commissionAlias=61b90e1115)

赞助后在 Ko-fi Commissions 或爱发电留言期望的勋章名，作者回发签名注册码（设置 → 注册 激活）。

## 许可证

[GNU AGPL-3.0](LICENSE) — Copyright © 2026 [ifrees.top](https://ifrees.top)
