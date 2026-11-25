//! 从订阅 profile 生成带 Tailscale 共存规则的 mihomo config.yaml。
//!
//! 数据形状：订阅 yaml 是 mihomo 全量配置（proxies/proxy-groups/rules/dns/...）。
//! 我们不做全量建模，用 serde_yaml::Value 做手术式修改：
//! - 重写 tun 段（开启 TUN + 排除 Tailscale 网段）
//! - 重写 dns 段（fake-ip-filter 加 +.ts.net）
//! - 在 rules 头部插入 tailnet 直连规则
use crate::backend::gemini::{LISTENER_BASE_PORT, LISTENER_PREFIX};
use anyhow::Result;
use serde_yaml::Value;

/// Tailscale 相关的 IPv4 网段：CGNAT 100.64.0.0/10
pub const TAILSCALE_V4: &str = "100.64.0.0/10";
/// Tailscale 的 IPv6 ULA 前缀
pub const TAILSCALE_V6: &str = "fd7a:115c:a1e0::/48";

fn merge_tun(profile: &Value) -> Value {
    let mut tun = profile
        .get("tun")
        .cloned()
        .filter(Value::is_mapping)
        .unwrap_or_default();
    tun["enable"] = Value::Bool(true);
    tun["stack"] = tun
        .get("stack")
        .cloned()
        .filter(|v| !v.is_null())
        .unwrap_or_else(|| Value::String("mixed".into()));
    tun["auto-route"] = Value::Bool(true);
    tun["auto-detect-interface"] = Value::Bool(true);
    if !tun["dns-hijack"].is_sequence() {
        tun["dns-hijack"] = serde_yaml::from_str("[any:53]").expect("valid default");
    }
    if !tun["route-exclude-address"].is_sequence() {
        tun["route-exclude-address"] = Value::Sequence(Vec::new());
    }
    let exclusions = tun["route-exclude-address"]
        .as_sequence_mut()
        .expect("sequence initialized");
    for cidr in [TAILSCALE_V4, TAILSCALE_V6] {
        let value = Value::String(cidr.into());
        if !exclusions.contains(&value) {
            exclusions.push(value);
        }
    }
    tun
}

#[cfg(test)]
mod tun_preservation_tests {
    use super::*;

    #[test]
    fn generated_config_uses_requested_tun_state() {
        let profile: Value = serde_yaml::from_str("tun:\n  enable: true\nrules: []\n").unwrap();
        let output = generate(&profile, "/tmp/test.sock", false).unwrap();
        let config: Value = serde_yaml::from_str(&output).unwrap();
        assert_eq!(config["tun"]["enable"], false);
    }

    #[test]
    fn a_subscription_secret_never_locks_the_gui_out() {
        let profile: Value = serde_yaml::from_str("secret: from-sub\nrules: []\n").unwrap();
        let config: Value =
            serde_yaml::from_str(&generate(&profile, "/tmp/test.sock", true).unwrap()).unwrap();
        assert!(config.get("secret").is_none_or(|s| s == ""), "{config:?}");
    }
}

