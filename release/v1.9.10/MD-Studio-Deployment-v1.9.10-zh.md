# Markdown Studio 发布部署方案

版本：v1.9.10；日期：2026-11-19；平台：Windows x64。

## 版本说明

本版本将项目公开发布为 Markdown Studio，使用 `galen2027/md-studio` 的更新、下载、反馈入口，并重写中英文 README、隐私、支持及发布说明。保留原项目版权与许可证。

保留 v1.9.9 的格式化文字编辑、Mermaid 节点编辑/删除/撤销、分栏语义同步、DOCX 导出、图片加载及文件冲突保护。程序显示名称更新为 Markdown Studio；EXE 文件名、命令、配置目录和文件关联标识继续兼容旧版本。

v1.9.9 及更早程序内置的是原项目更新源，首次升级请手动下载本版本。v1.9.10 起检查本仓库更新，不从原作者仓库下载替换程序。

## 发布物与校验

| 项目 | 值 |
| --- | --- |
| 主程序 | MD-Preview-windows-x64.exe |
| 实际大小 | 11,182,592 字节 |
| SHA-256 | F9B398F4B5AEA638EC96A93CDC32C10088F8E9FA820C9BCDF92E46841DFE9675 |
| Authenticode | NotSigned |

正式发布页面：[v1.9.10](https://github.com/galen2027/md-studio/releases/tag/v1.9.10)。

- `MD-Preview-windows-x64.exe`：单个可运行程序。
- `MD-Preview-windows-x64.exe.sha256`：EXE 校验值。
- `MD-Studio-v1.9.10-windows-x64.zip` 及 `.sha256`：完整便携包和校验值。
- `MD-Studio-Deployment-v1.9.10-zh.md`：本说明的下载副本。
- `MD-Studio-v1.9.10-licenses.zip` 及 `.sha256`：第三方声明与许可证。

完整便携包内含程序、校验值、部署说明、第三方声明与 licenses/。分享应用时保留相应许可证。Git 记录源码和文档，EXE/ZIP 通过 Releases 分发。

## 必需与可选组件

建议 Windows 11 x64，界面必须使用 Microsoft Edge WebView2 Runtime。通常系统已自带；裁剪镜像可能缺失，程序会提供可操作提示。[微软 WebView2 Runtime 下载](https://developer.microsoft.com/microsoft-edge/webview2/)。

无需另装 VC++ Runtime、Node、Python、Rust、Mermaid 或 Vditor。资源离线内嵌，CRT 静态链接。

直接导出 PDF 需要本机 Edge 或 Chrome。生成 DOCX 不需要 Word；查看生成文件需要 Word、WPS 或 LibreOffice。没有捆绑浏览器，也未发布 ARM64 原生版。

当前未进行 Authenticode 签名，Windows 可能显示未知发布者。请核对来源和上述 SHA-256，不需要关闭系统安全保护。

## 安装和升级步骤

1. 保存当前编辑，退出所有旧版窗口。单实例机制会复用仍运行的旧程序，因此不要直接覆盖正在运行的 EXE。
2. 在本仓库 Releases 下载 EXE 或完整 ZIP。ZIP 要先解压，不要在压缩包内直接运行。
3. 保留旧 EXE，将新版放到固定本机目录，例如 `C:\Tools\MD-Studio`。首次试用请先复制 MD 和图片目录作为测试副本。
4. 在程序目录打开 PowerShell，核验：

```powershell
Get-FileHash .\MD-Preview-windows-x64.exe -Algorithm SHA256
.\MD-Preview-windows-x64.exe --version
.\MD-Preview-windows-x64.exe --help
```

校验值应与同一 Release 的 `.sha256` 文件一致；版本输出应为 `Markdown Studio 1.9.10`。

5. 双击 EXE，打开或拖入 MD。若提示缺少 WebView2，按提示从微软安装 Evergreen Runtime，再重新启动。
6. 点击“所见即所得编辑”后，在标题、段落、列表、引用或表格中直接输入。停顿约一秒自动保存，Ctrl+S 立即保存。
7. 点击分栏按钮，验证左预览和右源码分别滚动且对应同一段落；Ctrl+H 搜索文档后部的词，检查两栏定位。
8. 点击 Mermaid 图放大，再点“编辑节点”。点图中节点或从下拉框选择，修改文字后点“应用文字”；删除需要再次确认，可撤销当前图编辑会话中的修改。
9. 下载 SVG 和 PNG，检查内容和清晰度；导出 DOCX/PDF 时确认使用最新编辑内容。
10. 确认正常后再设置 MD 默认打开方式。移动或更名 EXE 后先运行一次，再到 Windows“打开方式”选择新位置，避免失效软链接。

## 配置、路径与备份恢复

配置目录沿用 `%LOCALAPPDATA%\md-preview`，其中保存会话、窗口状态、诊断日志及 `backups/`。`MD_PREVIEW_CONFIG_DIR` 仍可指定其他位置。名称更新不会自动迁移或删除原配置。

进入格式化文字编辑、应用图修改前，对磁盘原文件做精确字节备份；失败时不应用。备份名含时间、路径哈希和原文件名，后缀 `.bak`，不覆盖已有备份、不自动清理。未保存的新文档没有磁盘原文备份。

恢复时退出程序，先另外保留当前 MD；在 backups 按原文件名和时间选取备份，复制到文档目录并去掉 `.bak` 后打开核对。只有内容确认正确后才替换要恢复的文档。

图片按 MD 所在目录解析。跨电脑迁移时一起复制图片目录并保持相对结构；绝对盘符路径要求目标机器存在对应文件。内部显示协议 URL 不会保存到 Markdown。

## 风险与边界

- 自动保存直接写回原文。富文本编辑可能规范化空白、列表和表格源码排版及 CRLF；撤销历史不跨模式和标签保留。
- Mermaid 图形修改只覆盖 flowchart/graph 节点；不包含节点拖拽、边文字编辑和其他图类型。复杂扩展语法或数字索引 linkStyle 的节点删除无法安全处理时，使用源码编辑。
- 分栏按块映射，特别长的单段、自定义 HTML 等内容只能近似对齐。
- 真正的外部内容冲突会暂停自动保存并保留编辑。要覆盖磁盘版本时显式 Ctrl+S；要保留外部版本时先另存当前编辑，再打开原文件。
- 只读或权限失败继续提供另存为、放弃关闭、取消的恢复路径。不要删除唯一文件副本来解决保存问题。
- 文档中的远程图片或点击的外部链接可能联网；程序会在首屏后查询本仓库更新。没有账号、遥测或 Markdown 内容上传功能。

## 验证与发布验收

验收入口：`cargo fmt --check`、`cargo test --locked`、`cargo build --release --locked`、`scripts/verify.sh`、PE 依赖门禁及 Windows 原生工作流。必须通过 CLI 版本、主程序与下载字节 SHA-256 的一致性核验。

当前编辑机制已有 98 项 Rust 用例，以及文字/表格、文件冲突、分栏、图节点操作和图片/图下载回归；v1.9.10 的实际执行结果见源码 `tests/README.md` 和 Release 说明。仓库公开前检查凭据、日志、截图和业务内容范围；发布包不包含本机配置、诊断日志或业务文档。

2026-11-19 本机结果：98 项 Rust 测试、fmt、release 构建和 scripts/verify.sh 均通过；原生工作流及结构化编辑测试通过，验证了外部冲突、双向分栏、节点编辑/删除/撤销、原文件回写、备份、图片连续保存和 SVG/3 倍 PNG 下载。PNG 实际宽度 942 px，对应 SVG 314 px。EXE 的帮助、版本、PE 依赖和压缩包内容已核验。

物理中文输入法人工输入、干净 Windows 11 VM、本版本 Word/WPS/LibreOffice 人工视觉检查仍有未完成项。其他平台保留源码和编译检查，不能据此承诺原生运行验收；本仓库没有发布或维护上游商店应用。

手动 Windows CI 工作流只生成草稿；正式版本不可覆盖。当前正式资产通过本机验收、上传和下载校验后发布。开发与打包细节见 `release/README.md`。

## 回滚步骤

1. 保存或另存当前编辑，退出 v1.9.10。
2. 启动保留的 v1.9.9 EXE；若需要恢复默认打开方式，先运行旧版，再在 Windows 中重新选择。
3. 回退程序不会撤销已经保存的文档。需要恢复内容时先保留当前文件，再按上述备份恢复步骤操作。
4. 无数据库迁移；不删除 MD、图片、模板、配置和备份。v1.9.9 的更新源仍是上游，后续升级建议从本仓库手动下载。
