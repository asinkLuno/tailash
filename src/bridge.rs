//! UI(单线程) <-> worker(std::thread) 桥。
//! 方向 1：UiCmd 经 std mpsc 到 worker。
//! 方向 2：worker 把状态快照放进 Share（RwLock），UI 每次渲染前读快照 + 定时器触发重绘。
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

#[derive(Debug, Clone)]
pub enum UiCmd {
    /// 经 systemd 用户服务启动/停止内核（不弹权限窗，详见 runtime::service_start）。
    ToggleCore,
    /// mihomo 缺 cap_net_admin 时经 pkexec 补授权（唯一弹系统授权窗的操作）。
    GrantCap,
    Shutdown,
    ToggleTun(bool),
    SetMode(crate::mihomo_api::ProxyMode),
    /// 在某个组里选中某个节点（组 = 组名，不是"节点选择"硬编码）
    Select {
        group: String,
        node: String,
    },
    TestAll,
    /// 拉取托管订阅（url 空则用 settings.json 里保存的）
    UpdateSub {
        url: String,
    },
    /// 下载线程 → worker：订阅落盘结果（Ok = proxy 数）
    UpdateSubDone(Result<usize, String>),
}

/// worker -> UI 的可见结果
#[derive(Default, Clone, PartialEq)]
pub struct Snapshot {
    pub core_running: bool,
    /// 内核二进制缺 cap_net_admin（getcap 探测），true 时 UI 显示「授权 TUN」按钮
    pub cap_missing: bool,
    /// systemd 用户实例在线（启动时探测一次）；false 时 UI 不显示启停按钮
    pub systemd_available: bool,
    pub mode: crate::mihomo_api::ProxyMode,
    pub tun_enabled: bool,
    /// 节点/组的展示模型（顺序来自订阅，见 proxy_view）
    pub view: crate::proxy_view::ProxyView,
    pub message: String,
    pub failed_delays: Vec<String>,
}

impl Snapshot {
    pub fn is_failed(&self, name: &str) -> bool {
        self.failed_delays.iter().any(|failed| failed == name)
    }
}

pub type SharedSnapshot = Arc<RwLock<Snapshot>>;

