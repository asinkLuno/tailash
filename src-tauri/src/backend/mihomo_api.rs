//! 通过 external-controller (unix socket HTTP RESTful) 控制 mihomo。
//! 协议：HTTP/1.1 over unix socket，JSON body，无鉴权
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Read, Write};

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

fn read_bounded_line(reader: &mut impl BufRead, limit: usize, error: &str) -> Result<String> {
    let mut line = String::new();
    loop {
        let available = reader.fill_buf()?;
        anyhow::ensure!(!available.is_empty(), "{error}");
        let take = available
            .iter()
            .position(|b| *b == b'\n')
            .map_or(available.len(), |i| i + 1);
        anyhow::ensure!(line.len() + take <= limit, "HTTP line too large");
        let done = available[..take].contains(&b'\n');
        line.push_str(&String::from_utf8_lossy(&available[..take]));
        reader.consume(take);
        if done {
            return Ok(line);
        }
    }
}
use std::os::unix::net::UnixStream;

/// IPC 序列化为 mihomo API 的小写形式（"rule"/"direct"/"global"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    #[default]
    Rule,
    Direct,
    Global,
}

impl ProxyMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Rule => "策略",
            Self::Direct => "直连",
            Self::Global => "全局",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Direct => "direct",
            Self::Global => "global",
        }
    }

    /// 解析 `GET /configs` 里的 `mode`（mihomo 返回小写，兼容大小写）。
    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "rule" => Some(Self::Rule),
            "direct" => Some(Self::Direct),
            "global" => Some(Self::Global),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct MihomoClient {
    socket_path: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Version {
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ProxyDelay {
    pub delay: u32,
}

/// `/traffic` 流每 ~1 秒推送的一帧流量采样（单位：字节/秒与累计字节）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficSample {
    #[serde(default)]
    pub up: u64,
    #[serde(default)]
    pub down: u64,
    #[serde(default)]
    pub up_total: u64,
    #[serde(default)]
    pub down_total: u64,
}

/// /proxies 里 history 的单条记录（只关心延迟值）
#[derive(Debug, Clone, serde::Deserialize)]
pub struct DelayRecord {
    pub delay: u32,
}

/// /proxies 里单个代理条目（裁剪版）。
///
/// 关键点：`all` 存在说明这是一"组"（selector/url-test/…），不存在才是真节点。
/// 用 `all` 判组比用 `type` 字符串黑名单稳（新组类型不会漏判）。
/// 组顺序与组内成员顺序都由 mihomo 给出，不要自己排序，否则会"丢节点"。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ProxyEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub now: Option<String>,
    /// 仅组有：成员名单（顺序即 mihomo 的选择顺序）
    #[serde(default)]
    pub all: Option<Vec<String>>,
    /// 最近若干次延迟记录，最后一条即当前展示值
    #[serde(default)]
    pub history: Vec<DelayRecord>,
    /// UDP 能力（节点才有意义）
    #[serde(default)]
    pub udp: bool,
}

impl ProxyEntry {
    pub fn is_group(&self) -> bool {
        self.all.is_some()
    }

    /// history 最后一条；0 表示超时（保留 0，由 UI 分类展示）
    pub fn last_delay(&self) -> Option<u32> {
        self.history.last().map(|h| h.delay)
    }
}
impl MihomoClient {
    pub fn new(socket_path: impl Into<String>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    fn connect(&self) -> Result<UnixStream> {
        UnixStream::connect(&self.socket_path)
            .with_context(|| format!("connect unix socket {}", self.socket_path))
    }

    /// 执行一次 HTTP 请求，返回 (status, body_bytes)
    fn request(&self, method: &str, path: &str, body: Option<&str>) -> Result<(u16, String)> {
        let mut stream = self.connect()?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
        let body_str = body.unwrap_or("");
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body_str}",
            len = body_str.len()
        );
        stream.write_all(req.as_bytes())?;
        let mut reader = BufReader::new(stream);
        let status_line = read_bounded_line(&mut reader, 8192, "empty HTTP response")?;
        let mut status_parts = status_line.split_whitespace();
        anyhow::ensure!(
            status_parts.next() == Some("HTTP/1.1"),
            "bad HTTP status line"
        );
        let status: u16 = status_parts
            .next()
            .context("missing HTTP status")?
            .parse()
            .context("invalid HTTP status")?;
        anyhow::ensure!((100..=599).contains(&status), "HTTP status out of range");

