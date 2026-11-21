//! tailash Tauri 版装配层：数据目录、单实例锁、worker 与快照事件。
//! 业务逻辑全部在 backend/（自 gpui 分支迁移），本文件只做壳。
pub mod backend;
mod commands;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;

use tauri::Emitter;
use tauri::Manager;
use tauri::RunEvent;

use backend::bridge;
use backend::bridge::SharedSnapshot;
use backend::bridge::Snapshot;
use backend::bridge::UiCmd;
use backend::runtime::RuntimeCfg;
use commands::AppState;

#[cfg(unix)]
pub struct InstanceLock(std::fs::File);

#[cfg(unix)]
impl InstanceLock {
    fn acquire(path: &std::path::Path) -> anyhow::Result<Self> {
        use std::os::fd::AsRawFd;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        anyhow::ensure!(result == 0, "tailash 已在运行，不能启动第二个实例");
        Ok(Self(file))
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

/// XDG data home；fallback ~/.local/share
fn dirs_home() -> PathBuf {
    std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let h = std::env::var("HOME").unwrap();
            PathBuf::from(h).join(".local/share")
        })
}

/// gpui 版的 500ms 渲染轮询 → Tauri 事件：快照变化才 emit，前端 listen 即可。
fn spawn_snapshot_emitter(snapshot: SharedSnapshot, handle: tauri::AppHandle) {
    std::thread::spawn(move || {
        let mut last = snapshot.read().unwrap().clone();
        let _ = handle.emit("snapshot", &last);
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let current = snapshot.read().unwrap().clone();
            if current != last {
                let _ = handle.emit("snapshot", &current);
                last = current;
            }
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = dirs_home().join("tailash");
            std::fs::create_dir_all(&data_dir).expect("创建 tailash 数据目录失败");
            let instance_lock = InstanceLock::acquire(&data_dir.join("tailash.lock"))
                .expect("tailash 已在运行或无法创建单实例锁");
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700))
                    .expect("设置数据目录权限失败");
            }

            // 启动预填：上次保存的托管订阅地址；预填失败不阻塞启动。
            let saved_url = backend::subscription::load_settings(&data_dir).subscribe_url;
            let cfg = RuntimeCfg {
                profile_path: data_dir.join("profile.yaml"),
                workdir: data_dir.clone(),
            };
            let snapshot: SharedSnapshot = Arc::new(RwLock::new(Snapshot {
                message: "正在连接内核…".into(),
                ..Default::default()
            }));

            let (tx, rx) = std::sync::mpsc::channel::<UiCmd>();
            let worker = bridge::spawn_worker(snapshot.clone(), rx, tx.clone(), cfg);
            spawn_snapshot_emitter(snapshot.clone(), app.handle().clone());

            app.manage(AppState {
                tx,
                snapshot,
                worker: Mutex::new(Some(worker)),
                saved_url,
                _instance_lock: instance_lock,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::send_cmd,
            commands::get_snapshot,
            commands::get_saved_sub_url
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| match event {
            // 退出请求（窗口全关等）：通知 worker 收尾（取消测速线程等）。
            RunEvent::ExitRequested { .. } => {
                let _ = app.state::<AppState>().tx.send(UiCmd::Shutdown);
            }
            RunEvent::Exit => {
                let state = app.state::<AppState>();
                if let Some(worker) = state.worker.lock().unwrap().take() {
                    let _ = worker.join();
                }
            }
            _ => {}
        });
}