pub fn spawn_worker(
    snapshot: SharedSnapshot,
    rx: std::sync::mpsc::Receiver<UiCmd>,
    tx: std::sync::mpsc::Sender<UiCmd>,
    cfg: crate::runtime::RuntimeCfg,
) -> std::thread::JoinHandle<()> {
    let updating_sub = Arc::new(AtomicBool::new(false));
    std::thread::spawn(move || {
        let mut link = CoreLink::new(cfg.client());
        let delay_running = Arc::new(AtomicBool::new(false));
        let mut delay_cancel: Option<Arc<AtomicBool>> = None;
        // getcap 有子进程开销，只在启动时探测一次，之后仅 GrantCap 成功后翻回 false。
        // 找不到内核时不显示授权按钮；缺内核的提示在点「启动内核」时给出。
        let cap_missing = crate::runtime::find_mihomo()
            .is_ok_and(|binary| !crate::runtime::has_cap_net_admin(&binary));
        let systemd_available = crate::runtime::systemd_available();
        set_snap(&snapshot, |snap| {
            snap.cap_missing = cap_missing;
            snap.systemd_available = systemd_available;
        });
        link.poll(&cfg, &snapshot, &mut delay_cancel);
        // 按当前订阅刷新 config.yaml，让接下来启动的内核用上最新订阅。
        if cfg.profile_path.is_file()
            && let Err(e) = crate::runtime::write_config(&cfg)
        {
            set_snap(&snapshot, |snap| {
                snap.message = format!("生成内核配置失败: {e:#}")
            });
        }
        loop {
            let cmd = match rx.recv_timeout(std::time::Duration::from_secs(1)) {
                Ok(cmd) => cmd,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    link.poll(&cfg, &snapshot, &mut delay_cancel);
                    continue;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            };
            if matches!(cmd, UiCmd::Shutdown) {
                if let Some(cancel) = delay_cancel.take() {
                    cancel.store(true, Ordering::Relaxed);
                }
                break;
            }
            let core = link.live();
            match cmd {
                UiCmd::Shutdown => unreachable!(),
                UiCmd::ToggleCore => {
                    if crate::runtime::service_active() {
                        match crate::runtime::service_stop() {
                            Ok(()) => set_snap(&snapshot, |snap| {
                                snap.message = "正在停止内核…".into();
                            }),
                            Err(e) => set_snap(&snapshot, |snap| {
                                snap.message = format!("停止内核失败: {e:#}")
                            }),
                        }
                    } else {
                        match crate::runtime::service_start(&cfg.workdir) {
                            Ok(()) => set_snap(&snapshot, |snap| {
                                snap.message = "正在启动内核…".into();
                            }),
                            Err(e) => set_snap(&snapshot, |snap| {
                                snap.message = format!("启动内核失败: {e:#}")
                            }),
                        }
                    }
                    // 启停是异步的，等下一轮 poll 反映真实状态。
                    link.poll(&cfg, &snapshot, &mut delay_cancel);
                }
                UiCmd::GrantCap => {
                    // pkexec 要等用户在授权窗里操作，放到线程里，别卡住 poll 与其它命令。
                    set_snap(&snapshot, |snap| snap.message = "等待授权…".into());
                    let snapshot = snapshot.clone();
                    std::thread::spawn(move || {
                        let granted = crate::runtime::find_mihomo()
                            .and_then(|binary| crate::runtime::grant_cap_net_admin(&binary));
                        set_snap(&snapshot, |snap| match granted {
                            Ok(()) => {
                                snap.cap_missing = false;
                                snap.message =
                                    "已授权；若内核正在运行，停止后重新启动即可生效".into();
                            }
                            Err(e) => snap.message = format!("授权失败: {e:#}"),
                        });
                    });
                }
                UiCmd::ToggleTun(on) => {
                    if let Some(client) = &core {
                        match client.set_tun(on) {
                            Ok(_) => set_snap(&snapshot, |snap| {
                                snap.tun_enabled = on;
                                snap.message = if on { "TUN 已开启" } else { "TUN 已关闭" }.into();
                            }),
                            Err(e) => {
                                let msg = format!("TUN 切换失败: {e}");
                                set_snap(&snapshot, |snap| snap.message = msg);
                            }
                        }
                    } else {
                        set_snap(&snapshot, |snap| snap.message = NOT_RUNNING.into());
                    }
                }
                UiCmd::SetMode(mode) => {
                    if let Some(c) = &core {
                        match c.set_mode(mode) {
                            Ok(()) => set_snap(&snapshot, |snap| {
                                snap.mode = mode;
                                snap.message = format!("已切换到{}模式", mode.label());
                            }),
                            Err(e) => set_snap(&snapshot, |snap| {
                                snap.message = format!("模式切换失败: {e}")
                            }),
                        }
                    } else {
                        set_snap(&snapshot, |snap| snap.message = NOT_RUNNING.into());
                    }
                }
                UiCmd::Select { group, node } => {
                    if let Some(client) = &core {
                        if let Err(e) = client.select(&group, &node) {
                            let msg = format!("切换失败: {e}");
                            set_snap(&snapshot, |snap| snap.message = msg);
                        } else {
                            set_snap(&snapshot, |snap| {
                                if let Some(g) =
                                    snap.view.groups.iter_mut().find(|g| g.name == group)
                                {
                                    g.now = Some(node.clone());
                                }
                                snap.message = "已切换".into();
                            });
                        }
                    }
                }
                UiCmd::UpdateSubDone(Ok(count)) => {
                    updating_sub.store(false, Ordering::Release);
                    if let Some(c) = &core {
                        match reload_core(c, &cfg) {
                            Ok(view) => set_snap(&snapshot, |snap| {
                                snap.view = view;
                                snap.failed_delays.clear();
                                snap.message = format!("订阅已更新，{count} 个节点已重载");
                            }),
                            Err(e) => set_snap(&snapshot, |snap| {
                                snap.message = format!("订阅已下载，但重载失败: {e}");
                            }),
                        }
                    } else {
                        let staged = cfg.workdir.join(crate::subscription::PROFILE_STAGED);
                        let committed =
                            crate::subscription::atomic_replace(&staged, &cfg.profile_path)
                                .and_then(|()| crate::runtime::write_config(&cfg));
                        set_snap(&snapshot, |snap| {
                            snap.message = match committed {
                                Ok(()) => format!("订阅已更新，{count} 个节点；启动内核后生效"),
                                Err(e) => format!("订阅已下载，但保存失败: {e:#}"),
                            };
                        });
                    }
                }
                UiCmd::UpdateSubDone(Err(e)) => {
                    updating_sub.store(false, Ordering::Release);
                    set_snap(&snapshot, |snap| {
                        snap.message = format!("订阅更新失败: {e}");
                    });
                }
                UiCmd::UpdateSub { url } => {
                    if updating_sub.swap(true, Ordering::Relaxed) {
                        set_snap(&snapshot, |snap| snap.message = "订阅更新已在进行".into());
                        continue;
                    }
                    set_snap(&snapshot, |snap| snap.message = "正在下载订阅…".into());
                    launch_subscription_update(tx.clone(), cfg.workdir.clone(), url);
                }
                UiCmd::TestAll => {
                    if let Some(c) = &core {
                        if delay_running.swap(true, Ordering::Relaxed) {
                            set_snap(&snapshot, |snap| snap.message = "测延迟已在进行".into());
                            continue;
                        }
                        let names = snapshot
                            .read()
                            .unwrap()
                            .view
                            .nodes
                            .iter()
                            .map(|n| n.name.clone())
                            .collect();
                        let cancel = Arc::new(AtomicBool::new(false));
                        delay_cancel = Some(cancel.clone());
                        set_snap(&snapshot, |snap| {
                            snap.message = "测延迟中…".into();
                            snap.failed_delays.clear();
                        });
                        launch_delay_test(
                            c.clone(),
                            snapshot.clone(),
                            names,
                            cancel,
                            delay_running.clone(),
                        );
                    } else {
                        set_snap(&snapshot, |snap| {
                            snap.message = "内核未运行，无法测延迟".into()
                        });
                    }
                }
            }
        }
    })
}

