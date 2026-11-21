//! Tauri IPC 胶水：前端命令 → bridge::UiCmd；快照经 `snapshot` 事件推给前端（亦可 get_snapshot 拉取）。
//! 前端事件/命令字段均为 camelCase（serde rename_all）。
use serde::Deserialize;
use tauri::State;

use crate::backend::bridge::Snapshot;
use crate::backend::bridge::SharedSnapshot;
use crate::backend::bridge::UiCmd;
use crate::backend::mihomo_api::ProxyMode;

/// App 级状态：命令发送端、共享快照、worker 句柄（退出时收尾）与单实例锁。
pub struct AppState {
    pub tx: std::sync::mpsc::Sender<UiCmd>,
    pub snapshot: SharedSnapshot,
    pub worker: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
    /// flock 单实例锁存进状态，让它活到进程退出（drop 即解锁）。
    pub _instance_lock: crate::InstanceLock,
}

/// 前端可发的命令。Shutdown / UpdateSubDone 属于 worker 内部协议，不对前端暴露。
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Cmd {
    /// 经 systemd 用户服务启动/停止内核
    ToggleCore,
    /// mihomo 缺 cap_net_admin 时经 pkexec 补授权
    GrantCap,
    ToggleTun { on: bool },
    SetMode { mode: ProxyMode },
    /// 在某个组里选中某个节点
    Select { group: String, node: String },
    TestAll,
    /// 测单个节点延迟
    TestNode { name: String },
    /// 更新某条订阅（id 空则更新当前活动订阅）
    UpdateSub { id: String },
    /// 添加订阅（下载校验通过后登记；第一条自动启用）
    AddSub { name: String, url: String },
    /// 删除某条订阅
    RemoveSub { id: String },
    /// 切换活动订阅
    SwitchSub { id: String },
}

impl From<Cmd> for UiCmd {
    fn from(cmd: Cmd) -> Self {
        match cmd {
            Cmd::ToggleCore => Self::ToggleCore,
            Cmd::GrantCap => Self::GrantCap,
            Cmd::ToggleTun { on } => Self::ToggleTun(on),
            Cmd::SetMode { mode } => Self::SetMode(mode),
            Cmd::Select { group, node } => Self::Select { group, node },
            Cmd::TestAll => Self::TestAll,
            Cmd::TestNode { name } => Self::TestNode { name },
            Cmd::UpdateSub { id } => Self::UpdateSub { id },
            Cmd::AddSub { name, url } => Self::AddSub { name, url },
            Cmd::RemoveSub { id } => Self::RemoveSub { id },
            Cmd::SwitchSub { id } => Self::SwitchSub { id },
        }
    }
}

/// 前端发一条控制命令；结果异步体现在快照的 message / 状态字段上。
#[tauri::command]
pub fn send_cmd(state: State<'_, AppState>, cmd: Cmd) {
    let _ = state.tx.send(cmd.into());
}

/// 初始快照；之后靠 `snapshot` 事件增量更新。
#[tauri::command]
pub fn get_snapshot(state: State<'_, AppState>) -> Snapshot {
    state.snapshot.read().unwrap().clone()
}
