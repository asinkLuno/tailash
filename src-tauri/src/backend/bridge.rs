//! UI(单线程) <-> worker(std::thread) 桥。
//! 方向 1：UiCmd 经 std mpsc 到 worker。
//! 方向 2：worker 把状态快照放进 Share（RwLock），UI 每次渲染前读快照 + 定时器触发重绘。
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

#[derive(Debug, Clone)]
pub enum UiCmd {
    /// 启动/停止内核（优先 systemd 用户服务，无 systemd 时回退到脱离会话的后台进程）。
    ToggleCore,
    /// mihomo 缺 cap_net_admin 时经 pkexec 补授权（唯一弹系统授权窗的操作）。
    GrantCap,
    Shutdown,
    ToggleTun(bool),
    SetMode(crate::backend::mihomo_api::ProxyMode),
    /// 在某个组里选中某个节点（组 = 组名，不是"节点选择"硬编码）
    Select {
        group: String,
        node: String,
    },
    TestAll,
    /// 测单个节点延迟
    TestNode { name: String },
    /// 拉取某条订阅（id 空则更新当前活动订阅）
    UpdateSub { id: String },
    /// 添加订阅（下载校验通过后由 worker 登记）
    AddSub { name: String, url: String },
    /// 删除某条订阅（连同其内容文件）
    RemoveSub { id: String },
    /// 切换活动订阅（内核在跑则热重载，否则物化 profile 供下次启动）
    SwitchSub { id: String },
    /// 下载线程 → worker：订阅内容已下载并校验，由 worker 统一落盘/改 settings。
    /// id 为 None 表示新增订阅（此时 name/url 必有）；Ok 携带 profile 文本。
    SubFetched {
        id: Option<String>,
        name: Option<String>,
        url: Option<String>,
        result: Result<String, String>,
    },
}

/// 订阅列表的展示条目（settings.json 的 SubMeta 的前端视图，camelCase）
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubStatus {
    pub id: String,
    pub name: String,
    pub url: String,
    pub active: bool,
    pub node_count: Option<usize>,
    /// 最近一次更新成功的 unix 秒
    pub updated_at: Option<u64>,
}

/// 网速折线图历史点（单位：字节/秒）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficPoint {
    pub up: u64,
    pub down: u64,
}

const TRAFFIC_HISTORY_LEN: usize = 60;