fn delay_summary(succeeded: usize, failed: usize) -> String {
    format!("测速完成：成功 {succeeded} 个，失败 {failed} 个")
}

#[cfg(test)]
mod delay_summary_tests {
    use super::delay_summary;

    #[test]
    fn reports_success_and_failure_counts() {
        assert_eq!(delay_summary(3, 29), "测速完成：成功 3 个，失败 29 个");
    }
}

fn launch_delay_test(
    client: crate::mihomo_api::MihomoClient,
    snapshot: SharedSnapshot,
    names: Vec<String>,
    cancel: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
) {
    let queue = Arc::new(Mutex::new(VecDeque::from(names)));
    let remaining_workers = Arc::new(AtomicUsize::new(8));
    let succeeded = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicUsize::new(0));
    for _ in 0..8 {
        let client = client.clone();
        let snapshot = snapshot.clone();
        let cancel = cancel.clone();
        let queue = queue.clone();
        let running = running.clone();
        let remaining_workers = remaining_workers.clone();
        let succeeded = succeeded.clone();
        let failed = failed.clone();
        std::thread::spawn(move || {
            loop {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let name = queue.lock().unwrap().pop_front();
                let Some(name) = name else { break };
                match client.delay(
                    &name,
                    crate::proxy_view::DEFAULT_TIMEOUT_MS,
                    crate::runtime::TEST_URL,
                ) {
                    Ok(delay) => {
                        succeeded.fetch_add(1, Ordering::Relaxed);
                        set_snap(&snapshot, |snap| {
                            snap.view.set_delay(&name, Some(delay.delay));
                            snap.failed_delays.retain(|failed| failed != &name);
                        });
                    }
                    Err(_) => {
                        failed.fetch_add(1, Ordering::Relaxed);
                        set_snap(&snapshot, |snap| {
                            snap.view.set_delay(&name, None);
                            if !snap.is_failed(&name) {
                                snap.failed_delays.push(name.clone());
                            }
                        });
                    }
                }
            }
            if remaining_workers.fetch_sub(1, Ordering::AcqRel) == 1 {
                running.store(false, Ordering::Release);
                if !cancel.load(Ordering::Relaxed) {
                    set_snap(&snapshot, |snap| {
                        snap.message = delay_summary(
                            succeeded.load(Ordering::Relaxed),
                            failed.load(Ordering::Relaxed),
                        );
                    });
                }
            }
        });
    }
}