        let mut content_length = None;
        let mut chunked = false;
        let mut header_bytes = 0usize;
        loop {
            let line = read_bounded_line(&mut reader, 8192, "incomplete HTTP headers")?;
            header_bytes += line.len();
            anyhow::ensure!(header_bytes <= 64 * 1024, "HTTP headers too large");
            if line == "\r\n" || line == "\n" {
                break;
            }
            let (name, value) = line
                .trim_end_matches(['\r', '\n'])
                .split_once(':')
                .context("malformed HTTP header")?;
            if name.eq_ignore_ascii_case("content-length") {
                let parsed: usize = value.trim().parse().context("invalid content-length")?;
                anyhow::ensure!(parsed <= MAX_RESPONSE_BYTES, "HTTP response too large");
                if let Some(previous) = content_length {
                    anyhow::ensure!(previous == parsed, "conflicting content-length headers");
                }
                content_length = Some(parsed);
            } else if name.eq_ignore_ascii_case("transfer-encoding") {
                anyhow::ensure!(
                    value.trim().eq_ignore_ascii_case("chunked"),
                    "unsupported transfer-encoding"
                );
                chunked = true;
            }
        }
        anyhow::ensure!(
            !(chunked && content_length.is_some()),
            "conflicting HTTP framing headers"
        );

        let mut body = Vec::new();
        if chunked {
            loop {
                let size_line = read_bounded_line(&mut reader, 8192, "incomplete chunk header")?;
                let size_text = size_line
                    .trim_end_matches(['\r', '\n'])
                    .split(';')
                    .next()
                    .unwrap_or("");
                let size = usize::from_str_radix(size_text, 16).context("invalid chunk size")?;
                anyhow::ensure!(
                    size <= MAX_RESPONSE_BYTES - body.len(),
                    "HTTP response too large"
                );
                if size == 0 {
                    loop {
                        let trailer =
                            read_bounded_line(&mut reader, 8192, "incomplete chunk trailers")?;
                        if trailer == "\r\n" || trailer == "\n" {
                            break;
                        }
                        anyhow::ensure!(trailer.contains(':'), "malformed chunk trailer");
                    }
                    break;
                }
                let old_len = body.len();
                body.resize(old_len + size, 0);
                reader.read_exact(&mut body[old_len..])?;
                let mut crlf = [0u8; 2];
                reader.read_exact(&mut crlf)?;
                anyhow::ensure!(crlf == *b"\r\n", "invalid chunk terminator");
            }
        } else if let Some(length) = content_length {
            body.resize(length, 0);
            reader.read_exact(&mut body)?;
        } else {
            reader
                .take((MAX_RESPONSE_BYTES + 1) as u64)
                .read_to_end(&mut body)?;
            anyhow::ensure!(body.len() <= MAX_RESPONSE_BYTES, "HTTP response too large");
        }
        Ok((status, String::from_utf8_lossy(&body).into_owned()))
    }

    pub fn version(&self) -> Result<Version> {
        let (st, body) = self.request("GET", "/version", None)?;
        anyhow::ensure!(st == 200, "GET /version -> {st}");
        Ok(serde_json::from_str(&body)?)
    }

    pub fn proxies(&self) -> Result<Vec<ProxyEntry>> {
        let (st, body) = self.request("GET", "/proxies", None)?;
        anyhow::ensure!(st == 200, "GET /proxies -> {st}");
        #[derive(serde::Deserialize)]
        struct Wrapper {
            #[serde(default)]
            proxies: std::collections::HashMap<String, ProxyEntry>,
        }
        let w: Wrapper = serde_json::from_str(&body)?;
        let mut v: Vec<ProxyEntry> = w.proxies.into_values().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(v)
    }

    /// 在一个 select 组里选中某节点
    pub fn select(&self, group: &str, node: &str) -> Result<()> {
        let payload = serde_json::json!({ "name": node }).to_string();
        let path = format!("/proxies/{}", urlenc(group));
        let (st, body) = self.request("PUT", &path, Some(&payload))?;
        anyhow::ensure!(st == 204 || st == 200, "PUT {path} -> {st} {body}");
        Ok(())
    }

    /// 对代理测延迟
    pub fn delay(&self, name: &str, timeout_ms: u32, test_url: &str) -> Result<ProxyDelay> {
        let path = format!(
            "/proxies/{}/delay?timeout={}&url={}",
            urlenc(name),
            timeout_ms,
            urlencode(test_url)
        );
        let (st, body) = self.request("GET", &path, None)?;
        anyhow::ensure!(st == 200, "delay {name} -> {st} {body}");
        Ok(serde_json::from_str(&body)?)
    }

    /// 测试整个组。内核借此重跑该组的 url-test 选择逻辑：
    /// 外部对单个节点的测速不会触发组内重算（fastNode 有 tolerance 且周期独立）。
    pub fn group_delay(&self, name: &str, timeout_ms: u32, test_url: &str) -> Result<()> {
        let path = format!(
            "/group/{}/delay?timeout={}&url={}",
            urlenc(name),
            timeout_ms,
            urlencode(test_url)
        );
        let (st, body) = self.request("GET", &path, None)?;
        anyhow::ensure!(st == 200, "group delay {name} -> {st} {body}");
        Ok(())
    }

    /// 开关 TUN
    pub fn set_tun(&self, enable: bool) -> Result<()> {
        let payload = serde_json::json!({ "tun": { "enable": enable } }).to_string();
        let (st, body) = self.request("PATCH", "/configs", Some(&payload))?;
        anyhow::ensure!(st == 204 || st == 200, "PATCH /configs -> {st} {body}");
        Ok(())
    }

    /// 更新当前运行模式（mihomo 不重启，现有规则配置不变）。
    pub fn set_mode(&self, mode: ProxyMode) -> Result<()> {
        let payload = serde_json::json!({ "mode": mode.as_str() }).to_string();
        let (st, body) = self.request("PATCH", "/configs", Some(&payload))?;
        anyhow::ensure!(st == 204 || st == 200, "PATCH /configs -> {st} {body}");
        Ok(())
    }

    /// 当前全局状态（含 tun.enable）
    pub fn configs(&self) -> Result<serde_json::Value> {
        let (st, body) = self.request("GET", "/configs", None)?;
        anyhow::ensure!(st == 200, "GET /configs -> {st}");
        Ok(serde_json::from_str(&body)?)
    }

    /// 重新加载配置（PUT /configs?force=true）
    pub fn reload(&self, path: &str) -> Result<()> {
        let payload = serde_json::json!({ "path": path }).to_string();
        let (st, body) = self.request("PUT", "/configs?force=true", Some(&payload))?;
        anyhow::ensure!(st == 204 || st == 200, "PUT /configs -> {st} {body}");
        Ok(())
    }

    /// 清空内核的 Fake-IP 缓存（`POST /cache/fakeip/flush`）。
    /// 切换/更新订阅后调用，避免旧订阅的 fake-ip-range 或 fake-ip-filter 残留影响分流。
    pub fn flush_fakeip_cache(&self) -> Result<()> {
        let (st, body) = self.request("POST", "/cache/fakeip/flush", None)?;
        anyhow::ensure!(
            st == 204 || st == 200,
            "POST /cache/fakeip/flush -> {st} {body}"
        );
        Ok(())
    }

    /// 订阅实时流量流（`GET /traffic`，mihomo 每秒推一帧 JSON）。
    /// 当 `cancel` 置为 true 或对端关闭连接时返回 `Ok(())`。
    pub fn stream_traffic(
        &self,
        cancel: &std::sync::atomic::AtomicBool,
        on_sample: impl FnMut(TrafficSample),
    ) -> Result<()> {
        let mut stream = self.connect()?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        stream.write_all(
            b"GET /traffic HTTP/1.1\r\nHost: localhost\r\nAccept: application/json\r\n\r\n",
        )?;
        let mut reader = BufReader::new(stream);
        read_traffic_stream(&mut reader, cancel, on_sample)
    }
}

