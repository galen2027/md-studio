# 《MD Preview 发布部署方案》_v1.9.5_261009

更新时间：2026-10-09  
适用平台：Windows 11 x64  
发布目录：`release/v1.9.5/`  
主程序：`MD-Preview-windows-x64.exe`  
当前状态：v1.9.5 发布构建、自动测试、Microsoft Word 实机渲染和本机文件关联验收已完成；干净 Windows 11 VM、真实 NTFS 软链接及 WPS/LibreOffice 实机验证未完成，详见第八节和第十一节

## 一、版本结论

MD Preview v1.9.5 延续 v1.9.4 的 Windows x64 便携单 EXE 方案：MSVC C Runtime 静态链接，接收方不需要另装 Visual C++ Redistributable；图形界面仍使用 Windows 的 Microsoft Edge WebView2 Runtime。

本版聚焦三个问题：

1. DOCX 表格的外框和内部横纵线统一增强为 0.75 pt、`#808080` 的 `single` 实线，避免原 0.5 pt 浅灰线在 Microsoft Word 低缩放比下近乎不可见。
2. Windows 单实例改为先绑定、再仅对冲突端口做有界探测，并将候选端口分散到较低范围，可快速跳过不可绑定端口，避免约 10 秒启动等待。
3. Windows 文件关联不再主动 canonicalize 或解析 EXE 链接；注册时把 `current_exe` 转为 Explorer 可接受的普通 Win32 路径，校验真实注册表命令，并同时维护当前 EXE 文件名和兼容 alias `Applications\md-preview.exe`。

本版不新增 CLI 参数，不改变 Markdown 语法、图片相对路径规则、预览/编辑布局、打印或 PDF 页面方向。Windows ARM64 原生版本和 Authenticode 签名仍不在本次范围内。

## 二、发布目录清单

最终 `release/v1.9.5/` 只放本版交付文件：

1. `MD-Preview-windows-x64.exe`
2. `MD-Preview-windows-x64.exe.sha256`
3. `《MD Preview 发布部署方案》_v1.9.5_261009.md`

发布目录已使用最终 release 构建封装，并记录文件大小、SHA-256 和签名状态。任何一次重新构建都会使旧文件大小和哈希失效，必须重新计算并同步本文件和 `.sha256` 文件。

`template.docx` 是可选用户模板，不属于必需发布物。构建缓存、测试文档、日志、截图、临时注册表导出和 `target/` 下的中间文件不得进入发布目录。

## 三、必需与可选依赖

| 项目 | 类型 | 说明 |
|---|---|---|
| Windows 11 x64 | 必需 | 本版发布目标；不提供 Windows ARM64 原生包。 |
| Microsoft Edge WebView2 Runtime（Evergreen） | 必需 | 显示应用界面。完整更新的 Windows 11 通常已预装，企业裁剪镜像可能缺失。 |
| Visual C++ Redistributable | 不需要 | MSVC CRT 静态链接；发布门禁禁止导入 `VCRUNTIME*`、`MSVCP*`、`CONCRT*`。 |
| Node.js、Python、Rust | 不需要 | 仅开发和构建阶段使用；Mermaid、KaTeX、代码高亮及图标均内嵌。 |
| Microsoft Word | 生成 DOCX 不需要 | DOCX 由程序直接生成 OOXML；打开或人工验收文件时可使用 Word、WPS 或 LibreOffice。 |
| Microsoft Edge 或 Google Chrome | 仅导出 PDF 时需要 | 普通预览、编辑和 DOCX 导出不依赖外部浏览器。 |
| `template.docx` | 可选 | 放在 EXE 同目录，或用 `MD_PREVIEW_TEMPLATE` 指向；模板纸张和页边距参与表格列宽计算。 |

WebView2 官方下载入口：<https://developer.microsoft.com/microsoft-edge/webview2/>  
WebView2 分发说明：<https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution>

## 四、v1.9.5 变更说明

### 4.1 DOCX 表格实线边框

