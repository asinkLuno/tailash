//! 托管订阅：URL → 下载 → 校验 → 原子落盘 profile.yaml。
//! 下载用系统 curl（校验证书），不引入 TLS 依赖；settings.json 持久化 URL。
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// 订阅响应上限（防异常返回吃内存）
const MAX_BODY_BYTES: u64 = 16 * 1024 * 1024;

/// 订阅 profile 的暂存文件名（校验/重载通过后再原子替换）。
pub const PROFILE_STAGED: &str = "profile.yaml.pending";
/// 活动订阅的内容物化位置：runtime 的 profile_path 固定指到这里。
pub const PROFILE_NAME: &str = "profile.yaml";

/// 一条订阅的元信息（settings.json 持久化；后端私有格式，沿用 snake_case）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SubMeta {
    pub id: String,
    pub name: String,
    pub url: String,
    /// 最近一次下载成功时的节点数
    #[serde(default)]
    pub node_count: Option<usize>,
    /// 最近一次下载成功的 unix 秒
    #[serde(default)]
    pub updated_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct Settings {
    #[serde(default)]
    pub subscriptions: Vec<SubMeta>,
    /// 当前生效的订阅 id；None = 尚未选过
    #[serde(default)]
    pub active: Option<String>,
    /// 自动更新间隔（小时）；None = 关闭
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_update_hours: Option<u64>,
    /// 上一轮自动更新开始时的 unix 秒；None = 从未自动更新过
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_updated_at: Option<u64>,
    /// 旧版单订阅地址：只为读兼容，迁移后清空且不再写出
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subscribe_url: String,
}

/// 自动更新是否到期：关闭（None/0）永不到期；从未更新过视为立即到期。
pub fn auto_update_due(hours: Option<u64>, last: Option<u64>, now: u64) -> bool {
    let Some(hours) = hours.filter(|h| *h > 0) else {
        return false;
    };
    last.is_none_or(|last| now.saturating_sub(last) >= hours.saturating_mul(3600))
}

pub fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.json")
}

/// 读持久化设置；文件缺失/损坏时返回默认值（损坏不报错，避免卡死启动）
pub fn load_settings(data_dir: &Path) -> Settings {
    std::fs::read_to_string(settings_path(data_dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// 订阅 id 只能进入文件名，白名单字符，挡住 settings.json 被手改成路径穿越。
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// 生成新订阅 id（纳秒时间戳，十六进制，天然满足 valid_id）。
pub fn new_sub_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("s{nanos:x}")
}

/// 旧版单订阅 settings.json → 多订阅：包成一条订阅并设为活动；
/// 已有 profile.yaml 就归档为该订阅的内容文件，切换回来不丢。
pub fn migrate_legacy(settings: Settings, data_dir: &Path) -> Settings {
    if !settings.subscriptions.is_empty() || settings.subscribe_url.trim().is_empty() {
        return settings;
    }
    let id = new_sub_id();
    if profile_path(data_dir).is_file()
        && let Ok(text) = std::fs::read_to_string(profile_path(data_dir))
    {
        let _ = save_sub_profile(data_dir, &id, &text);
    }
    Settings {
        subscriptions: vec![SubMeta {
            id: id.clone(),
            name: "默认订阅".into(),
            url: settings.subscribe_url.clone(),
            node_count: None,
            updated_at: None,
        }],
        active: Some(id),
        auto_update_hours: None,
        auto_updated_at: None,
        subscribe_url: String::new(),
    }
}

pub fn profile_path(data_dir: &Path) -> PathBuf {
    data_dir.join(PROFILE_NAME)
}

fn subs_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("subs")
}

fn sub_profile_path(data_dir: &Path, id: &str) -> PathBuf {
    subs_dir(data_dir).join(format!("{id}.yaml"))
}

/// 读某条订阅的已下载内容
pub fn load_sub_profile(data_dir: &Path, id: &str) -> Option<String> {
    debug_assert!(valid_id(id), "订阅 id 必须先过 valid_id 校验");
    std::fs::read_to_string(sub_profile_path(data_dir, id)).ok()
}

/// 写某条订阅的内容（临时文件 + 原子替换，避免半截文件）
pub fn save_sub_profile(data_dir: &Path, id: &str, text: &str) -> Result<()> {
    debug_assert!(valid_id(id), "订阅 id 必须先过 valid_id 校验");
    let dir = subs_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!(".{id}.tmp"));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, sub_profile_path(data_dir, id))
        .with_context(|| format!("提交订阅文件 {id} 失败"))
}

pub fn remove_sub_profile(data_dir: &Path, id: &str) {
    let _ = std::fs::remove_file(sub_profile_path(data_dir, id));
}

