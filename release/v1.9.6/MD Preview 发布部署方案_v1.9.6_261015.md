# MD Preview 发布部署方案_v1.9.6_261015

更新时间：2026-10-15  
适用平台：Windows 11 x64  
发布目录：`release/v1.9.6/`  
主程序：`MD-Preview-windows-x64.exe`

## 一、版本说明

v1.9.6 在 v1.9.5 的便携单 EXE、快速启动、文件关联、图片加载和 DOCX 表格能力上继续做三项桌面交互修复：

1. Mermaid 灯箱放大后支持按住空格键并用鼠标左键拖动，可精确移动到图表局部；滚轮缩放、适应窗口、SVG/PNG 下载保持兼容。
2. 只读文件或无写权限文件保存失败时，关闭窗口或标签会显示“另存为 / 放弃修改并退出（关闭标签）/ 取消”，不再反复强制保存而无法退出。
3. Windows 编辑器修复中文输入法首次聚焦问题；IME 组合期间暂停自动保存和快捷键处理，组合结束后再保存，窗口切回前台时恢复 WebView2 和编辑器焦点。

本版不改变 CLI 参数、Markdown 语法、图片路径规则、DOCX/PDF 格式、单实例协议和移动端交互。

## 二、依赖边界

| 项目 | 要求 |
|---|---|
| Windows | Windows 11 x64 |
| Visual C++ Redistributable | 不需要，MSVC CRT 静态链接 |
| Microsoft Edge WebView2 Runtime | 必需；完整更新的 Windows 11 通常已安装 |
| Node.js、Python、Rust | 使用发布 EXE 时不需要 |
| Word/WPS/LibreOffice | 仅用于打开 DOCX；生成 DOCX 不需要 |
| Edge 或 Chrome | 仅“导出 PDF”需要 |
| Authenticode | 本版未签名，SmartScreen 可能显示未知发布者 |

WebView2 下载入口：<https://developer.microsoft.com/microsoft-edge/webview2/>

## 三、傻瓜式安装与升级

