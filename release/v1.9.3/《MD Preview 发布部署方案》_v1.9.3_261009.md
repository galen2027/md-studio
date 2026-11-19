# MD Preview v1.9.3 发布部署方案

更新时间：2026-10-09  
适用平台：Windows x64  
发布目录：`release/v1.9.3/`  
主程序：`MD-Preview-windows-x64.exe`

## 一、版本说明

v1.9.3 在 v1.9.2 的 `Ctrl+H` 查找替换基础上完成四项升级：

1. 修复 Windows WebView2 本地图片预览。标准 Markdown 图片引用始终以已打开的 `.md` 文件目录为基准，不依赖程序 EXE 位于哪个目录；支持相对路径、`../`、Windows 反斜杠、中文/空格及百分号编码、本机绝对路径和本机 `file:` URL。
2. 新增“左右分栏”按钮：左侧为渲染预览，右侧为 Markdown 编辑器。分栏沿用 dirty 状态、700 ms 自动保存、`Ctrl+H` 替换、实时统计和未命名草稿预览。
3. 新增 Windows 单实例文件打开：已有窗口运行时，再双击其他 `.md` 文件会恢复并聚焦原窗口，然后新增或激活标签；不会再启动一个长期运行的新程序。活动标签有未保存内容时不会被新文件覆盖。
4. EXE 被复制或移动到新目录后，首次启动会刷新 Windows 文件关联命令，避免注册表继续指向旧程序位置。

## 二、图片路径与可移植性边界

- 推荐写法：`![说明](images/示意图.png)`。只要复制 Markdown 时同时复制 `images` 目录并保持相对结构，换电脑、换磁盘、换 EXE 目录后都能正常显示。
- 支持 `../共享图片/示意图.png`、反斜杠路径、本机绝对路径以及本机 `file:` URL。
- 本机绝对路径虽然可读取，但例如 `C:\项目\images\a.png` 在另一台电脑上未必存在，因此不属于跨电脑可移植写法。
- 为防止不可信文档触发网络共享访问，UNC 路径和远程 `file://server/...` 会被拒绝。
- 本次重写范围是标准 Markdown 图片语法 `![]()`；原始 HTML `<img src="...">` 的本地路径不经过该受控资源通道。

## 三、发布前校验

在项目根目录执行：

```powershell
cargo fmt --check
cargo test
cargo build --release
.\target\release\md-preview.exe --help
.\target\release\md-preview.exe --version
```

浏览器回归使用工作区自带的 Node/Playwright 与本机 Edge：

```powershell
$env:NODE_PATH="C:\Users\luosh\.cache\codex-runtimes\codex-primary-runtime\dependencies\node\node_modules"
$env:MD_PREVIEW_TEST_BROWSER="C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
$node="C:\Users\luosh\.cache\codex-runtimes\codex-primary-runtime\dependencies\node\bin\node.exe"
& $node scripts\verify-desktop-reading-tools.mjs
& $node scripts\verify-desktop-search.mjs
& $node scripts\verify-anchor-navigation.mjs
& $node scripts\verify-mermaid-export.mjs
```

本次已执行的关键验证：

- Rust 单元/契约测试：70/70 通过。
- 指定业务文档共 6 个本地图片引用，6 个文件全部解析、读取和解码成功，总计 `5,964,236` 字节。
- 真实 Windows WebView2：6/6 张图 `naturalWidth`、`naturalHeight` 均大于 0，尺寸为 `1920×1080`、`1280×720`、`3840×2160`、`1280×720`、`1280×720`、`1280×720`。
- 浏览器专项：分栏/阅读工具、搜索替换、锚点导航、Mermaid SVG/3× PNG/300-PPI Word 回退全部通过。
- 双进程集成：第二进程退出码 0；首进程会话最终为 2 个标签，第二个文档成为活动标签。

## 四、傻瓜式部署步骤

1. 下载同一目录中的三个文件：
   - `MD-Preview-windows-x64.exe`
   - `MD-Preview-windows-x64.exe.sha256`
   - `《MD Preview 发布部署方案》_v1.9.3_261009.md`
2. 在下载目录空白处按住 Shift 并点击鼠标右键，选择“在此处打开 PowerShell”。
3. 执行校验：

   ```powershell
   Get-FileHash .\MD-Preview-windows-x64.exe -Algorithm SHA256
   Get-Content .\MD-Preview-windows-x64.exe.sha256
   ```

4. 确认两处 SHA-256 值完全一致；不一致时不要运行程序，重新从可信发布源下载。
5. 关闭所有旧版 MD Preview 窗口。
6. 把旧程序重命名为 `MD-Preview-windows-x64-v1.9.2.bak.exe`，不要直接删除。
7. 把新版 `MD-Preview-windows-x64.exe` 复制到准备长期使用的目录。程序是单 EXE，不需要安装服务、数据库或额外运行脚本。
8. 执行版本检查：

   ```powershell
   .\MD-Preview-windows-x64.exe --version
   ```