/// worker -> UI 的可见结果（经 Tauri 事件/命令序列化给前端，camelCase）
#[derive(Default, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub core_running: bool,
    /// 内核二进制缺 cap_net_admin（getcap 探测），true 时 UI 显示「授权 TUN」按钮
    pub cap_missing: bool,
    /// systemd 用户实例在线（启动时探测一次）；false 时回退到直接后台进程管理
    pub systemd_available: bool,
    pub mode: crate::backend::mihomo_api::ProxyMode,
    pub tun_enabled: bool,
    /// 实时网速与累计流量（来自 mihomo `/traffic`）
    pub traffic: crate::backend::mihomo_api::TrafficSample,
    /// 最近 60 秒的网速历史，供前端折线图绘制
    pub traffic_history: Vec<TrafficPoint>,
    /// 节点/组的展示模型（顺序来自订阅，见 proxy_view）
    pub view: crate::backend::proxy_view::ProxyView,
    /// 全部订阅（settings.json 的前端视图）
    pub subs: Vec<SubStatus>,
    /// 单点测速进行中的节点名
    pub testing: Vec<String>,
    /// 「测全部」进行中（UI 据此灰掉测速按钮）
    pub test_all_running: bool,
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
    cfg: crate::backend::runtime::RuntimeCfg,
) -> std::thread::JoinHandle<()> {
    let updating_sub = Arc::new(AtomicBool::new(false));
    std::thread::spawn(move || {
        let mut link = CoreLink::new(cfg.client());
        let delay_running = Arc::new(AtomicBool::new(false));
        let mut delay_cancel: Option<Arc<AtomicBool>> = None;
        // getcap 有子进程开销，只在启动时探测一次，之后仅 GrantCap 成功后翻回 false。
        // 找不到内核时不显示授权按钮；缺内核的提示在点「启动内核」时给出。
        let cap_missing = crate::backend::runtime::find_mihomo()
            .is_ok_and(|binary| !crate::backend::runtime::has_cap_net_admin(&binary));
        let core_manager = crate::backend::runtime::CoreManager::detect();
        let systemd_available = core_manager.uses_systemd();
        set_snap(&snapshot, |snap| {
            snap.cap_missing = cap_missing;
            snap.systemd_available = systemd_available;
        });
            // 订阅登记：读 settings.json（旧版单订阅自动迁移成一条订阅）；
            // 活动订阅的内容还没物化成 profile.yaml 就补上，保证接下来启动的内核用得上。
            let mut settings = {
                let raw = crate::backend::subscription::load_settings(&cfg.workdir);
                let migrated =
                    crate::backend::subscription::migrate_legacy(raw.clone(), &cfg.workdir);
                if migrated != raw
                    && let Err(e) =
                        crate::backend::subscription::save_settings(&cfg.workdir, &migrated)
                {
                    eprintln!("保存迁移后的订阅设置失败: {e}");
                }
                migrated
            };
            if let Some(active) = settings.active.clone()
                && let Some(text) =
                    crate::backend::subscription::load_sub_profile(&cfg.workdir, &active)
                && !cfg.profile_path.is_file()
            {
                let _ = std::fs::write(&cfg.profile_path, &text);
            }
            set_snap(&snapshot, |snap| snap.subs = sub_status_list(&settings));
            link.poll(&cfg, &snapshot, &mut delay_cancel);
        // 按当前订阅刷新 config.yaml，让接下来启动的内核用上最新订阅。
        if cfg.profile_path.is_file()
            && let Err(e) = crate::backend::runtime::write_config(&cfg)
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
                link.stop_traffic();
                if let Some(cancel) = delay_cancel.take() {
                    cancel.store(true, Ordering::Relaxed);
                }
                break;
            }
            let core = link.live();
            match cmd {
                UiCmd::Shutdown => unreachable!(),
                UiCmd::ToggleCore => {
                    if core_manager.is_active(&cfg) {
                        match core_manager.stop(&cfg) {
                            Ok(()) => set_snap(&snapshot, |snap| {
                                snap.message = "正在停止内核…".into();
                            }),
                            Err(e) => set_snap(&snapshot, |snap| {
                                snap.message = format!("停止内核失败: {e:#}")
                            }),
                        }
                    } else {
                        match core_manager.start(&cfg) {
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
                    let cfg = cfg.clone();
                    std::thread::spawn(move || {
                        let granted = crate::backend::runtime::find_mihomo().and_then(|binary| {
                            crate::backend::runtime::grant_cap_net_admin(&binary)
                        });
                        // 已经在跑的内核拿不到新授权（file caps 只在 exec 时生效），重启一次让它带上
                        let restarted = granted.is_ok()
                            && core_manager.is_active(&cfg)
                            && core_manager.stop(&cfg).is_ok()
                            && core_manager.start(&cfg).is_ok();
                        set_snap(&snapshot, |snap| match granted {
                            Ok(()) => {
                                snap.cap_missing = false;
                                snap.message = if restarted {
                                    "已授权并重启内核，TUN 现在可用".into()
                                } else {
                                    "已授权；若内核正在运行，停止后重新启动即可生效".into()
                                };
                            }
                            Err(e) => snap.message = format!("授权失败: {e:#}"),
                        });
                    });
                }
                UiCmd::ToggleTun(on) => {
                    if let Some(client) = &core {
                        match client.set_tun(on) {
                            Ok(_) => {
                                // 建卡是内核侧的同步动作，失败只写日志不回 HTTP，所以立刻看实际网卡
                                let up = !on || crate::backend::runtime::tun_interface_up(None);
                                set_snap(&snapshot, |snap| {
                                    snap.tun_enabled = on && up;
                                    snap.message = match (on, up) {
                                        (false, _) => "TUN 已关闭".into(),
                                        (true, true) => "TUN 已开启".into(),
                                        (true, false) => {
                                            "TUN 开启失败：内核可能还没有 cap_net_admin，重启内核后重试"
                                                .into()
                                        }
                                    };
                                });
                            }
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
                UiCmd::SubFetched {
                    id,
                    name,
                    url,
                    result,
                } => {
                    updating_sub.store(false, Ordering::Release);
                    let adding = id.is_none();
                    match result {
                        Ok(text) => {
                            let outcome = apply_fetched_sub(
                                &mut settings,
                                &cfg.workdir,
                                id.as_deref(),
                                name.as_deref(),
                                url.as_deref(),
                                &text,
                            );
                            let (count, activate) = match outcome {
                                Ok(pair) => pair,
                                Err(e) => {
                                    let prefix = if adding { "订阅添加失败" } else { "订阅更新失败" };
                                    set_snap(&snapshot, |snap| {
                                        snap.message = format!("{prefix}: {e:#}")
                                    });
                                    continue;
                                }
                            };
                            // 活动订阅的内容变了：内核在跑 → 热重载；否则物化 profile + 重写 config
                            let mut view = None;
                            let mut committed = Ok(());
                            if activate {
                                match activate_profile_text(core.as_ref(), &cfg, &text) {
                                    Ok(v) => view = v,
                                    Err(e) => committed = Err(e),
                                }
                            }
                            set_snap(&snapshot, |snap| {
                                snap.subs = sub_status_list(&settings);
                                if let Some(v) = view {
                                    snap.view = v;
                                    snap.failed_delays.clear();
                                }
                                let prefix = if adding { "订阅已添加" } else { "订阅已更新" };
                                snap.message = match (&committed, activate, core.is_some()) {
                                    (Err(e), _, _) => format!("{prefix}，但重载失败: {e:#}"),
                                    (_, false, _) => prefix.into(),
                                    (_, true, true) => format!("{prefix}，{count} 个节点已重载"),
                                    (_, true, false) => {
                                        format!("{prefix}，{count} 个节点；启动内核后生效")
                                    }
                                };
                            });
                        }
                        Err(e) => set_snap(&snapshot, |snap| {
                            let prefix = if adding { "订阅添加失败" } else { "订阅更新失败" };
                            snap.message = format!("{prefix}: {e}");
                        }),
                    }
                }
                UiCmd::UpdateSub { id } => {
                    let sub_id = if id.trim().is_empty() {
                        settings.active.clone()
                    } else {
                        Some(id)
                    };
                    let Some(sub_id) = sub_id else {
                        set_snap(&snapshot, |snap| snap.message = "还没有订阅，请先添加".into());
                        continue;
                    };
                    let Some(meta) = settings.subscriptions.iter().find(|s| s.id == sub_id) else {
                        set_snap(&snapshot, |snap| snap.message = "订阅不存在".into());
                        continue;
                    };
                    let url = meta.url.clone();
                    if updating_sub.swap(true, Ordering::Relaxed) {
                        set_snap(&snapshot, |snap| snap.message = "订阅更新已在进行".into());
                        continue;
                    }
                    set_snap(&snapshot, |snap| snap.message = "正在下载订阅…".into());
                    launch_sub_fetch(tx.clone(), Some(sub_id), None, url);
                }
                UiCmd::AddSub { name, url } => {
                    if updating_sub.swap(true, Ordering::Relaxed) {
                        set_snap(&snapshot, |snap| snap.message = "订阅更新已在进行".into());
                        continue;
                    }
                    set_snap(&snapshot, |snap| snap.message = "正在添加订阅…".into());
                    launch_sub_fetch(tx.clone(), None, Some(name), url);
                }
                UiCmd::RemoveSub { id } => {
                    if !crate::backend::subscription::valid_id(&id) {
                        set_snap(&snapshot, |snap| snap.message = "无效的订阅标识".into());
                        continue;
                    }
                    crate::backend::subscription::remove_sub_profile(&cfg.workdir, &id);
                    settings.subscriptions.retain(|s| s.id != id);
                    if settings.active.as_deref() == Some(id.as_str()) {
                        settings.active = None;
                    }
                    if let Err(e) = crate::backend::subscription::save_settings(&cfg.workdir, &settings) {
                        eprintln!("保存订阅设置失败: {e}");
                    }
                    set_snap(&snapshot, |snap| {
                        snap.subs = sub_status_list(&settings);
                        snap.message = "订阅已删除".into();
                    });
                }
                UiCmd::SwitchSub { id } => {
                    let meta = settings.subscriptions.iter().find(|s| s.id == id).map(|s| s.name.clone());
                    let Some(name) = meta else {
                        set_snap(&snapshot, |snap| snap.message = "订阅不存在".into());
                        continue;
                    };
                    if settings.active.as_deref() == Some(id.as_str()) {
                        set_snap(&snapshot, |snap| snap.message = "已是当前订阅".into());
                        continue;
                    }
                    let Some(text) =
                        crate::backend::subscription::load_sub_profile(&cfg.workdir, &id)
                    else {
                        set_snap(&snapshot, |snap| {
                            snap.message = "该订阅还没有内容，请先更新再切换".into();
                        });
                        continue;
                    };
                    match activate_profile_text(core.as_ref(), &cfg, &text) {
                        Ok(view) => {
                            settings.active = Some(id.clone());
                            if let Err(e) =
                                crate::backend::subscription::save_settings(&cfg.workdir, &settings)
                            {
                                eprintln!("保存订阅设置失败: {e}");
                            }
                            set_snap(&snapshot, |snap| {
                                snap.subs = sub_status_list(&settings);
                                if let Some(v) = view {
                                    snap.view = v;
                                    snap.failed_delays.clear();
                                }
                                snap.message = format!("已切换到「{name}」");
                            });
                        }
                        Err(e) => set_snap(&snapshot, |snap| {
                            snap.message = format!("切换失败: {e:#}");
                        }),
                    }
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
            snap.test_all_running = true;
            // 点击测全部即清空旧延迟：测试期间未出结果的节点显示「未测」
            for node in &mut snap.view.nodes {
                node.delay = None;
            }
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
                UiCmd::TestNode { name } => {
                    if let Some(c) = &core {
                        if snapshot.read().unwrap().testing.contains(&name) {
                            set_snap(&snapshot, |snap| {
                                snap.message = "该节点测延迟已在进行".into();
                            });
                            continue;
                        }
                        set_snap(&snapshot, |snap| {
                            snap.testing.push(name.clone());
                            snap.message = "测延迟中…".into();
                        });
                        launch_delay_test_single(c.clone(), snapshot.clone(), name);
                    } else {
                        set_snap(&snapshot, |snap| snap.message = NOT_RUNNING.into());
                    }
                }
            }
        }
    })
}

fn delay_summary(succeeded: usize, failed: usize) -> String {
    format!("测速完成：成功 {succeeded} 个，失败 {failed} 个")
}

/// 测速完成后回拉一次 /proxies 同步各组的 now：
/// url-test 组在内核里按成员实时延迟动态选最小，个别节点测完它的选择就可能变。
fn refresh_group_now(
    client: &crate::backend::mihomo_api::MihomoClient,
    snapshot: &SharedSnapshot,
) {
    if let Ok(entries) = client.proxies() {
        set_snap(snapshot, |snap| {
            for group in &mut snap.view.groups {
                if let Some(entry) = entries.iter().find(|e| e.name == group.name) {
                    group.now = entry.now.clone();
                }
            }
        });
    }
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
    client: crate::backend::mihomo_api::MihomoClient,
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
                    crate::backend::proxy_view::DEFAULT_TIMEOUT_MS,
                    crate::backend::runtime::TEST_URL,
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
                            // 失败写入错误哨兵值：UI 分档显示「失败」（区别于未测），排序在超时后、未测前
                            snap.view
                                .set_delay(&name, Some(crate::backend::proxy_view::IMPLAUSIBLE_DELAY));
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
                    // url-test 组的 fastNode 只在组自身测试时按 tolerance 重算，
                    // 外部单点测速不会触发；强制内核重测一遍，让它的选中跟上最新延迟。
                    let urltest_groups: Vec<String> = snapshot
                        .read()
                        .unwrap()
                        .view
                        .groups
                        .iter()
                        .filter(|group| {
                            group.kind.replace('-', "").eq_ignore_ascii_case("urltest")
                        })
                        .map(|group| group.name.clone())
                        .collect();
                    for name in &urltest_groups {
                        let _ = client.group_delay(
                            name,
                            crate::backend::proxy_view::DEFAULT_TIMEOUT_MS,
                            crate::backend::runtime::TEST_URL,
                        );
                    }
                    refresh_group_now(&client, &snapshot);
                    set_snap(&snapshot, |snap| {
                        snap.message = delay_summary(
                            succeeded.load(Ordering::Relaxed),
                            failed.load(Ordering::Relaxed),
                        );
                    });
                }
                set_snap(&snapshot, |snap| snap.test_all_running = false);
            }
        });
    }
}

