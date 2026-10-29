# 第三方内容与许可说明

本仓库的**原创代码**采用 MIT 许可（见 [LICENSE](LICENSE)）。下列第三方内容不因随本仓库或发布包分发而改变其原有许可。

## Wuwa Tappo 国服官方资源

本项目使用《鸣潮》桌宠 Wuwa Tappo 国服客户端（当前锁定 **1.0.2**）的界面、角色、美术、音频、字体及其他内嵌资源，并通过官方服务完成登录、背包、联机等业务请求。

- 相关名称、商标、美术与素材版权归 **KURO GAMES** 与 **Irox Games** 及各自权利人所有。
- 官方资源**不适用**本仓库的 MIT 许可；本项目仅用于个人桌面美化，不得用于商业再分发。
- 官方下载入口：<https://download-mc-wuwatappo.kurogames-global.com/download/>
- 构建所用版本、下载地址、大小、SHA-256 与 Tauri 签名：[`upstream-cn.json`](upstream-cn.json)。

源码仓库不存放官方前端资源、Windows 安装包、账号、Cookie 或点击记录。构建与发布流程在本地/CI 中从官方地址下载并校验安装包，再静态提取资源嵌入二进制。使用本项目仍需遵守官方服务条款；本项目与官方发行渠道无关。

## 构建依赖

- **Tauri 2**、Rust crates：各自采用 MIT / Apache-2.0 等许可，版本锁定在 [`src-tauri/Cargo.lock`](src-tauri/Cargo.lock)。
- **Node.js 包**（含 `@tauri-apps/cli`、`acorn`）：版本锁定在 [`package-lock.json`](package-lock.json)。
- **7-Zip**：仅构建期用于静态解包官方安装包，不随应用分发。
- **WebKitGTK / GTK / libsoup**：系统运行库，由发行版提供，遵循各自 LGPL 等许可。

## 字体与图像

官方前端内嵌的思源黑体、GenSen Rounded、NanumGothic、Prompt 等字体，以及角色、宝箱、表情、徽章等图像，均来自官方安装包，版权归原权利人所有，不适用 MIT 许可。
