//! 连接已在命令行启动的 mihomo（默认数据目录），打印前 12 个节点的延迟。
fn main() {
    let workdir = std::env::var("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".local/share")
        })
        .join("tailash");
    let cfg = tailash::runtime::RuntimeCfg {
        profile_path: workdir.join("profile.yaml"),
        workdir,
    };
    let client = cfg.client();
    let view = match tailash::runtime::load_view(&client, &cfg.profile_path) {
        Ok(view) => view,
        Err(e) => {
            eprintln!(
                "连接内核失败: {e:#}\n请先在终端启动：{}",
                cfg.start_command()
            );
            std::process::exit(1);
        }
    };
    let mut ok = 0;
    let mut fail = 0;
    for node in view.nodes.iter().take(12) {
        match client.delay(&node.name, 8000, tailash::runtime::TEST_URL) {
            Ok(d) => {
                ok += 1;
                println!("OK {} = {}", node.name, d.delay);
            }
            Err(e) => {
                fail += 1;
                println!("ERR {}: {e}", node.name);
            }
        }
    }
    println!(
        "TOTAL ok={ok} fail={fail} nodes={} groups={}",
        view.nodes.len(),
        view.groups.len()
    );
}
