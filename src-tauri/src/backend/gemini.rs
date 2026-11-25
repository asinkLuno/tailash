//! Gemini 可用性检测：config_gen 为每个节点在本机开一个 HTTP 入口（listeners），
//! 请求直连该节点、不经过规则、也不改用户当前选择；拉一次 gemini.google.com 判断地区。
//! 实测页面里 `,2,1,200,"SGP"` 这样的三位国家码就是出口地区（与社区脚本的
//! `45631641,null,true` 标记二选一：标记现在多数情况不再出现，地区码更可靠）。
use std::path::Path;

const GEMINI_URL: &str = "https://gemini.google.com/";
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
/// 支持地区的页面里带这两个标记之一（社区脚本 RegionRestrictionCheck 的判据）
const AVAILABLE_MARKERS: [&str; 2] = ["45631641,null,true", "45617354,null,true"];
/// Gemini 不开放的国家/地区（ISO3）。开放的有 200 多个，短黑名单比长白名单更不容易过期。
const BLOCKED_REGIONS: [&str; 6] = ["CHN", "RUS", "IRN", "PRK", "SYR", "CUB"];

/// 每个节点一个本地入口，端口 = BASE + 节点在 proxies 里的下标（见 config_gen）
pub const LISTENER_PREFIX: &str = "tailash-gemini-";
pub const LISTENER_BASE_PORT: u16 = 17890;

/// 从页面里取出口地区码（`,2,1,200,"SGP"`）。
fn region(body: &str) -> Option<&str> {
    const NEEDLE: &str = ",2,1,200,\"";
    let start = body.find(NEEDLE)? + NEEDLE.len();
    body.get(start..start + 3)
        .filter(|code| code.chars().all(|c| c.is_ascii_uppercase()))
}

pub fn available(status: u16, body: &str) -> bool {
    status == 200
        && (AVAILABLE_MARKERS.iter().any(|marker| body.contains(marker))
            || region(body).is_some_and(|code| !BLOCKED_REGIONS.contains(&code)))
}

/// 从生成好的 config.yaml 读出（节点名, 本地入口端口）。
pub fn listeners(config_path: &Path) -> Vec<(String, u16)> {
    let Ok(text) = std::fs::read_to_string(config_path) else {
        return Vec::new();
    };
    let Ok(cfg) = serde_yaml::from_str::<serde_yaml::Value>(&text) else {
        return Vec::new();
    };
    cfg.get("listeners")
        .and_then(serde_yaml::Value::as_sequence)
        .map(|listeners| {
            listeners
                .iter()
                .filter_map(|listener| {
                    let name = listener.get("name")?.as_str()?;
                    if !name.starts_with(LISTENER_PREFIX) {
                        return None;
                    }
                    let proxy = listener.get("proxy")?.as_str()?;
                    let port = listener.get("port")?.as_u64()?;
                    Some((proxy.to_string(), u16::try_from(port).ok()?))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 入口是否已在监听：内核按旧配置启动时（app 升级后没重启内核）会全部连不上。
pub fn listening(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).is_ok()
}

/// 让内核按当前 config.yaml 重载（listeners 这时才会绑定），最多等 2 秒确认入口在听。
pub fn restart_listeners(
    client: Option<&crate::backend::mihomo_api::MihomoClient>,
    config_path: &Path,
    port: u16,
) -> bool {
    let Some(client) = client else {
        return false;
    };
    if client
        .reload(config_path.to_string_lossy().as_ref())
        .is_err()
    {
        return false;
    }
    (0..20).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(100));
        listening(port)
    })
}

/// 经某个节点的本地入口探测；任何失败都当作不可用。
pub fn probe(port: u16) -> bool {
    let out = std::process::Command::new("curl")
        .args([
            "-sS",
            "-L",
            "--compressed",
            "--max-time",
            "15",
            "--connect-timeout",
            "8",
            "-A",
            UA,
            "-H",
            "Accept-Language: en-US,en;q=0.9",
            "-w",
            "\n%{http_code}",
        ])
        .arg(format!("--proxy=http://127.0.0.1:{port}"))
        .arg(GEMINI_URL)
        .output();
    let Ok(out) = out else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let (body, status) = stdout.rsplit_once('\n').unwrap_or((&stdout, ""));
    status
        .trim()
        .parse::<u16>()
        .is_ok_and(|code| available(code, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_marker_wins() {
        assert!(available(200, "<script>45631641,null,true</script>"));
        assert!(available(200, "<script>45617354,null,true</script>"));
    }

    #[test]
    fn region_decides_when_markers_are_absent() {
        let page = |code: &str| format!(r#"<html>,2,1,200,"{code}"</html>"#);
        assert!(available(200, &page("SGP")));
        assert!(available(200, &page("TWN")));
        assert!(!available(200, &page("CHN")));
        assert!(!available(200, &page("RUS")));
        assert!(!available(403, &page("SGP")));
        assert!(!available(200, "<html>sorry</html>"));
    }

    #[test]
    fn listening_tells_bound_ports_from_closed_ones() {
        let bound = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = bound.local_addr().unwrap().port();
        assert!(listening(port));
        drop(bound);
        assert!(!listening(1));
    }
    #[test]
    fn listeners_are_read_from_generated_config() {
        let dir = std::env::temp_dir().join(format!("tailash-gemini-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        std::fs::write(
            &path,
            "listeners:\n- name: unrelated\n  port: 1\n- name: tailash-gemini-0\n  proxy: 日本\n  port: 17890\n",
        )
        .unwrap();
        assert_eq!(listeners(&path), vec![("日本".to_string(), 17890)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