/// 只做网络下载 + 校验；落盘/登记等所有状态变更留给 worker，避免并发写 settings.json。
fn launch_sub_fetch(
    tx: std::sync::mpsc::Sender<UiCmd>,
    id: Option<String>,
    name: Option<String>,
    url: String,
) {
    std::thread::spawn(move || {
        use anyhow::Context as _;
        let result = (|| -> anyhow::Result<String> {
            let text = crate::backend::subscription::fetch_subscribe(&url)?;
            crate::backend::subscription::validate_profile_text(&text)
                .context("校验订阅失败")?;
            Ok(text)
        })()
        .map_err(|e| format!("{e:#}"));
        // 无论成败都回执 worker，由它统一收尾（更新 UI 状态 / 释放进行中标志）
        let _ = tx.send(UiCmd::SubFetched {
            id,
            name,
            url: Some(url),
            result,
        });
    });
}

/// 单个节点的延迟测试；结果直接写回快照（含 testing 列表摘除）。
fn launch_delay_test_single(
    client: crate::backend::mihomo_api::MihomoClient,
    snapshot: SharedSnapshot,
    name: String,
) {
    std::thread::spawn(move || {
        let result = client.delay(
            &name,
            crate::backend::proxy_view::DEFAULT_TIMEOUT_MS,
            crate::backend::runtime::TEST_URL,
        );
        set_snap(&snapshot, |snap| {
            snap.testing.retain(|testing| testing != &name);
            match &result {
                Ok(delay) => {
                    snap.view.set_delay(&name, Some(delay.delay));
                    snap.failed_delays.retain(|failed| failed != &name);
                    snap.message = format!("{name}: {}ms", delay.delay);
                }
                Err(_) => {
                    // 失败写入错误哨兵值：UI 分档显示「失败」（区别于未测）
                    snap.view
                        .set_delay(&name, Some(crate::backend::proxy_view::IMPLAUSIBLE_DELAY));
                    if !snap.is_failed(&name) {
                        snap.failed_delays.push(name.clone());
                    }
                    snap.message = format!("{name}: 测速失败");
                }
            }
        });
        refresh_group_now(&client, &snapshot);
    });
}

