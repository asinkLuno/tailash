//! 运行时约定：tailash 生成配置，经 systemd 用户服务（或用户自己在终端）启动的 mihomo 内核读取它。
//!
//! 约定的位置都在数据目录（workdir）下：
//! - `config.yaml`：由订阅生成，内核以 `mihomo -d <workdir> -f <workdir>/config.yaml` 启动；
//! - `mihomo.sock`：配置里写好的 external-controller-unix，GUI 经它读状态、下发命令。
//!
//! API 不设 secret：socket 位于 0700 的数据目录里，文件权限已把其他用户挡在外面。
pub const TEST_URL: &str = "http://www.gstatic.com/generate_204";
/// `config.yaml` 的暂存文件名（写完再原子替换）。
pub const CONFIG_STAGED: &str = "config.yaml.pending";

#[derive(Clone)]
pub struct RuntimeCfg {
    pub profile_path: std::path::PathBuf,
    pub workdir: std::path::PathBuf,
}

impl RuntimeCfg {
    pub fn socket_path(&self) -> std::path::PathBuf {
        self.workdir.join("mihomo.sock")
    }

    pub fn config_path(&self) -> std::path::PathBuf {
        self.workdir.join("config.yaml")
    }

    pub fn client(&self) -> crate::backend::mihomo_api::MihomoClient {
        crate::backend::mihomo_api::MihomoClient::new(self.socket_path().to_string_lossy().to_string())
    }

    /// 没有 systemd 用户实例时，提示用户在终端执行的启动命令。
    pub fn start_command(&self) -> String {
        format!(
            "mihomo -d {} -f {}",
            self.workdir.display(),
            self.config_path().display()
        )
    }
}

use anyhow::{Context as _, Result};

/// 从内核拉取 `/proxies` 并和订阅 profile 合成展示模型。
///
/// 之所以还要读 profile：`/proxies` 是无序 map，组顺序和信息卡顺序只有订阅里才有；
/// 顺序一旦被重排，用户就会觉得"节点丢了"。
pub fn load_view(
    client: &crate::backend::mihomo_api::MihomoClient,
    profile_path: &std::path::Path,
) -> Result<crate::backend::proxy_view::ProxyView> {
    let profile = std::fs::read_to_string(profile_path)
        .ok()
        .and_then(|text| serde_yaml::from_str::<serde_yaml::Value>(&text).ok())
        .unwrap_or(serde_yaml::Value::Null);
    let entries = client.proxies()?;
    Ok(crate::backend::proxy_view::build(&profile, entries))
}

/// 用 `profile` 生成 mihomo 配置文本（socket 指向约定位置、无 secret）。
pub fn render_config(
    cfg: &RuntimeCfg,
    profile: &serde_yaml::Value,
    tun_enabled: Option<bool>,
) -> Result<String> {
    let socket = cfg.socket_path().to_string_lossy().to_string();
    crate::backend::config_gen::generate(profile, &socket, tun_enabled.unwrap_or(true))
}

/// 按当前订阅重写 `config.yaml`。geoip 数据由 mihomo 自己在工作目录下载维护。
/// 已在运行的内核不受影响（它只在启动/重载时读配置）。
pub fn write_config(cfg: &RuntimeCfg) -> Result<()> {
    std::fs::create_dir_all(&cfg.workdir).context("创建运行目录失败")?;
    let profile_text = std::fs::read_to_string(&cfg.profile_path).context("读订阅 profile 失败")?;
    let profile: serde_yaml::Value =
        serde_yaml::from_str(&profile_text).context("解析订阅 profile 失败")?;
    let text = render_config(cfg, &profile, None)?;
    let staged = cfg.workdir.join(CONFIG_STAGED);
    std::fs::write(&staged, text).context("写 config 失败")?;
    crate::backend::subscription::atomic_replace(&staged, &cfg.config_path())
}

/// tailash 不内置内核：找不到时把这句话原样展示给用户。
const MIHOMO_MISSING: &str = "找不到 mihomo 内核：请从 https://github.com/MetaCubeX/mihomo/releases 下载，\
     命名为 mihomo、加可执行权限后放进 PATH（如 ~/.local/bin）";

/// 找 mihomo 二进制：源码目录 bin/（开发用），再查 PATH；都没有就报错让用户自行下载。
pub fn find_mihomo() -> Result<std::path::PathBuf> {
    let local = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bin");
    let path = std::env::var_os("PATH").unwrap_or_default();
    find_mihomo_in(std::iter::once(local).chain(std::env::split_paths(&path)))
}

