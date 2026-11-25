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
│   └─ 注入脚本：platform.js + websocket.js + 登录账号历史      │
│                    │ Tauri IPC (invoke / event)              │
│                    ▼                                         │
│  Rust 后端（src-tauri/src）                                  │
│   ├─ lib.rs      命令路由、插件与生命周期                     │
│   ├─ windows.rs  窗口生命周期、几何、命中区域、光标穿透         │
│   ├─ input.rs    evdev 全局键鼠监听                          │
│   ├─ socket.rs   官方 WebSocket 原生代理                     │
│   ├─ storage.rs  点击计数持久化                              │
│   ├─ system.rs   日志/诊断/自启动/网络状态                    │
│   └─ menu.rs     系统托盘与 Dock 原生菜单                     │
└─────────────────────────────────────────────────────────────┘
```

## 启动流程

0. `main.rs` 在 GTK/WebKit 初始化前补齐 `GDK_BACKEND=x11` 与 `WEBKIT_FORCE_COMPOSITING_MODE=1`（已有值时尊重用户设置）。
1. `lib.rs::run` 先获取进程级实例锁：`XDG_RUNTIME_DIR/wuwa-tappo-linux.instance.lock`（无 `XDG_RUNTIME_DIR` 时回退到 `/tmp/wuwa-tappo-linux-<uid>.instance.lock`）。锁由内核在进程退出时释放，锁 fd 设为 `FD_CLOEXEC`，不会被 WebKit 子进程继承。拿不到锁时直接退出，避免直接运行二进制或绕过 `run.sh` 时出现双实例。退出时在 `RunEvent::Exit` 中释放锁；重启则先释放锁、启动替换进程，再让当前进程退出，避免新实例被实例锁挡掉。
2. `lib.rs::run` 注册插件、`WindowState`、`InputMonitor`、`SocketState`，安装 IPC 处理器。
3. `setup` 创建 `CounterStore`，初始化系统托盘/Dock 菜单与网络监视器。托盘不可用时菜单功能降级，关闭主窗口会直接退出。
4. `windows::setup` 创建 `main` 窗口（900×650 初始尺寸），随后按当前显示器铺满，显示并默认开启鼠标穿透。
5. 启动输入监听线程；若当前用户不在 `input` 组，`get_input_status` 会返回失败，前端显示提示。
6. 前端上报命中区域后，后端把它们换算为物理像素并写成主窗口的 **XShape input region**。区域外的点击由 XWayland/KWin 直接穿透；输入线程只在指针位于区域内（或 swallow 期间）上报 `mouse-move`，避免 Wayland 下 X11 指针坐标冻结驱动悬停；区域外按键另发一次窗外哨兵移动，防止官方前端用缓存坐标命中桌宠（见「指针穿透与悬停」）。
7. `get_main_desktop_safe_area` 通过 EWMH (`_NET_ACTIVE_WINDOW`、`_NET_WM_STATE_FULLSCREEN`、dock 窗口) 返回前台全屏状态、`foregroundApplicationFullscreen` 和 `autoHideEdge`，与官方用于自动游戏模式/任务栏避让的字段保持一致。

`main` 之外的所有窗口都是按需创建、关闭时仅 `hide`，再次打开复用实例。主窗口关闭时，如果托盘创建成功则隐藏到托盘；托盘菜单的“退出”会走前端 `quitApp` 的保存计数/断开 Socket 流程。托盘不可用或创建失败时关闭主窗口直接退出，避免留下无法唤出的后台进程和单实例锁。`F9` 在窗口隐藏到托盘时也会恢复窗口。登录窗口通过 `login_window_ready` + `login-modal-open` 事件避免首帧竞态。

退出与重启由原生层结束进程。前端 `quitApp` / `restartApp` 先发 `cat-persist-clicks-before-exit` 与 `cat-disconnect-socket-before-exit` 并等待对应的 `done` 事件；后端立即回 `cat-persist-clicks-done`（点击计数由前端在每次上报成功时写盘）并在 `cat-disconnect-socket-before-exit` 时关闭 Socket，再回 `cat-disconnect-socket-done`。随后 `exit_app` / `restart_app` 隐藏窗口，在独立线程中等待 100 ms 后调用 `std::process::exit(0)`：在 GTK 事件回调里同步走 Tauri 的优雅退出会在 WebKitGTK teardown 阶段卡死，现有的关闭窗口路径也使用同一方式。重启会先释放单实例锁并 spawn 当前可执行文件，再退出当前进程。

## 命令路由

`lib.rs::handle` 拦截两类命令：

- `windows::COMMANDS`：窗口、命中区域、光标、更新/维护提示等，必须在主线程执行。
- `SYSTEM_COMMANDS`：日志、诊断、自启动、版本检查等，走 `spawn_blocking`。

其余命令由 `tauri::generate_handler!` 注册的 typed command 处理（计数器、重置归档、网络诊断、输入状态、WebSocket）。

新增官方命令时，`tests/backend-coverage.test.mjs` 会检查 `tests/contracts.json` 中的每个运行时命令都能在 Rust 源码中找到对应字符串或函数。

## 窗口与命中区域

- `main`：透明、无边框、`always_on_top`、`shadow(false)`，起始鼠标穿透。
- `panel`：普通窗口，最小 1100×760；折叠时最小宽度 958 并按 142 px 收缩。
- `login` / `about` / `oss-notice` / `update` / `toast`：居中于主窗口所在显示器；关闭请求被拦截后隐藏。
- 前端通过 `set_main_hit_region` 上报 CSS 像素矩形：
  - `regions`：需要接收鼠标的区域。
  - `input_regions`：联机时带 `memberId` 的角色命中区域。
  - `window_passthrough`：整窗穿透开关。
- 原生后端把 `regions` / `input_regions` 的并集通过 XShape 设为 `ShapeInput`；`set_main_swallow_mouse(true)` 时临时扩成全窗口，`set_main_force_cursor_ignore(true)` 只记账、不清空区域（见「指针穿透与悬停」）。
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
  - 手柄键码 `0x120–0x12b`、`0x130–0x13f`、`0x220–0x223`、`0x2c0–0x2e7` 与手柄方向键 → `gamepad-button-press`；方向键按四个方向独立检测上升沿，斜向组合会各发一次。
  - 其他键码 < 256 → `key-press` / `key-release`，使用浏览器 `KeyboardEvent.key` 风格名称（如 `a`、`F7`、`ControlLeft`、空格为 `" "`）。按键通过 libxkbcommon 按当前 XKB 布局映射到字符和 DOM `keyCode`，非 US 布局下的自定义快捷键与字母名称与浏览器一致。
  - `EV_KEY value=2` 是自动重复，不产生 `key-release`；只有真实松手才结束按键。
  - 模拟扳机（`ABS_Z`/`ABS_RZ`，兼容 `ABS_GAS`/`ABS_BRAKE`）超过轴范围中点时发一次 `gamepad-button-press`，松手不发 release。
- 指针位置通过独立的 XCB 连接查询 X11 根窗口，避免 Tao/GDK 在后台线程返回启动时的缓存坐标。输入区域生效后，指针位于桌宠上时 XWayland 会持续更新该坐标。
- 鼠标移动以 33 ms 节流轮询：`pointer_inside`（指针位于主窗口输入区域，由 GDK enter/leave 通知维护）为真时上报实时采样；区域外一律不转发移动（冻结采样不是真值，其他 X 窗口上的实时采样按本窗口 CSS 空间命中测试会点亮幻影悬停），只做辅助窗口判定与自动恢复检测，区域外按键的伴随移动改用窗外哨兵坐标。自最近一次 leave 起没有指针运动、且采样落回当前生效命中区域内时，自动恢复 `pointer_inside`（KWin 在区域重新盖住静止指针时不会重发 enter）。swallow（拖动/点击转发）期间即使指针离开普通命中区域也使用实时坐标。
- 按键去重、鼠标按下状态与手柄按住状态由后端维护，避免重复计数。
- 需要 `input` 组读权限；失败时前端显示可恢复提示，并提供 `resume_input_monitor`。

### 快捷键

官方前端不在 webview 内匹配 F7/F8/F9，而是通过原生命令与事件管理快捷键：

- `shortcuts.rs` 维护 `动作 → {key, modifiers}` 表，默认 `gameMode`=F7、`switchScreen`=F8、`togglePet`=F9，持久化到应用数据目录的 `shortcuts.json`。
- `get_app_shortcuts` 返回当前表；`set_app_shortcuts` 校验后写盘并广播 `app-shortcuts-changed`（重复组合返回 `duplicate`）；`set_shortcut_capture` 在设置页录制期间暂停匹配。
- 输入线程用独立的 `held_keys` 跟踪修饰键（忽略自动重复），按当前 XKB 布局把 evdev 键码翻译成官方前端使用的 DOM `keyCode`，命中后广播 `app-shortcut`（负载为动作名，如 `"togglePet"`）。
- 因此快捷键在窗口失焦时同样生效，但依赖 `input` 监听；监听不可用时快捷键与点击计数一起失效。

## 原生 WebSocket 代理

`scripts/websocket.js` 只在目标为 `wss://api-mc-wuwatappo.kurogames-global.com/socket.io/` 时替换浏览器 `WebSocket`：