/// 生成 config。`profile` 是订阅原始 yaml 的 Value。
pub fn generate(profile: &Value, socket_path: &str, tun_enabled: bool) -> Result<String> {
    let mut cfg = profile.clone();
    cfg["tun"] = merge_tun(profile);
    cfg["tun"]["enable"] = Value::Bool(tun_enabled);

    // ---- dns 段：保留订阅的配置，但 fake-ip-filter 追加 ts.net 相关 ----
    let ts_filters: Vec<Value> = ["+.ts.net", "+.tailscale.com"]
        .iter()
        .map(|s| Value::String(s.to_string()))
        .collect();
    if !cfg["dns"].is_mapping() {
        cfg["dns"] = Value::Mapping(serde_yaml::Mapping::new());
    }
    {
        let filter = &mut cfg["dns"]["fake-ip-filter"];
        match filter {
            Value::Sequence(seq) => {
                for f in ts_filters {
                    if !seq.contains(&f) {
                        seq.push(f);
                    }
                }
            }
            _ => {
                cfg["dns"]["fake-ip-filter"] = Value::Sequence(
                    ["+.ts.net", "+.tailscale.com"]
                        .iter()
                        .map(|s| Value::String(s.to_string()))
                        .collect(),
                )
            }
        }
        // nameserver-policy：ts.net 的查询强制交给 MagicDNS，不要走订阅的上游
        let policy = &mut cfg["dns"]["nameserver-policy"];
        if !policy.is_mapping() {
            cfg["dns"]["nameserver-policy"] =
                serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
        }
        cfg["dns"]["nameserver-policy"]["+.ts.net"] = Value::String("100.100.100.100".to_string());
    }

    // ---- external controller：unix socket，供 GUI 控制 ----
    // mihomo 1.19 起不认 `external-controller: unix://…`（会报 invalid network address），
    // unix socket 只走 external-controller-unix 这一个键。
    cfg["external-controller-unix"] = Value::String(socket_path.to_string());
    // GUI 不带 secret 访问 API；订阅里自带的 secret 会把 GUI 挡在外面，去掉。
    if let Some(map) = cfg.as_mapping_mut() {
        map.remove("secret");
    }
    cfg["log-level"] = Value::String("info".to_string());

    // ---- 规则头部插入 tailnet 直连 ----
    let direct_rules: Vec<Value> = vec![
        // tailnet 域名直连
        Value::String("DOMAIN-SUFFIX,ts.net,DIRECT".to_string()),
        Value::String("IP-CIDR,100.64.0.0/10,DIRECT,no-resolve".to_string()),
        Value::String(format!("IP-CIDR,{TAILSCALE_V6},DIRECT,no-resolve")),
        Value::String("IP-CIDR,100.100.100.100/32,DIRECT,no-resolve".to_string()),
    ];
    match cfg["rules"] {
        Value::Sequence(ref mut seq) => {
            for (i, r) in direct_rules.into_iter().enumerate() {
                seq.insert(i, r);
            }
        }
        _ => {
            cfg["rules"] = Value::Sequence(direct_rules);
        }
    }

    merge_gemini_listeners(&mut cfg);

    let out = serde_yaml::to_string(&cfg)?;
    Ok(out)
}

fn is_gemini_listener(entry: &Value) -> bool {
    entry
        .get("name")
        .and_then(Value::as_str)
        .is_some_and(|name| name.starts_with(LISTENER_PREFIX))
}

fn collect_used_ports(cfg: &Value, listeners: &[Value]) -> std::collections::HashSet<u16> {
    let mut used = std::collections::HashSet::new();
    for key in [
        "port",
        "socks-port",
        "redir-port",
        "tproxy-port",
        "mixed-port",
    ] {
        if let Some(p) = cfg
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|v| u16::try_from(v).ok())
        {
            used.insert(p);
        }
    }
    for listener in listeners {
        if let Some(p) = listener
            .get("port")
            .and_then(Value::as_u64)
            .and_then(|v| u16::try_from(v).ok())
        {
            used.insert(p);
        }
    }
    used
}

fn build_gemini_listener(index: usize, port: u16, proxy_name: &str) -> Value {
    let mut listener = serde_yaml::Mapping::new();
    listener.insert(
        Value::String("name".into()),
        Value::String(format!("{LISTENER_PREFIX}{index}")),
    );
    listener.insert(Value::String("type".into()), Value::String("http".into()));
    listener.insert(
        Value::String("listen".into()),
        Value::String("127.0.0.1".into()),
    );
    listener.insert(Value::String("port".into()), Value::Number(port.into()));
    listener.insert(
        Value::String("proxy".into()),
        Value::String(proxy_name.to_string()),
    );
    Value::Mapping(listener)
}

