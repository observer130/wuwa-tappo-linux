# 故障排查

先看日志：

```bash
tail -n 200 ~/.local/share/dev.w33d.wuwatappo.cn.linux/logs/app.log
```

也可以在官方设置/关于页使用“打开日志目录”。

## 主窗口空白或完全透明

请优先通过 `run.sh` 启动。它会设置：

```bash
GDK_BACKEND=x11
WEBKIT_FORCE_COMPOSITING_MODE=1
```

二进制启动时也会在未显式设置的情况下补齐这两个变量；若你手动覆盖了 `GDK_BACKEND` / `WEBKIT_FORCE_COMPOSITING_MODE`，请确认值正确。缺少 `WEBKIT_FORCE_COMPOSITING_MODE=1` 时，WebKitGTK 的透明合成层可能只显示空白。确认：

```bash
GDK_BACKEND=x11 WEBKIT_FORCE_COMPOSITING_MODE=1 ./run.sh
```

如果仍然空白，尝试关闭其他合成器特效或切换 X11 会话，并提供 `app.log` 与显卡/驱动信息。

## 菜单、按钮和拖动都没反应

先按一次 `F7`。官方“游戏模式”会禁用与桌宠的交互，而且这个开关会持久化；日志中 `[SETTINGS] ... game_mode=true` 表示它正在生效。再按 `F7` 或重新打开设置关闭游戏模式即可恢复。

如果 `F7` 没有反应，先确认快捷键没有被改掉（设置页可查看），再检查 `[INPUT] granted=true running=true` 与 `[HIT]` 日志。1.0.3 起快捷键由原生层从 `evdev` 匹配，输入监听未启动时不会生效。

## 键盘/点击计数不动

桌宠需要读取 `/dev/input/event*`：

```bash
id -nG "$USER"            # 应包含 input
ls -l /dev/input/event*   # 通常 root:input，权限 660
```

修复：

```bash
sudo usermod -aG input "$USER"
# 当前会话立即生效：
newgrp input
./run.sh
```

手柄按键也依赖同一权限。若使用 Flatpak/Snap 或容器运行，`input` 组与 `/dev/input` 可能不可见，请改用发行版原生包。

## 手柄按键不计入点击

1.0.2 起官方前端会把 `gamepad-button-press` 计入点击（1.0.3 同样如此）。Linux 后端直接从 `/dev/input/event*` 读取手柄按键与方向键，同样需要 `input` 组权限；确认：

```bash
ls -l /dev/input/by-id/ | grep -i -E 'joystick|gamepad'
id -nG "$USER"            # 应包含 input
```

若手柄被 Steam Input 映射成键盘/鼠标，系统可能不再暴露原始手柄事件；关闭 Steam Input 或改用原生模式后再试。蓝牙手柄重新连接后，后端最多 2 秒内会自动重新扫描设备。

## 点击宝箱后闪退

1.0.2-1 在点击宝箱且积分不足 1000 时会触发 Toast；此时隐藏的 Toast 窗口尚未 realize，设置点击穿透会触发 tao 的 `Option::unwrap()` panic 并 abort。1.0.2-2 起改为先显示 Toast 再设置点击穿透，现在的 1.0.3-1 已包含该修复；如果你仍在旧构建上，请更新到最新发布版。

## 宝箱一直是灰色图标

如果你使用的是 1.0.2-1 之前的构建，宝箱按钮可能一直处于灰色加载态，且倒计时不出现。1.0.2-1 起的构建包含宝箱计时修复（当前 1.0.3-1 同样包含）：宝箱可领取时会显示彩色按钮，领取后才会显示下一轮倒计时。

## 提示“无法读取键鼠监听状态”/“监听尚未启动”

- 确认 `webkit2gtk-4.1` 运行库存在：`ldd bin/wuwatappo-linux | grep webkit`。
- 确认没有其他实例占用锁：`run.sh` 使用 `$XDG_RUNTIME_DIR/wuwa-tappo-linux.lock`，二进制自身使用同目录下的 `wuwa-tappo-linux.instance.lock`；没有 `XDG_RUNTIME_DIR` 时文件名会带上当前 uid（`/tmp/wuwa-tappo-linux-<uid>*.lock`）。
- 查看日志中 `[INPUT]` 行的 `granted` / `running` / `error`。

## 分数缩放下列表/按钮点不中或窗口偏移

先看日志：

```bash
grep '\[WEBVIEW\]' ~/.local/share/dev.w33d.wuwatappo.cn.linux/logs/app.log | tail
```

应显示 `devicePixelRatio=1.25/1.5/2.0`，且随后有：

```bash
grep '\[HIT\]' ~/.local/share/dev.w33d.wuwatappo.cn.linux/logs/app.log | tail
```

`[HIT] regions=N` 表示官方前端已上报命中区域；`[HIT] first non-empty regions: ...` 会给出 CSS 像素矩形。若 `devicePixelRatio` 与实际不符，或命中区域一直为 0，请附上这些日志提交 Issue。

如果只有某个显示器混合缩放时偏移，尝试把主窗口切到该显示器后重新打开面板；跨显示器不同缩放仍是已知限制。

点击桌宠时还可以检查原生鼠标事件：

```bash
grep '\[MOUSE\]' ~/.local/share/dev.w33d.wuwatappo.cn.linux/logs/app.log | tail -20
```

- `target=true`：事件会送到官方前端。
- `aux=true` / `target=false`：光标被判定在关于/面板等辅助窗口上，主窗口不应处理这次点击。若位置明显不对，请附上该行日志。

