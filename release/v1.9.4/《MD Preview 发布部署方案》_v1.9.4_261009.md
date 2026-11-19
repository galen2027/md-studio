# 《MD Preview 发布部署方案》_v1.9.4_261009

更新时间：2026-10-09
适用平台：Windows 11 x64
发布目录：`release/v1.9.4/`
主程序：`MD-Preview-windows-x64.exe`
发布状态：本机发布验收完成；干净 Windows 11 VM、缺失 WebView2 环境、WPS 和 LibreOffice 尚未完成本版本实机验证

## 一、版本结论

MD Preview v1.9.4 的 Windows x64 发布物采用便携单 EXE：MSVC C Runtime 已静态链接，接收方不再需要额外安装 Visual C++ Redistributable。这里的“便携”不表示完全脱离 Windows 系统组件；图形界面仍必须使用 Microsoft Edge WebView2 Runtime。

本版同时重构 DOCX 表格输出：列宽由页面可用宽度、模板页边距和表格内容共同决定，并写入固定 OOXML 网格，不再交给 Word/WPS 随机 autofit。此变更只影响 DOCX 导出，不改变 Markdown 语法、预览表格、打印/PDF 表格、PDF 页面方向、CLI 参数或既有环境变量。

本次不包含 Windows ARM64 原生版本，也不包含 Authenticode 代码签名。最终 EXE 若确认仍为未签名，Windows SmartScreen 可能显示“未知发布者”；必须先核对 SHA-256，并只从可信发布源获取文件。

## 二、发布目录清单

最终 `release/v1.9.4/` 必须只包含本版交付需要的文件，至少包括：

1. `MD-Preview-windows-x64.exe`
2. `MD-Preview-windows-x64.exe.sha256`
3. `《MD Preview 发布部署方案》_v1.9.4_261009.md`

`target-crt-static/` 是静态 CRT 可行性验证产物，不得进入提交或发布目录。`template.docx` 是用户可选模板，不属于必需发布物。

发布目录封装前必须确认：EXE、`.sha256` 文件和本文第十节记录的是同一个最终二进制；任何一次重新构建都会使旧文件大小和 SHA-256 失效，必须重新生成并复核。

## 三、必需与可选依赖

| 项目 | 类型 | 说明 |
|---|---|---|
| Windows 11 x64 | 必需 | 本版发布目标。Windows ARM64 原生版本不在本次范围内。 |
| Microsoft Edge WebView2 Runtime（Evergreen） | 必需 | 用于显示应用界面。完整更新的 Windows 11 通常已预装；企业裁剪镜像仍可能缺失。应用不捆绑体积较大的 Fixed Version Runtime。 |
| Visual C++ Redistributable | 不需要 | v1.9.4 Windows x64 EXE 静态链接 MSVC CRT；发布门禁禁止导入 `VCRUNTIME*`、`MSVCP*` 或 `CONCRT*`。 |
| Node.js、Python、Rust | 不需要 | 仅开发/构建阶段使用；Mermaid、KaTeX、代码高亮和图标均已内嵌。 |
| Microsoft Word | 生成 DOCX 不需要 | DOCX 由程序直接生成 OOXML。查看和人工验收 DOCX 时需要 Word、WPS 或 LibreOffice 之一。 |
| Microsoft Edge 或 Google Chrome | 仅导出 PDF 时需要 | 普通预览、编辑和 DOCX 导出不依赖外部浏览器；“导出 PDF”会调用本机 Edge/Chrome。 |
| `template.docx` | 可选 | 放在 EXE 同目录，或用 `MD_PREVIEW_TEMPLATE` 指向。列宽计算读取模板的实际纸张尺寸和页边距；无模板时使用内置样式。 |

WebView2 官方下载入口：<https://developer.microsoft.com/microsoft-edge/webview2/>
WebView2 分发说明：<https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution>

## 四、v1.9.4 变更说明

### 4.1 Windows 便携发布

