# Wuwa Tappo CN · Linux 社区移植版

[![License: MIT](https://img.shields.io/badge/code-MIT-blue.svg)](LICENSE)
[![Official UI](https://img.shields.io/badge/official%20UI-1.0.3-c8a2ff.svg)](upstream-cn.json)

把《鸣潮》官方桌宠 **Wuwa Tappo 国服** 的完整官方桌面端移植到 Linux。界面、角色、字体与在线协议保持官方原样，原生后端使用 **Rust + Tauri 2 + WebKitGTK** 重建，不需要 Wine 或虚拟机。

## 功能

- **官方桌宠主界面**：透明、无边框、全屏覆盖层，保留官方角色、宝箱、点击计数、设置入口与多语言文案。
- **全局键盘、鼠标与手柄**：统计打字、点击与手柄按键；快捷键由后端全局匹配（支持自定义），并支持点击穿透和按角色区域命中。
- **联机房间**：支持多人联机房间。

## 系统要求

- **x86_64 Linux**，X11 或 XWayland。
- **WebKitGTK 4.1**、GTK 3、`libsoup-3.0`。
- 预编译发布包在 Ubuntu 24.04 环境构建，要求 **glibc ≥ 2.39**（Ubuntu 24.04+、Fedora 40+、Arch 等）。更旧的发行版请从源码构建。
- 读取全局键盘、鼠标和手柄需要当前用户属于 **`input` 组**。

各发行版安装运行依赖：

```bash
# Arch Linux
sudo pacman -S webkit2gtk-4.1 gtk3 libappindicator-gtk3

# Ubuntu 24.04 / Debian 13
sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0 libayatana-appindicator3-1

# Fedora
sudo dnf install webkit2gtk4.1 gtk3 libappindicator-gtk3
```

## 快速开始

### 使用发布包

从 [Releases](https://github.com/observer130/wuwa-tappo-linux/releases) 下载 `wuwa-tappo-linux-<version>-x86_64.tar.gz`，校验后解压：

```bash
sha256sum -c wuwa-tappo-linux-x.y.z-1-x86_64.tar.gz.sha256  # 可选
tar -xzf wuwa-tappo-linux-x.y.z-1-x86_64.tar.gz
cd wuwa-tappo-linux-x.y.z-1-x86_64
./install.sh   # 加入 input 组、注册桌面菜单项
./run.sh       # 启动
```

之后也可以在应用菜单搜索 **Wuwa Tappo CN Linux**。

> **推荐通过 `run.sh` 启动。** 它会应用 `input` 组、启动器互斥锁，并设置 `GDK_BACKEND=x11` 与 `WEBKIT_FORCE_COMPOSITING_MODE=1`。二进制本身也会在未显式指定时补齐这两个变量，并持有进程级实例锁，因此直接运行二进制也不会出现双实例；但 `run.sh` 仍是经过验证的启动方式。

### 从源码构建

需要 Node.js 20+、Rust 1.97.1、7-Zip，以及 WebKitGTK 4.1 开发包。官方前端需要从 Windows 安装包中静态提取，后再嵌入二进制：

```bash
npm ci
npm run upstream:update
npm run icons
npm run prepare:frontend
cargo build --release --manifest-path src-tauri/Cargo.toml
mkdir -p bin && cp src-tauri/target/release/wuwatappo-linux bin/
npm run package
```

完整步骤、手动提取、deb 打包和中国大陆镜像配置见 [docs/building.md](docs/building.md)。

### 登录

1. 打开设置入口，选择“通行证登录”或“游戏 UID 登录”。
2. 使用自己的国服账号完成登录。登录态由 WebKitGTK 的 Cookie/LocalStorage 保存，不会上传到本仓库。
3. 未登录时部分面板会停留在加载态，登录后由官方前端恢复。

## 快捷键

| 默认按键 | 行为 |
| --- | --- |
| `F7` | 切换游戏模式（临时禁用与桌宠的交互） |
| `F8` | 在多显示器之间切换主窗口 |
| `F9` | 显示 / 隐藏桌宠 |

当前版本没有托盘图标。关闭主窗口或任务栏关闭会直接退出应用；`F9` 隐藏桌宠后进程仍在，再按一次 `F9` 即可恢复。

## 数据与隐私

- 应用数据：`~/.local/share/dev.w33d.wuwatappo.cn.linux/`
  - `counters.json`：按键与点击计数（按账号/环境区分）。
  - `shortcuts.json`：设置页保存的快捷键绑定。
  - `logs/app.log`：脱敏后的运行日志，超过 2 MiB 自动滚动。
  - WebKit 的 Cookie / LocalStorage 也位于该应用数据目录下。
- 日志会对 URL 查询参数、token、Cookie、Authorization 等做脱敏；不会主动上传。
- 重置全部本地状态：先退出应用，再删除上述目录。这会同时清除登录态与点击计数。

## 第三方内容与许可

本仓库的**原创代码**采用 MIT 许可（见 [LICENSE](LICENSE)）。下列第三方内容不因随本仓库或发布包分发而改变其原有许可。

### Wuwa Tappo 国服官方资源

本项目使用《鸣潮》桌宠 Wuwa Tappo 国服客户端的界面、角色、美术、音频、字体及其他内嵌资源，并通过官方服务完成登录、背包、联机等业务请求。

- 相关名称、商标、美术与素材版权归 **KURO GAMES** 与 **Irox Games** 及各自权利人所有。
- 官方资源**不适用**本仓库的 MIT 许可；本项目仅用于个人桌面美化，不得用于商业再分发。
- 官方下载入口：<https://download-mc-wuwatappo.kurogames-global.com/download/>
- 构建所用版本、下载地址、大小、SHA-256 与 Tauri 签名：[`upstream-cn.json`](upstream-cn.json)。

源码仓库不存放官方前端资源、Windows 安装包、账号、Cookie 或点击记录。构建与发布流程在本地/CI 中从官方地址下载并校验安装包，再静态提取资源嵌入二进制。使用本项目仍需遵守官方服务条款；本项目与官方发行渠道无关。

### 构建依赖

- **Tauri 2**、Rust crates：各自采用 MIT / Apache-2.0 等许可，版本锁定在 [`src-tauri/Cargo.lock`](src-tauri/Cargo.lock)。
- **Node.js 包**（含 `@tauri-apps/cli`、`acorn`）：版本锁定在 [`package-lock.json`](package-lock.json)。
- **7-Zip**：仅构建期用于静态解包官方安装包，不随应用分发。
- **WebKitGTK / GTK / libsoup**：系统运行库，由发行版提供，遵循各自 LGPL 等许可。

### 字体与图像

官方前端内嵌的思源黑体、GenSen Rounded、NanumGothic、Prompt 等字体，以及角色、宝箱、表情、徽章等图像，均来自官方安装包，版权归原权利人所有，不适用 MIT 许可。