/// 为每个节点生成本地 Gemini 探测 listener，直连节点、绕开规则与用户选择。
/// 幂等：先清除旧的 `tailash-gemini-*` 项，再避开配置中已占用的端口顺序分配。
fn merge_gemini_listeners(cfg: &mut Value) {
    let mut listeners = cfg
        .get("listeners")
        .and_then(Value::as_sequence)
        .cloned()
        .unwrap_or_default();
    listeners.retain(|entry| !is_gemini_listener(entry));

    let mut used_ports = collect_used_ports(cfg, &listeners);
    let mut next_port: u32 = LISTENER_BASE_PORT as u32;
    let proxy_names: Vec<String> = cfg
        .get("proxies")
        .and_then(Value::as_sequence)
        .into_iter()
        .flatten()
        .filter_map(|p| p.get("name").and_then(Value::as_str).map(str::to_string))
        .collect();

    for (index, name) in proxy_names.iter().enumerate() {
        while next_port <= u16::MAX as u32 && used_ports.contains(&(next_port as u16)) {
            next_port += 1;
        }
        let Ok(port) = u16::try_from(next_port) else {
            break;
        };
        used_ports.insert(port);
        next_port += 1;
        listeners.push(build_gemini_listener(index, port, name));
    }
    if !listeners.is_empty() {
        cfg["listeners"] = Value::Sequence(listeners);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_profile() -> Value {
        let text = include_str!("../../tests/fixtures/subscription.yaml");
        serde_yaml::from_str(text).expect("parse fixture")
    }

    #[test]
    fn test_generate_keeps_proxies_and_rules() {
        let p = load_profile();
        let n_proxies = p["proxies"].as_sequence().unwrap().len();
        let n_rules = p["rules"].as_sequence().unwrap().len();
        let out = generate(&p, "/tmp/tailash-mihomo.sock", true).unwrap();
        let cfg: Value = serde_yaml::from_str(&out).unwrap();
        assert_eq!(cfg["proxies"].as_sequence().unwrap().len(), n_proxies);
        assert_eq!(
            cfg["rules"].as_sequence().unwrap().len(),
            n_rules + 4,
            "rules 应在头部追加 4 条 tailnet 直连"
        );
    }

    #[test]
    fn every_proxy_gets_a_gemini_probe_listener() {
        let p = load_profile();
        let n_proxies = p["proxies"].as_sequence().unwrap().len();
        let out = generate(&p, "/tmp/tailash-mihomo.sock", true).unwrap();
        let cfg: Value = serde_yaml::from_str(&out).unwrap();
        let listeners = cfg["listeners"].as_sequence().unwrap();
        assert_eq!(listeners.len(), n_proxies);
        assert_eq!(listeners[0]["port"], LISTENER_BASE_PORT);
        assert_eq!(
            listeners[0]["proxy"],
            p["proxies"][0]["name"].clone(),
            "入口必须直连对应节点"
        );
    }

    #[test]
    fn gemini_listeners_generation_is_idempotent() {
        let p = load_profile();
        let n_proxies = p["proxies"].as_sequence().unwrap().len();
        let first = generate(&p, "/tmp/tailash-mihomo.sock", true).unwrap();
        let first_val: Value = serde_yaml::from_str(&first).unwrap();
        let second = generate(&first_val, "/tmp/tailash-mihomo.sock", true).unwrap();
        let second_val: Value = serde_yaml::from_str(&second).unwrap();
        let listeners = second_val["listeners"].as_sequence().unwrap();
        assert_eq!(
            listeners.len(),
            n_proxies,
            "重复生成不能叠加旧的 gemini listeners"
        );
    }

    #[test]
    fn gemini_listeners_skip_ports_already_used_in_profile() {
        let profile: Value = serde_yaml::from_str(
            "mixed-port: 17890\nlisteners:\n  - name: user-http\n    type: http\n    port: 17891\nproxies:\n  - name: JP-1\n    type: vmess\nrules: []\n",
        )
        .unwrap();
        let out = generate(&profile, "/tmp/tailash-mihomo.sock", true).unwrap();
        let cfg: Value = serde_yaml::from_str(&out).unwrap();
        let listeners = cfg["listeners"].as_sequence().unwrap();
        assert_eq!(listeners.len(), 2);
        assert_eq!(listeners[0]["name"], "user-http");
        assert_eq!(listeners[1]["proxy"], "JP-1");
        assert_eq!(listeners[1]["port"], 17892);
    }

    #[test]
    fn test_tun_excludes_tailscale() {
        let p = load_profile();
        let out = generate(&p, "/tmp/tailash-mihomo.sock", true).unwrap();
        let cfg: Value = serde_yaml::from_str(&out).unwrap();
        assert_eq!(cfg["tun"]["enable"], Value::Bool(true));
        let excl = cfg["tun"]["route-exclude-address"].as_sequence().unwrap();
        assert!(excl.contains(&Value::String(TAILSCALE_V4.into())));
        assert!(excl.contains(&Value::String(TAILSCALE_V6.into())));
    }

    #[test]
    fn test_dns_fakeip_filter_has_ts_net() {
        let p = load_profile();
        let out = generate(&p, "/tmp/tailash-mihomo.sock", true).unwrap();
        let cfg: Value = serde_yaml::from_str(&out).unwrap();
        let f = cfg["dns"]["fake-ip-filter"].as_sequence().unwrap();
        assert!(f.iter().any(|v| v == "+.ts.net"));
        let np = &cfg["dns"]["nameserver-policy"];
        assert_eq!(np["+.ts.net"], Value::String("100.100.100.100".into()));
    }

    #[test]
    fn missing_dns_gets_tailscale_policy_and_filter() {
        let profile: Value = serde_yaml::from_str("proxies: []\nrules: []\n").unwrap();
        let output = generate(&profile, "/tmp/tailash-mihomo.sock", true).unwrap();
        let cfg: Value = serde_yaml::from_str(&output).unwrap();
        assert_eq!(
            cfg["dns"]["nameserver-policy"]["+.ts.net"],
            "100.100.100.100"
        );
        let filters = cfg["dns"]["fake-ip-filter"].as_sequence().unwrap();
        assert!(filters.contains(&Value::String("+.ts.net".into())));
        assert!(filters.contains(&Value::String("+.tailscale.com".into())));
    }

    #[test]
    fn existing_tailscale_filters_are_not_duplicated() {
        let profile: Value = serde_yaml::from_str(
            "dns:\n  enable: true\n  fake-ip-filter: ['+.ts.net', '+.tailscale.com']\nrules: []\n",
        )
        .unwrap();
        let cfg: Value =
            serde_yaml::from_str(&generate(&profile, "/tmp/tailash-mihomo.sock", true).unwrap())
                .unwrap();
        let filters = cfg["dns"]["fake-ip-filter"].as_sequence().unwrap();
        assert_eq!(filters.iter().filter(|v| *v == "+.ts.net").count(), 1);
        assert_eq!(
            filters.iter().filter(|v| *v == "+.tailscale.com").count(),
            1
        );
    }

    #[test]
    fn test_rules_head_is_direct_tailnet() {
        let p = load_profile();
        let out = generate(&p, "/tmp/tailash-mihomo.sock", true).unwrap();
        let cfg: Value = serde_yaml::from_str(&out).unwrap();
        let rules = cfg["rules"].as_sequence().unwrap();
        assert_eq!(
            rules[0],
            Value::String("DOMAIN-SUFFIX,ts.net,DIRECT".into())
        );
        assert_eq!(
            rules[1],
            Value::String("IP-CIDR,100.64.0.0/10,DIRECT,no-resolve".into())
        );
    }

    #[test]
    fn test_controller_socket_configured() {
        let p = load_profile();
        let out = generate(&p, "/tmp/tailash-mihomo.sock", true).unwrap();
        let cfg: Value = serde_yaml::from_str(&out).unwrap();
        assert!(
            cfg["external-controller-unix"]
                .as_str()
                .unwrap()
                .ends_with("/tmp/tailash-mihomo.sock")
        );
    }
    #[test]
    fn preserves_existing_tun_options_and_exclusions() {
        let profile: Value = serde_yaml::from_str(
            "tun:\n  stack: system\n  auto-redirect: true\n  route-exclude-address:\n    - 10.0.0.0/8\n  dns-hijack:\n    - 127.0.0.1:53\n",
        )
        .unwrap();
        let tun = merge_tun(&profile);
        assert_eq!(tun["stack"], "system");
        assert_eq!(tun["auto-redirect"], true);
        assert_eq!(tun["route-exclude-address"].as_sequence().unwrap().len(), 3);
        assert_eq!(tun["dns-hijack"][0], "127.0.0.1:53");
    }
}