1. 关闭所有 MD Preview 窗口。
2. 把旧版 EXE 重命名为 `MD-Preview-windows-x64-v1.9.5.bak.exe`，先不要删除。
3. 新建固定本机目录，例如 `C:\Tools\MD-Preview\`，把本版 `MD-Preview-windows-x64.exe` 复制进去；不要把下载临时目录、构建输出目录或会失效的软链接作为默认程序入口。
4. 在该目录打开 PowerShell，执行：

   ```powershell
   Get-FileHash .\MD-Preview-windows-x64.exe -Algorithm SHA256
   Get-Content .\MD-Preview-windows-x64.exe.sha256
   .\MD-Preview-windows-x64.exe --version
   ```

5. 确认两个 SHA-256 一致，且版本输出为 `MD Preview 1.9.6`。
6. 双击 EXE 启动一次，以刷新当前真实路径的“打开方式”注册。
7. 在 `.md` 文件上右键，选择“打开方式”→“选择其他应用”→“在电脑上选择应用”，选择固定目录中的真实 EXE。
8. 若 SmartScreen 提示未知发布者，仅在下载来源可信且 SHA-256 完全一致时继续。

## 四、使用验证

### 4.1 图表拖动

1. 打开带 Mermaid 图表的 Markdown，点击图表进入灯箱。
2. 用滚轮或“+”放大，直到出现横向或纵向滚动范围。
3. 按住空格键，再按住鼠标左键拖动；应能移动到图表局部，鼠标指针显示抓取状态。
4. 松开鼠标和空格键；再次滚轮缩放、点击“适应窗口”、下载 SVG/PNG、按 Escape 关闭，均应正常。

### 4.2 只读文件安全退出

1. 复制一个测试 `.md`，在文件属性中勾选“只读”。
2. 用 MD Preview 打开并编辑，然后关闭窗口或当前标签。
3. 保存失败后应出现三个选择：
   - “另存为…”：选择新路径，保存成功后完成原来的关闭动作；
   - “放弃修改并退出/关闭标签”：不写原文件，立即退出或关闭标签；
   - “取消”：保留窗口、标签和编辑内容。
4. 任一路径都不应再次无条件弹回保存流程。

### 4.3 中文输入法

1. 打开已有内容的 Markdown，按 `Ctrl+E` 进入编辑。
2. 切换微软拼音或其他 Windows 中文输入法，直接输入中文；不应要求先切换到其他程序。
3. `Alt+Tab` 离开后再切回，继续输入中文；候选框和组合输入应正常。
4. 组合过程中按 Escape 只交给输入法处理，不应退出编辑；组合完成并停顿约 700 ms 后才自动保存。

## 五、发布门禁

在项目根目录执行：

```powershell
cargo fmt --check
cargo test
cargo build --release
bash scripts/verify.sh
powershell -ExecutionPolicy Bypass -File scripts\verify-windows-pe-dependencies.ps1 -Path target\release\md-preview.exe
.\target\release\md-preview.exe --help
.\target\release\md-preview.exe --version
```

发布目录只包含：

1. `MD-Preview-windows-x64.exe`
2. `MD-Preview-windows-x64.exe.sha256`
3. `MD Preview 发布部署方案_v1.9.6_261015.md`

发布前必须确认 EXE 不导入 `VCRUNTIME*`、`MSVCP*` 或 `CONCRT*`，`--version` 为 1.9.6，并重新计算最终文件的大小与 SHA-256。

## 六、风险与注意事项

- Windows 中文 IME 问题来自 WebView2/Windows TSF 的首焦点交互。本版通过首次聚焦规避、组合事件保护和窗口焦点恢复降低触发概率；仍需用目标电脑上的实际输入法人工验收。
- “放弃修改”会丢弃本次尚未写盘的编辑，但不会删除原文件；不确定时选择“取消”或“另存为”。
- 相对图片路径仍以 Markdown 所在目录为基准。复制到其他电脑时必须一起复制 Markdown 和图片目录并保持相对结构。
- WebView2 是图形界面的必需系统运行时；企业裁剪镜像缺失时，按程序提示安装 Evergreen Runtime。
- 发布 EXE 未签名；必须通过可信渠道分发并核对 SHA-256。

## 七、回滚方案

1. 关闭 v1.9.6；若有未保存内容，先另存为。
2. 将 v1.9.6 重命名为 `MD-Preview-windows-x64-v1.9.6.failed.exe`，保留用于诊断。
3. 把备份的 v1.9.5 恢复为 `MD-Preview-windows-x64.exe`。
4. 执行 `--version`，确认输出 `MD Preview 1.9.5`。
5. 启动 v1.9.5 一次，再从 Windows“打开方式”重新选择固定目录中的 EXE。
6. 打开测试 Markdown，验证预览、图片和编辑。

回滚不删除 Markdown、图片、模板、用户配置或会话数据；但会重新出现 v1.9.6 修复的图表无法拖动、只读文件关闭循环和中文输入法首焦点问题。

## 八、最终发布记录

- 文件大小：`6,925,312` 字节
- SHA-256：`3DBDD5AAA64C7871E5E094E33244C2C5B52F8477FD56502EEDE7715ECD347487`
- Authenticode：`NotSigned`
- Rust/脚本测试：`cargo fmt --check` 通过；`cargo test` 92 项通过；`cargo build --release`、`scripts/verify.sh`、PE 静态 CRT 门禁、`--help` 和 `--version` 通过
- 浏览器交互：无界面 Edge 已通过 Mermaid 空格拖动、SVG/PNG 导出、中文 composition/autosave 顺序、搜索替换和阅读工具回归
- Windows GUI 冒烟：最终发布 EXE 的内置基准模式正常启动并退出，首帧 ready 为 `1,072 ms`
- 人工验证：真实 Windows 中文输入法候选框、只读原生三按钮对话框的三个分支尚未由人工逐项点击验收，发布前应按第四节复核
- 干净 Windows 11 x64 VM：如无可用环境，必须明确标记未验证，不得写成已通过