- 继续使用模板页面宽度、内容权重、固定 `tblGrid`、`tblLayout=fixed` 和每格 `tcW=dxa`。
- `tblBorders` 的 `top/left/bottom/right/insideH/insideV` 六类边框全部使用 `single + 0.75 pt + #808080`；格式标准化模式的表头底线保持 `single + 1 pt + #808080`。
- 保留单元格留白、数字列自动右对齐、Markdown 显式对齐、跨页重复表头、正文行跨页、表格内链接/inline code/图片及显式换行。
- 默认表头仍使用浅色底纹和粗体；格式标准化模式仍保持无底色、无粗体。
- 该变更仅影响 DOCX 导出，不改变预览、打印或 PDF 中的 HTML 表格。

### 4.2 Windows 启动速度

- 单实例仍通过本机回环地址通信，不开放局域网访问，也不增加外部服务依赖。
- 程序先尝试绑定端口，绑定成功即立即启动；只有确认端口冲突时才做有界转发探测。候选端口在 `20000..39999` 内以步长分散，不再连续落入同一段 Windows/Hyper-V 保留区。
- 首个实例启动、空白启动、带文件启动和第二次双击转发现有窗口都必须分别计时验收。
- `--help`、`--version` 继续在本地直接输出，不启动 GUI，也不联系现有实例。

### 4.3 Windows 文件关联与链接路径

