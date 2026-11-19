# MD Preview v1.9.2 发布部署方案

更新时间：2026-10-09  
适用平台：Windows x64  
发布文件：`release/v1.9.2/MD-Preview-windows-x64.exe`

## 一、版本说明

本版本在 v1.9.1 的本地图片、Mermaid 高清导出和 Word 导出修复基础上，新增 Markdown 源码查找替换：

1. `Ctrl+H` 从预览或源码模式打开双行替换栏，并进入 Markdown 源码编辑。
2. 支持上一个/下一个命中、单项“替换”和“全部替换”；匹配默认不区分大小写，与现有 `Cmd/Ctrl+F` 预览搜索一致。
3. 查找内容按字面量匹配，替换内容也按字面量写入；`$&`、`$1`、反斜杠、中文和 Markdown 标记不会被当成正则替换语法。
4. 替换复用现有 dirty 状态、实时字数统计、原生撤销/重做和 700 ms 自动保存；手工修改源码后会重新计算命中位置，避免旧索引改错内容。
5. `Esc` 只关闭替换栏并保留源码编辑；`Ctrl+E` 返回预览时会同步关闭替换状态。

## 二、发布前校验

在项目根目录执行：

```powershell
cargo fmt -- --check
cargo test
cargo build --release
.\target\release\md-preview.exe --help
.\target\release\md-preview.exe --version
```

浏览器专项验证：

```powershell
$env:NODE_PATH="C:\Users\luosh\.cache\codex-runtimes\codex-primary-runtime\dependencies\node\node_modules"
$env:MD_PREVIEW_TEST_BROWSER="C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
& "C:\Users\luosh\.cache\codex-runtimes\codex-primary-runtime\dependencies\node\bin\node.exe" scripts\verify-desktop-search.mjs
& "C:\Users\luosh\.cache\codex-runtimes\codex-primary-runtime\dependencies\node\bin\node.exe" scripts\verify-desktop-reading-tools.mjs
```

预期结果：

- `cargo fmt -- --check` 无输出并返回 0；
- Rust 测试 61/61 通过；
- 搜索替换与阅读工具脚本均输出 `OK`；
- `--help` 包含 `--version` 和 `--edit`；
- `--version` 输出 `MD Preview 1.9.2`，且不打开图形界面。

## 三、傻瓜式部署步骤

1. 下载同一目录中的以下两个文件：
   - `MD-Preview-windows-x64.exe`
   - `MD-Preview-windows-x64.exe.sha256`
2. 在下载目录打开 PowerShell，执行：

   ```powershell
   Get-FileHash .\MD-Preview-windows-x64.exe -Algorithm SHA256
   ```

3. 确认输出哈希与 `.sha256` 文件完全一致。
4. 关闭所有正在运行的 MD Preview 窗口。
5. 在旧程序目录中，把旧 EXE 重命名为 `MD-Preview-windows-x64-v1.9.1.bak.exe`。
6. 把新的 `MD-Preview-windows-x64.exe` 复制到原程序目录。
7. 执行版本检查：

   ```powershell
   .\MD-Preview-windows-x64.exe --version
   ```

8. 双击程序打开任意 Markdown 文件，按 `Ctrl+H` 做一次单项替换和一次全部替换；等待 1 秒，关闭并重新打开文件，确认内容已保存。
9. 打开业务样例：

   ```powershell
   .\MD-Preview-windows-x64.exe "C:\DevOps\012\机顶盒统一主页与后台各系统交互设计方案.md"
   ```

10. 确认 6 张本地图片可见，并抽查 Mermaid 的 SVG 矢量下载、3× PNG 下载和 Word 导出。

程序无需安装服务、数据库或额外配置；发布包仍为单 EXE。配置、日志和 WebView2 缓存继续使用程序既有目录策略，本次无数据迁移。

## 四、验收清单

- [ ] `--version` 显示 `MD Preview 1.9.2`。
- [ ] `Cmd/Ctrl+F` 仍能在预览中查找、前后导航和关闭高亮。
- [ ] `Ctrl+H` 打开替换栏并进入源码模式。
- [ ] 单项替换只修改当前命中，计数正确并自动定位下一处。
- [ ] 全部替换数量正确；中文、`$&`、`$1` 和反斜杠按原文写入。
- [ ] 空查找词和 0 命中时按钮禁用，不修改文件。
- [ ] 替换后 `Ctrl+Z`/`Ctrl+Y` 可撤销/重做。
- [ ] 替换栏打开时手工编辑源码，后续替换位置仍正确。
- [ ] `Esc` 关闭替换栏但保留编辑；`Ctrl+E` 返回预览且不残留替换栏。
- [ ] 等待 1 秒后重新打开文件，替换内容已经保存。
- [ ] 390 px 窄窗口中替换栏不超出窗口；长文档末尾命中会滚动到可见位置。
- [ ] 样例 6 张本地图片、Mermaid SVG/PNG 和 Word 导出无回归。

## 五、风险与注意事项

- 当前提供字面量、不区分大小写的查找替换，不支持正则表达式、大小写开关或整词匹配；这是本次有意控制的功能边界。
- 本次 Windows EXE 的 Authenticode 状态为 `NotSigned`。从网络下载时 Windows SmartScreen 可能提示未知发布者；部署前必须先校验 SHA-256，并仅从可信渠道取得文件。
- `Ctrl+H` 是本次明确支持的替换快捷键；macOS 的 `Cmd+H` 是系统“隐藏应用”习惯快捷键，本版本不占用也不承诺 `Cmd+H`。
- 替换会修改 Markdown 原文件。批量替换前建议保留 Git 提交或副本；保存失败时程序沿用既有 dirty 保护，不会主动清空编辑内容。
- 本地图片协议仍只读当前活动文档已登记的资源；切换标签或刷新后旧资源地址失效属于安全设计。

## 六、回滚方案

1. 关闭 MD Preview。
2. 删除或移走 v1.9.2 EXE。
3. 将 `MD-Preview-windows-x64-v1.9.1.bak.exe` 改回 `MD-Preview-windows-x64.exe`。
4. 执行 `--version`，确认输出 `MD Preview 1.9.1`。
5. 打开一份 Markdown，确认预览和编辑正常。

回滚只替换程序文件，不修改 Markdown、图片、模板、配置或数据库；无需数据迁移。已经由 v1.9.2 保存的文本替换不会被程序回滚，如需恢复文档内容，请使用 Git、备份或编辑器撤销历史。

## 七、发布校验值

- 文件大小：`6,773,248` 字节
- SHA-256：`2ad783e4a94965ef1d2dc06dca05408364df68ba74774a6a922e0d89aa7e7170`
- Authenticode：`NotSigned`
