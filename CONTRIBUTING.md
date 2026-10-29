# 贡献指南

感谢参与 Wuwa Tappo Linux 社区移植。这个项目同时包含**官方静态前端**与**重建的 Linux 后端**，改动时需要同时考虑两端。

## 可以贡献什么

- 发行版运行依赖、打包（deb/Flatpak/AUR）与桌面集成。
- Wayland、混合 DPI、多显示器、输入法的兼容性修复。
- Rust 后端的健壮性、测试与日志质量。
- 文档、故障排查与翻译。
- 新官方版本的适配：更新契约、实现新增命令、修复前端补丁锚点。

## 版本号

移植版采用 `<官方版本>-<移植修订号>`，例如 `1.0.2-1`：

- 首次适配某个官方版本：修订号从 `1` 开始，例如 `1.0.2-1`。
- 只修复 Linux 移植代码或打包问题：修订号递增，例如 `1.0.2-2`。
- 官方版本升级：修订号重置，例如官方 `1.0.3` 对应 `1.0.3-1`。
- **不要发布裸官方版本号**（如 `1.0.2`），也不要用 `1.0.2.1`、`1.0.2+1` 或 `1.0.2-hotfix1`；这些格式会破坏 Tauri/SemVer 或版本排序。
- Git Tag 加 `v` 前缀（`v1.0.2-1`），应用内版本与产物名使用不带 `v` 的 `1.0.2-1`。
- 稳定版 Release 不要标记为 GitHub Pre-release，否则 `releases/latest` 可能不会指向它。

`upstream-cn.json` 中的 `version` 始终是官方版本（如 `1.0.2`），不要写入移植修订号。

## 开发环境

```bash
npm ci
npm run upstream:update      # 下载官方 1.0.2 并提取 frontend/
npm run icons
npm run prepare:frontend
npm run test:rust
npm test
```

没有官方安装包时，可只跑不依赖官方资源的检查：

```bash
npm ci
npm run scaffold
npm test
npm run test:rust
```

运行调试版：

```bash
npm run prepare:frontend
cargo run --manifest-path src-tauri/Cargo.toml
```

调试版支持 `--inspect`（打开开发者工具）和 `--self-test`（注入运行时自检脚本，需 `build/frontend` 为真实官方前端）。

## 提交前检查

```bash
npm test
npm run test:rust
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked -- -D warnings
```

涉及窗口/输入/联机的改动，请补充手工验证记录：

- 主界面渲染与点击穿透。
- 背包 / 图鉴 / 合成 / 联机 / 设置面板打开与关闭。
- 登录窗口首帧、登录后计数恢复。
- 双显示器切换与面板居中。
- `F7` / `F8` / `F9` 行为。

## 代码约定

- Rust 使用 `cargo fmt` 默认格式；错误信息优先使用稳定的 snake_case 代码（如 `input_permission_required`），面向用户的文案由前端负责。
- 前端补丁必须放在 `scripts/frontend-patches.mjs`，使用精确锚点并校验命中次数；不要用“找不到就跳过”的宽松逻辑。
- 不引入新的远程代码、遥测或账号上传。
- 修改官方命令集时同步更新 `tests/contracts.json` 与 `tests/backend-coverage.test.mjs`。

## 更新官方版本

1. `npm run upstream:check` 确认新版本。
2. `npm run upstream:update` 更新锁并替换 `frontend/`。
3. 重新生成契约：

   ```bash
   node scripts/inventory-contract.mjs frontend/assets tests/contracts.json
   ```

4. `npm test` 找出未实现的命令，在 `src-tauri/src/windows.rs` / `system.rs` 中补齐。
5. `npm run prepare:frontend` 检查补丁锚点。
6. 本地登录、联机、输入、面板全流程验证。
7. 更新版本号与 `CHANGELOG.md`。

## 安全与隐私

- 不要提交账号、Cookie、token、`counters.json`、`app.log` 或含个人信息的截图。
- Issue 中的日志请先删除 URL 查询参数、Authorization、Cookie 等内容。
- 不要扩展 WebSocket 代理到非官方域名。
- 官方资源版权归 KURO GAMES / Irox Games；提交代码即表示同意以 MIT 许可发布你的原创部分。

## PR

- 一个 PR 聚焦一个问题；描述中说明复现环境与验证方式。
- UI/窗口相关改动附修改前后截图（桌面背景可打码）。
- 破坏兼容性或需要重新提取官方资源的改动，请在描述中明确。