9. 预期输出：`MD Preview 1.9.3`。
10. 启动一次新版程序。该步骤会把 Windows `.md` 文件关联刷新到当前 EXE 路径；以后若再次移动 EXE，也应在新位置启动一次。
11. 打开任意带相对图片的 Markdown，确认图片显示；点击分栏按钮，确认左侧预览、右侧编辑。
12. 保持窗口打开，再双击另一份 `.md`，确认原窗口新增标签且没有第二个长期运行的 MD Preview 窗口。

用户配置、会话和诊断日志使用应用既有的每用户目录；可通过 `MD_PREVIEW_CONFIG_DIR` 指定相对数据目录，例如先执行 `$env:MD_PREVIEW_CONFIG_DIR='.\data'` 再启动。文档和图片从各自 Markdown 所在目录解析，不从配置目录或 EXE 目录解析。

## 五、验收清单

- [ ] `--help` 显示文件参数、`--edit`、`--version`；`--version` 显示 `MD Preview 1.9.3` 且不打开窗口。
- [ ] 打开 `C:\DevOps\012\机顶盒统一主页与后台各系统交互设计方案.md`，6 张图片全部清晰可见。
- [ ] 把 EXE 复制到另一目录后打开同一份文档，图片显示不受影响。
- [ ] 把 Markdown 与图片目录整体复制到另一目录并保持相对结构，图片仍能显示。
- [ ] 中文、空格、百分号编码、`../`、反斜杠、本机绝对路径和本机 `file:` 图片都按预期加载。
- [ ] 点击分栏按钮后左侧为预览、右侧为编辑器，二者不重叠；再次点击回到纯预览。
- [ ] 分栏中输入内容后预览更新，等待约 1 秒关闭并重开，内容已经保存。
- [ ] 分栏中按 `Ctrl+H` 保持分栏并显示替换栏；单项替换、全部替换和撤销/重做正常。
- [ ] 已有窗口运行时双击另一份 `.md`，原窗口恢复并新增标签，第二进程自动退出。
- [ ] 重复打开同一路径只激活已有标签，不创建重复标签。
- [ ] 活动标签存在未保存内容时外部打开新文件，原编辑内容和活动状态不丢失。
- [ ] 移动 EXE 后从新目录启动一次，再双击 `.md` 会调用新目录中的 EXE。
- [ ] Mermaid 下载的 SVG 为矢量原图，PNG 为 3× 高清图；Word 导出尺寸无回归。
- [ ] 发布 EXE 的 SHA-256 与 `.sha256` 文件、本文第八节完全一致。

## 六、风险与注意事项

- Markdown 本地图片是原文件流式显示，不会为了预览复制图片；删除、移动或改名图片后，原引用自然会失效。
- 相对路径的可移植性依赖 Markdown 与图片目录保持原来的相对关系；绝对盘符路径无法保证跨电脑有效。
- UNC 和远程 `file:` 路径被主动拒绝，这是防止 SMB/网络共享意外访问的安全边界。
- 单实例使用仅绑定 `127.0.0.1` 的本机 IPC，并校验实例键、协议版本、帧大小和 ACK；端口全部被占用时会降级为独立窗口，保证文件仍能打开。
- 本次发布物若 Authenticode 状态为 `NotSigned`，Windows SmartScreen 可能提示未知发布者；必须先核对 SHA-256，并仅从可信渠道获取文件。
- 编辑和替换会修改 Markdown 原文件。重要文档建议先提交 Git 或保留副本；保存失败时程序保留 dirty 标签和编辑内容。

## 七、回滚方案

1. 关闭所有 MD Preview 窗口。
2. 把 v1.9.3 EXE 移出当前目录，保留用于问题定位，不要覆盖文档文件。
3. 将 `MD-Preview-windows-x64-v1.9.2.bak.exe` 改回 `MD-Preview-windows-x64.exe`。
4. 执行 `.\MD-Preview-windows-x64.exe --version`，确认输出 `MD Preview 1.9.2`。
5. 启动一次 v1.9.2，使文件关联回到回滚后的 EXE；再打开一份 Markdown 验证预览和编辑。

回滚只替换程序，不修改 Markdown、图片、模板或会话数据；已经由新版保存的文本内容不会随程序回滚，如需恢复请使用 Git 或备份。

## 八、发布校验值

- 文件大小：`6,793,216` 字节（约 `6.48 MiB`）
- SHA-256：`be8c9fb3f6a1c64e731236225f379dcae86134019908912604e0a9f5c7367984`
- Authenticode：`NotSigned`
