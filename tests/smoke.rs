//! 端到端冒烟（不需要 GUI）：worker 生成 config.yaml -> 测试代替用户启动 mihomo ->
//! worker 探测到运行中、节点可见 -> 选节点/测延迟 -> 杀掉内核 -> worker 探测到已退出。
//! 需要真实 mihomo 二进制（bin/mihomo）；无它则跳过。
#[cfg(test)]
mod smoke {
    use tailash::bridge::UiCmd;
    use tailash::bridge::spawn_worker;
    use tailash::runtime::RuntimeCfg;

    fn find_mihomo() -> Option<std::path::PathBuf> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bin/mihomo");
        p.exists().then_some(p)
    }

    fn wait_until(
        snapshot: &tailash::bridge::SharedSnapshot,
        timeout: std::time::Duration,
        cond: impl Fn(&tailash::bridge::Snapshot) -> bool,
    ) -> bool {
        let begin = std::time::Instant::now();
        while begin.elapsed() < timeout {
            if cond(&snapshot.read().unwrap()) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        cond(&snapshot.read().unwrap())
    }

    #[test]
    #[ignore = "launches the real mihomo core and may modify host networking"]
    fn smoke_core_status() {
        let Some(mihomo) = find_mihomo() else {
            eprintln!("skip: no bin/mihomo");
            return;
        };
        let workdir = std::env::temp_dir().join(format!(
            "tailash-smoke-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&workdir).expect("create temporary workdir");
        let profile_path = workdir.join("profile.yaml");
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/subscription.yaml"),
            &profile_path,
        )
        .expect("copy fixture profile");
        let cfg = RuntimeCfg {
            profile_path,
            workdir: workdir.clone(),
        };
        let config_path = cfg.config_path();
        let client = cfg.client();
        let snapshot: tailash::bridge::SharedSnapshot =
            std::sync::Arc::new(std::sync::RwLock::new(Default::default()));
        let (tx, rx) = std::sync::mpsc::channel::<UiCmd>();
        let handle = spawn_worker(snapshot.clone(), rx, tx.clone(), cfg);

        // 没有内核时：显示未运行，并已为用户生成 config.yaml。
        assert!(wait_until(
            &snapshot,
            std::time::Duration::from_secs(5),
            |s| { !s.core_running && config_path.is_file() }
        ));

        // 模拟用户在终端启动内核（探活窗口 35s，首次 geoip 初始化较慢）
        let mut child = std::process::Command::new(mihomo)
            .arg("-d")
            .arg(&workdir)
            .arg("-f")
            .arg(&config_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn mihomo");
        let up = wait_until(&snapshot, std::time::Duration::from_secs(35), |s| {
            s.core_running
        });
        if !up {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "worker 应探测到内核运行；message={}",
                snapshot.read().unwrap().message
            );
        }
        {
            let snap = snapshot.read().unwrap();
            assert!(!snap.view.nodes.is_empty(), "节点列表应非空");
            assert!(!snap.view.groups.is_empty(), "组列表应非空");
        }
        assert!(
            client
                .version()
                .expect("socket 应可用")
                .version
                .starts_with('v')
        );

        // 选节点 + 测延迟（只验证命令链路，不依赖节点质量）
        tx.send(UiCmd::Select {
            group: "节点选择".into(),
            node: "自动选择".into(),
        })
        .unwrap();
        tx.send(UiCmd::TestAll).unwrap();
        assert!(
            wait_until(&snapshot, std::time::Duration::from_secs(3), |s| {
                s.message.starts_with("测延迟中") || s.message.starts_with("测速完成：成功")
            }),
            "应进入测延迟流程"
        );

        // 用户在外部停掉内核：worker 应探测到并清空状态。
        let _ = child.kill();
        let _ = child.wait();
        assert!(
            wait_until(&snapshot, std::time::Duration::from_secs(5), |s| !s
                .core_running),
            "内核退出后应显示未运行"
        );

        tx.send(UiCmd::Shutdown).unwrap();
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&workdir);
    }
}