- Windows x64 release 构建固定启用静态 CRT，使本地 `cargo build --release` 与发布流水线产出遵循同一依赖边界。
- 发布构建增加 PE 依赖门禁；发现 `VCRUNTIME*`、`MSVCP*` 或 `CONCRT*` 导入时必须失败，不得继续封装。
- WebView2 创建失败时不再静默闪退：程序显示中文原生提示，给出微软 WebView2 Runtime 下载入口，并将错误写入诊断日志。
- 继续使用 Evergreen WebView2，不把 Fixed Version Runtime 打进 EXE。
- 保持 Windows x64 单 EXE；本版不提供 ARM64 原生包和 Authenticode 签名。

### 4.2 DOCX 表格规则

DOCX 表格先收集为内部表格模型，再一次性生成 OOXML。模型保留行、单元格、Markdown 显式对齐、链接、inline code、图片和单元格内换行。

列宽和总宽度规则：

- ASCII 字符按 1 个视觉单位计算，中文及全角字符按 2 个视觉单位计算。
- 每个单元格参与列宽计算的权重限制在 `4..32`，避免一个超长段落独占整页。
- 1–2 列短表至少使用页面可用宽度的 50%；3–4 列至少使用 70%；5 列以上，或包含长段落/多行内容时使用 100%。
- 每列最小宽度为 `480 twips`；最终宽度会重新平衡，并严格不超过模板纸张尺寸减去左右页边距后的可用宽度。
- OOXML 明确输出 `tblGrid/gridCol`、`tblLayout=fixed` 和每格 `tcW=dxa`，不使用随机 autofit。

排版规则：

- 边框颜色 `#D9D9D9`，宽度 0.5 pt。
- 单元格上/下边距 80 twips，左/右边距 120 twips。
- 表格内段前、段后为 0，行距为 1.15 倍。
- 默认表头使用 `#EAF2F8` 浅色底纹和粗体；“格式标准化”模式继续保持无底色、无粗体，仅加强表头下边框。
- 单行行垂直居中，多行行顶部对齐；7 列及以上使用表格局部紧凑字号，不改变正文样式。
- Markdown 显式左/中/右对齐优先保留；未声明时，只有整列均为数字、百分比或金额才自动右对齐，其他列左对齐。
- 表头设置 `tblHeader` 和 `cantSplit`，跨页自动重复且表头自身不拆分；正文行允许正常跨页，避免超长单元格被截断。
- 支持 Markdown hard break，以及 `<br>`、`<br/>`、`<br />`，单元格中的显式换行不再丢失。

## 五、发布构建与机器门禁

在项目根目录使用 release 配置执行：

```powershell
cargo fmt --check
cargo test
cargo build --release
bash scripts/verify.sh
```

随后必须完成以下检查：

1. 先运行 PE 门禁脚本自测：

   ```powershell
   .\scripts\verify-windows-pe-dependencies.ps1 -SelfTest
   ```

2. 普通本机构建后检查 `target\release\md-preview.exe`：

   ```powershell
   powershell -ExecutionPolicy Bypass -File scripts\verify-windows-pe-dependencies.ps1 -Path target\release\md-preview.exe
   ```

   发布流水线的显式 Windows target 使用：

   ```powershell
   .\scripts\verify-windows-pe-dependencies.ps1 -Path target\x86_64-pc-windows-msvc\release\md-preview.exe
   ```

   任一命令发现 `VCRUNTIME*`、`MSVCP*` 或 `CONCRT*` 导入时必须以退出码 1 失败。
3. 执行：

   ```powershell
   .\target\release\md-preview.exe --help
   .\target\release\md-preview.exe --version
   ```

4. `--version` 必须输出 `MD Preview 1.9.4`，且两个命令都不应打开 GUI。
5. 生成最终文件后再计算 SHA-256；不得复用 v1.9.3 或任何中间构建的哈希。
6. 在封装发布目录前记录最终文件大小、SHA-256 和 Authenticode 状态，并替换本文第十节的三个占位标记。

## 六、傻瓜式部署步骤

### 6.1 下载与校验

1. 从同一可信发布源下载以下三个文件，并放在同一个目录：
   - `MD-Preview-windows-x64.exe`
   - `MD-Preview-windows-x64.exe.sha256`
   - `《MD Preview 发布部署方案》_v1.9.4_261009.md`
2. 在该目录空白处按住 Shift 并点击鼠标右键，选择“在终端中打开”或“在此处打开 PowerShell”。
3. 执行：

   ```powershell
   Get-FileHash .\MD-Preview-windows-x64.exe -Algorithm SHA256
   Get-Content .\MD-Preview-windows-x64.exe.sha256
   ```

