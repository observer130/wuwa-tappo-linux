# 构建指南

## 依赖

| 组件 | 版本 / 说明 |
| --- | --- |
| Rust | `rust-toolchain.toml` 锁定 1.97.1 |
| Node.js | 20+（CI 使用 24） |
| 7-Zip | 仅构建期解包官方安装包，推荐 `7zz` 或 `7z` |
| WebKitGTK | 4.1（开发包） |
| GTK | 3.x 开发包 |

发行版安装示例：

```bash
# Arch Linux
sudo pacman -S --needed base-devel rust nodejs npm 7zip \
  webkit2gtk-4.1 gtk3 libappindicator-gtk3

# Ubuntu 24.04 / Debian 13
sudo apt install build-essential pkg-config libssl-dev nodejs npm 7zip \
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf

# Fedora
sudo dnf install gcc gcc-c++ make pkgconf-pkg-config openssl-devel nodejs npm 7zip \
  webkit2gtk4.1-devel gtk3-devel libappindicator-gtk3-devel librsvg2-devel
```

## 获取官方前端

官方前端来自 Windows 安装包，**不进入 git**。推荐让脚本读取 `upstream-cn.json`，下载、校验 SHA-256 并静态提取：

```bash
npm ci
npm run upstream:update      # 等价于 node scripts/sync-upstream.mjs --update-lock
```

只想使用已锁定版本（不更新 `upstream-cn.json`）：

```bash
node scripts/sync-upstream.mjs
```

手动提取（已有安装包时）：

```bash
node scripts/extract-assets.mjs \
  --installer /path/to/WuwaTappo-cn_1.0.2_x64-setup.exe \
  --sevenzip "$(command -v 7z)" \
  --output frontend
```

脚本不会执行安装包，只解析 PE 中 Tauri 嵌入的 Brotli 资源表，并校验静态引用完整性。

## 准备并构建

```bash
npm run icons              # 从 frontend/app-icon.png 生成 Tauri 图标
npm run prepare:frontend   # 复制到 build/frontend 并打 Linux 补丁
cargo build --release --locked --manifest-path src-tauri/Cargo.toml
mkdir -p bin
cp src-tauri/target/release/wuwatappo-linux bin/
npm run package            # 生成 dist/wuwa-tappo-linux-<version>-x86_64.tar.gz
```

也可以生成 deb（需要 `dpkg` 工具链）：

```bash
npm run build              # tauri build --bundles deb
```

deb 适合发行版打包测试；它不会把用户加入 `input` 组，安装后需手动执行 `sudo usermod -aG input "$USER"` 并重新登录。桌面菜单直接启动时，二进制会自行设置透明窗口所需环境变量。

### 没有官方资源时只做编译检查

```bash
npm ci
npm run scaffold           # 写入占位页和占位图标（不覆盖已有文件）
npm test
npm run test:rust
```

`scaffold` 只用于开发和 CI 编译，打出的包无法显示官方界面。

## 前端补丁

补丁集中在 `scripts/frontend-patches.mjs`：

| 补丁 | 目的 |
| --- | --- |
| `patchDomesticSdkUrls` | 把官方协议相对 SDK 地址改为 HTTPS，适配 `tauri://localhost` |
| `patchLoginReady` | 登录窗口监听器挂载后回报 `login_window_ready`，消除首帧竞态 |
| `patchAbout` | 关于页标注“Linux 社区移植版”，并把原 Windows 配置改为二级标题 |
| `patchIndexLanguage` | `<html lang>` 设为 `zh-CN` |

每个函数都会校验锚点数量，锚点不匹配时抛错。官方升级后先运行 `npm test` 与 `npm run prepare:frontend`，按报错更新锚点，不要静默跳过。

## 更新官方版本

```bash
npm run upstream:check     # 只检查新版本，不下载
npm run upstream:update    # 下载、校验、更新 upstream-cn.json、替换 frontend/
node scripts/inventory-contract.mjs frontend/assets tests/contracts.json
npm test                   # 若出现未实现命令，backend-coverage 会失败
```

1. 根据契约差异补 Rust 命令。
2. 检查 `prepare-frontend` 是否仍能命中全部锚点。
3. 在本地验证登录、面板、输入计数和联机。
4. 更新 `CHANGELOG.md` 与版本号（`package.json`、`src-tauri/Cargo.toml`、`tauri.conf.json`）。

`upstream-cn.json` 由 `sync-upstream` 维护，包含官方下载地址、大小、SHA-256 和 Tauri 签名，请勿手工编造。

## 中国大陆网络

仓库默认使用 crates.io。若访问缓慢，可本地创建 `src-tauri/.cargo/config.toml`（该文件已被 git 忽略）：

```toml
[source.crates-io]
replace-with = "rsproxy-sparse"

[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"

[net]
retry = 5
```

npm 可设置 `npm_config_registry=https://registry.npmmirror.com`，或使用 `npm ci --cache /tmp/npm-cache`。

## 发布

推送 `v*` 标签会触发 `.github/workflows/release.yml`：Ubuntu 24.04 上安装依赖、下载并校验官方安装包、提取前端、构建、跑测试、打包 tar.gz 并发布到 GitHub Releases。发布包不包含账号数据或构建机缓存。
