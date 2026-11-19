# MD Preview v1.9.1 发布部署方案

更新时间：2026-10-07  
适用平台：Windows x64  
发布文件：`release/v1.9.1/MD-Preview-windows-x64.exe`

## 一、版本说明

本版本修复两个问题：

1. Mermaid 流程图导出清晰度
   - “下载 SVG（矢量原图）”导出保留 `viewBox` 和矢量节点，不受灯箱当前缩放比例影响。
   - “下载 PNG（高清 3×）”先把 SVG 源设置为目标像素尺寸，再按 3× 一比一绘制，避免低分辨率源被 Canvas 二次放大。
   - Word 导出采用 300 PPI PNG 兼容回退，并把 SVG 逻辑尺寸与 PNG 采样像素分离，放置尺寸不会随采样倍率变大。
   - Word/WPS/LibreOffice 对 Mermaid `foreignObject`、HTML 标签和字体的 SVG 支持不一致，因此 v1.9.1 不直接把 SVG 写入 DOCX；需要真正矢量原图时请下载 SVG。

2. Markdown 本地图片加载
   - 不再把本地图片转换为 Base64 后塞入整页 JavaScript。
   - 相对图片通过受控只读 `mdpreview-image://` 资源地址按需读取。
   - 只允许读取当前文档目录内、已登记且已规范化的图片；绝对路径、`..`、编码穿越、目录外符号链接和未登记地址均拒绝。
   - 图片响应使用 `Cache-Control: no-store`，切换文档或重新渲染后旧资源地址立即失效。

## 二、发布前校验

在项目根目录执行：

```powershell
cargo test
cargo build --release
.\target\release\md-preview.exe --help
.\target\release\md-preview.exe --version
```

预期结果：

- Rust 测试全部通过；
- Release 构建成功；
- `--help` 退出码为 `0`，帮助中包含 `--version` 和 `--edit`；
- `--version` 输出 `MD Preview 1.9.1`，且不打开图形界面。

本次发布的专项验证结果：

- 指定样例 `C:\DevOps\012\机顶盒统一主页与后台各系统交互设计方案.md` 的 6/6 张本地图片全部登记并读取成功；图片总计 5,964,236 字节，预览 HTML 为 61,211 字节且不含本地图片 Base64。
- 同一样例中的 10/10 个 Mermaid 代码块均实际渲染为 300 PPI Word 回退图。
- 浏览器像素校验：1000×500 SVG 的 Word PNG 为 3125×1563；灯箱 3× PNG 为 3000×1500；SVG 下载仍为 1000×500 `viewBox` 矢量结构。

## 三、傻瓜式部署步骤

1. 下载 `MD-Preview-windows-x64.exe`。
2. 对照同目录 `.sha256` 文件校验：

   ```powershell
   Get-FileHash .\MD-Preview-windows-x64.exe -Algorithm SHA256
   ```

3. 关闭正在运行的旧版 MD Preview。
4. 把新 EXE 复制到原程序目录；建议先把旧 EXE 重命名为 `MD-Preview-windows-x64-v1.9.0.bak.exe`。
5. 双击新 EXE，或用命令直接打开文档：

   ```powershell
   .\MD-Preview-windows-x64.exe "C:\DevOps\012\机顶盒统一主页与后台各系统交互设计方案.md"
   ```

6. 检查 6 张业务截图是否全部显示；点击任意 Mermaid 图，在灯箱中分别验证“下载 SVG（矢量原图）”和“下载 PNG（高清 3×）”。
7. 导出 Word，放大到 200% 和 400% 检查文字、连线、箭头和中文标签。

程序无需安装服务、数据库或额外配置；WebView2 缓存和日志继续写入用户配置目录，发布包本身仍是单 EXE。

## 四、验收清单

- [ ] `--version` 显示 `1.9.1`。
- [ ] 样例 6 张本地图片全部可见。
- [ ] 图片路径含中文、空格和百分号编码时可加载。
- [ ] `../`、`%2e%2e`、盘符、UNC 和目录外链接不能读取。
- [ ] SVG 下载文件包含 `<svg>`、`viewBox` 和矢量节点，不含内嵌 PNG 替代图。
- [ ] PNG 下载像素尺寸为 SVG 逻辑尺寸的 3×。
- [ ] Word 流程图有效采样不低于 300 PPI，且图片版面宽度不随采样倍率变化。
- [ ] 10 个流程图无遗漏、无裁剪、顺序正确。
- [ ] 搜索、锚点导航、正文缩放和编辑/预览切换无回归。

## 五、风险与注意事项

- Word 导出仍使用 PNG 兼容回退，不等同于 SVG 矢量原图；这是为兼容 Word/WPS/LibreOffice 的稳定策略。
- 极端流程图限制为单边最多 8192 像素、总计最多 3200 万像素；超过限制会等比降采样，防止内存失控。
- 本地图片协议只服务当前活动文档已登记的图片。切换标签或刷新后旧地址失效属于安全设计。
- 发布前不得覆盖用户的 Markdown 和图片目录；程序只读图片，编辑 Markdown 时仍沿用原有保存逻辑。

## 六、回滚方案

1. 关闭 MD Preview。
2. 删除或移走 v1.9.1 EXE。
3. 将 `MD-Preview-windows-x64-v1.9.0.bak.exe` 改回原文件名。
4. 执行 `--version` 确认已回到 `1.9.0`。

回滚只替换程序文件，不修改 Markdown、图片、模板和数据库；无需数据迁移。

## 七、发布校验值

`MD-Preview-windows-x64.exe` SHA-256：`771e1ef07f6f58343dadf1d50c03c48b085fc8eb638bec4cc98459b86a3d886c`
