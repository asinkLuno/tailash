//! 端到端探针：url-test 组的内核选中必须跟上「测全部」后的最新延迟。
//!
//! 场景：三个假节点（人工延迟 约50/300/900ms）组成「自动选择」url-test 组，
//! 成员顺序故意为 慢/中/快（首个成员 ≠ 最小延迟节点）。
//! 假代理延后 10s 才监听 → 内核启动时的首次组测试必然失败（fastNode 卡在首个成员）；
//! 之后节点就绪、worker 发 TestAll → 测完后内核的 now 必须是 FA-快（最小延迟）。
//!
//! 需要真实 mihomo（src-tauri/bin/mihomo 或 PATH）+ python3；
//! 用 `cargo test --test urltest_probe -- --ignored --nocapture` 运行。
#[cfg(test)]
mod urltest_probe {
    use std::net::TcpStream;
    use std::time::Duration;

    use tailash_lib::backend::bridge::{self, SharedSnapshot, UiCmd};
    use tailash_lib::backend::runtime::RuntimeCfg;

    const PROFILE: &str = r#"mixed-port: 18010
log-level: info
proxies:
  - {name: FA-慢, type: http, server: 127.0.0.1, port: 18003, udp: false}
  - {name: FA-中, type: http, server: 127.0.0.1, port: 18002, udp: false}
  - {name: FA-快, type: http, server: 127.0.0.1, port: 18001, udp: false}
proxy-groups:
  - {name: 自动选择, type: url-test, proxies: [FA-慢, FA-中, FA-快], url: 'http://www.gstatic.com/generate_204', interval: 600, tolerance: 20}
rules:
  - MATCH,DIRECT
"#;

    fn wait_until(
        snapshot: &SharedSnapshot,
        timeout: Duration,
        cond: impl Fn(&tailash_lib::backend::bridge::Snapshot) -> bool,
    ) -> bool {
        let begin = std::time::Instant::now();
        while begin.elapsed() < timeout {
            if cond(&snapshot.read().unwrap()) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        cond(&snapshot.read().unwrap())
    }

    fn wait_port(port: u16, timeout: Duration) -> bool {
        let begin = std::time::Instant::now();
        while begin.elapsed() < timeout {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        false
    }

    #[test]
    #[ignore = "launches the real mihomo core and fake proxies"]
    fn urltest_group_follows_min_delay_after_test_all() {
        let workdir = std::env::temp_dir().join(format!(
            "tailash-urltest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&workdir).unwrap();

        // 1. 假代理：延后 10s 才监听，内核启动时的组测试必然全失败
        let proxy_script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/fake_proxy.py");
        let mut python = std::process::Command::new("python3")
            .arg("-I")
            .arg(&proxy_script)
            .arg("10")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn python3 fake proxies");

        // 2. profile + worker（worker 启动时会按订阅生成 config.yaml）
        let profile_path = workdir.join("profile.yaml");
        std::fs::write(&profile_path, PROFILE).unwrap();
        let cfg = RuntimeCfg {
            profile_path: profile_path.clone(),
            workdir: workdir.clone(),
        };
        let snapshot: SharedSnapshot =
            std::sync::Arc::new(std::sync::RwLock::new(Default::default()));
        let (tx, rx) = std::sync::mpsc::channel::<UiCmd>();
        let worker = bridge::spawn_worker(snapshot.clone(), rx, tx.clone(), cfg.clone());
        std::thread::sleep(Duration::from_secs(2));

        // 3. 覆写 config.yaml：关掉 TUN（探针不需要接管路由，免 cap）
        let profile: serde_yaml::Value =
            serde_yaml::from_str(PROFILE).expect("parse probe profile");
        let config_text =
            tailash_lib::backend::runtime::render_config(&cfg, &profile, Some(false))
                .expect("render probe config");
        std::fs::write(cfg.config_path(), config_text).unwrap();

        // 4. 启动内核（此时假代理还没就绪 → 内核首次组测试失败）
        let mihomo_bin = tailash_lib::backend::runtime::find_mihomo().expect("find mihomo");
        let mut core = std::process::Command::new(&mihomo_bin)
            .arg("-d")
            .arg(&workdir)
            .arg("-f")
            .arg(cfg.config_path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn mihomo");

        assert!(
            wait_until(&snapshot, Duration::from_secs(20), |s| s.core_running),
            "内核应运行；message={}",
            snapshot.read().unwrap().message
        );
        let group = snapshot
            .read()
            .unwrap()
            .view
            .group("自动选择")
            .cloned()
            .expect("url-test 组应在节点视图里");
        assert_eq!(group.kind, "URLTest", "组类型来自内核");
        assert_eq!(group.now.as_deref(), Some("FA-慢"),
            "内核首次组测试全失败时 fastNode 应停在首个成员（慢）——这是用户看到「没选最小」的现场");

        // 5. 假代理就绪后测全部
        assert!(wait_port(18001, Duration::from_secs(20)), "假代理应就绪");
        tx.send(UiCmd::TestAll).unwrap();
        assert!(
            wait_until(&snapshot, Duration::from_secs(30), |s| {
                s.message.starts_with("测速完成")
            }),
            "测全部应完成；message={}",
            snapshot.read().unwrap().message
        );

        // 6. 校验：内核 now 与「实测最小延迟成员」一致（worker 会在测完后强制重测该组）
        let snap = snapshot.read().unwrap();
        let group = snap.view.group("自动选择").cloned().unwrap();
        let mut members: Vec<(&String, Option<u32>)> = group
            .members
            .iter()
            .map(|name| (name, snap.view.node(name).and_then(|n| n.delay)))
            .collect();
        members.sort_by_key(|(_, delay)| delay.unwrap_or(u32::MAX));
        println!("节点延迟: {members:?}");
        println!("内核 now: {}", group.now.as_deref().unwrap_or("(无)"));
        let min_member = members.first().map(|(name, _)| name.as_str()).unwrap();
        assert_eq!(min_member, "FA-快", "最小延迟应是快节点");
        assert_eq!(
            group.now.as_deref(),
            Some(min_member),
            "测全部后内核的 url-test 选中必须是最小延迟节点"
        );
        drop(snap);

        // 收尾
        tx.send(UiCmd::Shutdown).unwrap();
        worker.join().unwrap();
        let _ = core.kill();
        let _ = core.wait();
        let _ = python.kill();
        let _ = python.wait();
        let _ = std::fs::remove_dir_all(&workdir);
    }
}