/// settings 的订阅列表 → 快照展示条目（带 active 标记）。
fn sub_status_list(settings: &crate::backend::subscription::Settings) -> Vec<SubStatus> {
    settings
        .subscriptions
        .iter()
        .map(|s| SubStatus {
            id: s.id.clone(),
            name: s.name.clone(),
            url: s.url.clone(),
            active: settings.active.as_deref() == Some(s.id.as_str()),
            node_count: s.node_count,
            updated_at: s.updated_at,
        })
        .collect()
}

/// 数 profile 文本里的节点数（校验已保证 proxies 非空）。
fn count_proxies(text: &str) -> anyhow::Result<usize> {
    use anyhow::Context as _;
    let profile: serde_yaml::Value =
        serde_yaml::from_str(text).context("解析订阅失败")?;
    Ok(profile
        .get("proxies")
        .and_then(|v| v.as_sequence())
        .map(|s| s.len())
        .unwrap_or(0))
}

/// SubFetched(Ok) 的落盘与登记：写 subs/<id>.yaml、更新元数据、保存 settings。
/// 返回 (节点数, 是否需要把该内容提交为活动 profile)：
/// 新增时还没有活动订阅 → 是；更新当前活动订阅 → 是；其余否。
fn apply_fetched_sub(
    settings: &mut crate::backend::subscription::Settings,
    data_dir: &std::path::Path,
    id: Option<&str>,
    name: Option<&str>,
    url: Option<&str>,
    text: &str,
) -> anyhow::Result<(usize, bool)> {
    let count = count_proxies(text)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let sub_id = match (id, name, url) {
        (None, Some(name), Some(url)) => {
            let id = crate::backend::subscription::new_sub_id();
            settings
                .subscriptions
                .push(crate::backend::subscription::SubMeta {
                    id: id.clone(),
                    name: name.to_string(),
                    url: url.to_string(),
                    node_count: Some(count),
                    updated_at: Some(now),
                });
            id
        }
        (Some(id), _, _) => {
            anyhow::ensure!(
                settings.subscriptions.iter().any(|m| m.id == id),
                "订阅 {id} 不存在"
            );
            let meta = settings
                .subscriptions
                .iter_mut()
                .find(|m| m.id == id)
                .expect("上面已确认存在");
            meta.node_count = Some(count);
            meta.updated_at = Some(now);
            id.to_string()
        }
        _ => anyhow::bail!("新增订阅缺少 name/url"),
    };
    crate::backend::subscription::save_sub_profile(data_dir, &sub_id, text)?;
    if settings.active.is_none() && id.is_none() {
        settings.active = Some(sub_id.clone());
    }
    let activate = settings.active.as_deref() == Some(sub_id.as_str());
    crate::backend::subscription::save_settings(data_dir, settings)?;
    Ok((count, activate))
}