4. 确认两处 SHA-256 完全一致，并与本文第十节一致。不一致时不要运行，删除本次下载并从可信发布源重新获取。

### 6.2 安装或升级

1. 关闭所有旧版 MD Preview 窗口。
2. 如果当前目录已有 v1.9.3，把旧 EXE 重命名为 `MD-Preview-windows-x64-v1.9.3.bak.exe`，先不要删除。
3. 将 v1.9.4 的 `MD-Preview-windows-x64.exe` 复制到准备长期使用的目录。不要放在临时下载解压目录后再频繁移动。
4. 执行版本检查：

   ```powershell
   .\MD-Preview-windows-x64.exe --version
   ```

5. 预期输出：`MD Preview 1.9.4`。
6. 双击 EXE 启动一次。该步骤会刷新 `.md` 文件的“打开方式”注册位置；以后移动 EXE 后，也应在新位置启动一次。
7. 如果 SmartScreen 显示“Windows 已保护你的电脑”，先再次核对 SHA-256 和下载来源。确认无误后才能选择“更多信息”并运行；来源或哈希不明时必须取消。
8. 打开一份 Markdown，确认预览、图片、编辑和保存正常。

### 6.3 数据目录与可选模板

默认情况下，用户配置、会话和诊断日志位于 `%LOCALAPPDATA%\md-preview\`（无该变量时回退到 `%APPDATA%\md-preview\`）。需要让数据跟随当前目录时，可在启动前设置相对目录：

```powershell
$env:MD_PREVIEW_CONFIG_DIR='.\data'
.\MD-Preview-windows-x64.exe
```

使用自定义 Word 模板时，可把 `template.docx` 放在 EXE 同目录，或设置相对路径：

```powershell
$env:MD_PREVIEW_TEMPLATE='.\template.docx'
.\MD-Preview-windows-x64.exe
```

## 七、部署后验证

### 7.1 基本功能

- [x] `--help` 正常输出且不启动 GUI。
- [x] `--version` 输出 `MD Preview 1.9.4` 且不启动 GUI。
- [ ] Windows 11 x64 上仅复制单个 EXE 和测试文档目录即可启动，不要求安装 Visual C++ Redistributable。
- [x] 带相对图片的真实 Markdown 已在当前 Windows 11 主机离线预览，6/6 张图片均通过受限本地传输加载；Mermaid 3× PNG、SVG 和 300-PPI Word fallback 自动回归通过。
- [ ] 源码编辑、自动保存、分栏和多标签正常。
- [x] 左预览、右编辑的分栏布局已通过真实 WebView2 自动验证；源码编辑、自动保存和多标签已有自动回归，尚未在干净 VM 重测。
- [x] DOCX 导出成功，生成过程不调用或要求安装 Word。
- [ ] 安装 Edge 或 Chrome 时“导出 PDF”成功；未安装时只影响 PDF 导出，不影响预览、编辑和 DOCX。

### 7.2 DOCX XML 与版面

- [x] 回归样例覆盖 2/3/4/6/10 列、中英文、长 URL、数字/百分比/金额、显式对齐、inline code、链接、图片、空单元格、Markdown hard break 和三种 `<br>` 写法。
- [x] 自定义 `template.docx` 的纸张尺寸和页边距能够改变页面可用宽度，所有表格均不越过页边距。
- [x] 默认模式表头为浅蓝底粗体；格式标准化模式无底色、无粗体，仅表头下边框加强。
- [x] 短标签列明显窄于长说明列，文字与边框之间有留白，长 URL 不会迫使其他列失去可读性。
- [x] 7 列及以上只缩小表格局部字号，正文样式不变。
- [x] 100 行压力表格经 Microsoft Word 真实渲染为 9 页；每页重复表头，表头不拆分、不孤立，正文 1–100 行无丢失。
- [x] 使用 `机顶盒统一主页与后台各系统交互设计方案.md` 导出 DOCX；Microsoft Word 渲染为 42 页、45 张表和 16 张图片，短标签列、长说明列、页边距及单元格留白符合预期。

### 7.3 实机矩阵

- [x] Microsoft Word 16.0.19426.20218 完成真实渲染检查；真实文档 42 页、压力文档 9 页，45/45 与 2/2 张表的重复表头和禁止拆分属性均由 Word 识别。
- [ ] 干净 Windows 11 x64 VM 仅复制 EXE 和测试文档目录，验证启动、图片、编辑、DOCX 导出和离线预览。
- [ ] 在缺少 WebView2 Runtime 的环境中验证：程序显示可操作的中文提示而不是闪退，提示含微软下载入口，诊断日志记录原因。当前已完成错误分类和提示契约自动测试，尚缺真实缺失运行时环境。
- [ ] WPS/LibreOffice：OOXML 兼容设计已保留；若发布环境没有相应软件，则本版本状态必须标记为“OOXML 兼容，未完成本版本实机验证”，不得写成已验证。

当前文档状态：Microsoft Word 实机验收已完成。WPS/LibreOffice 为 OOXML 兼容设计，未完成本版本实机验证；干净 Windows 11 VM 和真实缺失 WebView2 Runtime 环境也未完成，因此不得宣称全平台、全环境验收通过。

## 八、常见问题处理

### 8.1 启动时提示缺少 WebView2

1. 打开提示中的微软下载入口，安装 Evergreen Standalone Installer x64。
2. 安装完成后重新启动 MD Preview。
3. 若仍失败，查看 `%LOCALAPPDATA%\md-preview\md-preview.log`；使用 `MD_PREVIEW_CONFIG_DIR` 时查看所指定目录下的 `md-preview.log`。

### 8.2 PDF 导出提示找不到浏览器

安装 Microsoft Edge 或 Google Chrome 后重试。也可使用既有 `MD_PREVIEW_PDF_BROWSER` 环境变量指向浏览器可执行文件；这不影响 DOCX 导出。

### 8.3 DOCX 表格与预期不符

先确认使用的是 v1.9.4，并检查 EXE 旁是否存在 `template.docx`，或是否设置了 `MD_PREVIEW_TEMPLATE`。模板的纸张尺寸和页边距会直接参与列宽计算。保留原 Markdown、模板和生成的 DOCX 供问题定位，不要只提供截图。

### 8.4 SmartScreen 提示未知发布者

本版不包含 Authenticode 签名。只有在下载来源可信、EXE SHA-256 与 `.sha256` 文件及本文第十节完全一致时才继续；否则取消运行并重新下载。

## 九、回滚到 v1.9.3

1. 关闭所有 MD Preview 窗口。
2. 把 v1.9.4 EXE 重命名为 `MD-Preview-windows-x64-v1.9.4.failed.exe` 并保留用于问题定位，不要删除 Markdown、图片、模板或配置目录。
3. 将备份的 `MD-Preview-windows-x64-v1.9.3.bak.exe` 改回 `MD-Preview-windows-x64.exe`。
4. 执行：

   ```powershell
   .\MD-Preview-windows-x64.exe --version
   ```

5. 确认输出 `MD Preview 1.9.3`。
6. 启动一次 v1.9.3，使 Windows 文件关联刷新到回滚后的 EXE；再打开一份 Markdown 验证预览和编辑。

回滚不涉及数据迁移，也不删除 Markdown、图片、`template.docx`、用户配置或会话数据。已经由新版保存的文本内容不会随程序回滚，如需恢复文档内容，请使用 Git 或事先保留的文件副本。

## 十、最终发布校验值

以下数值对应 `release/v1.9.4/MD-Preview-windows-x64.exe`；重新构建或替换 EXE 后必须同步重算三处发布记录。

- 文件大小：`6,911,488` 字节（约 `6.59 MiB`）
- SHA-256：`417138DC965553223AADAA422A107DEF64E58930824F2039CFDCCEDD2031CD41`
- Authenticode：`NotSigned`（未签名；可能触发 SmartScreen“未知发布者”提示）

建议记录命令：

```powershell
(Get-Item .\MD-Preview-windows-x64.exe).Length
Get-FileHash .\MD-Preview-windows-x64.exe -Algorithm SHA256
Get-AuthenticodeSignature .\MD-Preview-windows-x64.exe | Format-List Status,StatusMessage,SignerCertificate
```
