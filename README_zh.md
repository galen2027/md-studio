# MD Preview

**[English](README.md) · 简体中文**

[![GitHub stars](https://img.shields.io/github/stars/vorojar/md-preview)](https://github.com/vorojar/md-preview/stargazers)
[![Release](https://img.shields.io/github/v/release/vorojar/md-preview)](https://github.com/vorojar/md-preview/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux%20%7C%20iOS%20%7C%20Android-lightgrey)](https://github.com/vorojar/md-preview/releases)
[![App Store](https://img.shields.io/badge/App%20Store-Local%20Markdown%20Preview-blue?logo=appstore)](https://apps.apple.com/cn/app/local-markdown-preview/id6779451523)
[![Binary size](https://img.shields.io/badge/binary-~6.59%20MiB-green)](https://github.com/vorojar/md-preview/releases)

> 多份 Markdown，一个轻量窗口。沿本地文档链接跳转、查看统计、缩放正文并自动保存编辑，不必启动一整个 IDE。

MD Preview 是用 **Rust** 和系统 **WebView** 写的本地优先 Markdown 预览与快速编辑工具，桌面端覆盖 macOS、Windows、Linux，手机端提供 iOS 和 Android 原生外壳，方便从文件管理器、微信、企业微信和系统分享面板打开 Markdown。它不打包 Chromium，不依赖 Electron，渲染资源全部离线内置。桌面端可以把多份本地文档放进同一组标签，重启后回到上次活动文档；macOS 上还能从 Finder 新建 Markdown 并立即开始编辑。

![MD Preview 截图](https://vorojar.github.io/md-preview/hero.jpg)

## 为什么做它

AI 编程工具现在会生成大量 Markdown：`README.md`、`plan.md`、任务说明、架构笔记、变更记录、KaTeX 公式和 Mermaid 图。很多 Markdown 工具仍然要么是完整写作套件，要么是编辑器插件。MD Preview 刻意保持更小：

- **打开快**：原生二进制、系统 WebView，不带一份浏览器运行时。
- **本地渲染**：Markdown、代码高亮、数学公式、Mermaid 图表都在本机完成。
- **文档放在一起**：在一个标签窗口里打开多份 Markdown 和文本，下次启动继续上次会话。
- **沿文档目录跳转**：相对或绝对的本地 Markdown / 文本链接直接打开或激活标签，不会离开当前预览。
- **编辑少绕路**：从标签栏或 Finder 新建 Markdown 后立即输入，停顿片刻就自动保存。
- **按自己的节奏阅读**：预览与源码保留阅读进度，实时显示字与字符数，并且只缩放文档正文。
- **跟随外部编辑**：用 Vim、VS Code、Cursor、Zed 或任何编辑器保存文件，预览自动刷新。
- **阅读不打扰**：工具栏只在 hover 时出现，空白首页提供打开文件和最近文件，文档始终是主角。
- **覆盖真实文档**：代码块、表格、任务列表、公式、图表、图片、链接、打印都能离线工作。

## 适合 AI 编程工作流

把它当成专门承接这些文档的轻量预览工作区：

- 把 Claude Code / Codex / Cursor 生成的计划、任务笔记和 README 放进同一组标签，不必打开完整 IDE。
- 重启后恢复原来的标签顺序和活动文档；后台文件只在点击时从磁盘加载。
- 直接在 MD Preview 里完成小幅源码修改，同时保留外部编辑器写盘后的实时刷新。
- macOS 上从 Finder 新建 Markdown 后直接开始编辑，不再为了一个空文件先打开 VS Code。
- 需要干净 PDF 时，直接打印渲染后的预览。

## 下载

从 [GitHub Releases](https://github.com/vorojar/md-preview/releases) 下载最新版。

| 平台 | 包名 | 说明 |
|---|---|---|
| macOS | `MD-Preview-macOS-universal.dmg` | Apple Silicon 和 Intel 通用。Release 版本会签名、公证。 |
| Windows | `MD-Preview-windows-x64.exe` | 便携式 x64 单文件应用，MSVC CRT 已静态链接。应用内更新会下载新版 exe，校验 SHA-256，退出后替换自己并重启。 |
| Linux | `MD-Preview-linux-x64.tar.gz` | 需要系统 WebKitGTK 运行时。 |
| iOS / iPadOS | [App Store 上的 Local Markdown Preview](https://apps.apple.com/cn/app/local-markdown-preview/id6779451523) | 原生 iPhone / iPad 预览器，可从“文件”和 iOS 分享面板打开 Markdown。 |
| Android | `MD-Preview-Android.apk` | 原生 Android 预览器，可从文件管理器、微信、企业微信和分享面板打开 Markdown。 |

Android 版本以单独的 mobile release 发布，例如 [mobile-android-v1.0.7](https://github.com/vorojar/md-preview/releases/tag/mobile-android-v1.0.7)。iOS 版本已经在 App Store 上架，名称为 [Local Markdown Preview](https://apps.apple.com/cn/app/local-markdown-preview/id6779451523)。

### Windows 便携性与导出依赖

Windows x64 发布版静态链接 MSVC C Runtime，接收方不需要另装 Visual C++ Redistributable。界面仍必须使用 Microsoft Edge WebView2 Runtime；完整更新的 Windows 11 通常已经自带，缺失时 MD Preview 会显示原生下载提示并写入诊断日志。Mermaid、KaTeX、代码高亮和图标都已内嵌在 EXE 中。

生成 DOCX 不要求安装 Microsoft Word；只有打开生成文件时才需要 Word、WPS 或 LibreOffice。“导出 PDF”是唯一需要额外浏览器的路径，目标电脑必须安装 Microsoft Edge 或 Google Chrome。当前 Windows 发布物未做 Authenticode 签名，SmartScreen 可能提示“未知发布者”；运行前请先核对发布页给出的 SHA-256。

v1.9.9 支持“所见即所得编辑”：标题、含粗体/链接/行内代码的段落、列表、引用和表格可直接修改，自动保存 / Ctrl+S 写回原 MD。左预览、右源码分栏采用独立滚动和源码位置同步；真正外部文件冲突继续保护编辑，自身保存的延迟通知不再误报。

点击 Mermaid 图放大，再点“编辑节点”：可在图上点选 flowchart/graph 节点，修改文字、删除节点及关联连线、撤销图修改。先备份、校验和渲染，再应用到原 MD；保留放大、空格拖动、高清 PNG 与 SVG。复杂扩展语法或数字索引样式的删除操作会提示改用源码，不支持节点拖拽。详见 [模块设计](docs/预览编辑模块设计_v1.2_261119.md)。

进入所见即所得编辑前备份原文，打开但未修改不重写文件；编辑后 Markdown 排版可能规范化，撤销栈不跨模式/标签保留。

v1.9.6 为大尺寸 Mermaid 图增加“按住空格键 + 鼠标左键拖动”平移；只读文件保存失败时可选择另存为、放弃并关闭或取消，不再陷入无法退出的循环；同时修复 Windows 中文输入法首次聚焦与切回程序后的输入上下文。最稳妥的部署方式仍是把 EXE 复制到固定的本机目录并启动一次后再设置默认应用。

也可以从源码构建：

```bash
git clone https://github.com/vorojar/md-preview.git
cd md-preview
cargo build --release
./target/release/md-preview README.md
```

本地打包 macOS `.app`：

```bash
chmod +x bundle.sh
./bundle.sh
cp -r "target/MD Preview.app" /Applications/
```

## 使用

```bash
# 直接打开一份或多份文件
md-preview README.md plan.md task.md

# 或空启动后点打开文件、选择最近文件，或拖入文件
md-preview
```

MD Preview 支持通过拖拽、打开对话框、最近文件或命令行打开 `.md` / `.txt` 文件。桌面端会用标签承载文档；Windows 上已有程序运行时，再双击或启动打开其他文件会复用当前进程并新增或激活标签。Windows 单实例启动会先绑定分散候选端口，只在确认冲突时做有界探测，避免旧版出现多秒超时等待。使用标签栏 `+` 或 `Cmd/Ctrl+N` 可在当前文档旁新建 Markdown，并立即进入源码编辑。重启后恢复标签顺序和活动文档，后台文档正文仍留在磁盘，点击时才加载。标准 Markdown 图片引用和受支持的本地文档链接始终按当前 Markdown 所在目录解析，不依赖 EXE 所在目录。

若要把文档复制到其他电脑后仍正常显示，请把 Markdown 和图片目录一起复制并保持相对目录结构。程序可以读取当前电脑上存在的绝对路径和本机 `file:` 图片，但带盘符的绝对路径本身不具备跨电脑可移植性。

如果标签对应的文件被移动或删除，标签不会静默消失。点击后可以重新定位文件，或者关闭标签。

### macOS Finder 右键操作

已公证的 macOS 应用内置 Finder 扩展。把 `MD Preview.app` 拖到“应用程序”后打开一次；如果 macOS 没有自动启用，请前往 **系统设置 → 通用 → 登录项与扩展 → Finder 扩展**。

在 Finder 文件夹空白处右键，可以新建 Markdown、文本、JSON、HTML，复制目录路径，或在终端打开。选择**新建 Markdown**后会自动避开重名，并直接在 MD Preview 源码编辑器里打开新文件。

iPhone 和 iPad 上，Local Markdown Preview 可以从“文件”和 iOS 分享面板打开 Markdown / 文本文件。Android 上，MD Preview 会出现在 Markdown 文件的“打开方式”和分享流程中。Recent 文件会缓存到应用私有目录，从微信、企业微信等临时来源打开过的文档后续也能继续打开；如果条目失效，会安全移除而不是闪退。

## 功能

| 功能 | 说明 |
|---|---|
| 桌面标签 | 在一个窗口打开多份 Markdown 或文本；Windows 后续文件启动复用现有进程，重复路径只激活已有标签，并通过先绑定、分散候选端口保持快速启动。 |
| 会话恢复 | 重启后恢复标签顺序和活动文档，但不缓存后台文档正文。 |
| 缺失文件 | 文件移动或删除后保留缺失标签，提供重新定位和关闭操作。 |
| Finder 工作流 | macOS 上从 Finder 新建 Markdown，并立即在 MD Preview 里编辑。 |
| 可靠自动保存 | 源码输入停顿片刻即保存；预览、切换标签、关闭标签/窗口或退出前强制写盘；只读文件保存失败时可另存为、放弃关闭或取消，并始终保留编辑内容。 |
| 本地文档链接 | 相对或绝对的现有 Markdown / 文本链接会打开或激活标签；无效本地目标不会替换当前预览。 |
| 头部元信息 | 文档开头的 YAML metadata 保持元信息排版，不再被挤成标题。 |
| 实时统计 | 标签栏显示非空白字符数与总字符数，源码编辑时同步更新。 |
| 正文缩放 | 在 70% 到 200% 之间缩放渲染正文或源码，不改变标签栏和工具栏。 |
| 滚动连续 | 预览和源码高度不同时，切换仍按归一化阅读进度恢复位置。 |
| 启动首页 | 空白启动时显示打开文件和本机最近文件，没加载文档也有明确入口。 |
| 手机端打开 | iOS 支持从“文件”和分享面板打开 Markdown；Android 支持从文件管理器、微信、企业微信和系统分享面板打开 Markdown。 |
| 拖拽打开 | 把 Markdown 文件拖进窗口即可打开。 |
| 命令行打开 | `md-preview path/to/file.md` 直接从 shell 打开。 |
| 查找与替换 | `Cmd/Ctrl+F` 在渲染预览中查找；`Ctrl+H` 进入 Markdown 源码并打开替换栏，支持单项和全部替换。 |
| 实时刷新 | 外部编辑保存后，预览自动更新。 |
| 源码编辑 | `Cmd/Ctrl+E` 切到源码模式；编辑会自动保存，`Cmd/Ctrl+S` 可立即保存；Windows 中文 IME 在首次聚焦和切回窗口后可直接组合输入。 |
| 左右分栏 | 工具栏按钮可同时显示左侧预览和右侧源码，沿用自动保存、替换和未命名草稿预览。 |
| 原生打印 | `Cmd/Ctrl+P` 打开系统打印对话框，只打印预览内容。 |
| 代码高亮 | highlight.js 离线内置，首屏之后再注入，不阻塞打开。 |
| 数学公式 | KaTeX 按需渲染 `$...$`、`$$...$$`、`\(...\)`、`\[...\]`。 |
| 图表 | Mermaid fenced code block 只在文档实际使用时本地渲染；灯箱放大后可按住空格键并用鼠标左键拖动到局部位置。 |
| 图表导出 | 灯箱下载明确区分 SVG 矢量原图与 3× PNG；Word 导出使用版面尺寸稳定的 300 PPI PNG 兼容回退。 |
| DOCX 表格 | Word 导出按模板页面和内容权重生成固定列网格，六类表级边框统一为深色 0.75 pt 实线，并保留单元格留白、数字对齐和跨页重复表头；Markdown/HTML 换行继续保留，不再依赖不可预测的 autofit。 |
| 本地图片 | 支持文档相对路径、父目录、Windows 反斜杠、本机绝对路径和本机 `file:` 的 JPG/PNG/GIF/WebP/SVG/BMP/ICO/AVIF；通过受控只读资源通道流式加载，并拒绝 UNC 与远程 `file:` 共享。 |
| GitHub Alerts | `[!NOTE]`、`[!TIP]`、`[!IMPORTANT]`、`[!WARNING]`、`[!CAUTION]` blockquote 会渲染成提示块。 |
| 高亮标记 | `==高亮==` 会渲染成 marked text，适合笔记和 AI 生成文档。 |
| 暗色模式 | 自动跟随 macOS、Windows、Linux 的系统主题。 |
| GFM 支持 | 表格、任务列表、删除线、heading attributes、标题锚点。 |
| 外链跳转 | `http`、`https`、`mailto` 链接交给系统浏览器或邮件客户端。 |
| 窗口恢复 | 下次启动恢复上次位置和大小；断开显示器后自动回到可见屏幕。 |
| 更新 | 首屏之后检查桌面版 GitHub Releases。macOS 使用 Sparkle 做签名校验和应用内更新；Windows 校验 SHA-256 后自替换单 exe；Linux 打开对应 release 下载。 |

## 快捷键

| 快捷键 | 作用 |
|---|---|
| `Cmd/Ctrl + N` | 新建 Markdown 并进入源码编辑 |
| `Cmd/Ctrl + O` | 打开文件 |
| `Cmd/Ctrl + F` | 在预览里搜索 |
| `Ctrl + H` | 进入源码模式并打开查找替换 |
| `Cmd/Ctrl + E` | 切换预览 / 源码编辑 |
| `Cmd/Ctrl + S` | 源码编辑模式下保存 |
| `Cmd/Ctrl + P` | 打印预览 |
| `Cmd/Ctrl + W` | 关闭当前标签；没有文档标签时关闭窗口 |
| `Cmd/Ctrl +` | 放大文档正文 |
| `Cmd/Ctrl -` | 缩小文档正文 |
| `Cmd/Ctrl 0` | 重置文档正文缩放 |
| `Esc` | 退出源码编辑模式，并在需要时保存 |
| `空格键 + 鼠标左键拖动` | 在 Mermaid 灯箱中平移已放大的图表 |

## Markdown 支持

MD Preview 先用 `pulldown-cmark` 完成基础 Markdown 解析，再只在需要时增强渲染结果：

- CommonMark 加 GFM 风格表格、任务列表、删除线、heading attributes
- GitHub 风格 Alerts blockquote，支持 note、tip、important、warning、caution
- 很多笔记工具常用的 `==高亮==` 文本标记
- 40+ 语言离线代码高亮，包含 Delphi / Pascal
- KaTeX 离线数学公式渲染，并保护公式不被 Markdown 强调语法破坏
- Mermaid 离线渲染 ```` ```mermaid ```` fenced code block
- 标准 Markdown 图片引用按已打开文档目录解析，并通过受控只读资源通道加载
- 支持跳转到相对或绝对路径的本地 Markdown / 文本文档
- 保持 `---` 或 `...` 包围的 YAML front matter 清晰可读
- 打印样式自动移除应用工具栏和源码编辑区

普通 Markdown 走轻路径：文档先可见，highlight.js、KaTeX、Mermaid 这类较重的增强逻辑延迟到首屏之后，或只在文档真正需要时加载。

## 为什么能小

MD Preview 不是 Tauri，也不是 Electron。它主要使用：

- **Rust**：原生 shell 和 Markdown 管线
- **wry**：系统 WebView，macOS 是 WebKit，Windows 是 WebView2，Linux 是 WebKitGTK
- **tao**：跨平台窗口和事件循环
- **pulldown-cmark**：Markdown 解析
- **notify**：文件监听
- **rfd**：原生打开文件对话框

Release profile 使用面向体积的优化、LTO、单 codegen unit、符号裁剪和 `panic = "abort"`。

## 隐私

MD Preview 没有账号、没有 telemetry、没有 analytics。你的 Markdown 文件留在本地磁盘，渲染也在本机完成。桌面应用自身唯一的网络请求是首屏之后的可选更新检测；失败会静默忽略，不影响启动和预览。macOS 更新由 Sparkle 使用应用内置的 EdDSA 公钥校验；Windows 自更新会先校验 GitHub Releases 返回的 SHA-256 digest，再替换当前 exe。

## 常见问题

**Linux 启动不了**

请安装发行版对应的 WebKitGTK 4.1 包。Debian / Ubuntu 可以执行：

```bash
sudo apt-get install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev
```

**Linux NVIDIA 环境打开后窗口空白**

MD Preview 会在检测到 Linux 系统已加载 NVIDIA 驱动时自动启用更保守的 WebKitGTK fallback。如果你的发行版仍然出现 WebView 空白，可以手动这样启动：

```bash
WEBKIT_DISABLE_DMABUF_RENDERER=1 md-preview your-file.md
```

如果仍然异常，再尝试：

```bash
WEBKIT_DISABLE_COMPOSITING_MODE=1 md-preview your-file.md
```

**Windows 不能自动设为默认 Markdown 应用**

Windows 不允许应用静默接管文件关联。MD Preview 会把自己注册到“打开方式”列表，你可以在资源管理器或 Windows 设置里选择它。v1.9.5 会校验实际注册表命令、移除 Explorer 不接受的 Windows verbatim 路径前缀，并同时注册当前 EXE 文件名及兼容 alias。若旧默认项指向已失效的软链接或硬链接，请把 EXE 复制到固定本机目录，在新位置启动一次，再从“打开方式”重新选择。

**公式或图表显示成普通文本**

先确认语法是合法的 Markdown / KaTeX / Mermaid。公式和图表都是按需加载的，不包含这些语法的文档不会承担额外启动成本。

## 开发

```bash
cargo build
cargo test
cargo build --release
```

CI 会构建 macOS、Windows、Linux。推送匹配 `v*` 的 tag 后，GitHub Actions 会产出 macOS DMG、Windows 单文件 EXE 和 Linux tarball。

维护者发版流程：

```bash
scripts/release.sh v1.2.3
```

脚本会前台执行验证、推送 `master` 和 tag、等待 GitHub Actions、签名/公证/staple macOS DMG、上传 `appcast.xml`，并验证最终 Release assets。

## 许可证

[MIT](LICENSE)