1. `native_ws_connect` 校验调用窗口（`main` / `panel` / `login`）、窗口 URL 与目标 URL。
2. `socket.rs` 用 `tokio-tungstenite` 建立连接，通过 Tauri Channel 推送 `open` / `text` / `binary` / `error` / `close`。
3. `native_ws_send` / `native_ws_close` 负责发送与清理；窗口刷新或销毁时关闭该窗口的全部连接。
4. 不转发任意请求头、Cookie 或 Origin，避免把代理变成通用网络出口。
5. 原生 TLS 使用 rustls：必须启用 `ring` provider feature，并在启动时显式安装为进程默认 provider（`tokio-tungstenite` 不会自动选择 provider；未启用任何 provider feature 时，构造 TLS 配置会 panic）。rustls 关闭默认 feature 以避开 `aws-lc-rs`，因此必须保留 `tls12` feature：官方 endpoint 只协商 TLS 1.2，只提供 TLS 1.3 会被服务端用 `protocol_version` alert 拒绝。升级 `tokio-tungstenite` 时要同步对齐 rustls 大版本，避免同时编译两个 rustls。

## 计数与日志

- `storage.rs` 按 `environment` 和 `cuid` 分桶保存按键/点击计数；写入使用临时文件 + `fsync` + `rename`，目录 0700、文件 0600。
- 计数上限为 JavaScript 安全整数 `2^53-1`，超出直接报错而不是静默截断。
- `system.rs` 对日志做 URL 查询串、token、Cookie、Authorization、Bearer 等脱敏；单文件超过 2 MiB 滚动为 `app.log.1`。
- `reset_local_data`（设置页“清除全部本地缓存”）先按 `environment`/`cuid` 把重置前的总点击数写入 `reset_archive`，再清空该环境的计数与快捷键配置，并写入 `.reset-pending` 标记后重启；下次启动在创建 WebView 之前删除 WebKit 的缓存与 LocalStorage（保留计数与日志），再次登录时 `load_reset_click_archive` 返回存档值，前端据此恢复总点击数。
- `run_network_diagnostics` 只接受官方 HTTPS 主机（API host 固定，资源 host 限定 `*.kurogames-global.com`），对 443 做带超时的连通性探测，返回 `status`（`complete`/`timeout`/`network_error`）与资源地址；只探测 TCP 443 可达性，不代表 TLS 握手或 HTTP 请求可用。同一 `runId` 的重复调用返回 `already_collected`，不会重复探测，且只有 `complete` 结果会被记忆，失败后的重试仍会重新探测。

