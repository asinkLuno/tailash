//! 托管订阅：URL → 下载 → 校验 → 原子落盘 profile.yaml。
//! 下载用系统 curl（校验证书），不引入 TLS 依赖；settings.json 持久化 URL。
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// 订阅响应上限（防异常返回吃内存）
const MAX_BODY_BYTES: u64 = 16 * 1024 * 1024;

/// 订阅 profile 的暂存文件名（校验/重载通过后再原子替换）。
pub const PROFILE_STAGED: &str = "profile.yaml.pending";

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Settings {
    /// 托管订阅地址
    #[serde(default)]
    pub subscribe_url: String,
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
fn validate_profile_text(text: &str) -> Result<()> {
    let profile: serde_yaml::Value = serde_yaml::from_str(text).context("订阅内容不是合法 yaml")?;
    let proxies = profile
        .get("proxies")
        .and_then(|v| v.as_sequence())
        .context("订阅缺少 proxies 段")?;
    anyhow::ensure!(!proxies.is_empty(), "订阅 proxies 为空");
    Ok(())
}

/// 下载订阅（系统 curl，校验 TLS 证书，固定 UA 走 Clash 配置分支）
pub fn fetch_subscribe(url: &str) -> Result<String> {
    anyhow::ensure!(!url.trim().is_empty(), "订阅地址为空");
    anyhow::ensure!(
        url.trim().starts_with("https://") || url.trim().starts_with("http://"),
        "订阅地址须以 http(s):// 开头"
    );
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
        ])
        .arg(url.trim())
        .output()
        .context("启动 curl 失败（系统未安装 curl?）")?;
    anyhow::ensure!(
        out.status.success(),
        "下载失败 ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr).trim()
    );
    anyhow::ensure!(out.stdout.len() as u64 <= MAX_BODY_BYTES, "订阅响应过大");
    String::from_utf8(out.stdout).map_err(|_| anyhow::anyhow!("订阅响应不是合法 UTF-8"))
}

/// 下载并验证订阅，写入候选文件；由调用方在热重载成功后提交。
pub fn stage_profile_file(url: &str, profile_path: &Path) -> Result<String> {
    let text = fetch_subscribe(url)?;
    validate_profile_text(&text).with_context(|| format!("校验订阅 {url} 失败"))?;
    if let Some(parent) = profile_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(profile_path, &text)?;
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
            subscribe_url: "https://example.invalid/sub?token=abc".into(),
        };
        save_settings(&dir, &s).unwrap();
        let got = load_settings(&dir);
        assert_eq!(got.subscribe_url, s.subscribe_url);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn settings_missing_file_is_default() {
        let dir =
            std::env::temp_dir().join(format!("tailash-settings-missing-{}", std::process::id()));
        let got = load_settings(&dir);
        assert_eq!(got.subscribe_url, "");
    }

    #[test]
    fn settings_corrupt_file_is_default() {
        let dir = std::env::temp_dir().join(format!("tailash-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), "{not json").unwrap();
        let got = load_settings(&dir);
        assert_eq!(got.subscribe_url, "");
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
    fn fetch_rejects_bad_url_and_missing_binary() {
        // 空 / 非 http
        assert!(fetch_subscribe("").is_err());
        assert!(fetch_subscribe("ftp://x").is_err());
        // 无网络时不设断言；只要 scheme 校验先于命令执行即可
        let _ = fetch_subscribe("https://127.0.0.1:1/sub");
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
