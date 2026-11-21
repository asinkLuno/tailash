# tailash

最小化 Linux 代理客户端：gpui GUI + mihomo 内核（TUN 模式），**与 Tailscale 共存**。

## 安装

**Arch / EndeavourOS**：先装内核 `paru -S mihomo`（AUR 包自带 GeoIP，`/usr/bin/mihomo`），
再装 tailash 本体（二选一）：

```bash
# 方式一：pacman 包（Releases 页 tailash-*-x86_64.pkg.tar.zst，装到 /usr/bin/tailash）
sudo pacman -U tailash-*-x86_64.pkg.tar.zst

# 方式二：tar.gz 解压跑 install.sh（装到 ~/.local/bin，无需 root）
tar xzf tailash-*-linux-x86_64.tar.gz
cd tailash-*-linux-x86_64 && ./install.sh
```


**其它发行版**：内核自行从 [mihomo releases](https://github.com/MetaCubeX/mihomo/releases) 下载
放进 PATH；GeoIP 不用管（内核首启自动下载）。运行依赖见 Release notes 的「运行依赖」。


### 放进应用启动器（桌面菜单）

二进制装好后，KDE/GNOME 等桌面环境还需要一个 `.desktop` 入口，才能从应用启动器（开始菜单）启动。
写到用户目录即可，无需 root：

```bash
BIN="$(command -v tailash)"   # pacman 装的是 /usr/bin/tailash；install.sh 是 ~/.local/bin/tailash
mkdir -p ~/.local/share/applications
cat > ~/.local/share/applications/tailash.desktop <<EOF
[Desktop Entry]
Type=Application
Name=tailash
GenericName=mihomo GUI
Comment=mihomo GUI 客户端（GPUI）
Exec=${BIN}
TryExec=${BIN}
Icon=preferences-system-network-proxy
Terminal=false
Categories=Network;
Keywords=proxy;mihomo;clash;
EOF

update-desktop-database ~/.local/share/applications 2>/dev/null || true
kbuildsycoca6 2>/dev/null || true   # KDE 需要；其它桌面可省略
```

- 刷新后在应用启动器搜索 tailash（KDE 里位于「互联网」分类）；想常驻可右键添加到面板/收藏夹。
- `Icon=preferences-system-network-proxy` 是 Breeze 自带的网络代理图标。想换专属图标，把 SVG 放到
  `~/.local/share/icons/hicolor/scalable/apps/tailash.svg`，再把 `Icon=` 改成 `tailash` 并重跑刷新命令。
- 移除菜单入口：`rm ~/.local/share/applications/tailash.desktop` 后重跑刷新命令。

## 为什么不用 clash-verge-rev

clash-verge 的 TUN 开启时会吞掉所有进程的出站流量——包括 tailscaled 自己的，
导致 MagicDNS 解析、tailnet 互通出现随机故障。tailash 的配置生成器在 TUN 路由层
在 TUN 路由配置中排除 tailnet 地址段，并让 MagicDNS 域名绕过 fake-IP。该配置不等价于绕过 tailscaled 到公网 DERP/peer/control endpoint 的 underlay；该路径仍需按本机路由验证。

## 架构

```
┌─────────────┐   UiCmd (mpsc)   ┌──────────────┐
│  gpui GUI   │ ───────────────► │ worker 线程   │
│  (ui.rs)    │ ◄─────────────── │  (bridge.rs) │
└─────────────┘  SharedSnapshot  └──────┬───────┘
                                        │ unix socket（轮询 /configs）
                                        ▼
                                 ┌────────────┐
                                 │ mihomo 内核 │──► Meta TUN
                                 │ (用户自启)  │    cap_net_admin
                                 └────────────┘
```

- **不重写协议栈**：vmess/anytls/hysteria2 全部复用 mihomo 内核（数据面）。
- **GUI 不提权**：`cap_net_admin` 只授予内核二进制，GUI 以普通用户跑。
- **内核独立于 GUI**：GUI 每秒探测控制 socket 展示「运行中/未运行」，连上后提供 TUN、模式、选节点、
  测延迟、订阅热重载。关闭 GUI 不影响内核。
- **启停内核（systemd 用户服务）**：主界面「启动内核/停止内核」按钮经 `systemctl --user` 控制
  `~/.config/systemd/user/tailash-mihomo.service`（每次启动都按找到的 mihomo 与数据目录重写单元）；
  内核作为独立服务后台运行，崩了会自动拉起（`Restart=on-failure`），日志 `journalctl --user -u tailash-mihomo`。
  systemd 用户实例不在线时按钮隐藏，仍按终端方式启停。终端手动启动的内核照样能被探测显示。
- **状态流**：UI 发命令 → worker 操作内核 → worker 写 `Arc<RwLock<Snapshot>>` → UI 500ms 轮询刷新。
- **运行模式**：支持策略（rule）、直连（direct）、全局（global），运行中通过 mihomo API 切换。

## 使用

```bash
# 0. tailash 不内置 mihomo 内核。Arch 直接 paru -S mihomo（GeoIP 数据 clash-geoip
#    会作为依赖自动装好）；其它发行版从 https://github.com/MetaCubeX/mihomo/releases 下载，
#    命名为 mihomo、加可执行权限后放进 PATH（如 ~/.local/bin/mihomo）。
#    源码运行时项目的 bin/mihomo 优先。找不到内核时「启动内核」会报错提示。
# 1. 内核需要 TUN 能力（首次安装、升级或替换内核文件后设置；也可在界面点「授权 TUN」）
sudo setcap cap_net_admin=ep "$(command -v mihomo)"
getcap "$(command -v mihomo)"   # 预期输出含 cap_net_admin=ep

# 2. 关掉其他代理客户端的 TUN（双 TUN 会打架）

# 3. 跑起来 GUI
cargo run --release

# 4. GUI 里：填托管订阅地址 → 点「更新订阅」（生成 ~/.local/share/tailash/config.yaml）

# 5. 点「启动内核」（经 systemd 用户服务后台运行，日志 journalctl --user -u tailash-mihomo）
#    也仍可在终端手动启动：mihomo -d ~/.local/share/tailash -f ~/.local/share/tailash/config.yaml
# 停止内核：GUI 点「停止内核」，或在终端 Ctrl-C / pkill -x mihomo
```

订阅走托管：在主界面「配置托管」栏填订阅 URL（首次启动会预填上一次保存的地址），
点「更新订阅」下载；内核运行中则热重载，未运行则只更新 `config.yaml`，下次启动内核生效。
GUI 每次启动也会按当前订阅重写 `config.yaml`。控制 socket 固定为 `~/.local/share/tailash/mihomo.sock`，
不设 API secret（目录权限 0700 已限制为当前用户）。订阅保存在 `~/.local/share/tailash/settings.json`，
下载内容校验后原子写入 `~/.local/share/tailash/profile.yaml`；
运行数据与生成配置也在 `~/.local/share/tailash/`（目录权限限制为当前用户）。不依赖 Clash Verge。

tailash 不内置 mihomo 内核和 GeoIP 数据：Arch 直接 `paru -S mihomo`，内核装到 `/usr/bin/mihomo`，
GeoIP（`clash-geoip`，Country.mmdb）作为其依赖自动装到 `/etc/clash/Country.mmdb`，两者一步到位。
其它发行版需自行从 https://github.com/MetaCubeX/mihomo/releases 下载内核；GeoIP 不用管，
mihomo 启动时若运行目录 `~/.local/share/tailash/` 里没有 `geoip.metadb` 会自行下载。

### Mihomo 的 TUN 权限

GUI 以普通用户运行；只用 `sudo setcap` 给实际运行的 Mihomo 二进制授予 `CAP_NET_ADMIN`，用于创建 TUN 和配置路由。源码运行或 rsync 同步后，在项目目录执行：

```bash
sudo setcap cap_net_admin=ep /home/guozr/CODE/tailash/bin/mihomo
getcap /home/guozr/CODE/tailash/bin/mihomo
# 预期输出：/home/guozr/CODE/tailash/bin/mihomo cap_net_admin=ep
```

用 `paru -S mihomo` 安装的是 `/usr/bin/mihomo`，自行下载的常放在 `~/.local/bin/mihomo`，将上面的路径换成该文件。可用 `pgrep -a -x mihomo` 确认正在使用哪个内核。`rsync -av` 和包管理器升级都不保留文件 capability，更新或替换内核后应重新检查并授权，再重启内核。只重新编译安装 `tailash` 不会给 Mihomo 授权，也不需要用 sudo 启动 GUI。

### firewalld 放行 TUN 接口

使用 firewalld 的机器还需放行 Mihomo 的 TUN 接口。arch-blue 实测：内核权限、节点和 DNS 均正常，显式 HTTP 代理能访问 Google，但 TUN 连接超时；将 `Meta` 接口加入 `trusted` 后恢复访问。

先在终端启动内核，用 `ip -br link` 确认接口名。以下命令适用于名为 `Meta` 的 TUN 接口：

```bash
# 当前会话立即生效
sudo firewall-cmd --zone=trusted --add-interface=Meta

# 验证流量经 TUN 访问外网，预期 HTTP 200
curl --noproxy '*' --max-time 15 \
  -o /dev/null -w 'HTTP %{http_code}\n' https://www.google.com

# 测试成功后保存，重启或重载防火墙后仍生效
sudo firewall-cmd --permanent --zone=trusted --add-interface=Meta
```

两条添加命令分别修改运行配置和永久配置；都执行后无需再 reload。`trusted` 会信任来自该接口的流量，只给 TUN 接口设置，不要将 `wlan0` 等物理接口加入。若修改 TUN 接口名，请同步调整规则。不使用 firewalld 的机器跳过此步骤。

撤销运行配置和永久配置：

```bash
sudo firewall-cmd --zone=trusted --remove-interface=Meta
sudo firewall-cmd --permanent --zone=trusted --remove-interface=Meta
```

## 与 Tailscale 共存的原理

生成的配置含三层防护（`src/config_gen.rs`）：

1. `tun.route-exclude-address` 合并保留订阅原有值，并加入 `100.64.0.0/10` 与 `fd7a:115c:a1e0::/48`。这是 Mihomo TUN 的地址路由设置，不保证所有发行版/路由拓扑下都直接转发到 `tailscale0`。
2. `dns.nameserver-policy["+.ts.net"] = 100.100.100.100`，并把 Tailscale 域名加入 fake-IP 排除列表；MagicDNS 可达性仍依赖本机 Tailscale DNS 和启动状态。
3. 规则中的 `DIRECT` 只决定 Mihomo 出站选择，不负责系统路由。tailscaled 使用公网 endpoint 时，必须另外确认该 underlay 没有被送入 Mihomo TUN；tailash 当前不做进程级绕过或自动验证系统路由。
4. 启动和运行时状态只按 Mihomo `/configs` API 的 `tun.enable` 显示，不代表 TUN 接口、系统路由、MagicDNS 或 tailnet peer 已端到端验证。

## 测试

```bash
cargo test                # 单元测试（配置生成、API 解析）
cargo test --test smoke -- --ignored --nocapture # 真实内核：生成配置、探测运行、节点、测速命令、探测退出
```

TUN 实测记录（2026-11-18，见 git log）：MagicDNS 返回真实 tailnet IP（非 fake-ip）、
`tailscale ping` 24ms、TCP peer:22 通、Google 200 经代理、Baidu 200 DIRECT。

## 模块

| 文件 | 职责 |
|---|---|
| `src/config_gen.rs` | 订阅 yaml → mihomo 配置（TUN + Tailscale 排除） |
| `src/mihomo_api.rs` | 手写 HTTP/1.1 over UnixStream 控制 API |
| `src/runtime.rs` | 约定路径（config.yaml / mihomo.sock）、生成配置、拉节点视图、systemd 用户服务启停、pkexec 授权 cap_net_admin |
| `src/bridge.rs` | worker 线程：轮询内核状态（CoreLink）+ 命令处理 + 并发测延迟 |
| `src/ui.rs` | gpui 界面：内核状态、TUN/模式、节点列表、延迟显示、配置托管 |
| `src/subscription.rs` | 托管订阅：URL 下载（curl）→ 校验 → 原子落盘；settings.json 持久化 |
| ~~`src/core.rs`~~ | 已删除：mihomo 子进程托管与看门狗由 systemd 用户服务取代 |
| `src/main.rs` | 装配：数据目录、订阅预填、窗口 |