/// 保存设置（父目录不存在则先创建）
pub fn save_settings(data_dir: &Path, settings: &Settings) -> Result<()> {
    let path = settings_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(settings)?;
    std::fs::write(&path, text)?;
    Ok(())
}

/// 校验文本是否为可用订阅：必须能解析 yaml 且含非空 proxies 段。
/// 支持 base64 订阅也能被 mihomo 读的场合不管——这里只收 mihomo 全量 yaml。
pub fn validate_profile_text(text: &str) -> Result<()> {
    let profile: serde_yaml::Value = serde_yaml::from_str(text).context("订阅内容不是合法 yaml")?;
    let proxies = profile
        .get("proxies")
        .and_then(|v| v.as_sequence())
        .context("订阅缺少 proxies 段")?;
    anyhow::ensure!(!proxies.is_empty(), "订阅 proxies 为空");
    Ok(())
}

/// 下载结果：profile 文本 + 订阅在响应头里自报的名称（没报则 None）。
pub struct FetchedSub {
    pub text: String,
    pub name: Option<String>,
}

/// 下载订阅（系统 curl，校验 TLS 证书，固定 UA 走 Clash 配置分支）。
/// 顺手取响应头里的订阅名：`profile-title`（可带 base64: 前缀）或
/// Content-Disposition 的 filename，多数机场/转换器会带其中一个。
pub fn fetch_subscribe(url: &str) -> Result<FetchedSub> {
    anyhow::ensure!(!url.trim().is_empty(), "订阅地址为空");
    anyhow::ensure!(
        url.trim().starts_with("https://") || url.trim().starts_with("http://"),
        "订阅地址须以 http(s):// 开头"
    );
    let header_path = std::env::temp_dir().join(format!(
        "tailash-sub-{}-{}.headers",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let out = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "30",
            "--connect-timeout",
            "10",
            "--max-filesize",
            &MAX_BODY_BYTES.to_string(),
            "-A",
            "clash-verge/v1.7.7",
            "-D",
        ])
        .arg(&header_path)
        .arg(url.trim())
        .output();
    let headers = std::fs::read_to_string(&header_path).unwrap_or_default();
    let _ = std::fs::remove_file(&header_path);
    let out = out.context("启动 curl 失败（系统未安装 curl?）")?;
    anyhow::ensure!(out.status.success(), "下载失败（curl {}）", out.status);
    anyhow::ensure!(out.stdout.len() as u64 <= MAX_BODY_BYTES, "订阅响应过大");
    let text =
        String::from_utf8(out.stdout).map_err(|_| anyhow::anyhow!("订阅响应不是合法 UTF-8"))?;
    Ok(FetchedSub {
        text,
        name: parse_sub_name(&headers),
    })
}

/// 从响应头解析订阅自报的名称；重定向链取最后一个响应。
pub fn parse_sub_name(headers: &str) -> Option<String> {
    let header = |key: &str| {
        headers.lines().rev().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim().eq_ignore_ascii_case(key).then(|| value.trim())
        })
    };
    if let Some(raw) = header("profile-title") {
        let plain = raw
            .get(..7)
            .filter(|prefix| prefix.eq_ignore_ascii_case("base64:"))
            .map(|_| decode_base64(&raw[7..]))
            .unwrap_or(None);
        if let Some(name) = clean_name(plain.as_deref().unwrap_or(raw)) {
            return Some(name);
        }
    }
    header("content-disposition").and_then(disposition_name)
}

/// `attachment; filename="x"; filename*=UTF-8''y` → y（百分号解码），否则 x。
fn disposition_name(value: &str) -> Option<String> {
    let mut fallback = None;
    for part in value.split(';').skip(1) {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix("filename*=") {
            let raw = rest.split("''").nth(1).unwrap_or(rest);
            if let Some(name) = clean_name(&percent_decode(raw.trim_matches('"'))) {
                return Some(name);
            }
        } else if let Some(rest) = part.strip_prefix("filename=") {
            fallback = clean_name(rest.trim_matches('"'));
        }
    }
    fallback
}

/// 名称只做展示：去控制字符、去空白、限长，避免响应头塞进超长或换行内容。
fn clean_name(raw: &str) -> Option<String> {
    let name: String = raw.chars().filter(|c| !c.is_control()).collect();
    let name = name.trim();
    (!name.is_empty()).then(|| name.chars().take(60).collect())
}