fn launch_subscription_update(
    tx: std::sync::mpsc::Sender<UiCmd>,
    data_dir: std::path::PathBuf,
    url: String,
) {
    use anyhow::Context as _;
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<usize> {
            let url = resolve_url(&url, &data_dir)?;
            let staged_profile = data_dir.join(crate::subscription::PROFILE_STAGED);
            let text = crate::subscription::stage_profile_file(&url, &staged_profile)?;

            // 保存 URL 到 settings.json，供下次启动预填
            let settings = crate::subscription::Settings {
                subscribe_url: url.clone(),
            };
            if let Err(e) = crate::subscription::save_settings(&data_dir, &settings) {
                eprintln!("保存设置失败: {e}");
            }

            // 数 proxy 数作为结果反馈（subscription 层已校验过非空）
            let profile: serde_yaml::Value =
                serde_yaml::from_str(&text).context("解析已落盘订阅失败")?;
            let count = profile
                .get("proxies")
                .and_then(|v| v.as_sequence())
                .map(|s| s.len())
                .unwrap_or(0);
            Ok(count)
        })();
        // 无论成败都回执 worker，由它统一收尾（更新 UI 状态 / 释放进行中标志）
        let _ = tx.send(UiCmd::UpdateSubDone(result.map_err(|e| format!("{e:#}"))));
    });
}

/// 空 url → settings.json 里保存的；都没有 → 报错
fn resolve_url(url: &str, data_dir: &std::path::Path) -> anyhow::Result<String> {
    let url = url.trim();
    if !url.is_empty() {
        return Ok(url.to_string());
    }
    let saved = crate::subscription::load_settings(data_dir).subscribe_url;
    anyhow::ensure!(!saved.trim().is_empty(), "请先填写订阅地址");
    Ok(saved)
}

/// 热重载候选订阅，只有内核接受后才提交为活动 profile/config。
fn reload_core(
    client: &crate::mihomo_api::MihomoClient,
    cfg: &crate::runtime::RuntimeCfg,
) -> anyhow::Result<crate::proxy_view::ProxyView> {
    use anyhow::Context as _;
    let staged_profile = cfg.workdir.join(crate::subscription::PROFILE_STAGED);
    let staged_config = cfg.workdir.join(crate::runtime::CONFIG_STAGED);
    let profile_text = std::fs::read_to_string(&staged_profile).context("读候选 profile 失败")?;
    let profile: serde_yaml::Value =
        serde_yaml::from_str(&profile_text).context("解析候选 profile 失败")?;
    let tun = client
        .configs()?
        .get("tun")
        .and_then(|tun| tun.get("enable"))
        .and_then(serde_json::Value::as_bool)
        .context("读取当前 Mihomo TUN 状态失败")?;
    let config_text = crate::runtime::render_config(cfg, &profile, Some(tun))?;
    std::fs::write(&staged_config, &config_text).context("写候选 config 失败")?;
    client.reload(staged_config.to_string_lossy().as_ref())?;

    crate::subscription::atomic_replace(&staged_profile, &cfg.profile_path)?;
    crate::subscription::atomic_replace(&staged_config, &cfg.config_path())?;

    // 用新 profile 的顺序（不是旧的 cfg.profile_path 缓存）
    let view = crate::proxy_view::build(&profile, client.proxies()?);
    Ok(view)
}

const NOT_RUNNING: &str = "内核未运行";

