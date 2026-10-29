# 架构说明

Wuwa Tappo Linux 由两半组成：**官方静态前端**和**重建的 Linux 原生后端**。官方前端被完整嵌入二进制，负责所有业务逻辑与界面；Rust 只补上 Windows 原版由 WebView2/Win32 提供的能力。

## 总览

```
┌─────────────────────────────────────────────────────────────┐
│ Tauri 进程                                                   │
│                                                             │
│  官方 Vue 前端（build/frontend，嵌入二进制）                  │
│   ├─ main：透明全屏桌宠覆盖层                                │
│   ├─ panel / login / about / oss-notice / update / toast     │
│   └─ 注入脚本：platform.js + websocket.js                    │
│                    │ Tauri IPC (invoke / event)              │
│                    ▼                                         │
│  Rust 后端（src-tauri/src）                                  │
│   ├─ lib.rs      命令路由、插件与生命周期                     │
│   ├─ windows.rs  窗口生命周期、几何、命中区域、光标穿透         │
│   ├─ input.rs    evdev 全局键鼠监听                          │
│   ├─ socket.rs   官方 WebSocket 原生代理                     │
│   ├─ storage.rs  点击计数持久化                              │
│   ├─ system.rs   日志/诊断/自启动/网络状态                    │
│   └─ menu.rs     原生菜单占位（Linux 无菜单栏）               │
└─────────────────────────────────────────────────────────────┘
```

## 启动流程

0. `main.rs` 在 GTK/WebKit 初始化前补齐 `GDK_BACKEND=x11` 与 `WEBKIT_FORCE_COMPOSITING_MODE=1`（已有值时尊重用户设置）。
1. `lib.rs::run` 注册插件、`WindowState`、`InputMonitor`、`SocketState`，安装 IPC 处理器。
2. `setup` 创建 `CounterStore`，初始化菜单占位与网络监视器。
3. `windows::setup` 创建 `main` 窗口（900×650 初始尺寸），随后按当前显示器铺满，显示并默认开启鼠标穿透。
4. 启动输入监听线程；若当前用户不在 `input` 组，`get_input_status` 会返回失败，前端显示提示。
5. 前端上报命中区域后，后端把它们换算为物理像素并写成主窗口的 **XShape input region**。区域外由 XWayland/KWin 直接穿透，不再轮询光标，避免 Wayland 下 X11 指针坐标冻结的问题。

`main` 之外的所有窗口都是按需创建、关闭时仅 `hide`，再次打开复用实例。主窗口没有托盘入口，因此用户关闭主窗口时直接退出应用，避免留下无法唤出的后台进程和单实例锁。登录窗口通过 `login_window_ready` + `login-modal-open` 事件避免首帧竞态。

## 命令路由

`lib.rs::handle` 拦截两类命令：

- `windows::COMMANDS`：窗口、命中区域、光标、更新/维护提示等，必须在主线程执行。
- `SYSTEM_COMMANDS`：日志、诊断、自启动、版本检查等，走 `spawn_blocking`。

其余命令由 `tauri::generate_handler!` 注册的 typed command 处理（计数器、输入状态、WebSocket）。

新增官方命令时，`tests/backend-coverage.test.mjs` 会检查 `tests/contracts.json` 中的每个运行时命令都能在 Rust 源码中找到对应字符串或函数。

## 窗口与命中区域

- `main`：透明、无边框、`always_on_top`、`shadow(false)`，起始鼠标穿透。
- `panel`：普通窗口，最小 1100×760；折叠时最小宽度 958 并按 142 px 收缩。
- `login` / `about` / `oss-notice` / `update` / `toast`：居中于主窗口所在显示器；关闭请求被拦截后隐藏。
- 前端通过 `set_main_hit_region` 上报 CSS 像素矩形：
  - `regions`：需要接收鼠标的区域。
  - `input_regions`：联机时带 `memberId` 的角色命中区域。
  - `window_passthrough`：整窗穿透开关。
- 原生后端把 `regions` / `input_regions` 的并集通过 XShape 设为 `ShapeInput`；`set_main_swallow_mouse(true)` 时临时扩成全窗口，`set_main_force_cursor_ignore(true)` 时清空。
- 辅助窗口的 `HitRegion` 以全局 CSS 坐标保存，原生鼠标事件落在面板/关于窗口上时不再转发给主窗口。
- 主窗口、面板、登录、关于、OSS 声明等窗口统一使用透明原生表面；官方路由本身是圆角卡片，否则窗口默认白底会在圆角外露出矩形白边。

## 缩放与坐标

Linux 上 GTK 可能通过 XSettings 应用 1.25/1.5 等分数缩放，而 Tauri/tao 在 X11 下仍报告 1.0。两者不一致时，官方前端按 CSS 像素上报命中区域，原生指针却是物理像素，窗口会“点不中”。

处理方式：