fn find_mihomo_in(
    dirs: impl IntoIterator<Item = std::path::PathBuf>,
) -> Result<std::path::PathBuf> {
    dirs.into_iter()
        .map(|dir| dir.join("mihomo"))
        .find(|candidate| is_executable(candidate))
        // setcap 拒绝对符号链接操作（Invalid file ... for capability operation），
        // 授权 TUN 需要真实文件路径，这里统一解析。
        .map(|candidate| std::fs::canonicalize(&candidate).unwrap_or(candidate))
        .ok_or_else(|| anyhow::Error::msg(MIHOMO_MISSING))
}

fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// mihomo 二进制是否已具备 cap_net_admin（TUN 必需）。getcap 不可用视为未知 → 返回 true（不拦截）。
pub fn has_cap_net_admin(binary: &std::path::Path) -> bool {
    match std::process::Command::new("getcap").arg(binary).output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout).contains("cap_net_admin"),
        Err(_) => true,
    }
}

/// TUN 网卡是否真的建起来了。`/configs` 里的 `tun.enable` 只是期望值，
/// 建卡失败（例如内核没 cap_net_admin）只写日志、不回错误，所以得看内核网卡列表。
/// shortcut: 只认 tun.device 或 mihomo 默认名 Meta，别处的同名网卡会误判。
pub fn tun_interface_up(device: Option<&str>) -> bool {
    let name = device.filter(|name| !name.is_empty()).unwrap_or("Meta");
    std::path::Path::new("/sys/class/net").join(name).exists()
}

/// systemd 用户服务的单元名与 `~/.config/systemd/user` 下的单元文件路径。
pub const SERVICE_UNIT: &str = "tailash-mihomo.service";

/// `systemctl --user` 可用（systemd 用户实例在线）才显示启停按钮。
pub fn systemd_available() -> bool {
    systemctl_cmd(&["is-system-running"])
        .output()
        .is_ok_and(|out| {
            let state = String::from_utf8_lossy(&out.stdout);
            // degraded = 在运行但有个别单元失败；只排除 offline/starting。
            // is-system-running 对 degraded 返回码非 0，所以只看 stdout 文本。
            !state.contains("offline")
                && !state.contains("starting")
                && !state.contains("maintenance")
        })
}

/// systemd 用户服务下内核是否已经启动（unknown/不存在视为未启动）。
pub fn service_active() -> bool {
    systemctl_cmd(&["is-active", SERVICE_UNIT])
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim() == "active")
}

/// 用户单元文件路径：`$XDG_CONFIG_HOME/systemd/user`（缺省 `~/.config`）。
fn service_file_path() -> Result<std::path::PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
        })
        .context("无法确定配置目录（无 HOME）")?;
    Ok(config.join("systemd/user").join(SERVICE_UNIT))
}

/// 写 systemd 用户单元；内容总是按当前内核路径与运行目录重写，内核换了位置也能跟上。
fn write_service_file(
    unit: &std::path::Path,
    binary: &std::path::Path,
    workdir: &std::path::Path,
) -> Result<()> {
    let content = format!(
        "[Unit]\nDescription=tailash 管理的 mihomo 内核\nAfter=network-online.target\n\n\
         [Service]\nType=simple\nExecStart=\"{bin}\" -d \"{dir}\" -f \"{dir}/config.yaml\"\n\
         Restart=on-failure\nRestartSec=1\n\n[Install]\nWantedBy=default.target\n",
        bin = binary.display(),
        dir = workdir.display(),
    );
    if std::fs::read_to_string(unit).is_ok_and(|old| old == content) {
        return Ok(());
    }
    let dir = unit.parent().context("单元文件路径无父目录")?;
    std::fs::create_dir_all(dir).with_context(|| format!("创建 {} 失败", dir.display()))?;
    std::fs::write(unit, content).with_context(|| format!("写 {} 失败", unit.display()))
}

/// 经 systemd 用户服务启动内核（后台运行、退出 GUI 不受影响）。
pub fn service_start(workdir: &std::path::Path) -> Result<()> {
    let binary = find_mihomo()?;
    write_service_file(&service_file_path()?, &binary, workdir)?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["start", SERVICE_UNIT])
}

/// 经 systemd 用户服务停止内核。
pub fn service_stop() -> Result<()> {
    systemctl(&["stop", SERVICE_UNIT])
}

/// 用 pkexec 提权给 mihomo 补上 cap_net_admin（唯一需要 root 的操作，弹系统授权窗）。
pub fn grant_cap_net_admin(binary: &std::path::Path) -> Result<()> {
    let status = std::process::Command::new("pkexec")
        .args(["setcap", "cap_net_admin=ep"])
        .arg(binary)
        .status()
        .map_err(|e| anyhow::Error::msg(format!("无法启动 pkexec: {e}")))?;
    anyhow::ensure!(status.success(), "授权未完成或被拒绝");
    Ok(())
}