## 前端补丁流水线

```
官方安装包
  └─ scripts/extract-assets.mjs   解析 PE + Brotli 资源表，静态提取，不执行安装包
       └─ frontend/               原始官方前端（不入库）
            └─ scripts/prepare-frontend.mjs
                 ├─ frontend-patches.mjs   SDK HTTPS / 登录就绪 / 宝箱计时 / 关于页 / 语言 / 登录态存储 / 登录恢复重试 / 桌宠光标跟随前端命令
                 └─ build/frontend/        嵌入二进制的最终前端（不入库）
```

所有补丁都使用精确锚点并验证命中数量。官方升级导致锚点变化时，`prepare-frontend` 直接失败，不会产出行为不完整的前端。

### 宝箱计时

官方前端在计算宝箱倒计时前会等待活动配置的 `server_time` 就绪。若登录态在 Vue 挂载前已经恢复，这个就绪标记可能一直为 `false`，两个宝箱按钮会停留在 `reward-btn--loading` 的灰色禁用态。

`patchChestTimers` 在首次计算宝箱状态时直接标记就绪；计时仍优先使用官方 `server_time`，拿不到时回退到本地 `Date.now()`。可见性保持官方规则：加载态与点击余额 ≥2000 时，悬停桌宠显示左右两个倒计时；余额 1000~2000 时按官方单侧显示；余额 <1000 且配置就绪时悬停不显示；宝箱可领取时无需悬停即显示彩色按钮，领取后显示倒计时。未登录时会和官方一样显示两个宝箱按钮，点击进入登录流程。移植版的“总是展示宝箱”在已登录时绕过余额门槛并同时显示左右两个宝箱；领取仍受官方 `totalClickCount` 校验。