/// GUI 与用户在命令行启动的 mihomo 之间的连接状态。
///
/// tailash 不持有内核进程，只能靠轮询 API 判断它在不在：`GET /configs` 成功即视为运行中。
struct CoreLink {
    client: crate::mihomo_api::MihomoClient,
    /// None = 还没探测过；用来让"未运行"提示只在状态变化时写一次，不覆盖用户操作的反馈。
    connected: Option<bool>,
}

impl CoreLink {
    fn new(client: crate::mihomo_api::MihomoClient) -> Self {
        Self {
            client,
            connected: None,
        }
    }

    fn live(&self) -> Option<crate::mihomo_api::MihomoClient> {
        (self.connected == Some(true)).then(|| self.client.clone())
    }

    fn poll(
        &mut self,
        cfg: &crate::runtime::RuntimeCfg,
        snapshot: &SharedSnapshot,
        delay_cancel: &mut Option<Arc<AtomicBool>>,
    ) {
        let config = match self.client.configs() {
            Ok(config) => config,
            Err(_) => {
                if self.connected != Some(false) {
                    if let Some(cancel) = delay_cancel.take() {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    let start = if snapshot.read().is_ok_and(|s| s.systemd_available) {
                        "点「启动内核」".to_string()
                    } else {
                        format!("在终端运行 {}", cfg.start_command())
                    };
                    let message = if !cfg.profile_path.is_file() {
                        format!("{NOT_RUNNING}；请先更新订阅生成配置，再启动内核")
                    } else if self.connected.is_none() {
                        format!("{NOT_RUNNING}，请{start}")
                    } else {
                        format!("内核已退出，可{start}重新启动")
                    };
                    mark_core_stopped(snapshot, &message);
                    self.connected = Some(false);
                }
                return;
            }
        };
        let tun = config["tun"]["enable"].as_bool().unwrap_or(false);
        let mode = config["mode"]
            .as_str()
            .and_then(crate::mihomo_api::ProxyMode::parse);
        if self.connected == Some(true) {
            set_snap(snapshot, |snap| {
                snap.tun_enabled = tun;
                if let Some(mode) = mode {
                    snap.mode = mode;
                }
            });
            return;
        }
        // 刚连上：拉一次节点视图。失败就留到下次轮询重试。
        match crate::runtime::load_view(&self.client, &cfg.profile_path) {
            Ok(view) => {
                set_snap(snapshot, |snap| {
                    snap.core_running = true;
                    snap.tun_enabled = tun;
                    if let Some(mode) = mode {
                        snap.mode = mode;
                    }
                    snap.view = view;
                    snap.failed_delays.clear();
                    snap.message = "已连接内核".into();
                });
                self.connected = Some(true);
            }
            Err(e) => set_snap(snapshot, |snap| {
                snap.message = format!("已连上内核，但读取节点失败: {e:#}");
            }),
        }
    }
}

fn mark_core_stopped(snapshot: &SharedSnapshot, message: &str) {
    set_snap(snapshot, |snap| {
        snap.core_running = false;
        snap.tun_enabled = false;
        snap.view = Default::default();
        snap.failed_delays.clear();
        snap.message = message.into();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unexpected_exit_clears_running_state() {
        let snapshot = SharedSnapshot::default();
        set_snap(&snapshot, |s| {
            s.core_running = true;
            s.tun_enabled = true;
            s.view.nodes.push(crate::proxy_view::NodeInfo {
                name: "node".into(),
                kind: "Vmess".into(),
                delay: None,
                udp: false,
            });
            s.failed_delays.push("node".into());
        });

        mark_core_stopped(&snapshot, "内核意外退出");
        let snapshot = snapshot.read().unwrap();
        assert!(!snapshot.core_running);
        assert!(!snapshot.tun_enabled);
        assert!(snapshot.view.nodes.is_empty());
        assert!(snapshot.failed_delays.is_empty());
        assert_eq!(snapshot.message, "内核意外退出");
    }
}

fn set_snap(s: &SharedSnapshot, f: impl FnOnce(&mut Snapshot)) {
    let mut w = s.write().unwrap();
    f(&mut w);
}