fn decode_base64(text: &str) -> Option<String> {
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0u32;
    for byte in text.bytes().filter(|b| !b.is_ascii_whitespace()) {
        if byte == b'=' {
            break;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    String::from_utf8(out).ok()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let Some(pair) = bytes.get(i + 1..i + 3)
            && let (Some(hi), Some(lo)) = (hex(pair[0]), hex(pair[1]))
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 下载并验证订阅，写入候选文件；由调用方在热重载成功后提交。
pub fn stage_profile_file(url: &str, profile_path: &Path) -> Result<String> {
    let text = fetch_subscribe(url)?.text;
    validate_profile_text(&text).context("校验订阅失败")?;
    if let Some(parent) = profile_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let staged = profile_path.with_file_name(format!(
        ".{}.{}.tmp",
        profile_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)?;
    use std::io::Write as _;
    if let Err(error) = file.write_all(text.as_bytes()) {
        let _ = std::fs::remove_file(&staged);
        return Err(error.into());
    }
    drop(file);
    if let Err(error) = std::fs::rename(&staged, profile_path) {
        let _ = std::fs::remove_file(&staged);
        return Err(error.into());
    }
    Ok(text)
}

pub fn atomic_replace(staged: &Path, target: &Path) -> Result<()> {
    std::fs::rename(staged, target).with_context(|| format!("提交文件 {} 失败", target.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "tailash-settings-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let s = Settings {
            subscriptions: vec![SubMeta {
                id: "s1".into(),
                name: "机场 A".into(),
                url: "https://example.invalid/sub?token=abc".into(),
                node_count: Some(42),
                updated_at: Some(1_700_000_000),
            }],
            active: Some("s1".into()),
            auto_update_hours: Some(12),
            auto_updated_at: Some(1_700_000_500),
            subscribe_url: String::new(),
        };
        save_settings(&dir, &s).unwrap();
        let got = load_settings(&dir);
        assert_eq!(got.subscriptions, s.subscriptions);
        assert_eq!(got.active, s.active);
        assert_eq!(got.auto_update_hours, Some(12));
        assert_eq!(got.auto_updated_at, Some(1_700_000_500));
        assert_eq!(got.subscribe_url, "", "迁移后的 legacy 字段不再写出");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn legacy_settings_are_migrated_and_profile_archived() {
        let dir = std::env::temp_dir().join(format!("tailash-migrate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            settings_path(&dir),
            r#"{"subscribe_url":"https://old.example/sub"}"#,
        )
        .unwrap();
        std::fs::write(dir.join(PROFILE_NAME), "proxies: []\n").unwrap();

        let migrated = migrate_legacy(load_settings(&dir), &dir);

        assert_eq!(migrated.subscriptions.len(), 1);
        assert_eq!(migrated.subscriptions[0].url, "https://old.example/sub");
        assert_eq!(migrated.active, Some(migrated.subscriptions[0].id.clone()));
        assert_eq!(
            load_sub_profile(&dir, migrated.active.as_deref().unwrap()).as_deref(),
            Some("proxies: []\n"),
            "旧 profile.yaml 应归档为该订阅的内容文件"
        );
        // 再读再迁移不应变化（幂等）
        save_settings(&dir, &migrated).unwrap();
        assert_eq!(migrate_legacy(load_settings(&dir), &dir), migrated);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn migration_skipped_when_no_legacy_url() {
        let dir =
            std::env::temp_dir().join(format!("tailash-migrate-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = Settings::default();
        assert_eq!(migrate_legacy(s.clone(), &dir), s);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sub_profile_files_roundtrip() {
        let dir = std::env::temp_dir().join(format!("tailash-subfile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        save_sub_profile(&dir, "s1", "proxies: []\n").unwrap();
        assert_eq!(
            load_sub_profile(&dir, "s1").as_deref(),
            Some("proxies: []\n")
        );
        // 覆盖写
        save_sub_profile(&dir, "s1", "proxies: [n]\n").unwrap();
        assert_eq!(
            load_sub_profile(&dir, "s1").as_deref(),
            Some("proxies: [n]\n")
        );
        assert!(load_sub_profile(&dir, "missing").is_none());
        remove_sub_profile(&dir, "s1");
        assert!(load_sub_profile(&dir, "s1").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn valid_id_rejects_traversal_and_empty() {
        assert!(valid_id("s18f3a"));
        assert!(valid_id("sub-1_a"));
        assert!(!valid_id(""));
        assert!(!valid_id("../../etc"));
        assert!(!valid_id("a/b"));
        assert!(!valid_id("a b"));
        assert!(!valid_id("../1"));
    }

    #[test]
    fn settings_missing_file_is_default() {
        let dir =
            std::env::temp_dir().join(format!("tailash-settings-missing-{}", std::process::id()));
        let got = load_settings(&dir);
        assert!(got.subscriptions.is_empty());
        assert_eq!(got.active, None);
    }

    #[test]
    fn settings_corrupt_file_is_default() {
        let dir = std::env::temp_dir().join(format!("tailash-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), "{not json").unwrap();
        let got = load_settings(&dir);
        assert!(got.subscriptions.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn validate_rejects_bad_yaml_and_missing_proxies() {
        assert!(validate_profile_text("{{{{").is_err());
        assert!(validate_profile_text("mode: rule\n").is_err());
        assert!(validate_profile_text("proxies: []\n").is_err());
        assert!(validate_profile_text("proxies:\n  - {name: n, type: vmess}\n").is_ok());
    }

    #[test]
    fn invalid_response_hides_url_and_valid_response_replaces_candidate() {
        use std::io::{Read, Write};
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/sub?token=secret", server.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            for body in ["rules: []", "proxies: [node]"] {
                let (mut socket, _) = server.accept().unwrap();
                socket.read(&mut [0; 4096]).unwrap();
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        let dir = std::env::temp_dir().join(format!("tailash-stage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pending = dir.join(PROFILE_STAGED);
        std::fs::write(&pending, "old").unwrap();
        let old = dir.join("old-inode");
        std::fs::hard_link(&pending, &old).unwrap();
        let error = format!("{:#}", stage_profile_file(&url, &pending).unwrap_err());
        assert!(error.contains("缺少 proxies") && !error.contains("token=secret"));
        assert_eq!(std::fs::read_to_string(&pending).unwrap(), "old");
        stage_profile_file(&url, &pending).unwrap();
        assert_eq!(
            std::fs::read_to_string(&pending).unwrap(),
            "proxies: [node]"
        );
        assert_eq!(std::fs::read_to_string(old).unwrap(), "old");
        thread.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn parses_sub_name_from_headers() {
        assert_eq!(
            parse_sub_name("profile-title: 机场A\r\n"),
            Some("机场A".into())
        );
        assert_eq!(
            parse_sub_name("profile-title: base64:5py65Zy6QQ==\r\n"),
            Some("机场A".into())
        );
        // 重定向链：取最后一个响应的头
        assert_eq!(
            parse_sub_name("profile-title: 旧\r\nprofile-title: 新\r\n"),
            Some("新".into())
        );
        assert_eq!(
            parse_sub_name(
                "Content-Disposition: attachment; filename*=UTF-8''%E6%9C%BA%E5%9C%BA%41.yaml; filename=\"sub.yaml\"\r\n"
            ),
            Some("机场A.yaml".into())
        );
        assert_eq!(
            parse_sub_name("Content-Disposition: attachment; filename=\"sub.yaml\"\r\n"),
            Some("sub.yaml".into())
        );
        assert_eq!(parse_sub_name("HTTP/1.1 200 OK\r\n"), None);
        assert_eq!(
            parse_sub_name("profile-title: base64:%%%\r\n"),
            Some("base64:%%%".into())
        );
    }

    #[test]
    fn fetch_takes_name_from_response_headers() {
        use std::io::{Read, Write};
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/sub", server.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = server.accept().unwrap();
            socket.read(&mut [0; 4096]).unwrap();
            let body = "proxies: [node]\n";
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nprofile-title: base64:5py65Zy6QQ==\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let fetched = fetch_subscribe(&url).unwrap();
        thread.join().unwrap();
        assert_eq!(fetched.name.as_deref(), Some("机场A"));
        assert!(fetched.text.contains("proxies"));
    }

    #[test]
    fn fetch_rejects_bad_url_and_missing_binary() {
        // 空 / 非 http
        assert!(fetch_subscribe("").is_err());
        assert!(fetch_subscribe("ftp://x").is_err());
        // 无网络时不设断言；只要 scheme 校验先于命令执行即可
        let _ = fetch_subscribe("https://127.0.0.1:1/sub");
    }

    #[test]
    fn auto_update_due_only_when_enabled_and_interval_elapsed() {
        // 关闭：无论多久都不到期
        assert!(!auto_update_due(None, Some(0), 10_000_000));
        assert!(!auto_update_due(Some(0), None, 10_000_000));
        // 从未更新过：立即到期
        assert!(auto_update_due(Some(6), None, 1));
        // 未到间隔 / 刚好到间隔
        assert!(!auto_update_due(Some(6), Some(1000), 1000 + 6 * 3600 - 1));
        assert!(auto_update_due(Some(6), Some(1000), 1000 + 6 * 3600));
        // 时钟回拨（last > now）不刷屏：按未到期处理
        assert!(!auto_update_due(Some(6), Some(9999), 1000));
    }

    #[test]
    fn tmp_rename_atomic() {
        let dir = std::env::temp_dir().join(format!("tailash-prof-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("profile.yaml");
        let staged = dir.join(PROFILE_STAGED);
        std::fs::write(&target, "old").unwrap();
        std::fs::write(&staged, "new").unwrap();
        atomic_replace(&staged, &target).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        assert!(!staged.exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