### 指针穿透与悬停

XWayland/Wayland 会话中，指针离开桌宠的 XShape 输入区域后，XWayland 不再提供全局指针坐标：采样冻结在离开时的最后位置。官方 Windows 前端依赖系统级钩子持续获得真坐标，Linux 上没有等价数据源，原生层按下列规则补偿：

- 主窗口控件层级的 `enter-notify-event` / `leave-notify-event` 维护输入线程的 `pointer_inside`。leave 时向官方前端发送一次无 payload 的 `mouse-leave` 事件，官方前端据此清除悬停并让自动隐藏计时下陷；此后冻结采样不再驱动悬停/光标，也不再发送携冻结坐标的合成 `mouse-move`。`mouse-leave` 的唯一来源是这里的 crossing 通知（swallow 期间跳过），按键事件不再发送该事件。
- 区域外的鼠标按键仍上报 `mouse-button` 供官方前端累计点击计数，但不携带实时坐标：伴随的 `mouse-move` 是派生自当前窗口原点与缩放的窗外哨兵坐标（CSS 约 -100000），官方前端用缓存的窗口原点换算后得到负视口坐标，`elementFromPoint` 不会命中桌宠、宝箱或拖动热区；`memberId` 只在 `pointer_inside` 或 swallow 时携带。区域外的移动采样一律不转发：Wayland 会话下冻结采样不是真值；即使指针正停在其他 XWayland 窗口上、坐标可证实实时，该坐标仍会按本窗口 CSS 空间做命中测试，落在宠物矩形内时会点亮一次「幻影悬停」，指针停手后没有后续事件让它常驻。官方前端的悬停清除与自动隐藏唤醒分别由 `mouse-leave` 事件和按键事件承担，任务栏贴边探测只消费几何签名，不依赖区域外的移动流。
- KWin 的输入区域事件不对称：区域从静止指针下方移除会触发 leave，区域重新盖住静止指针不会触发 enter。输入线程用 evdev 运动计数补偿——自最近一次 leave 起没有指针运动时，冻结采样即真实位置；XQueryPointer 采样若落回当前生效的命中区域内，自动恢复 `pointer_inside` 并重新上报一次位置，指针停在原处也能重新触发悬停。任何真实指针运动都会使该证明失效，直到下一次 leave 重新建立。
- 运动计数本身分不清「指针停到桌面上」与「区域从指针下方移走」：物理离开时 evdev 先读到离开运动，随后指针停下，纯计数判据会把冻结样本误判为仍在区域内。因此自动恢复还需要「uncovered 见证」——采样至少有一次不被当时的生效命中区域覆盖。区域同步在主线程应用成功后就地判定，33 ms 轮询不会漏掉毫秒级过渡；判定不依赖 `pointer_inside`，因为形状变更产生的 leave 通知要等下一轮主循环才到达 GTK 线程，先于它到达的过渡同样要计入。见证按 evdev 运动纪元打标而不是在下次 leave 时清零，所以「过渡先于 leave 到达」的次序不会丢掉见证，物理移动抬高纪元后旧见证自动失效。该判据的前提是物理离开后命中区域在无运动期间保持不动；若前端恰好在这个窗口内让区域先移开再盖回冻结样本，仍可能误恢复一次——悬停面板重现通常需要指针移动并抬纪元，因此概率很低。真正的区域重建（自动隐藏收起、拖拽后区域同步）经过不覆盖瞬间，照常放行。
- 桌宠光标：官方前端经 `set_native_input_cursor` / `set_overlay_cursor` 下发 `default/pointer/grab/grabbing`（Windows 全局 SetCursor 语义，宠物本体 CSS 无 cursor 规则）。X11 显示的光标由指针下最深层窗口决定，只设 GTK toplevel 会被 WebKit 子窗口覆盖；原生层把命名光标递归设置到整棵 GDK 窗口树（含 WebKit 子窗口），并按 `default→left_ptr→arrow`、`grab→closed-hand→hand2` 等别名链回退，兼容精简的 Xvfb/旧主题环境。前端只在下发的 kind 变化时发命令（`er` 闩锁），而 WebKitGTK 会在指针移动、悬停 UI 挂载等时机重算 DOM 光标并写回子窗口——两个写入者不同值时，光标会逐帧在手形与箭头间闪烁。因此原生层把命令的 kind 同时镜像进样式表自定义属性 `--wuwa-pet-cursor`，由 `.pet-dock,.pet-dock *{cursor:var(--wuwa-pet-cursor,auto)}` 让 WebKit 的重算结果与命令一致；禁用移动时前端判定为 `default`（箭头）、拖拽启用时为 `grab`（手形）、宝箱按钮上为 `pointer`，均按官方判定跟随，无需原生猜模式。原生层另外在指针处于命中区域内时以 33 ms 节拍重复应用该 kind（GDK 按名字缓存光标对象，重放不会制造新资源），并每秒重写一次样式属性，以便前端重新加载文档后恢复；离开区域后停止维护，桌面光标不受影响。
- `set_main_force_cursor_ignore(true)` 是官方前端的临时整窗穿透状态（启动、拖拽结束）。Windows 原版此时清空命中区域，靠全局钩子的真坐标流驱动下一次交互；Linux 上清空会让 KWin 发出 leave、把静止指针的 `pointer_inside` 卡死在 false（区域恢复也不会产生 enter），交互永不恢复。因此原生层对该状态只记账、不清空 XShape：拖拽结束后命中区域保持原样，指针未离开窗口时 `pointer_inside` 全程为真，下一次按键照常经 `set_main_swallow_mouse` 恢复交互。与 Windows 的差别是拖拽后指针静止时，点击宠物区域会被本应用窗口消费而不是落透到桌面；全屏游戏/视频等窗口与桌宠重叠时同样如此。官方前端对两种来源的响应一致，用户感知相同。
- swallow（拖动/点击转发）期间使用实时坐标且不发送 `mouse-leave`，避免影响拖拽；纯 X11 会话下坐标本身不会冻结，但同样按 `pointer_inside` 门控上报。