/// 把一段订阅文本提交为活动 profile：内核在跑 → stage + 热重载；否则原子替换
/// profile.yaml 并按新订阅重写 config.yaml（下次启动生效）。
fn activate_profile_text(
    core: Option<&crate::backend::mihomo_api::MihomoClient>,
    cfg: &crate::backend::runtime::RuntimeCfg,
    text: &str,
) -> anyhow::Result<Option<crate::backend::proxy_view::ProxyView>> {
    use anyhow::Context as _;
    let staged = cfg
        .workdir
        .join(crate::backend::subscription::PROFILE_STAGED);
    std::fs::write(&staged, text).context("写候选订阅失败")?;
    match core {
        Some(client) => Ok(Some(reload_core(client, cfg)?)),
        None => {
            crate::backend::subscription::atomic_replace(&staged, &cfg.profile_path)?;
            crate::backend::runtime::write_config(cfg)?;
            Ok(None)
        }
    }
}

/// 热重载候选订阅，只有内核接受后才提交为活动 profile/config。
fn reload_core(
    client: &crate::backend::mihomo_api::MihomoClient,
    cfg: &crate::backend::runtime::RuntimeCfg,
) -> anyhow::Result<crate::backend::proxy_view::ProxyView> {
    use anyhow::Context as _;
    let staged_profile = cfg
        .workdir
        .join(crate::backend::subscription::PROFILE_STAGED);
    let staged_config = cfg.workdir.join(crate::backend::runtime::CONFIG_STAGED);
    let profile_text = std::fs::read_to_string(&staged_profile).context("读候选 profile 失败")?;
    let profile: serde_yaml::Value =
        serde_yaml::from_str(&profile_text).context("解析候选 profile 失败")?;
    let tun = client
        .configs()?
        .get("tun")
        .and_then(|tun| tun.get("enable"))
        .and_then(serde_json::Value::as_bool)
        .context("读取当前 Mihomo TUN 状态失败")?;
    let config_text = crate::backend::runtime::render_config(cfg, &profile, Some(tun))?;
    std::fs::write(&staged_config, &config_text).context("写候选 config 失败")?;
    commit_reload(
        &staged_profile,
        &cfg.profile_path,
        &staged_config,
        &cfg.config_path(),
        |path| client.reload(path.to_string_lossy().as_ref()),
    )?;

    // 用新 profile 的顺序（不是旧的 cfg.profile_path 缓存）
    let view = crate::backend::proxy_view::build(&profile, client.proxies()?);
    Ok(view)
}

