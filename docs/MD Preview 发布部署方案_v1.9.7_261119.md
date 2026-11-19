# MD Preview 发布部署方案

版本：v1.9.7；日期：2026-11-19；发布性质：Windows x64 本地候选试用包，未上传公开 Release。

## 本次变化

新增“预览文字编辑（段落 / 表格）”：普通段落、纯文字表头和单元格可以直接在预览中修改，自动保存或 Ctrl+S 写回原 MD 文件。源码位置映射和页面编辑拆成独立模块，保持同一项目、单个 EXE，不增加服务或前端运行时。

Mermaid 放大、滚轮缩放、空格拖动、高清 PNG 和原始 SVG 下载保留。Mermaid 节点文字预览编辑属于第二阶段，本包尚未实现。

## 发布物及校验

本目录包含：

- MD-Preview-windows-x64.exe
- MD-Preview-windows-x64.exe.sha256
- MD Preview 发布部署方案_v1.9.7_261119.md

EXE 实际大小：6,938,112 字节；相较 v1.9.6 增加 12,800 字节。

SHA-256：`ED1B0BFB615A225E5A6117E4FD1D4861BC78EE14EACBE9884834D49ADFCBBC10`。

Authenticode：NotSigned。本地候选程序未签名，Windows 可能显示未知发布者提示；请先核对来源和校验值，不需要关闭系统安全保护。

## 必需和可选组件

必需：Windows x64、Microsoft Edge WebView2 Runtime。继续使用系统 Evergreen WebView2，没有捆绑 Fixed Runtime；缺失时程序显示下载提示。官方入口：[WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)。

无需另装：VC++ Runtime、Node.js、Python、Rust、Mermaid、KaTeX。EXE 已静态链接 CRT，渲染资源离线内置。

可选：Edge 或 Chrome 用于直接导出 PDF；Word、WPS 或 LibreOffice 用于查看 DOCX。生成 DOCX 本身不需要安装 Word。

## 安装与试用步骤

1. 关闭旧版 MD Preview，保留旧 EXE，不覆盖删除。
2. 将候选包放入一个固定的本机目录。先使用 Markdown 文件及图片目录的副本试用，避免自动保存直接改变唯一原件。
3. 在包目录打开 PowerShell，运行：

```powershell
Get-FileHash .\MD-Preview-windows-x64.exe -Algorithm SHA256
.\MD-Preview-windows-x64.exe --version
.\MD-Preview-windows-x64.exe --help
```

哈希应与上述值一致；版本输出应为 `MD Preview 1.9.7`。

4. 双击 EXE，打开测试 MD。若提示缺少 WebView2，按程序提示安装微软运行时后重试。
5. 鼠标移到窗口顶部工具栏，找到分栏按钮后、打印按钮前的铅笔按钮，悬停标题为“预览文字编辑（段落 / 表格）”。点击启用。
6. 点击虚线框内的普通段落、表头或单元格直接输入。停顿约 700 ms 自动保存，Ctrl+S 立即保存。Enter 在当前块内换行；Ctrl+Z/Y 撤销/重做；Escape 或再点按钮结束预览编辑。
7. 使用外部文本编辑器检查 MD 原文件，确认文字已经更新。切换标签再切回来，确认保存结果仍在。
8. 点击 Mermaid 图验证灯箱、缩放、空格拖动、PNG 和 SVG 下载。
9. 试用满意后再设置默认打开方式。EXE 移动或改名后先运行一次，再在 Windows“打开方式”中选择新路径；不要指向失效的软链接。

## 文件、配置和离线规则

MD 和图片路径以文档所在目录解析；复制到其他电脑时一起复制文档及图片文件夹，并保持相对目录结构。绝对盘符路径只有在目标机器存在相同位置时才能加载。

程序继续沿用用户配置目录 `%LOCALAPPDATA%\md-preview`，其中保存会话、窗口状态和诊断日志；没有新增数据库或配置项。已有 `MD_PREVIEW_CONFIG_DIR` 环境变量仍可指定其他位置。本包不包含本机用户配置或测试日志。

## 编辑边界与副作用

- 本阶段仅支持普通顶层段落和纯文字单元格；标题、列表、引用、粗体、链接、行内代码、公式、图片等复杂块使用原有源码/分屏编辑。没有虚线框的内容不可直接编辑。
- 不提供表格增删行列；原文缺省省略的表格列没有源码范围，保持只读。显式空白格可编辑。
- 输入的 Markdown 标记按字面文字保存，部分字符转为字符实体，换行保存为 `<br>`；不会因输入管道符增加表格列。修改片段的源码外观可能变化。
- 未修改的块不重新序列化；但既有 textarea 保存链路会把 CRLF 规范化为 LF，不承诺字节级仅变化当前片段。
- 自动保存会直接修改原文件。只读/权限失败继续复用原有另存为、放弃关闭或取消流程；失败后自动保存暂停，编辑内容保留。

## 验证记录

2026-11-19 本机实跑：

| 项目 | 结果 |
| --- | --- |
| cargo fmt --check | 通过 |
| cargo test | 98 passed，0 failed |
| cargo build --release | 通过 |
| scripts/verify.sh | 通过；平台缺少的原生工具按下表跳过 |
| 浏览器编辑回归 | 段落、空白格、表格对齐、字面标记、换行、纯文本粘贴、撤销重做、组合输入次序、光标保留、旧映射失效、390px 布局通过 |
| Windows WebView2 实际回写 | 隔离配置和文档；CRLF/CJK/emoji 输入、自动保存、Ctrl+S、脏标签切换、切回后显示保存内容通过；不修改系统文件关联 |
| Mermaid 导出 | 原始 SVG、3× PNG、300 PPI Word 位图及灯箱平移回归通过；预览编辑开启时下载通过 |
| 搜索替换、统计缩放、锚点导航 | 通过 |
| 移动端共享只读渲染 | 浏览器回归通过 |
| PE 依赖 | 无 VCRUNTIME、MSVCP、CONCRT 导入 |
| --help / --version | 实跑通过，版本 1.9.7 |
| Authenticode | NotSigned |

未完成：物理中文输入法人工输入、干净 Windows 11 VM、macOS/Linux 原生编辑实测。iOS/Android 构建因缺少对应原生工具跳过；Sparkle 签名验证仅 macOS 可用。此版本未重新进行 Word/WPS/LibreOffice 实机渲染测试。

详细开发测试索引：源码目录 `tests/README.md`。模块与第二阶段计划：`docs/预览编辑模块设计_v1.0_261119.md`。

## 回滚

1. 关闭候选 v1.9.7，必要时在只读保存失败对话框选择另存为、放弃或取消。
2. 启动保留的 v1.9.6 EXE；若要恢复默认关联，运行旧版一次并在 Windows 中重新选择。
3. 本迭代无数据迁移，不删除 MD、图片、模板或用户配置。
4. 已经保存到 MD 的文字修改不会因切回旧程序自动撤销；从试用前的文件备份或版本控制恢复内容。