fn read_stream_line(
    reader: &mut impl BufRead,
    limit: usize,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Option<String>> {
    use std::io::ErrorKind;
    use std::sync::atomic::Ordering;
    let mut line = String::new();
    let mut timeouts = 0u8;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        match reader.fill_buf() {
            Ok([]) => {
                if line.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(line));
            }
            Ok(available) => {
                timeouts = 0;
                let take = available
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(available.len(), |i| i + 1);
                anyhow::ensure!(line.len() + take <= limit, "HTTP stream line too large");
                let done = available[..take].contains(&b'\n');
                line.push_str(&String::from_utf8_lossy(&available[..take]));
                reader.consume(take);
                if done {
                    return Ok(Some(line));
                }
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                if cancel.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                timeouts += 1;
                anyhow::ensure!(timeouts < 5, "traffic stream timed out");
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn read_exact_cancelable(
    reader: &mut impl Read,
    mut buf: &mut [u8],
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<bool> {
    use std::io::ErrorKind;
    use std::sync::atomic::Ordering;
    let mut timeouts = 0u8;
    while !buf.is_empty() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        match reader.read(buf) {
            Ok(0) => return Ok(false),
            Ok(n) => {
                timeouts = 0;
                buf = &mut buf[n..];
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                if cancel.load(Ordering::Relaxed) {
                    return Ok(false);
                }
                timeouts += 1;
                anyhow::ensure!(timeouts < 5, "traffic stream timed out");
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(true)
}

fn read_traffic_stream(
    reader: &mut impl BufRead,
    cancel: &std::sync::atomic::AtomicBool,
    mut on_sample: impl FnMut(TrafficSample),
) -> Result<()> {
    use std::sync::atomic::Ordering;
    let Some(status_line) = read_stream_line(reader, 8192, cancel)? else {
        return Ok(());
    };
    let mut parts = status_line.split_whitespace();
    anyhow::ensure!(parts.next() == Some("HTTP/1.1"), "bad HTTP status line");
    let status: u16 = parts
        .next()
        .context("missing HTTP status")?
        .parse()
        .context("invalid HTTP status")?;
    anyhow::ensure!(status == 200, "GET /traffic -> {status}");

    let mut chunked = false;
    let mut header_bytes = 0usize;
    loop {
        let Some(line) = read_stream_line(reader, 8192, cancel)? else {
            return Ok(());
        };
        header_bytes += line.len();
        anyhow::ensure!(header_bytes <= 64 * 1024, "HTTP headers too large");
        if line == "\r\n" || line == "\n" {
            break;
        }
        let (name, value) = line
            .trim_end_matches(['\r', '\n'])
            .split_once(':')
            .context("malformed HTTP header")?;
        if name.eq_ignore_ascii_case("transfer-encoding")
            && value.trim().eq_ignore_ascii_case("chunked")
        {
            chunked = true;
        }
    }

    if chunked {
        let mut pending = Vec::new();
        while !cancel.load(Ordering::Relaxed) {
            let Some(size_line) = read_stream_line(reader, 8192, cancel)? else {
                break;
            };
            let size_text = size_line
                .trim_end_matches(['\r', '\n'])
                .split(';')
                .next()
                .unwrap_or("")
                .trim();
            if size_text.is_empty() {
                continue;
            }
            let size = usize::from_str_radix(size_text, 16).context("invalid chunk size")?;
            anyhow::ensure!(size <= 64 * 1024, "traffic chunk too large");
            if size == 0 {
                break;
            }
            let mut chunk = vec![0u8; size];
            if !read_exact_cancelable(reader, &mut chunk, cancel)? {
                break;
            }
            let mut crlf = [0u8; 2];
            if !read_exact_cancelable(reader, &mut crlf, cancel)? {
                break;
            }
            anyhow::ensure!(crlf == *b"\r\n", "invalid chunk terminator");
            pending.extend_from_slice(&chunk);
            anyhow::ensure!(pending.len() <= 64 * 1024, "traffic buffer too large");
            while let Some(pos) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = pending.drain(..=pos).collect();
                let trimmed = String::from_utf8_lossy(&line);
                let trimmed = trimmed.trim();
                if !trimmed.is_empty()
                    && let Ok(sample) = serde_json::from_str::<TrafficSample>(trimmed)
                {
                    on_sample(sample);
                }
            }
        }
    } else {
        while !cancel.load(Ordering::Relaxed) {
            let Some(line) = read_stream_line(reader, 64 * 1024, cancel)? else {
                break;
            };
            let trimmed = line.trim();
            if !trimmed.is_empty()
                && let Ok(sample) = serde_json::from_str::<TrafficSample>(trimmed)
            {
                on_sample(sample);
            }
        }
    }
    Ok(())
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn urlenc(s: &str) -> String {
    // mihomo 路径段接受 RFC3986 unreserved，中文/空格都要编码
    urlencode(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_modes_match_mihomo_config_values() {
        assert_eq!(ProxyMode::Rule.as_str(), "rule");
        assert_eq!(ProxyMode::Direct.as_str(), "direct");
        assert_eq!(ProxyMode::Global.as_str(), "global");
    }

    fn parse_response(response: &str) -> anyhow::Result<(u16, String)> {
        let path = std::env::temp_dir().join(format!(
            "tailash-http-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let listener = std::os::unix::net::UnixListener::bind(&path)?;
        let response = response.as_bytes().to_vec();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while request.read_line(&mut line).unwrap() > 0 && line != "\r\n" {
                line.clear();
            }
            stream.write_all(&response).unwrap();
        });
        let result = MihomoClient::new(path.to_string_lossy()).request("GET", "/", None);
        server.join().unwrap();
        let _ = std::fs::remove_file(path);
        result
    }

    #[test]
    fn response_parser_handles_content_length_and_chunk_extensions() {
        assert_eq!(
            parse_response("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").unwrap(),
            (200, "ok".into())
        );
        assert_eq!(parse_response("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2;foo=bar\r\nok\r\n0\r\nX-Trailer: yes\r\n\r\n").unwrap(), (200, "ok".into()));
    }

    #[test]
    fn response_parser_rejects_invalid_or_ambiguous_framing() {
        assert!(parse_response("HTTP/1.1 200 OK\r\nContent-Length: nope\r\n\r\n").is_err());
        assert!(parse_response("HTTP/1.1 200 OK\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n").is_err());
        assert!(
            parse_response("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nx\r\n0\r\n\r\n")
                .is_err()
        );
    }

    #[test]
    fn test_urlencode() {
        assert_eq!(
            urlencode("节点选择"),
            "%E8%8A%82%E7%82%B9%E9%80%89%E6%8B%A9"
        );
        assert_eq!(urlencode("abc123"), "abc123");
        assert_eq!(urlencode("a b/c"), "a%20b%2Fc");
    }

    #[test]
    fn test_parse_version() {
        let v: Version = serde_json::from_str(r#"{"meta":true,"version":"v1.19.34"}"#).unwrap();
        assert_eq!(v.version, "v1.19.34");
    }

    #[test]
    fn traffic_stream_parses_chunked_and_split_json_lines() {
        let first = "{\"up\":120,\"down\":3400,\"upTotal\":500,\"downTotal\":9000}\n";
        let second_a = "{\"up\":50,\"do";
        let second_b = "wn\":800}\n";
        let raw = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
             {:x}\r\n{}\r\n{:x}\r\n{}\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            first.len(),
            first,
            second_a.len(),
            second_a,
            second_b.len(),
            second_b,
        );
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut cursor = std::io::Cursor::new(raw.into_bytes());
        let mut got = Vec::new();
        read_traffic_stream(&mut cursor, &cancel, |s| got.push(s)).unwrap();
        assert_eq!(
            got,
            vec![
                TrafficSample {
                    up: 120,
                    down: 3400,
                    up_total: 500,
                    down_total: 9000,
                },
                TrafficSample {
                    up: 50,
                    down: 800,
                    up_total: 0,
                    down_total: 0,
                },
            ]
        );
    }

    #[test]
    fn traffic_stream_parses_ndjson_and_stops_on_cancel() {
        let raw = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n\
                   {\"up\":1,\"down\":2}\n\
                   {\"up\":3,\"down\":4}\n";
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut cursor = std::io::Cursor::new(raw.as_bytes());
        let mut got = Vec::new();
        read_traffic_stream(&mut cursor, &cancel, |s| {
            got.push(s);
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        })
        .unwrap();
        assert_eq!(
            got,
            vec![TrafficSample {
                up: 1,
                down: 2,
                up_total: 0,
                down_total: 0,
            }]
        );
    }

    #[test]
    fn flush_fakeip_cache_posts_to_mihomo_endpoint() {
        let path = std::env::temp_dir().join(format!(
            "tailash-flush-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 0 && line != "\r\n" {
                line.clear();
            }
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            request_line
        });

        MihomoClient::new(path.to_string_lossy())
            .flush_fakeip_cache()
            .unwrap();
        let request_line = server.join().unwrap();
        let _ = std::fs::remove_file(path);
        assert_eq!(request_line, "POST /cache/fakeip/flush HTTP/1.1\r\n");
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// 与 /tmp/tailash-test.sock 上的测试实例对话（用 scripts/ 奔起来的）。
    /// 没有环境时直接跳过：live 测试只在 scripts/run-live-tests.sh 里跑。
    fn live() -> Option<MihomoClient> {
        let c = MihomoClient::new("/tmp/tailash-test.sock");
        if c.version().is_err() {
            eprintln!("skipping live test: no mihomo on /tmp/tailash-test.sock");
            return None;
        }
        Some(c)
    }

    #[test]
    #[ignore = "requires scripts/run-live-tests.sh environment"]
    fn live_version() {
        let Some(c) = live() else { return };
        let v = c.version().expect("version");
        assert!(v.version.starts_with('v'));
    }

    #[test]
    #[ignore = "requires scripts/run-live-tests.sh environment"]
    fn live_proxies_has_nodes() {
        let Some(c) = live() else { return };
        let ps = c.proxies().expect("proxies");
        assert!(ps.iter().any(|p| matches!(
            p.kind.as_str(),
            "Vmess" | "Hysteria2" | "AnyTLS" | "vmess" | "hysteria2" | "anytls"
        )));
    }
}
