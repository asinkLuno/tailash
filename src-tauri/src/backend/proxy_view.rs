//! 节点/组的展示模型。
//!
//! mihomo 的 `/proxies` 是一个无序的 map，`type` 也无法可靠区分"组"和"节点"
//! （新组类型层出，任何黑名单都会漏判）。所以这里：
//! - 用 `all` 字段是否存在判组（clash-verge 的 `partition_core` 做法）；
//! - 组的顺序、组内成员的顺序都取自订阅 profile（mihomo 运行时的同一份顺序），
//!   绝不按名字重排——排序会让用户觉得"节点丢了 / 乱了"；
//! - 组里引用的其他组不当作节点展示，但"全部节点"兜底列出所有真节点，
//!   保证任何一个节点都不会因为不在某个组里而消失。
use crate::backend::mihomo_api::ProxyEntry;
use serde_yaml::Value;

/// mihomo 延迟测试的默认超时；延迟达到它就等于超时
pub const DEFAULT_TIMEOUT_MS: u32 = 5000;
/// 明显不可能是真实延迟（mihomo 用 1e5/更大值表示错误）
pub const IMPLAUSIBLE_DELAY: u32 = 100_000;

/// 延迟分档，顺序与 clash-verge 的排序等级一致：测出的 < 超时 < 错误 < 未测。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DelayClass {
    Measured,
    Timeout,
    Error,
    Untested,
}

/// `/proxies` 的 `history` 里 `delay == 0` 表示超时，不能用 0ms 展示。
pub fn classify_delay(delay: Option<u32>, timeout_ms: u32) -> DelayClass {
    match delay {
        None => DelayClass::Untested,
        Some(0) => DelayClass::Timeout,
        Some(d) if d >= IMPLAUSIBLE_DELAY => DelayClass::Error,
        Some(d) if d >= timeout_ms => DelayClass::Timeout,
        Some(_) => DelayClass::Measured,
    }
}

