# AGENTS.md

本文件只记录长期有效的开发约束，不记录版本历史、已完成需求或临时计划。

## 行为基准

- 以 `upstream-cn.json` 锁定的官方前端及其原生行为为基准。新增或修改命令、事件与 payload 前，先对照官方前端契约和官方包行为，不要自行发明协议或交互。
- Linux 无法等价实现的能力应明确降级，并在 `docs/troubleshooting.md` 记录限制，不要静默偏离官方行为。
- 官方前端保持静态嵌入二进制；不要改成远程加载，也不要引入 Wine、虚拟机或重写业务前端。

## 官方资源与网络

- 官方前端、Windows 安装包、账号、Cookie、点击记录不得进入 git；`frontend/`、`build/`、`bin/`、`dist/` 仅是本地构建产物。
- 只从 `upstream-cn.json` 的锁定地址获取安装包，校验大小与 SHA-256；不执行 Windows 安装包，只做静态 PE/资源提取。
- 前端补丁必须使用精确锚点并校验命中数量；锚点不匹配时让构建失败，禁止静默跳过或放宽匹配。
- 原生 WebSocket 只允许官方国服 endpoint 与固定路径；不转发 Cookie、Origin 或任意请求头，不把代理扩展成通用网络出口。
- 日志落盘前脱敏；不增加遥测、后台上传或额外网络出口。

## 平台约束

- 目标是 x86_64 Linux、X11/XWayland、WebKitGTK 4.1 与 GTK 3；全局键鼠与手柄监听依赖 `input` 组。
- 保持单实例锁与 close-to-tray 语义：托盘可用时关闭主窗口只隐藏，退出必须走会保存计数、断开 Socket 的路径；托盘不可用时关闭窗口直接退出，避免留下无法唤出的后台进程。
- 修改窗口、指针或命中区域时，保留物理像素与 CSS 像素之间的 XShape 边界转换，并覆盖分数缩放。
- 不削弱日志脱敏、存储目录权限、WebSocket 来源校验等安全边界。

## 开发流程

- `dev` 是集成分支。功能、修复、文档、发布准备等所有改动都从最新 `dev` 拉短生命周期分支，命名 `<type>/<topic>`（`feat/`、`fix/`、`docs/`、`chore/`、`release/`）。
- 分支完成并通过“验证”小节后，用 `git merge --no-ff <branch>` 合入 `dev`，禁止 fast-forward 合入。
- 保留 `dev` 与 `main` 的合并提交；发布 tag 打在 `main` 的发布提交上，已推送的 tag 不移动、不重打。

## 验证

提交前在仓库根目录运行：

```bash
npm test
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --locked --all-targets --manifest-path src-tauri/Cargo.toml
cargo test --locked --manifest-path src-tauri/Cargo.toml
```

不要通过删除测试、放宽断言或跳过契约检查来让检查变绿。没有官方资源时可以用 `npm run scaffold` 做编译检查，但不能据此发布。

## 发布与 Release Notes

发布流程：

1. 确认 `dev` 已通过发布前 review 与“验证”小节的全部检查。
2. 从 `dev` 拉 `release/v<version>`，在同一个提交里提升版本号：`package.json`、`package-lock.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`、`src-tauri/tauri.conf.json`。
3. 按 `docs/building.md` 构建 release 二进制并运行 `npm run package`，用生成的 `.sha256` 在本地核对 `dist/wuwa-tappo-linux-<version>-x86_64.tar.gz`。
4. 按下面的格式编写 `dist/RELEASE_NOTES-v<version>.md`；`dist/` 是本地构建产物，不进入 git。
5. `release/v<version>` 以 `--no-ff` 合入 `dev`，`dev` 以 `--no-ff` 合入 `main`；在 `main` 的发布提交上创建 annotated tag `v<version>`，推送 `main` 与 tag。
6. 使用 `gh release create v<version> <tarball> --title "v<version>" --notes-file dist/RELEASE_NOTES-v<version>.md` 创建 GitHub Release；只上传 tar.gz，不上传 `.sha256` 资产；预发布加 `--prerelease`。
7. 发布完成后把 `dev` 快进到 `main` 并推送。

Release Notes 文本结构规定如下：

- 正文不加 H1，版本号由 GitHub Release 标题（`v<version>`）承载。
- 分类固定为：新增功能、体验优化、问题修复、其他变更；没有内容的分类省略。
- 分类标题使用 `###`；条目按影响排序，每条一句话，外部贡献者以 ` @handle` 标注。
- tag、GitHub Release 标题（`v<version>`）与 `dist/RELEASE_NOTES-v<version>.md` 文件名保持一致。

## 文档

- `README.md` 和 `docs/` 只描述当前行为、依赖、限制与维护流程；不写版本修复史、已完成任务清单、路线图或临时状态。
- 行为、依赖或平台限制变化时同步更新文档；故障排查只保留仍可复现的问题。
