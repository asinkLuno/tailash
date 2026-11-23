//! 运行时约定：tailash 生成配置，优先经 systemd 用户服务启动 mihomo；
//! 无 systemd 用户实例时，直接以脱离会话的后台进程方式管理 mihomo（退出 GUI 不受影响）。
//!
//! 约定的位置都在数据目录（workdir）下：
//! - `config.yaml`：由订阅生成，内核以 `mihomo -d <workdir> -f <workdir>/config.yaml` 启动；
//! - `mihomo.sock`：配置里写好的 external-controller-unix，GUI 经它读状态、下发命令；
//! - `mihomo.pid` / `mihomo.log`：无 systemd 模式下记录后台进程 PID 与运行日志。
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

    pub fn pid_path(&self) -> std::path::PathBuf {
        self.workdir.join("mihomo.pid")
    }

    pub fn log_path(&self) -> std::path::PathBuf {
        self.workdir.join("mihomo.log")
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

/// 内核进程管理器：优先使用 systemd 用户服务，不可用时回退到脱离会话的后台进程。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreManager {
    Systemd,
    Direct,
}

impl CoreManager {
    pub fn detect() -> Self {
        if systemd_available() {
            Self::Systemd
        } else {
            Self::Direct
        }
    }

    pub fn uses_systemd(self) -> bool {
        matches!(self, Self::Systemd)
    }

    pub fn is_active(self, cfg: &RuntimeCfg) -> bool {
        match self {
            Self::Systemd => service_active(),
            Self::Direct => direct_active(cfg),
        }
    }

    pub fn start(self, cfg: &RuntimeCfg) -> Result<()> {
        match self {
            Self::Systemd => service_start(&cfg.workdir),
            Self::Direct => direct_start(cfg),
        }
    }

    pub fn stop(self, cfg: &RuntimeCfg) -> Result<()> {
        match self {
            Self::Systemd => service_stop(),
            Self::Direct => direct_stop(cfg),
        }
    }
}

