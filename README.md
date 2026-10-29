# Wuwa Tappo CN · Linux 社区移植版

[![License: MIT](https://img.shields.io/badge/code-MIT-blue.svg)](LICENSE)
[![Official UI](https://img.shields.io/badge/official%20UI-1.0.2-c8a2ff.svg)](upstream-cn.json)

把《鸣潮》官方桌宠 **Wuwa Tappo 国服** 的完整官方桌面端移植到 Linux。界面、角色、字体与在线协议保持官方原样，原生后端使用 **Rust + Tauri 2 + WebKitGTK** 重建，不需要 Wine 或虚拟机。

> 这是社区非官方移植，不是库洛官方 Linux 客户端。登录、抽卡、联机等在线功能仍连接 Kuro 官方服务，请使用自己的国服账号。官方美术与素材版权归 **KURO GAMES / Irox Games**，不适用本仓库的 MIT 许可，详见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

## 功能

- **官方桌宠主界面**：透明、无边框、全屏覆盖层，保留官方角色、宝箱、点击计数、设置入口与多语言文案。
- **官方多窗口**：背包、图鉴、合成、联机、设置，登录窗口，关于、OSS 声明、更新/维护提示，以及 Toast。
- **全局键盘、鼠标与手柄**：基于 `evdev` 读取 `/dev/input/event*`，统计打字、点击与手柄按键；支持官方快捷键、点击穿透和按角色区域命中（联机时区分不同桌宠）。
- **原生 WebSocket 代理**：联机房间走 Rust `tokio-tungstenite`，绕开 WebKitGTK 对跨域 WebSocket 的限制，同时只允许官方国服端点。
- **多显示器与窗口几何**：工作区安全区域、任务栏 insets、显示器切换、窗口折叠、鼠标穿透命中测试。
- **本地数据**：点击计数原子写入；日志做脱敏和滚动；不内置任何账号、Cookie 或遥测上传。
- **开机自启**：复用官方设置页的“开机启动”开关（`tauri-plugin-autostart`）。

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
sha256sum -c wuwa-tappo-linux-1.0.2-1-x86_64.tar.gz.sha256  # 可选
tar -xzf wuwa-tappo-linux-1.0.2-1-x86_64.tar.gz
cd wuwa-tappo-linux-1.0.2-1-x86_64
./install.sh   # 加入 input 组、注册桌面菜单项
./run.sh       # 启动
```

之后也可以在应用菜单搜索 **Wuwa Tappo CN Linux**。

> **推荐通过 `run.sh` 启动。** 它会应用 `input` 组、单实例锁，并设置 `GDK_BACKEND=x11` 与 `WEBKIT_FORCE_COMPOSITING_MODE=1`。二进制本身也会在未显式指定时补齐这两个变量，但 `run.sh` 仍是经过验证的启动方式。

### 从源码构建

需要 Node.js 20+、Rust 1.97.1、7-Zip，以及 WebKitGTK 4.1 开发包。官方前端需要从 Windows 安装包中静态提取，后再嵌入二进制：

```bash
npm ci
npm run upstream:update          # 下载并校验官方 1.0.2 安装包，提取 frontend/
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

以下快捷键由官方前端注册（需窗口获得焦点/输入监听可用）：

| 按键 | 行为 |
| --- | --- |
| `F7` | 切换游戏模式（临时禁用与桌宠的交互） |
| `F8` | 在多显示器之间切换主窗口 |
| `F9` | 显示 / 隐藏桌宠 |

界面内还可点击右下角宝箱领取奖励、点击设置按钮打开背包 / 图鉴 / 合成 / 联机 / 设置。

当前版本没有托盘图标。关闭主窗口或任务栏关闭会直接退出应用；`F9` 隐藏桌宠后进程仍在，再按一次 `F9` 即可恢复。

## 数据与隐私

- 应用数据：`~/.local/share/dev.w33d.wuwatappo.cn.linux/`
  - `counters.json`：按键与点击计数（按账号/环境区分）。
  - `logs/app.log`：脱敏后的运行日志，超过 2 MiB 自动滚动。
  - WebKit 的 Cookie / LocalStorage 也位于该应用数据目录下。
- 日志会对 URL 查询参数、token、Cookie、Authorization 等做脱敏；不会主动上传。
- 重置全部本地状态：先退出应用，再删除上述目录。这会同时清除登录态与点击计数。
