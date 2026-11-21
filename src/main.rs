use gpui_kit::*;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

#[cfg(unix)]
struct InstanceLock(std::fs::File);

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

#[cfg(unix)]
impl Drop for InstanceLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn main() {
    let data_dir = dirs_home().join("tailash");
    let workdir = data_dir.clone();
    std::fs::create_dir_all(&workdir).expect("创建 tailash 数据目录失败");
    #[cfg(unix)]
    let instance_lock = InstanceLock::acquire(&workdir.join("tailash.lock"))
        .expect("tailash 已在运行或无法创建单实例锁");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&workdir, std::fs::Permissions::from_mode(0o700))
            .expect("设置数据目录权限失败");
    }
    let cfg = tailash::runtime::RuntimeCfg {
        profile_path: profile_path(&data_dir),
        workdir: workdir.clone(),
    };

    let snapshot: tailash::bridge::SharedSnapshot =
        Arc::new(std::sync::RwLock::new(tailash::bridge::Snapshot {
            message: "正在连接内核…".into(),
            ..Default::default()
        }));

    // 启动预填：上次保存的托管订阅地址；预填失败不阻塞启动。
    let saved_url = tailash::subscription::load_settings(&data_dir).subscribe_url;

    let (tx, rx) = std::sync::mpsc::channel::<tailash::bridge::UiCmd>();
    let mut worker = Some(tailash::bridge::spawn_worker(
        snapshot.clone(),
        rx,
        tx.clone(),
        cfg,
    ));

    gpui_kit::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);
        let _instance_lock = &instance_lock;
        let _worker = &worker;
        // detach：让 quit handler 存活到 App 退出。存局部变量会在 launch 闭包结束时
        // drop 并注销 handler，导致关窗时 worker（及测速线程）无人收尾。
        cx.on_app_quit({
            let tx = tx.clone();
            let mut worker = worker.take();
            move |_| {
                let tx = tx.clone();
                let worker = worker.take();
                async move {
                    let _ = tx.send(tailash::bridge::UiCmd::Shutdown);
                    if let Some(worker) = worker {
                        let _ = worker.join();
                    }
                }
            }
        })
        .detach();
        let root =
            cx.new(|_| tailash::ui::Ui::new(snapshot.clone(), Rc::new(tx), saved_url.clone()));
        let _ = open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(560.), px(640.)),
                    cx,
                ))),
                ..Default::default()
            },
            cx,
            |_, _| root.clone(),
        );
        tailash::ui::start_polling(&root, cx);
    });
}

fn profile_path(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("profile.yaml")
}

fn dirs_home() -> PathBuf {
    // XDG data home；fallback ~/.local/share
    std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let h = std::env::var("HOME").unwrap();
            PathBuf::from(h).join(".local/share")
        })
}