fn commit_reload(
    staged_profile: &std::path::Path,
    profile: &std::path::Path,
    staged_config: &std::path::Path,
    config: &std::path::Path,
    reload: impl Fn(&std::path::Path) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    anyhow::ensure!(config.is_file(), "缺少当前配置，不能安全热重载");
    let backup = profile.with_extension("yaml.rollback");
    // Keep the original inode until both renames succeed. Never overwrite a recovery copy.
    let saved = match std::fs::hard_link(profile, &backup) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e).context("备份当前订阅失败"),
    };
    let mut installed = false;
    let result = (|| -> anyhow::Result<()> {
        crate::backend::subscription::atomic_replace(staged_profile, profile)?;
        installed = true;
        reload(staged_config)?;
        crate::backend::subscription::atomic_replace(staged_config, config)
    })();
    if installed && let Err(error) = result {
        let disk = if saved {
            std::fs::rename(&backup, profile)
        } else {
            std::fs::remove_file(profile)
        };
        let core = reload(config);
        if disk.is_err() || core.is_err() {
            anyhow::bail!("更新失败: {error:#}；恢复订阅: {disk:?}；恢复内核: {core:?}");
        }
        return Err(error);
    }
    if saved {
        std::fs::remove_file(backup).context("清理订阅备份失败")?;
    }
    result
}
const NOT_RUNNING: &str = "内核未运行";

/// GUI 与用户在命令行启动的 mihomo 之间的连接状态。
///
/// tailash 不持有内核进程，只能靠轮询 API 判断它在不在：`GET /configs` 成功即视为运行中。
struct CoreLink {
    client: crate::backend::mihomo_api::MihomoClient,
    /// None = 还没探测过；用来让"未运行"提示只在状态变化时写一次，不覆盖用户操作的反馈。
    connected: Option<bool>,
    traffic_cancel: Option<Arc<AtomicBool>>,
}

impl CoreLink {
    fn new(client: crate::backend::mihomo_api::MihomoClient) -> Self {
        Self {
            client,
            connected: None,
            traffic_cancel: None,
        }
    }