### 桌宠光标

宠物本体的光标在 Windows 原版由全局 `SetCursor` 提供，前端不通过 CSS 表达：拖拽启用时是手形，勾选「禁用移动」后是箭头，宝箱按钮上是手形结点（`pointer`）。WebKitGTK 会在指针移动与悬停 UI 挂载时重算 DOM 光标并写回 WebKit 子窗口，而宠物本体没有 cursor 规则，重算结果恒为箭头；前端又只在 kind 变化时发命令，被 WebKit 覆盖后不会重发，两个写入者交替生效即表现为闪烁。

`patchPetCursor` 在官方样式表中追加 `html{--wuwa-pet-cursor:auto}` 与 `.pet-dock,.pet-dock *{cursor:var(--wuwa-pet-cursor,auto)}`，宠物子树的光标统一解析到一个自定义属性；原生层在收到前端命令时把 kind 写入该属性（`setProperty`），于是 WebKit 的重算结果与命令同值，不再与原生维护的光标冲突。未收到命令前属性为 `auto`（箭头），符合禁用移动的语义。补丁以两条精确的宠物布局规则作为锚点，任一锚点缺失或样式表不以上一条规则结束时构建失败。

### 登录态存储

官方前端把登录 token 保存为 `kuro_infoToken` Cookie（`document.cookie`）。WebKitGTK 不会向 `tauri://localhost` 自定义协议来源暴露 JavaScript 写入的 Cookie，表现为登录接口成功但后续 `user/me` 返回 10002，重启后也无法恢复登录。

