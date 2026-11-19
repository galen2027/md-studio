# MD Preview v1.9.8 编辑器第三方声明

本版本新增固定版本的离线编辑器资源。下面的许可正文随发布包放在 `licenses/`，分发时请保留。

| 组件 | 来源 | 许可正文 |
| --- | --- | --- |
| Vditor 4.0.1 | https://github.com/Vanessa219/vditor ，官方 npm `vditor@4.0.1` | licenses/Vditor-MIT.txt |
| Lute（上述 Vditor 包所带的 JS 引擎） | https://github.com/88250/lute | licenses/Lute-MulanPSL-2.0.txt |
| KaTeX / mhchem 相关资源 | https://github.com/KaTeX/KaTeX | licenses/KaTeX-MIT.txt |
| MD Preview | https://github.com/vorojar/md-preview | licenses/MD-Preview-MIT.txt |

未修改上游压缩资源。MD Preview 的编辑适配代码独立维护在 `assets/enhance/preview-edit.js`。所选 Vditor 文件的版本、来源、包 SHA-512 及每个文件的 SHA-256 记录在源码 `assets/editor/manifest.json`；构建门禁 `scripts/verify-editor-assets.mjs` 校验其完整性。

Mermaid、KaTeX 数学渲染、highlight.js 等原有组件继续使用项目已内嵌的版本。本声明补充新增编辑器资源的出处，不替代各上游许可正文。