    fn stop_traffic(&mut self) {
        if let Some(cancel) = self.traffic_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    fn start_traffic(&mut self, snapshot: &SharedSnapshot) {
        self.stop_traffic();
        let cancel = Arc::new(AtomicBool::new(false));
        self.traffic_cancel = Some(cancel.clone());
        spawn_traffic_stream(self.client.clone(), snapshot.clone(), cancel);
    }

    fn live(&self) -> Option<crate::backend::mihomo_api::MihomoClient> {
        (self.connected == Some(true)).then(|| self.client.clone())
    }

    fn poll(
        &mut self,
        cfg: &crate::backend::runtime::RuntimeCfg,
        snapshot: &SharedSnapshot,
        delay_cancel: &mut Option<Arc<AtomicBool>>,
    ) {
        let config = match self.client.configs() {
            Ok(config) => config,
            Err(_) => {
                if self.connected != Some(false) {
                    self.stop_traffic();
                    if let Some(cancel) = delay_cancel.take() {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    let start = "点「启动内核」";
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
        // enable 只是期望值，建卡失败只写日志，得配上网卡是否真的存在
        let tun = config["tun"]["enable"].as_bool().unwrap_or(false)
            && crate::backend::runtime::tun_interface_up(config["tun"]["device"].as_str());
        let mode = config["mode"]
            .as_str()
            .and_then(crate::backend::mihomo_api::ProxyMode::parse);
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
        match crate::backend::runtime::load_view(&self.client, &cfg.profile_path) {
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
                self.start_traffic(snapshot);
            }
            Err(e) => set_snap(snapshot, |snap| {
                snap.message = format!("已连上内核，但读取节点失败: {e:#}");
            }),
        }
    }
}

fn push_traffic_sample(
    snap: &mut Snapshot,
    mut sample: crate::backend::mihomo_api::TrafficSample,
) {
    // 兼容旧版内核：未提供 upTotal/downTotal 时自行累计。
    if sample.up_total == 0 && sample.down_total == 0 {
        sample.up_total = snap.traffic.up_total.saturating_add(sample.up);
        sample.down_total = snap.traffic.down_total.saturating_add(sample.down);
    }
    snap.traffic = sample;
    if snap.traffic_history.len() >= TRAFFIC_HISTORY_LEN {
        let drop_count = snap.traffic_history.len() + 1 - TRAFFIC_HISTORY_LEN;
        snap.traffic_history.drain(..drop_count);
    }
    snap.traffic_history.push(TrafficPoint {
        up: sample.up,
        down: sample.down,
    });
}

fn spawn_traffic_stream(
    client: crate::backend::mihomo_api::MihomoClient,
    snapshot: SharedSnapshot,
    cancel: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        while !cancel.load(Ordering::Relaxed) {
            let _ = client.stream_traffic(&cancel, |sample| {
                if !cancel.load(Ordering::Relaxed) {
                    set_snap(&snapshot, |snap| {
                        if snap.core_running {
                            push_traffic_sample(snap, sample);
                        }
                    });
                }
            });
            for _ in 0..10 {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    });
}

fn mark_core_stopped(snapshot: &SharedSnapshot, message: &str) {
    set_snap(snapshot, |snap| {
        snap.core_running = false;
        snap.tun_enabled = false;
        snap.traffic = Default::default();
        snap.traffic_history.clear();
        snap.view = Default::default();
        snap.testing.clear();
        snap.test_all_running = false;
        snap.failed_delays.clear();
        snap.message = message.into();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_rename_failure_restores_profile_and_reloads_old_config() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("tailash-rollback-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let profile = dir.join("profile.yaml");
        let config = dir.join("config.yaml");
        let pending = dir.join("profile.pending");
        let candidate = dir.join("config.pending");
        fs::write(&profile, "old profile").unwrap();
        fs::write(&pending, "new profile").unwrap();
        fs::write(&config, "old config").unwrap();
        fs::write(&candidate, "new config").unwrap();
        let loaded = std::cell::RefCell::new(Vec::new());
        let result = commit_reload(&pending, &profile, &candidate, &config, |path| {
            loaded.borrow_mut().push(fs::read_to_string(path)?);
            if path == candidate {
                fs::remove_file(path)?;
            }
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(loaded.into_inner(), ["new config", "old config"]);
        assert_eq!(fs::read_to_string(profile).unwrap(), "old profile");
        assert_eq!(fs::read_to_string(config).unwrap(), "old config");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn unexpected_exit_clears_running_state() {
        let snapshot = SharedSnapshot::default();
        set_snap(&snapshot, |s| {
            s.core_running = true;
            s.tun_enabled = true;
            s.test_all_running = true;
            s.view.nodes.push(crate::backend::proxy_view::NodeInfo {
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
        assert!(!snapshot.test_all_running);
        assert!(snapshot.view.nodes.is_empty());
        assert!(snapshot.failed_delays.is_empty());
        assert_eq!(snapshot.message, "内核意外退出");
    }

    fn sample_text(nodes: usize) -> String {
        let items: Vec<String> = (0..nodes)
            .map(|i| format!("  - {{name: n{i}, type: vmess}}"))
            .collect();
        format!("proxies:\n{}\n", items.join("\n"))
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tailash-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn first_added_sub_becomes_active_and_activates() {
        let dir = temp_dir("apply-first");
        let mut settings = crate::backend::subscription::Settings::default();
        let (count, activate) = apply_fetched_sub(
            &mut settings,
            &dir,
            None,
            Some("机场A"),
            Some("https://a.example/sub"),
            &sample_text(2),
        )
        .unwrap();
        assert_eq!(count, 2);
        assert!(activate, "第一条订阅应当自动成为活动订阅");
        assert_eq!(settings.subscriptions.len(), 1);
        assert_eq!(settings.subscriptions[0].name, "机场A");
        assert_eq!(settings.subscriptions[0].node_count, Some(2));
        let id = settings.active.clone().unwrap();
        assert_eq!(
            crate::backend::subscription::load_sub_profile(&dir, &id).is_some(),
            true,
            "订阅内容应落盘"
        );
        assert_eq!(
            crate::backend::subscription::load_settings(&dir).active,
            Some(id),
            "settings.json 应保存活动订阅"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn second_added_sub_does_not_steal_activation() {
        let dir = temp_dir("apply-second");
        let mut settings = crate::backend::subscription::Settings::default();
        apply_fetched_sub(&mut settings, &dir, None, Some("A"), Some("https://a"), &sample_text(1))
            .unwrap();
        let first_active = settings.active.clone();
        let (_, activate) = apply_fetched_sub(
            &mut settings,
            &dir,
            None,
            Some("B"),
            Some("https://b"),
            &sample_text(1),
        )
        .unwrap();
        assert!(!activate);
        assert_eq!(settings.active, first_active, "已有活动订阅时不切换");
        assert_eq!(settings.subscriptions.len(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn updating_active_sub_updates_meta_and_activates() {
        let dir = temp_dir("apply-update");
        let mut settings = crate::backend::subscription::Settings::default();
        apply_fetched_sub(&mut settings, &dir, None, Some("A"), Some("https://a"), &sample_text(1))
            .unwrap();
        let id = settings.active.clone().unwrap();
        let (count, activate) = apply_fetched_sub(
            &mut settings,
            &dir,
            Some(&id),
            None,
            None,
            &sample_text(3),
        )
        .unwrap();
        assert_eq!(count, 3);
        assert!(activate, "更新活动订阅应触发提交");
        assert_eq!(settings.subscriptions[0].node_count, Some(3));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn updating_non_active_sub_does_not_activate() {
        let dir = temp_dir("apply-inactive");
        let mut settings = crate::backend::subscription::Settings::default();
        apply_fetched_sub(&mut settings, &dir, None, Some("A"), Some("https://a"), &sample_text(1))
            .unwrap();
        apply_fetched_sub(&mut settings, &dir, None, Some("B"), Some("https://b"), &sample_text(1))
            .unwrap();
        let second = settings.subscriptions[1].id.clone();
        settings.active = Some(second.clone());
        let first = settings.subscriptions[0].id.clone();
        let (_, activate) = apply_fetched_sub(
            &mut settings,
            &dir,
            Some(&first),
            None,
            None,
            &sample_text(2),
        )
        .unwrap();
        assert!(!activate, "更新非活动订阅不触发提交");
        assert_eq!(
            settings.active.as_deref(),
            Some(second.as_str()),
            "活动订阅不应被改动"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn apply_fetched_sub_rejects_unknown_id_and_missing_name() {
        let dir = temp_dir("apply-bad");
        let mut settings = crate::backend::subscription::Settings::default();
        assert!(apply_fetched_sub(&mut settings, &dir, Some("ghost"), None, None, &sample_text(1)).is_err());
        assert!(
            apply_fetched_sub(&mut settings, &dir, None, None, None, &sample_text(1)).is_err(),
            "新增订阅缺 name/url 应报错"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sub_status_list_marks_active_and_carries_meta() {
        let settings = crate::backend::subscription::Settings {
            subscriptions: vec![
                crate::backend::subscription::SubMeta {
                    id: "s1".into(),
                    name: "A".into(),
                    url: "https://a".into(),
                    node_count: Some(7),
                    updated_at: Some(42),
                },
                crate::backend::subscription::SubMeta {
                    id: "s2".into(),
                    name: "B".into(),
                    url: "https://b".into(),
                    node_count: None,
                    updated_at: None,
                },
            ],
            active: Some("s2".into()),
            subscribe_url: String::new(),
        };
        let subs = sub_status_list(&settings);
        assert_eq!(subs.len(), 2);
        assert!(!subs[0].active);
        assert!(subs[1].active);
        assert_eq!(subs[0].node_count, Some(7));
        assert_eq!(subs[1].updated_at, None);
    }

    #[test]
    fn push_traffic_sample_caps_history_and_accumulates_missing_totals() {
        let mut snap = Snapshot::default();
        for i in 1..=(TRAFFIC_HISTORY_LEN as u64 + 5) {
            push_traffic_sample(
                &mut snap,
                crate::backend::mihomo_api::TrafficSample {
                    up: i,
                    down: i * 10,
                    up_total: 0,
                    down_total: 0,
                },
            );
        }
        assert_eq!(snap.traffic_history.len(), TRAFFIC_HISTORY_LEN);
        assert_eq!(snap.traffic_history[0], TrafficPoint { up: 6, down: 60 });
        assert_eq!(
            snap.traffic_history[TRAFFIC_HISTORY_LEN - 1],
            TrafficPoint { up: 65, down: 650 }
        );
        let expected_sum: u64 = (1..=65).sum();
        assert_eq!(snap.traffic.up_total, expected_sum);
        assert_eq!(snap.traffic.down_total, expected_sum * 10);

        // 内核上报了明确累计值时以内核为准
        push_traffic_sample(
            &mut snap,
            crate::backend::mihomo_api::TrafficSample {
                up: 100,
                down: 200,
                up_total: 9999,
                down_total: 88888,
            },
        );
        assert_eq!(snap.traffic.up_total, 9999);
        assert_eq!(snap.traffic.down_total, 88888);
    }
}

fn set_snap(s: &SharedSnapshot, f: impl FnOnce(&mut Snapshot)) {
    let mut w = s.write().unwrap();
    f(&mut w);
}