/// 排序键：先按分档，再按数值，哨兵值不会打乱顺序。
pub fn delay_sort_key(delay: Option<u32>, timeout_ms: u32) -> (DelayClass, u32) {
    (classify_delay(delay, timeout_ms), delay.unwrap_or(0))
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfo {
    pub name: String,
    /// mihomo 的大小写形式（Vmess / Hysteria2 / AnyTLS…），用于能力标签
    pub kind: String,
    pub delay: Option<u32>,
    pub udp: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupInfo {
    pub name: String,
    pub kind: String,
    /// 该组当前选中的成员
    pub now: Option<String>,
    /// mihomo 的 `all`，顺序即选择顺序
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyView {
    /// 全部真节点，订阅（profile）顺序在前，内核里多出来的追加在后
    pub nodes: Vec<NodeInfo>,
    /// 全部组，订阅里 proxy-groups 的顺序
    pub groups: Vec<GroupInfo>,
}

impl ProxyView {
    pub fn group(&self, name: &str) -> Option<&GroupInfo> {
        self.groups.iter().find(|g| g.name == name)
    }

    pub fn node(&self, name: &str) -> Option<&NodeInfo> {
        self.nodes.iter().find(|n| n.name == name)
    }

    pub fn is_group(&self, name: &str) -> bool {
        self.groups.iter().any(|g| g.name == name)
    }

    pub fn set_delay(&mut self, name: &str, delay: Option<u32>) {
        if let Some(node) = self.nodes.iter_mut().find(|n| n.name == name) {
            node.delay = delay;
        }
    }
}

/// 内建伪代理：既不是组也不是真节点，不该出现在节点列表里
fn is_builtin(kind: &str) -> bool {
    matches!(
        kind.to_ascii_lowercase().as_str(),
        "direct" | "reject" | "rejectdrop" | "pass" | "passrule" | "compatible"
    )
}

fn sequence_names(value: Option<&Value>, key: &str) -> Vec<String> {
    value
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|item| item.get("name").and_then(|n| n.as_str()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 用订阅 profile 的顺序 + `/proxies` 的内容拼出展示模型。
pub fn build(profile: &Value, mut entries: Vec<ProxyEntry>) -> ProxyView {
    // 稳定顺序：先订阅里的 proxies 顺序，剩下的按内核返回顺序追加
    let profile_order = sequence_names(Some(profile), "proxies");
    let group_order = sequence_names(Some(profile), "proxy-groups");

    let mut nodes: Vec<NodeInfo> = Vec::new();
    let push_node = |nodes: &mut Vec<NodeInfo>, entry: &ProxyEntry| {
        if entry.is_group() || is_builtin(&entry.kind) || entry.name == "GLOBAL" {
            return;
        }
        if nodes.iter().any(|n| n.name == entry.name) {
            return;
        }
        nodes.push(NodeInfo {
            name: entry.name.clone(),
            kind: entry.kind.clone(),
            delay: entry.last_delay(),
            udp: entry.udp,
        });
    };

    for name in &profile_order {
        if let Some(entry) = entries.iter().find(|e| &e.name == name) {
            push_node(&mut nodes, entry);
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    for entry in &entries {
        push_node(&mut nodes, entry);
    }

    let mut groups: Vec<GroupInfo> = Vec::new();
    let push_group = |groups: &mut Vec<GroupInfo>, entry: &ProxyEntry| {
        let Some(all) = &entry.all else { return };
        if entry.name == "GLOBAL" || groups.iter().any(|g| g.name == entry.name) {
            return;
        }
        groups.push(GroupInfo {
            name: entry.name.clone(),
            kind: entry.kind.clone(),
            now: entry.now.clone(),
            members: all.clone(),
        });
    };
    for name in &group_order {
        if let Some(entry) = entries.iter().find(|e| &e.name == name) {
            push_group(&mut groups, entry);
        }
    }
    // 订阅顺序里没有、但内核里存在的组（极少数：include-all 动态生成的）
    let mut extra: Vec<&ProxyEntry> = entries.iter().filter(|e| e.is_group()).collect();
    extra.sort_by(|a, b| a.name.cmp(&b.name));
    for entry in extra {
        push_group(&mut groups, entry);
    }

    ProxyView { nodes, groups }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, kind: &str) -> ProxyEntry {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "type": kind,
            "history": [],
        }))
        .unwrap()
    }

    fn group_entry(name: &str, kind: &str, members: &[&str]) -> ProxyEntry {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "type": kind,
            "now": members.first(),
            "all": members,
            "history": [],
        }))
        .unwrap()
    }

    fn profile(groups: &[(&str, &[&str])], proxies: &[&str]) -> Value {
        let proxies: Vec<Value> = proxies
            .iter()
            .map(|n| serde_yaml::from_str(&format!("name: {n}\ntype: vmess")).unwrap())
            .collect();
        let groups: Vec<Value> = groups
            .iter()
            .map(|(name, members)| {
                let members: Vec<Value> = members
                    .iter()
                    .map(|m| Value::String(m.to_string()))
                    .collect();
                let mut map = serde_yaml::Mapping::new();
                map.insert(
                    Value::String("name".into()),
                    Value::String(name.to_string()),
                );
                map.insert(Value::String("type".into()), Value::String("select".into()));
                map.insert(Value::String("proxies".into()), Value::Sequence(members));
                Value::Mapping(map)
            })
            .collect();
        let mut root = serde_yaml::Mapping::new();
        root.insert(Value::String("proxies".into()), Value::Sequence(proxies));
        root.insert(
            Value::String("proxy-groups".into()),
            Value::Sequence(groups),
        );
        Value::Mapping(root)
    }

    #[test]
    fn group_detection_uses_all_not_type_blacklist() {
        // 一个从没见过的新组类型：只要带 all 就是组
        let entries = vec![
            group_entry("智能选择", "Smart", &["香港", "日本"]),
            entry("香港", "Vmess"),
            entry("DIRECT", "Direct"),
        ];
        let view = build(&Value::Null, entries);
        assert!(view.is_group("智能选择"));
        assert!(!view.is_group("香港"));
        assert_eq!(view.nodes.len(), 1, "内建伪代理不该算节点");
    }

    #[test]
    fn subscription_order_is_preserved_for_nodes_and_groups() {
        let entries = vec![
            entry("美国", "Vmess"),
            entry("香港", "Vmess"),
            group_entry("节点选择", "Selector", &["香港", "美国"]),
            group_entry("自动选择", "URLTest", &["香港"]),
        ];
        let profile = profile(
            &[("节点选择", &["香港", "美国"]), ("自动选择", &["香港"])],
            &["香港", "美国"],
        );
        let view = build(&profile, entries);
        let names: Vec<&str> = view.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["香港", "美国"],
            "节点顺序跟订阅，而不是名字排序"
        );
        let groups: Vec<&str> = view.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(groups, vec!["节点选择", "自动选择"]);
        assert_eq!(
            view.group("节点选择").unwrap().members,
            vec!["香港", "美国"],
            "成员顺序取 all"
        );
    }

    #[test]
    fn nodes_outside_every_group_still_show_up_in_the_all_view() {
        let entries = vec![
            group_entry("节点选择", "Selector", &["香港"]),
            entry("香港", "Vmess"),
            entry("孤儿节点", "Vmess"),
        ];
        let view = build(&Value::Null, entries);
        let all: Vec<&str> = view.nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(
            all.contains(&"孤儿节点"),
            "不在任何组里的节点必须可见: {all:?}"
        );
    }

    #[test]
    fn history_last_delay_is_used_as_initial_value() {
        let entry: ProxyEntry = serde_json::from_value(serde_json::json!({
            "name": "香港",
            "type": "Vmess",
            "history": [{"time": "t1", "delay": 685}, {"time": "t2", "delay": 136}],
        }))
        .unwrap();
        assert_eq!(entry.last_delay(), Some(136));
        let view = build(&Value::Null, vec![entry]);
        assert_eq!(view.nodes[0].delay, Some(136));
    }

    #[test]
    fn delay_classes_follow_verge_ranking() {
        assert_eq!(classify_delay(Some(120), 5000), DelayClass::Measured);
        assert_eq!(classify_delay(Some(0), 5000), DelayClass::Timeout);
        assert_eq!(classify_delay(Some(5000), 5000), DelayClass::Timeout);
        assert_eq!(classify_delay(Some(200_000), 5000), DelayClass::Error);
        assert_eq!(classify_delay(None, 5000), DelayClass::Untested);
        assert!(DelayClass::Measured < DelayClass::Timeout);
        assert!(DelayClass::Timeout < DelayClass::Error);
        assert!(DelayClass::Error < DelayClass::Untested);
        assert!(delay_sort_key(Some(0), 5000) < delay_sort_key(None, 5000));
    }
}
