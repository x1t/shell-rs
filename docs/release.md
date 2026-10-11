# 发布

[返回 README](../README.zh-CN.md)

在 `CHANGELOG.md` 里写好 `## [x.y.z]` 一节，把 `Cargo.toml` 的版本改成 `x.y.z`，提交后推送 tag `vx.y.z`。`.github/workflows/release.yml` 依次：

1. 核对 tag 与 `Cargo.toml` 一致，截出更新说明，确认代码里有发布公钥；
2. 构建并打包：这个 fork 只发布 Windows，用 Inno Setup 打安装程序（`ShellRS-<版本>-windows-x86_64-setup.exe`，既是首次安装用的，也是自动更新下载的）。macOS 和 Linux 的打包脚本还在 `packaging/` 里，工作流里没有它们的任务；
3. 在 `release` 环境里人工批准后：给每个包签名，建 GitHub Release `v<版本>` 并上传安装包，**最后**才替换更新清单：清单是 `update-manifest` 这个 Release 的附件 `<通道>.json`（第一次发布时自动创建，标成预发布、不算 Latest，不要删）。客户端从 `https://github.com/x1t/shell-rs/releases/download/update-manifest/<通道>.json` 读清单，清单里每个包的下载地址是对应版本 Release 的附件（仓库是公开的，下载不用登录）。

版本号带 `-` 的（如 `0.3.0-beta.1`）只发到 beta 通道；正式版同时成为 beta 通道的最新版。在 Actions 里手动运行这个 workflow 只构建打包、不发布，用来演练。客户端只接受比自己新的版本，所以发错的版本撤不回来：把清单改回上一版能阻止更多人升级，修复要发新的补丁版。

一次性准备：

- **更新签名密钥**：`minisign -G -p shellrs-update.pub -s shellrs-update.key` 生成两对（一对日常用，一对离线保存备用），把两个公钥（`.pub` 文件的第二行）填进 `src/update/build_info.rs` 的 `TRUSTED_KEYS`。私钥和口令只放进 GitHub 的 `release` 环境，另外离线备份；丢了私钥，已发出去的 ShellRS 就再也收不到更新。换钥匙时先发一版带上新公钥，再换签名用的私钥。
- **图标**：`assets/logo/shellrs.ico`（Windows 安装程序）。
- **GitHub secrets**：`release` 环境（设为需要审批）里的 `MINISIGN_SECRET_KEY`（私钥文件的全部内容）和 `MINISIGN_PASSWORD`。发布用的 `GITHUB_TOKEN` 由 Actions 自带，不用另配。

清单格式见 `packaging/manifest.example.json`，由 `packaging/make-manifest.sh` 生成：一个文件里是清单原文和它的 minisign 签名，签名的 trusted comment 写明通道和版本。格式只增加字段、不改已有的；已发出去的 ShellRS 一直按 `update-manifest` 这个 Release 的地址检查，所以这个 Release 的名字和仓库地址都不能再改。