`patchCookieStorage` 在读取时先尝试 `document.cookie`，失败或无值时回退到同源 LocalStorage；写入时同时尝试 Cookie 与 LocalStorage（带过期时间），删除时两边一起清除。LocalStorage 由 WebKitGTK 持久化在应用数据目录，因此主窗口、登录窗口和重启后的实例共享同一登录态。应用数据目录在启动时会被收紧到 0700（无法 chmod 时只记录警告），token 为明文存储，依赖该目录的本地权限，不使用系统密钥环。

`patchSessionRestore` 为启动恢复加了传输层重试：冷启动时网络尚未就绪导致的 `network_error` / `timeout` 最多尝试 3 次（重试间隔 1s、2s），避免把仍然有效的 token 误判为未登录；`10002/10003` 等鉴权拒绝仍然立即清除登录态。服务端因“账号已在其他设备登录”（90002）或旧 token 失效返回 10003 时，仍会按官方逻辑登出，这一层无法由客户端绕过。

### 最近登录账号与手机号

WebKitGTK 没有登录框历史下拉能力，因此登录窗口额外注入 `scripts/login-account-history.js`：账号框或手机号框获得焦点、单击时，在输入框下方列出对应的最近成功登录值；支持鼠标选择、上下方向键、Enter 确认和 Esc 关闭，输入内容用于过滤列表。下拉以 `position: fixed` 贴在输入框所在 wrapper 下方，靠近视口底部时翻到上方。

历史只在 SDK `onLoginSuccessful` 确认成功后写入：账号密码登录记录用户在账号框里实际提交的标识（`wuwatappo-login-accounts`），手机号验证码登录记录手机号（`wuwatappo-login-phones`），失败尝试不写入；密码和验证码不经过该脚本。SDK 会在自己的冒泡 `input` 处理器里清洗输入（例如去掉手机号中的空格），脚本在下一个微任务读取清洗后的值，避免把原始粘贴写进历史。SDK 回传的用户名是规范名，邮箱或手机号登录时与输入的标识不同，因此不以逐字相等作为写入条件。两类记录各自去重并保留最近 5 个，保存在登录窗口同源 LocalStorage 中，应用数据目录权限仍是 0700。

该脚本作为独立的 initialization script 只注入 `login` 窗口，不修改官方前端资源；脚本自身异常不会拖垮 websocket/platform 等共用 shim。它依赖官方登录表单的固定锚点（`anchor-account`、`anchor-phone`）与 SDK 事件总线，前端升级导致锚点变化时静默降级为没有下拉（见 troubleshooting）。脚本幂等，重复注入不会重复挂载下拉或监听器。

## 安全边界

- 不执行官方 Windows 安装包；`extract-assets.mjs` 只读取 PE 与嵌入资源。
- 原生 WebSocket 仅允许官方国服 HTTPS/WSS 域名与固定路径。
- 日志脱敏后再落盘；窗口错误信息不包含 token/URL 查询参数。
- `csp: null` 是官方前端运行所必需，意味着前端可加载其原有远程 SDK；本项目不额外注入远程代码。