- 主窗口加载后由 `platform.js` 调用 `report_webview_metrics`，上报 `window.devicePixelRatio` 与 CSS 视口尺寸。
- 命中测试使用 `geometry::physical_to_css` 把物理光标换算到 CSS 空间，再与前端矩形比较。
- 原生鼠标事件回传前端时，用 `geometry::css_to_payload` 换算成官方前端 `Ht()` 期望的坐标空间。
- 面板、登录、关于等窗口的 CSS 尺寸与居中位置按 DPR 换算为物理像素；任务栏 insets 也以 CSS 像素返回。
- 日志中的 `[WEBVIEW] devicePixelRatio=...` 与 `[HIT] regions=...` 可用于确认缩放是否被正确识别。

## 全局输入

- 每 2 秒扫描 `/dev/input/event*`，以非阻塞方式打开。
- 处理 `EV_KEY` 与手柄方向键的 `EV_ABS`：
  - 鼠标键码 272–274 → `mouse-button`（含 `button`、`action`、可选 `memberId`）。
  - 手柄键码 `0x120–0x12b`、`0x130–0x13f`、`0x220–0x223`、`0x2c0–0x2e7` → `gamepad-button-press`；方向键 `ABS_HAT0X/Y` 从归零状态进入按下时也只发送一次。
  - 其他键码 < 256 → `key-press` / `key-release`，使用浏览器 `KeyboardEvent.key` 风格名称（如 `a`、`F7`、`ControlLeft`、空格为 `" "`）。
- 指针位置通过独立的 XCB 连接查询 X11 根窗口，避免 Tao/GDK 在后台线程返回启动时的缓存坐标。输入区域生效后，指针位于桌宠上时 XWayland 会持续更新该坐标。
- 鼠标移动以 33 ms 节流轮询，光标在辅助窗口上时不发送。
- 按键去重、鼠标按下状态与手柄按住状态由后端维护，避免重复计数。
- 需要 `input` 组读权限；失败时前端显示可恢复提示，并提供 `resume_input_monitor`。

## 原生 WebSocket 代理

`scripts/websocket.js` 只在目标为 `wss://api-mc-wuwatappo.kurogames-global.com/socket.io/` 时替换浏览器 `WebSocket`：

1. `native_ws_connect` 校验调用窗口（`main` / `panel` / `login`）、窗口 URL 与目标 URL。
2. `socket.rs` 用 `tokio-tungstenite` 建立连接，通过 Tauri Channel 推送 `open` / `text` / `binary` / `error` / `close`。
3. `native_ws_send` / `native_ws_close` 负责发送与清理；窗口刷新或销毁时关闭该窗口的全部连接。
4. 不转发任意请求头、Cookie 或 Origin，避免把代理变成通用网络出口。

## 计数与日志

- `storage.rs` 按 `environment` 和 `cuid` 分桶保存按键/点击计数；写入使用临时文件 + `fsync` + `rename`，目录 0700、文件 0600。
- 计数上限为 JavaScript 安全整数 `2^53-1`，超出直接报错而不是静默截断。
- `system.rs` 对日志做 URL 查询串、token、Cookie、Authorization、Bearer 等脱敏；单文件超过 2 MiB 滚动为 `app.log.1`。

## 前端补丁流水线

```
官方安装包
  └─ scripts/extract-assets.mjs   解析 PE + Brotli 资源表，静态提取，不执行安装包
       └─ frontend/               原始官方前端（不入库）
            └─ scripts/prepare-frontend.mjs
                 ├─ frontend-patches.mjs   SDK HTTPS / 登录就绪 / 关于页 / 语言 / 登录态存储
                 └─ build/frontend/        嵌入二进制的最终前端（不入库）
```

所有补丁都使用精确锚点并验证命中数量。官方升级导致锚点变化时，`prepare-frontend` 直接失败，CI 与发布流程随之中止。

### 登录态存储

官方前端把登录 token 保存为 `kuro_infoToken` Cookie（`document.cookie`）。WebKitGTK 目前不会向 `tauri://localhost` 自定义协议来源暴露 JavaScript 写入的 Cookie，表现为登录接口成功但后续 `user/me` 返回 10002，重启后也无法恢复登录。

`patchCookieStorage` 在读取时先尝试 `document.cookie`，失败或无值时回退到同源 LocalStorage；写入时同时尝试 Cookie 与 LocalStorage（带过期时间），删除时两边一起清除。LocalStorage 由 WebKitGTK 持久化在应用数据目录，因此主窗口、登录窗口和重启后的实例共享同一登录态。token 为明文存储，依赖应用数据目录的本地权限，不使用系统密钥环。

## 安全边界

- 不执行官方 Windows 安装包；`extract-assets.mjs` 只读取 PE 与嵌入资源。
- 原生 WebSocket 仅允许官方国服 HTTPS/WSS 域名与固定路径。
- 日志脱敏后再落盘；窗口错误信息不包含 token/URL 查询参数。
- `csp: null` 是官方前端运行所必需，意味着前端可加载其原有远程 SDK；本项目不额外注入远程代码。