主窗口的鼠标穿透由 XShape 输入区域实现。日志中的 `[SHAPE] mode=regions rects=N` 表示输入区域已更新；如果该行为 `rects=0` 且没有更新，桌宠区域不会接收点击。

## Wayland 下无法点击、焦点异常或位置偏移

当前验证路径是 **X11 / XWayland**。请：

1. 确认 `run.sh` 设置了 `GDK_BACKEND=x11`。
2. 在 GNOME/KDE 的 Wayland 会话中允许 XWayland。
3. 若面板窗口被显示器边缘裁切，可拖动标题栏；这属于窗口定位限制。

纯 Wayland 原生后端尚未实现，欢迎提交适配 PR。

## 面板/登录/联机一直加载

这些功能依赖官方服务与登录态：

- 未登录时部分面板会保持加载态，先完成国服账号登录。
- 如果登录窗口提示成功、主界面却仍显示未登录，或重启后登录态丢失，请确认使用包含 `patchCookieStorage` 修复的最新构建。WebKitGTK 不向 `tauri://` 来源暴露 JavaScript Cookie，旧构建会把 `infoToken` 丢掉；日志通常表现为登录后 `api/frontend/user/me` 返回 `10002`。
- 冷启动时 `user/me` 的 `network_error` 现在最多尝试 3 次（重试间隔 1s、2s），不会再因网络尚未就绪直接显示未登录。如果日志仍出现 `[AUTH] event=boot logged_in=false`，先看同一时段是否有 `code=10003`：这表示服务端已判定 token 失效（常见于在其他设备登录后旧 token 被顶掉），需要重新登录，客户端无法恢复。
- 当前账号服务是单会话策略：同一账号在新设备登录会生成新 token，旧 token 在下一次请求时返回 `10003`。多设备同时在线不是 Linux 移植层的设置，客户端无法绕过。
- 登录态保存在应用数据目录的 `localstorage/` 中，可用“彻底重置”重新登录；不要把该目录或 token 发给他人。
- 检查系统时间是否准确，TLS 与签名校验对时间敏感。
- 官方维护期间会显示维护提示；此时“退出”会关闭整个应用。
- 联机需要能访问 `api-mc-wuwatappo.kurogames-global.com`；代理只允许该官方端点。

## 联机时报 WebSocket 错误

原生代理只接受：

```
wss://api-mc-wuwatappo.kurogames-global.com/socket.io/
```

并校验调用窗口为 `main` / `panel` / `login`。公司代理、透明代理或 DNS 劫持可能改变证书/主机名，导致握手失败。查看日志中的 `WebSocket handshake rejected` 或 `TLS` 信息。

## 启动时提示已在运行

如果系统托盘可用，主窗口或任务栏的关闭操作会隐藏到托盘，退出请使用托盘菜单的“退出”。如果托盘不可用，关闭窗口会直接退出应用。如果你用 `F9` 隐藏了桌宠，进程会继续运行，再按一次 `F9` 即可恢复；窗口被隐藏到托盘时，`F9` 也会恢复窗口。也可以手动退出：

```bash
pkill -x wuwatappo-linux
```

如果进程确实已经不存在，仍提示占用，再清理锁：

```bash
pgrep -ax wuwatappo-linux
RUNTIME="${XDG_RUNTIME_DIR:-/tmp}"
SUFFIX=""
[[ -n "${XDG_RUNTIME_DIR:-}" ]] || SUFFIX="-$(id -u)"
fuser -v "$RUNTIME/wuwa-tappo-linux$SUFFIX.lock"
fuser -v "$RUNTIME/wuwa-tappo-linux$SUFFIX.instance.lock"
rm -f "$RUNTIME/wuwa-tappo-linux$SUFFIX.lock" \
      "$RUNTIME/wuwa-tappo-linux$SUFFIX.instance.lock"
```

不要在实例仍在运行时删除锁文件，否则可能启动第二个实例。`run.sh` 持有启动器锁；二进制自身还会持有进程级实例锁，因此直接运行 `bin/wuwatappo-linux` 也会被拦截。两把锁都由内核在进程退出时释放，锁 fd 不会继承给 WebKit 子进程。

## 托盘图标不显示

托盘依赖 `libayatana-appindicator3`（或 `libappindicator3`）。确认：

```bash
ldconfig -p | grep -E 'ayatana-appindicator|libappindicator3'
```

若缺失，安装 `libayatana-appindicator3-1`（Debian/Ubuntu）或 `libappindicator-gtk3`（Arch/Fedora）后重启应用。GNOME 需要 AppIndicator/KStatusNotifierItem 扩展；KDE、XFCE、Cinnamon 等通常自带支持。托盘不可用时应用会自动禁用“关闭到托盘”，关闭主窗口会直接退出。

## 预编译包报 `GLIBC_2.39 not found`

发布包在 Ubuntu 24.04 构建。Ubuntu 22.04、Debian 12 等旧系统请从源码构建（见 [building.md](building.md)），或使用更新的发行版。

## 彻底重置

```bash
pkill -f wuwatappo-linux || true
rm -rf ~/.local/share/dev.w33d.wuwatappo.cn.linux
```

这会删除登录态、点击计数和日志。桌面菜单项可用 `./install.sh --uninstall` 移除。

## 提交 Issue 时请附上

- 发行版与版本、`echo $XDG_SESSION_TYPE`、桌面环境、X11/XWayland。
- `app.log` 中相关片段（**先确认没有账号、Cookie、token 等敏感信息**）。
- 复现步骤与是否使用发布包/自编译。
