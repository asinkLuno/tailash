<div align="center">

<img src="logo/app-icon.svg" alt="tailash 图标" width="96"/>

# tailash

Linux 桌面端的 mihomo 代理客户端 —— 订阅管理、节点测延迟、TUN 模式，与 Tailscale 和平共存。

![平台](https://img.shields.io/badge/platform-Linux-blue)
![构建](https://img.shields.io/badge/Tauri%202%20·%20React%2019%20·%20daisyUI%205-技术栈-orange)

</div>

## 特性

- **多订阅管理** —— 添加 / 更新 / 切换 / 删除，下载后校验再原子落盘，订阅信息持久化
- **节点列表** —— 按订阅顺序、延迟或名称排序，一键测延迟
- **内核控制** —— 拉起 / 停止 mihomo（优先 systemd 用户服务，无 systemd 时自动回退脱离会话的后台进程），规则 / 直连 / 全局三种运行模式
- **实时网速** —— 订阅 mihomo `/traffic` 流，顶栏常驻上下行速率，内核页展示累计流量与 60 秒双线折线图
- **TUN 模式** —— 一键授权 `cap_net_admin`，虚拟网卡接管全局路由，自动排除 Tailscale 网段
- **克制的设计** —— 单实例锁、XDG 数据目录、系统 curl 下载（不引入 TLS 依赖）

## 安装

### 1. 安装 tailash

从 [Releases](https://github.com/asinkluno/tailash/releases) 下载安装包（文件名以实际版本为准，以下以 v0.1.1 为例）：

**Debian / Ubuntu**

```sh
sudo apt install ./tailash_0.1.1_amd64.deb
```

**AppImage**（免安装，下载后直接运行）

```sh
chmod +x tailash_0.1.1_amd64.AppImage
./tailash_0.1.1_amd64.AppImage
```

**Arch Linux**

```sh
sudo pacman -U tailash-0.1.1-1-x86_64.pkg.tar.zst
```

### 2. 安装 mihomo 内核

tailash 不内置内核，请自行从 [mihomo Releases](https://github.com/MetaCubeX/mihomo/releases) 下载对应架构的压缩包（如 `mihomo-linux-amd64-*.gz`），放到 `PATH` 里：

```sh
gzip -d mihomo-linux-amd64-*.gz
chmod +x mihomo-linux-amd64-*
sudo install mihomo-linux-amd64-* /usr/local/bin/mihomo

mihomo -v   # 能打印版本号即安装成功
```

Arch Linux 用户也可以用 AUR 助手直接安装：

```sh
yay -S mihomo    # 或 paru -S mihomo
```

开发时也可以把 mihomo 二进制放到源码目录 `bin/` 下，tailash 会优先使用它。

## 开发

```sh
yarn install
yarn tauri dev      # 开发调试
yarn tauri build    # 构建安装包
```

## 技术栈

Tauri 2 · React 19 · TypeScript · Tailwind CSS 4 · daisyUI 5 · Rust（业务逻辑在 `src-tauri/src/backend/`）