- 不再主动对当前 EXE 路径执行 canonicalize，因此不会由应用自行把用户启动路径强制改写成链接目标路径。
- Windows `\\?\` 或 `\\?\UNC\` verbatim 路径会转换成 Explorer/Shell 可执行的普通 Win32 路径后再写入注册表。
- 每次需要注册时会比对实际注册表命令，不再仅凭 `.md-preview-registered` marker 判断是否已经正确配置。
- 同时维护 `Applications\<当前EXE文件名>` 和兼容 alias `Applications\md-preview.exe`，减少历史“打开方式”条目因 EXE 改名失效的概率。
- Windows 对符号链接的 `current_exe` 行为仍由系统决定。推荐把真实 EXE 复制到固定的本机目录，不要把会随构建清理或替换失效的软链接、硬链接作为唯一默认处理程序。
- 移动或重命名 EXE 后，在新位置手动启动一次，再从“打开方式”重新选择 MD Preview。

## 五、发布构建与门禁

在项目根目录执行：

```powershell
cargo fmt --check
cargo test
cargo build --release
bash scripts/verify.sh
```

随后逐项检查：

1. PE 门禁脚本自测：

   ```powershell
   .\scripts\verify-windows-pe-dependencies.ps1 -SelfTest
   ```

2. 检查 release EXE：

   ```powershell
   powershell -ExecutionPolicy Bypass -File scripts\verify-windows-pe-dependencies.ps1 -Path target\release\md-preview.exe
   ```

3. 检查 CLI：

   ```powershell
   .\target\release\md-preview.exe --help
   .\target\release\md-preview.exe --version
   ```

   `--version` 必须输出 `MD Preview 1.9.5`，两条命令都不得弹出 GUI。

4. 检查 EXE 未导入 `VCRUNTIME*`、`MSVCP*`、`CONCRT*`。
5. 运行第八节验收矩阵；任一必测项失败都不能封装发布目录。
6. 所有修改完成后再构建一次最终 EXE。只对该最终文件计算 SHA-256、文件大小和 Authenticode 状态。

## 六、傻瓜式安装或升级

### 6.1 下载与校验

1. 从同一可信发布源下载 EXE、`.sha256` 和本部署文档，放进同一个目录。
2. 在该目录打开 PowerShell，执行：

   ```powershell
   Get-FileHash .\MD-Preview-windows-x64.exe -Algorithm SHA256
   Get-Content .\MD-Preview-windows-x64.exe.sha256
   ```

3. 确认两个 SHA-256 完全一致，并与本文第十一节最终记录一致。不一致时不要运行。

### 6.2 安装 v1.9.5

1. 关闭所有旧版 MD Preview 窗口。
2. 将现有 v1.9.4 EXE 重命名为 `MD-Preview-windows-x64-v1.9.4.bak.exe`，暂时保留。
3. 新建一个长期不变的本机目录，例如 `C:\Tools\MD-Preview\`，将 v1.9.5 EXE 复制进去。不要直接使用下载临时目录、`target\release\`、网络共享或会被构建清理的软链接目录。
4. 执行：

   ```powershell
   .\MD-Preview-windows-x64.exe --version
   ```

5. 确认输出 `MD Preview 1.9.5`。
6. 双击 EXE 启动一次，完成“打开方式”注册刷新。
7. 右键一份 `.md` 文件，选择“打开方式”→“选择其他应用”→ MD Preview，并按需勾选始终使用。
8. 再双击一份带中文、空格或书名号路径的 Markdown，确认当前窗口新增或激活标签。
9. 如果 SmartScreen 显示“未知发布者”，先核对 SHA-256 和来源；任何一项不确定都应取消运行。

### 6.3 便携数据与模板

默认用户配置、会话和诊断日志位于 `%LOCALAPPDATA%\md-preview\`，无该变量时回退到 `%APPDATA%\md-preview\`。若要让数据放在程序目录附近，可显式设置相对目录：

```powershell
$env:MD_PREVIEW_CONFIG_DIR='.\data'
.\MD-Preview-windows-x64.exe
```

使用自定义 Word 模板时：

```powershell
$env:MD_PREVIEW_TEMPLATE='.\template.docx'
.\MD-Preview-windows-x64.exe
```

相对路径以启动时的当前工作目录为准；通过资源管理器双击启动时，为避免歧义，优先把 `template.docx` 放在 EXE 同目录。

## 七、默认应用或软链接故障修复

当双击 `.md` 提示“Windows 无法访问指定设备、路径或文件”时，按以下顺序处理：

1. 先直接双击真实 EXE，确认它本身可以启动；若 EXE 也不能启动，检查 SmartScreen、杀毒软件隔离和文件权限。
2. 若此前默认应用指向软链接或硬链接，把真实 v1.9.5 EXE 复制到固定的本机目录，并在该目录启动一次。
3. 在 `.md` 文件上右键，选择“打开方式”→“选择其他应用”→“在电脑上选择应用”，定位到固定目录中的真实 EXE。
4. 若资源管理器仍保留旧图标或旧路径，注销并重新登录 Windows，或重启 Explorer 后再选一次默认应用。
5. 不要手工删除整棵 `HKEY_CURRENT_USER\Software\Classes`。需要诊断时只导出并检查以下位置：
   - `HKCU\Software\Classes\MDPreview.md`
   - `HKCU\Software\Classes\Applications\<当前EXE文件名>`
   - `HKCU\Software\Classes\Applications\md-preview.exe`
6. 保留 `%LOCALAPPDATA%\md-preview\md-preview.log` 和出错截图，便于定位；不要在日志中加入业务文档内容。

注意：Windows 的 `UserChoice` 带系统生成哈希，应用不能也不应静默篡改。MD Preview 只注册“打开方式”能力，最终默认应用必须由用户在 Windows 界面中选择。

## 八、发布验收矩阵

### 8.1 自动与结构验证

- [x] `cargo fmt --check` 通过。
- [x] `cargo test`：91 项通过，0 项失败。
- [x] `cargo build --release` 通过。
- [x] `bash scripts/verify.sh` 通过；Playwright、macOS、iOS、Android 项因当前 Windows 环境或依赖不可用按脚本规则跳过。
- [x] PE 门禁自测 6 项通过；最终 EXE 无 `VCRUNTIME*`、`MSVCP*`、`CONCRT*` 动态导入。
- [x] `--help` 和 `--version` 不启动 GUI；版本输出为 `MD Preview 1.9.5`。
- [x] 真实文档 DOCX XML 共 48 张表，48/48 的 `tblBorders` 六类边框均为 `single/sz=6/color=808080`，且无 `nil/none`；标准化模式回归确认表头底线为 `single/sz=8/color=808080`。
- [x] `.md` 注册命令包含带引号的 EXE 和 `"%1"`，且无 `\\?\` verbatim 前缀。
- [x] 当前 EXE 文件名和 `Applications\md-preview.exe` 兼容 alias 均已注册；改名后的发布 EXE和稳定安装名均已实测。

### 8.2 启动速度与单实例

- [x] 当前 Windows 11 x64 主机重复实测：空白启动到 WebView ready 为 832–914 ms；打开 83,348 字节真实 Markdown 为 1,172–1,299 ms。
- [x] 正常启动完成单实例注册仅 32–37 ms，不再出现旧版约 10 秒的连续端口超时等待。
- [x] 第二次通过 Windows Shell 打开 Markdown 在 167 ms 内复用当前进程，进程数保持 1，并保留两个文件标签。
- [ ] 当前窗口已有未保存编辑时的人工交互场景未单独复测；本版未修改脏标签保护逻辑，完整自动测试通过。
- [x] 人工占用首个候选端口时仍可在 307 ms 完成单实例注册；12 个候选全部被占用时约 3,189 ms 后安全回退为独立实例并正常启动。

### 8.3 文件关联路径

- [x] 稳定真实路径 `C:\Tools\md-preview.exe` 设置为“打开方式”后，通过 Windows Shell 打开含中文、空格和书名号的真实 `.md` 成功，退出码为 0。
- [ ] 当前主机没有创建 NTFS 软链接所需的开发者模式或管理员权限，未完成真实软链接端到端测试；已完成 verbatim 路径/普通路径系统 A/B 测试以及盘符、UNC 转换单元测试，注册命令不含 Explorer 不接受的前缀。
- [x] 发布 EXE 改名为 `MD-Preview-windows-x64.exe` 后启动一次，当前文件名和兼容 alias 均刷新；切回稳定安装名后再次正确刷新。
- [x] 用户原默认项已从 `\\?\` 路径修复为 `"C:\Tools\md-preview.exe" "%1"`；未重写 Windows `UserChoice`，也未删除 Markdown、图片或配置。
- [x] 程序未运行时通过 Shell 可正常冷启动；程序已运行时再次打开文件会复用当前窗口且仅保留一个进程。

### 8.4 DOCX 真实渲染

- [x] 2/3/4/6/10 列结构回归通过；Microsoft Word 实际渲染中的表格均显示完整连续实线。
- [x] 默认与格式标准化两种模式均已用 Microsoft Word 渲染，实线边框可见；标准化表头的 1 pt 底线保留。
- [x] 默认、标准化、自定义模板及 10 列压力用例确认表格未越过页边距，列宽、单元格留白、数字右对齐、inline code 和显式换行未回归。
- [x] 100 行压力文档在 Microsoft Word 中渲染为 9 页，每页重复表头，正文无丢失；第 9 页 10 列表格完整。
- [x] 使用 `《机顶盒统一主页与后台各系统交互设计方案》_v1.4.0_261009.md` 导出 11,126,518 字节 DOCX；Microsoft Word 渲染为 57 页，48 张表、21 张图片均保留，48/48 张表的六类边框均为 0.75 pt 单实线。
- [x] 本版已明确标记：WPS/LibreOffice 按 OOXML 兼容设计，但未完成本版本实机渲染验证；当前主机未安装 LibreOffice `soffice.exe`。

### 8.5 干净环境

- [ ] 当前没有可用的干净 Windows 11 x64 VM，未完成“只复制单个 EXE 和测试文档目录”的独立环境验收。
- [ ] 未在明确移除 Visual C++ Redistributable 的 VM 中实跑；PE 依赖门禁已确认最终 EXE 不导入动态 VC Runtime。
- [ ] 干净 VM 中的图片、编辑、分栏、DOCX 导出和离线预览未实测；当前 Windows 11 主机上的相应功能及真实文档导出已通过。
- [ ] 未在缺少 WebView2 Runtime 的系统镜像中实测提示窗；HRESULT 识别、中文提示内容与日志路径已有自动测试覆盖。

## 九、常见问题

### 9.1 启动时提示缺少 WebView2

安装 Microsoft Edge WebView2 Evergreen Standalone Installer x64，安装后重新启动。仍失败时查看 `%LOCALAPPDATA%\md-preview\md-preview.log`。

### 9.2 PDF 导出找不到浏览器

安装 Microsoft Edge 或 Google Chrome 后重试；也可用现有 `MD_PREVIEW_PDF_BROWSER` 指向浏览器 EXE。该问题不影响预览、编辑或 DOCX 导出。

### 9.3 DOCX 仍看不到表格实线

1. 先执行 `--version`，确认是 `1.9.5`。
2. 用 Microsoft Word 打开，不要只看资源管理器缩略图。
3. 检查是否用了自定义 `template.docx`，并同时保留模板、原 Markdown 和导出 DOCX。
4. 解压 DOCX 检查 `word/document.xml` 中 `tblBorders` 是否包含完整的 `top/left/bottom/right/insideH/insideV` 实线定义。
5. 若只有某个办公软件不显示，记录软件名称、完整版本和截图；在完成实机验证前不要宣称兼容。

### 9.4 SmartScreen 提示未知发布者

本版不包含 Authenticode 签名。只有下载来源可信，且 EXE SHA-256 与 `.sha256` 文件和本文最终记录完全一致时才继续；否则取消运行并重新下载。

## 十、回滚到 v1.9.4

1. 关闭所有 MD Preview 窗口。
2. 将 v1.9.5 EXE 重命名为 `MD-Preview-windows-x64-v1.9.5.failed.exe`，保留用于问题定位。
3. 将备份的 `MD-Preview-windows-x64-v1.9.4.bak.exe` 改回 `MD-Preview-windows-x64.exe`。
4. 执行：

   ```powershell
   .\MD-Preview-windows-x64.exe --version
   ```

5. 确认输出 `MD Preview 1.9.4`。
6. 在固定目录启动一次 v1.9.4，再从 Windows“打开方式”重新选择该 EXE。
7. 打开一份 Markdown，验证预览、图片和编辑。

回滚不涉及数据迁移，也不删除 Markdown、图片、`template.docx`、用户配置或会话数据。注意：v1.9.4 可能重新出现本版修复的表格边框、保留端口启动等待或链接路径文件关联问题。

## 十一、最终发布校验值

以下数值对应 `release/v1.9.5/MD-Preview-windows-x64.exe`。若重新构建或替换 EXE，必须重新执行本节命令并同步 `.sha256` 文件：

- 文件大小：`6,915,072` 字节
- SHA-256：`C24DC8B6BD22F33B8BCF51973C82DF16BAC21688D33BD426C5D30691A6603BCE`
- Authenticode：`NotSigned`（未签名；可能触发 SmartScreen“未知发布者”提示）
- 自动测试：`cargo fmt --check`、91 项 Rust 测试、release 构建、`scripts/verify.sh`、PE 依赖门禁及 CLI 冒烟均通过；脚本中不适用于当前环境的 Playwright/macOS/iOS/Android 项已按规则跳过
- Microsoft Word 实机渲染：已完成；真实文档 57 页、48 张表、21 张图片，48/48 张表六类边框均为 0.75 pt 单实线；100 行压力文档 9 页并重复表头
- 干净 Windows 11 x64 VM：未完成，本版没有可用干净 VM
- WPS/LibreOffice：按 OOXML 兼容设计，未完成本版本实机验证；当前主机未安装 LibreOffice `soffice.exe`

记录命令：

```powershell
(Get-Item .\MD-Preview-windows-x64.exe).Length
Get-FileHash .\MD-Preview-windows-x64.exe -Algorithm SHA256
Get-AuthenticodeSignature .\MD-Preview-windows-x64.exe | Format-List Status,StatusMessage,SignerCertificate
```
