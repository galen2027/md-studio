# Markdown Studio 发布与版本归档

当前公开版本：**v1.9.10，2026-11-19**。正式仓库为 [galen2027/md-studio](https://github.com/galen2027/md-studio)，Windows x64 发布文件放在 [GitHub Releases](https://github.com/galen2027/md-studio/releases)。

## 下载与分发

- [直接下载 Windows EXE](https://github.com/galen2027/md-studio/releases/latest/download/MD-Preview-windows-x64.exe)：直接运行；文件名沿用旧版，程序显示名称为 Markdown Studio。
- [完整便携包](https://github.com/galen2027/md-studio/releases/download/v1.9.10/MD-Studio-v1.9.10-windows-x64.zip)：包含 EXE、校验文件、部署说明和许可证。
- [部署、验证与回滚](<../docs/MD Studio 发布部署方案_v1.9.10_261119.md>)：含实际大小、SHA-256、签名状态和依赖说明。

源码仓库不存储 EXE/ZIP。克隆源码不会下载发布程序；每个发布物使用同名 `.sha256` 校验，不承诺不同环境重建得到相同字节。

Windows 需要 WebView2 Runtime；无需另外安装 VC++ Runtime、Node、Python 或 Rust。本版未进行 Authenticode 签名。其他平台源码与历史维护脚本保留，但不提供本版本安装包或原生验收承诺。

## 本地构建与打包

沿用 Cargo.lock 和已有工具链，在项目根目录执行：

```powershell
cargo fmt --check
cargo test --locked
cargo build --release --locked
& .\scripts\package-windows-release.ps1
```

再执行 `bash scripts/verify.sh` 和原生测试，具体见 [测试索引](../tests/README.md)。打包脚本从 Cargo.toml 读取版本，核验 EXE 的 `--version`、PE 依赖，输出 `release/v版本号/`。发布前将生成的部署文档实际校验值同步回 docs 的同名文档。

| 独立公开资产 | 内容 |
| --- | --- |
| `MD-Preview-windows-x64.exe` / `.sha256` | 主程序与校验值 |
| `MD-Studio-v版本号-windows-x64.zip` / `.sha256` | 完整便携包与校验值 |
| `MD-Studio-Deployment-v版本号-zh.md` | 中文部署文档下载别名 |
| `MD-Studio-v版本号-licenses.zip` / `.sha256` | 第三方声明、许可正文与校验值 |

本地同版本重建需明确传 `-Force`；这不授权覆盖公开资产。

## 草稿与正式发布

1. 同步版本、更新说明、帮助/界面和部署文档；保留原 MIT 版权和第三方许可。
2. 完成测试和打包，检查文档、截图、源码及 Git 历史是否含凭据或私人内容。
3. 提交 master，创建不可移动的版本标签并推送。`scripts/release.sh 版本号` 要求 GitHub CLI 已登录。
4. Actions 的 **Windows Release Draft** 仅通过手动 workflow_dispatch 构建已有标签并上传草稿。也可直接上传本地验收过的发布物至草稿；两种方式只选一种，避免重复资产。
5. 核对标签、七个资产、校验值、签名和部署文档后正式发布。已公开版本及资产不可覆盖，需要改动时递增版本。
6. 匿名访问仓库及 Release，从 README 的 EXE 链接重新下载，核对大小与 SHA-256。

自动更新使用本仓库的 Release API 和资产地址；旧版 v1.9.9 及更早版本仍可能检查上游，需手动升级一次。EXE 兼容文件名、配置目录和 CLI 参数保留。

`release-sign.sh` 是历史 macOS 签名工具，不属于 Windows 发布流程；不自动执行，也不承诺存在签名服务或 Apple 账户。

## 历史基线

v1.9.9 将 v1.5.0 之后累计改动统一入库，未补造中间版本提交。源码标签保留；原本地成品继续存放于 release/v1.9.9，未作为本次公开版本上传。

| v1.9.9 原发布物 | SHA-256 |
| --- | --- |
| MD-Preview-windows-x64.exe | 4C20AE71ED9D074A226AEC91E17A8ED7A5388F8A946762E3ED20D78B1B0019AA |
| MD-Preview-v1.9.9-windows-x64.zip | AE20836C8A11EB17F1E53D3B58F37ACD0CF01AA5CACEC534365B92851B1AC4A3 |

详细历史见 [CHANGELOG.md](../CHANGELOG.md)，旧版恢复见 [v1.9.9 部署说明](<v1.9.9/MD Preview 发布部署方案_v1.9.9_261119.md>)。