/// 在当前用户会话里执行 `systemctl --user <args…>`。
fn systemctl(args: &[&str]) -> Result<()> {
    let out = systemctl_cmd(args)
        .output()
        .map_err(|e| anyhow::Error::msg(format!("无法执行 systemctl: {e}")))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    anyhow::ensure!(
        out.status.success(),
        "systemctl {} 失败: {}",
        args.join(" "),
        if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        }
    );
    Ok(())
}

/// `systemctl --user <args…>`；从其它环境（如 SSH -X）启动时可能缺 XDG_RUNTIME_DIR，补上。
fn systemctl_cmd(args: &[&str]) -> std::process::Command {
    let mut cmd = std::process::Command::new("systemctl");
    cmd.arg("--user").args(args);
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        // SAFETY: getuid 无副作用、不会失败。
        let uid = unsafe { libc::getuid() };
        cmd.env("XDG_RUNTIME_DIR", format!("/run/user/{uid}"));
    }
    cmd
}

#[cfg(test)]
mod service_file_tests {
    use super::*;

    #[test]
    fn unit_uses_the_real_workdir_and_is_rewritten_when_stale() {
        let root = std::env::temp_dir().join(format!("tailash-unit-{}", std::process::id()));
        let workdir = root.join("data dir");
        let binary = std::path::Path::new("/opt/mihomo");
        std::fs::create_dir_all(&root).unwrap();
        let unit = root.join(SERVICE_UNIT);
        std::fs::write(&unit, "旧内容").unwrap();

        write_service_file(&unit, binary, &workdir).unwrap();

        let text = std::fs::read_to_string(&unit).unwrap();
        let expected_exec = format!(
            "ExecStart=\"/opt/mihomo\" -d \"{0}\" -f \"{0}/config.yaml\"",
            workdir.display()
        );
        assert!(text.contains(&expected_exec), "{text}");
        assert!(!text.contains("旧内容"));
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod find_mihomo_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn dir_with_mihomo(name: &str, mode: u32) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tailash-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let binary = dir.join("mihomo");
        std::fs::write(&binary, b"").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(mode)).unwrap();
        dir
    }

    #[test]
    fn finds_the_first_executable_mihomo() {
        let plain = dir_with_mihomo("find-plain", 0o644);
        let exec = dir_with_mihomo("find-exec", 0o755);
        let found = find_mihomo_in([plain.clone(), exec.clone()]).unwrap();
        assert_eq!(found, exec.join("mihomo"));
        std::fs::remove_dir_all(plain).unwrap();
        std::fs::remove_dir_all(exec).unwrap();
    }

    #[test]
    fn missing_mihomo_tells_the_user_where_to_download_it() {
        let plain = dir_with_mihomo("find-missing", 0o644);
        let error = find_mihomo_in([plain.clone()]).unwrap_err().to_string();
        assert!(error.contains("找不到 mihomo"), "{error}");
        assert!(
            error.contains("https://github.com/MetaCubeX/mihomo/releases"),
            "{error}"
        );
        std::fs::remove_dir_all(plain).unwrap();
    }

    #[test]
    fn symlinked_binary_is_canonicalized_for_setcap() {
        // setcap 拒绝操作符号链接，授权 TUN 必须拿到真实文件路径
        let real = dir_with_mihomo("find-real", 0o755);
        let link_dir = std::env::temp_dir().join(format!("find-link-{}", std::process::id()));
        std::fs::create_dir_all(&link_dir).unwrap();
        std::os::unix::fs::symlink(real.join("mihomo"), link_dir.join("mihomo")).unwrap();

        let found = find_mihomo_in([link_dir.clone()]).unwrap();
        assert_eq!(
            found,
            std::fs::canonicalize(real.join("mihomo")).unwrap(),
            "符号链接应解析为真实路径"
        );
        std::fs::remove_dir_all(link_dir).unwrap();
        std::fs::remove_dir_all(real).unwrap();
    }
}

#[cfg(test)]
mod tun_interface_tests {
    use super::*;

    #[test]
    fn blank_device_falls_back_to_meta_and_unknown_names_are_down() {
        let meta = std::path::Path::new("/sys/class/net/Meta").exists();
        assert_eq!(tun_interface_up(None), meta);
        assert_eq!(tun_interface_up(Some("")), meta);
        assert!(!tun_interface_up(Some("tailash-no-such-if")));
    }
}