/// `systemctl --user` 是否可用（systemd 用户实例在线）。
pub fn systemd_available() -> bool {
    systemctl_cmd(&["is-system-running"])
        .output()
        .is_ok_and(|out| {
            let state = String::from_utf8_lossy(&out.stdout);
            // degraded = 在运行但有个别单元失败；is-system-running 对 degraded 返回码非 0，
            // 所以只看 stdout 文本，并要求明确为 running/degraded（排除空输出、offline、starting 等）。
            matches!(state.trim(), "running" | "degraded")
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

/// PID 文件里记录的进程标识：PID + `/proc/<pid>/stat` 启动时间戳，防止 PID 复用导致误杀。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PidRecord {
    pid: u32,
    starttime: Option<u64>,
}

/// 解析 `/proc/<pid>/stat`，返回 `(进程状态字符, starttime)`。
/// 第 2 字段 `(comm)` 可能包含空格甚至括号，因此从最后一个 `')'` 之后切分。
fn parse_proc_stat(stat: &str) -> Option<(char, u64)> {
    let (_, rest) = stat.rsplit_once(')')?;
    let mut fields = rest.split_ascii_whitespace();
    let state = fields.next()?.chars().next()?;
    // 字段 3 是 state（已消费），字段 22 是 starttime → 跳过字段 4..=21 共 18 项。
    let starttime = fields.nth(18)?.parse::<u64>().ok()?;
    Some((state, starttime))
}

fn proc_info(pid: u32) -> Option<(char, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_proc_stat(&stat)
}

fn proc_alive(pid: u32, expected_starttime: Option<u64>) -> bool {
    if let Some((state, starttime)) = proc_info(pid) {
        if matches!(state, 'Z' | 'X' | 'x') {
            return false;
        }
        return expected_starttime.is_none_or(|expected| starttime == expected);
    }
    if !std::path::Path::new("/proc/self").exists() && expected_starttime.is_none() {
        // SAFETY: signal 0 仅探测进程是否存在，不发送信号。
        return unsafe { libc::kill(pid as libc::pid_t, 0) == 0 };
    }
    false
}

fn parse_pid_file(text: &str) -> Option<PidRecord> {
    let mut parts = text.split_ascii_whitespace();
    let pid = parts.next()?.parse::<u32>().ok()?;
    if pid == 0 {
        return None;
    }
    let starttime = match parts.next() {
        Some(raw) => Some(raw.parse::<u64>().ok()?),
        None => None,
    };
    Some(PidRecord { pid, starttime })
}

fn read_pid_file(path: &std::path::Path) -> Option<PidRecord> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_pid_file(&text)
}

fn write_pid_file(path: &std::path::Path, record: PidRecord) -> Result<()> {
    let content = match record.starttime {
        Some(start) => format!("{} {start}\n", record.pid),
        None => format!("{}\n", record.pid),
    };
    std::fs::write(path, content).with_context(|| format!("写 {} 失败", path.display()))
}

/// 通过 Unix domain socket 的 `SO_PEERCRED` 获取正在监听 `mihomo.sock` 的同用户进程 PID。
#[cfg(target_os = "linux")]
fn socket_peer_pid(socket_path: &std::path::Path) -> Option<u32> {
    use std::os::fd::AsRawFd;
    let stream = std::os::unix::net::UnixStream::connect(socket_path).ok()?;
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: cred 与 len 有效且对齐，fd 来自刚建立的 UnixStream。
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    // SAFETY: getuid 无副作用、不会失败。
    let my_uid = unsafe { libc::getuid() };
    if rc == 0 && cred.pid > 0 && cred.pid as u32 != std::process::id() && cred.uid == my_uid {
        Some(cred.pid as u32)
    } else {
        None
    }
}

#[cfg(not(target_os = "linux"))]
fn socket_peer_pid(_socket_path: &std::path::Path) -> Option<u32> {
    None
}

/// 查找当前活跃的直启内核进程：优先查 `mihomo.sock` 对端 PID，再回退到 `mihomo.pid`。
fn find_direct_process(cfg: &RuntimeCfg) -> Option<PidRecord> {
    if let Some(pid) = socket_peer_pid(&cfg.socket_path()) {
        let starttime = proc_info(pid).map(|(_, start)| start);
        return Some(PidRecord { pid, starttime });
    }
    let pid_path = cfg.pid_path();
    if let Some(rec) = read_pid_file(&pid_path) {
        if proc_alive(rec.pid, rec.starttime) {
            return Some(rec);
        }
        let _ = std::fs::remove_file(&pid_path);
    }
    None
}

/// 无 systemd 模式下内核是否在运行。
pub fn direct_active(cfg: &RuntimeCfg) -> bool {
    find_direct_process(cfg).is_some()
}

fn last_non_empty_line(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let line = text.lines().rev().find(|l| !l.trim().is_empty())?.trim();
    const MAX_CHARS: usize = 200;
    let clipped: String = line.chars().take(MAX_CHARS).collect();
    Some(clipped)
}

/// 无 systemd 模式下直接脱离会话启动 mihomo 后台进程（退出 GUI 不影响）。
pub fn direct_start(cfg: &RuntimeCfg) -> Result<()> {
    if direct_active(cfg) {
        return Ok(());
    }
    let binary = find_mihomo()?;
    if !cfg.config_path().is_file() {
        if cfg.profile_path.is_file() {
            write_config(cfg)?;
        } else {
            anyhow::bail!("尚未生成配置文件，请先添加并更新订阅");
        }
    }
    let workdir = cfg.workdir.to_string_lossy().into_owned();
    let config = cfg.config_path().to_string_lossy().into_owned();
    spawn_detached(cfg, &binary, &["-d", &workdir, "-f", &config])
}

fn spawn_detached(cfg: &RuntimeCfg, binary: &std::path::Path, args: &[&str]) -> Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    if direct_active(cfg) {
        return Ok(());
    }
    std::fs::create_dir_all(&cfg.workdir).context("创建运行目录失败")?;

    // 清理上次崩溃遗留的死 socket，避免新进程 bind 失败；若 socket 仍有监听者则直接视为已运行。
    let sock = cfg.socket_path();
    if sock.exists() {
        match std::os::unix::net::UnixStream::connect(&sock) {
            Ok(_) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                let _ = std::fs::remove_file(&sock);
            }
            Err(_) => {}
        }
    }

    let log_path = cfg.log_path();
    let (stdout, stderr) = match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_path)
    {
        Ok(file) => match file.try_clone() {
            Ok(err_file) => (Stdio::from(file), Stdio::from(err_file)),
            Err(_) => (Stdio::from(file), Stdio::null()),
        },
        Err(_) => (Stdio::null(), Stdio::null()),
    };

    let mut cmd = std::process::Command::new(binary);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    // SAFETY: setsid 是 POSIX async-signal-safe 系统调用，使子进程脱离当前会话/终端，
    // 退出 GUI 或终端发送 SIGINT/SIGHUP 均不会波及内核。
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("启动 {} 失败", binary.display()))?;
    let pid = child.id();

    // 短暂探测是否立刻退出（如配置错误），若 socket 提前就绪则立刻返回。
    for _ in 0..5 {
        if let Some(status) = child.try_wait().context("检查内核进程状态失败")? {
            let detail = last_non_empty_line(&log_path).unwrap_or_default();
            if detail.is_empty() {
                anyhow::bail!("内核启动后立即退出（{status}）");
            }
            anyhow::bail!("内核启动后立即退出（{status}）：{detail}");
        }
        if std::os::unix::net::UnixStream::connect(&sock).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }

    let starttime = proc_info(pid).map(|(_, start)| start);
    write_pid_file(&cfg.pid_path(), PidRecord { pid, starttime })?;

    // 在 GUI 存活期间负责回收子进程，避免停止或崩溃后残留僵尸进程；
    // GUI 退出时子进程因已 setsid 会被 init 接管并继续运行。
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// 无 systemd 模式下停止内核：先发 SIGTERM 让 mihomo 清理 TUN/路由，超时再 SIGKILL。
pub fn direct_stop(cfg: &RuntimeCfg) -> Result<()> {
    let Some(proc_rec) = find_direct_process(cfg) else {
        let _ = std::fs::remove_file(cfg.pid_path());
        return Ok(());
    };
    // SAFETY: proc_rec.pid 已由 SO_PEERCRED（同 UID）或 PID+starttime 校验。
    let rc = unsafe { libc::kill(proc_rec.pid as libc::pid_t, libc::SIGTERM) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::ESRCH) {
            anyhow::bail!("停止内核进程（PID {}）失败: {err}", proc_rec.pid);
        }
    }

    for _ in 0..60 {
        if !proc_alive(proc_rec.pid, proc_rec.starttime) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    if proc_alive(proc_rec.pid, proc_rec.starttime) {
        // SAFETY: SIGTERM 3 秒内未退出，兜底强制结束。
        let _ = unsafe { libc::kill(proc_rec.pid as libc::pid_t, libc::SIGKILL) };
        for _ in 0..10 {
            if !proc_alive(proc_rec.pid, proc_rec.starttime) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        anyhow::ensure!(
            !proc_alive(proc_rec.pid, proc_rec.starttime),
            "停止内核超时（PID {} 未退出）",
            proc_rec.pid
        );
    }

    let _ = std::fs::remove_file(cfg.pid_path());
    let sock = cfg.socket_path();
    if sock.exists() && std::os::unix::net::UnixStream::connect(&sock).is_err() {
        let _ = std::fs::remove_file(&sock);
    }
    Ok(())
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

#[cfg(test)]
mod direct_process_tests {
    use super::*;

    fn test_cfg(tag: &str) -> RuntimeCfg {
        let dir = std::env::temp_dir().join(format!(
            "tailash-direct-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        RuntimeCfg {
            profile_path: dir.join("profile.yaml"),
            workdir: dir,
        }
    }

    #[test]
    fn parse_proc_stat_handles_spaces_and_parens_in_comm() {
        let line = "4242 (my (weird) proc) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 987654 23 24";
        assert_eq!(parse_proc_stat(line), Some(('S', 987654)));
        let zombie = "4242 (mihomo) Z 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 111 23";
        assert_eq!(parse_proc_stat(zombie), Some(('Z', 111)));
    }

    #[test]
    fn pid_file_parses_valid_and_rejects_invalid() {
        assert_eq!(
            parse_pid_file("1234 56789\n"),
            Some(PidRecord {
                pid: 1234,
                starttime: Some(56789),
            })
        );
        assert_eq!(
            parse_pid_file("1234\n"),
            Some(PidRecord {
                pid: 1234,
                starttime: None,
            })
        );
        assert_eq!(parse_pid_file("0 123\n"), None);
        assert_eq!(parse_pid_file("not-a-pid\n"), None);
    }

    #[test]
    fn stale_pid_with_mismatched_starttime_is_ignored_and_cleaned() {
        let cfg = test_cfg("stale-pid");
        let my_pid = std::process::id();
        let my_start = proc_info(my_pid).map(|(_, s)| s).unwrap_or(1);
        let bogus_start = my_start.wrapping_add(12345);
        write_pid_file(
            &cfg.pid_path(),
            PidRecord {
                pid: my_pid,
                starttime: Some(bogus_start),
            },
        )
        .unwrap();

        assert!(!direct_active(&cfg));
        assert!(!cfg.pid_path().exists(), "过期的 pid 文件应被自动清理");
        std::fs::remove_dir_all(&cfg.workdir).unwrap();
    }

    #[test]
    fn spawn_detached_starts_tracks_and_stops_cleanly() {
        let cfg = test_cfg("lifecycle");
        spawn_detached(
            &cfg,
            std::path::Path::new("/bin/sh"),
            &["-c", "exec sleep 30"],
        )
        .unwrap();

        assert!(direct_active(&cfg), "启动后应检测到进程在线");
        let rec = read_pid_file(&cfg.pid_path()).expect("应写入 pid 文件");
        assert_ne!(rec.pid, std::process::id());
        // 校验子进程已脱离当前会话（setsid）
        let child_sid = unsafe { libc::getsid(rec.pid as libc::pid_t) };
        assert_eq!(child_sid, rec.pid as libc::pid_t, "子进程应成为新会话首进程");

        direct_stop(&cfg).unwrap();
        assert!(!direct_active(&cfg), "停止后进程应退出");
        assert!(!cfg.pid_path().exists(), "停止后应清理 pid 文件");
        std::fs::remove_dir_all(&cfg.workdir).unwrap();
    }

    #[test]
    fn spawn_detached_reports_immediate_exit_with_log_tail() {
        let cfg = test_cfg("early-exit");
        let err = spawn_detached(
            &cfg,
            std::path::Path::new("/bin/sh"),
            &["-c", "echo bad-config-line >&2; exit 2"],
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("立即退出"), "{err}");
        assert!(err.contains("bad-config-line"), "{err}");
        assert!(!direct_active(&cfg));
        std::fs::remove_dir_all(&cfg.workdir).unwrap();
    }
}
