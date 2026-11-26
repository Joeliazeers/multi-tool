//! Server HTTP lokal untuk tes: Range, ETag, putus koneksi, throttle 429, tanpa Content-Length, dll.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub struct Opts {
    pub ranges: bool,
    pub chunked: bool,
    pub etag: String,
    pub content_disposition: Option<String>,
    /// Putus koneksi setelah N byte body, untuk `drop_times` permintaan data pertama.
    pub drop_after: Option<usize>,
    pub drop_times: usize,
    /// Balas 429 bila ada lebih dari N permintaan data bersamaan.
    pub throttle_above: Option<usize>,
    pub html: bool,
    pub status: Option<u16>,
    /// Jeda setelah tiap 16 KiB body (membuat download cukup lambat untuk diuji jeda/batal).
    pub chunk_delay_ms: u64,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            ranges: true,
            chunked: false,
            etag: "\"v1\"".into(),
            content_disposition: None,
            drop_after: None,
            drop_times: 3,
            throttle_above: None,
            html: false,
            status: None,
            chunk_delay_ms: 0,
        }
    }
}

struct Shared {
    data: Mutex<(Vec<u8>, String)>,
    opts: Opts,
    pub requests: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
    cut: AtomicUsize,
    sent: AtomicU64,
    rejected: AtomicUsize,
    /// Setelah permintaan pertama, abaikan header Range (jawab 200 penuh)
    ignore_ranges_after_first: AtomicBool,
    status_override: Mutex<Option<u16>>,
    heads: Mutex<Vec<String>>,
    stop: AtomicBool,
}

pub struct TestServer {
    shared: Arc<Shared>,
    port: u16,
}

impl TestServer {
    pub fn start(data: Vec<u8>, opts: Opts) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let etag = opts.etag.clone();
        let shared = Arc::new(Shared {
            data: Mutex::new((data, etag)),
            opts,
            requests: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            cut: AtomicUsize::new(0),
            sent: AtomicU64::new(0),
            rejected: AtomicUsize::new(0),
            ignore_ranges_after_first: AtomicBool::new(false),
            status_override: Mutex::new(None),
            heads: Mutex::new(vec![]),
            stop: AtomicBool::new(false),
        });
        let sh = shared.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                if sh.stop.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(c) = conn {
                    let sh = sh.clone();
                    std::thread::spawn(move || {
                        let _ = handle(c, &sh);
                    });
                }
            }
        });
        TestServer { shared, port }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}/{}", self.port, path.trim_start_matches('/'))
    }

    pub fn requests(&self) -> usize {
        self.shared.requests.load(Ordering::SeqCst)
    }

    /// Total byte body yang sudah dikirim.
    pub fn bytes_sent(&self) -> u64 {
        self.shared.sent.load(Ordering::SeqCst)
    }

    /// Jumlah permintaan yang ditolak 429.
    pub fn rejected(&self) -> usize {
        self.shared.rejected.load(Ordering::SeqCst)
    }

    /// Kepala permintaan yang diterima (huruf kecil), untuk memeriksa header yang dikirim klien.
    pub fn heads(&self) -> Vec<String> {
        self.shared.heads.lock().unwrap().clone()
    }

    pub fn set_ignore_ranges_after_first(&self) {
        self.shared.ignore_ranges_after_first.store(true, Ordering::SeqCst);
        self.shared.requests.store(0, Ordering::SeqCst);
    }

    pub fn set_status(&self, st: Option<u16>) {
        *self.shared.status_override.lock().unwrap() = st;
    }

    pub fn set_data(&self, data: Vec<u8>, etag: &str) {
        *self.shared.data.lock().unwrap() = (data, etag.to_string());
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port)); // bangunkan accept()
    }
}

/// Data uji deterministik.
pub fn sample(len: usize) -> Vec<u8> {
    let mut x: u32 = 0x1234_5678;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 24) as u8
        })
        .collect()
}

fn handle(mut s: TcpStream, sh: &Shared) -> std::io::Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = s.read(&mut tmp)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let head = String::from_utf8_lossy(&buf).to_ascii_lowercase();
    sh.heads.lock().unwrap().push(head.clone());
    let is_head = head.starts_with("head ");
    sh.requests.fetch_add(1, Ordering::SeqCst);
    let header = |name: &str| -> Option<String> {
        head.lines()
            .find_map(|l| l.strip_prefix(&format!("{name}:")).map(|v| v.trim().to_string()))
    };
    let o = &sh.opts;

    let now = sh.active.fetch_add(1, Ordering::SeqCst) + 1;
    sh.max_active.fetch_max(now, Ordering::SeqCst);
    struct Dec<'a>(&'a AtomicUsize);
    impl Drop for Dec<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let _dec = Dec(&sh.active);

    let st_over = o.status.or(*sh.status_override.lock().unwrap());
    if let Some(st) = st_over {
        return s.write_all(format!("HTTP/1.1 {st} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes());
    }
    if o.throttle_above.is_some_and(|n| now > n) {
        sh.rejected.fetch_add(1, Ordering::SeqCst);
        return s.write_all(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    }

    let (data, etag) = sh.data.lock().unwrap().clone();
    let len = data.len();
    let mut start = 0usize;
    let mut end = len; // eksklusif
    let mut partial = false;
    let ignore = sh.ignore_ranges_after_first.load(Ordering::SeqCst);
    if o.ranges && !o.chunked && !o.html && len > 0 && !ignore {
        let if_range_ok = header("if-range").map(|v| v == etag.to_ascii_lowercase()).unwrap_or(true);
        if let (Some(r), true) = (header("range"), if_range_ok)
            && let Some(spec) = r.strip_prefix("bytes=")
            && let Some((a, b)) = spec.split_once('-')
            && let Ok(a) = a.parse::<usize>()
        {
            if a >= len {
                return s.write_all(
                    format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .as_bytes(),
                );
            }
            start = a;
            end = b.parse::<usize>().map(|b| (b + 1).min(len)).unwrap_or(len);
            partial = true;
        }
    }

    let mut h = String::new();
    if partial {
        h += "HTTP/1.1 206 Partial Content\r\n";
        h += &format!("Content-Range: bytes {}-{}/{}\r\n", start, end.saturating_sub(1), len);
    } else {
        h += "HTTP/1.1 200 OK\r\n";
    }
    h += &format!("Content-Type: {}\r\n", if o.html { "text/html; charset=utf-8" } else { "application/octet-stream" });
    if o.ranges && !o.chunked {
        h += "Accept-Ranges: bytes\r\n";
    }
    if !etag.is_empty() {
        h += &format!("ETag: {etag}\r\n");
    }
    h += "Last-Modified: Wed, 01 Jan 2025 00:00:00 GMT\r\n";
    if let Some(cd) = &o.content_disposition {
        h += &format!("Content-Disposition: {cd}\r\n");
    }
    if o.chunked {
        h += "Transfer-Encoding: chunked\r\n";
    } else {
        h += &format!("Content-Length: {}\r\n", end - start);
    }
    h += "Connection: close\r\n\r\n";
    s.write_all(h.as_bytes())?;
    if is_head {
        return Ok(());
    }

    let mut limit = usize::MAX;
    if let Some(n) = o.drop_after
        && sh.cut.fetch_add(1, Ordering::SeqCst) < o.drop_times
    {
        limit = n;
    }
    let mut sent = 0usize;
    for piece in data[start..end].chunks(16 * 1024) {
        let piece = &piece[..piece.len().min(limit - sent)];
        if o.chunked {
            s.write_all(format!("{:x}\r\n", piece.len()).as_bytes())?;
            s.write_all(piece)?;
            s.write_all(b"\r\n")?;
        } else {
            s.write_all(piece)?;
        }
        sent += piece.len();
        sh.sent.fetch_add(piece.len() as u64, Ordering::SeqCst);
        if sent >= limit {
            return Ok(()); // putus di tengah (Content-Length tak terpenuhi)
        }
        if o.chunk_delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(o.chunk_delay_ms));
        }
    }
    if o.chunked {
        s.write_all(b"0\r\n\r\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(url: &str, range: Option<&str>) -> (u16, std::collections::HashMap<String, String>, Vec<u8>) {
        let c = reqwest::blocking::Client::new();
        let mut r = c.get(url);
        if let Some(rg) = range {
            r = r.header("Range", rg);
        }
        let resp = r.send().unwrap();
        let st = resp.status().as_u16();
        let hs = resp.headers().iter().map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap().to_string())).collect();
        (st, hs, resp.bytes().unwrap().to_vec())
    }

    #[test]
    fn serves_ranges() {
        let data = sample(1000);
        let srv = TestServer::start(data.clone(), Opts::default());
        let (st, h, body) = get(&srv.url("f.bin"), Some("bytes=10-19"));
        assert_eq!(st, 206);
        assert_eq!(h["content-range"], "bytes 10-19/1000");
        assert_eq!(body, &data[10..20]);
        let (st, h, body) = get(&srv.url("f.bin"), None);
        assert_eq!((st, body.len()), (200, 1000));
        assert_eq!(h["accept-ranges"], "bytes");
    }

    #[test]
    fn can_ignore_ranges() {
        let srv = TestServer::start(sample(100), Opts { ranges: false, ..Default::default() });
        let (st, h, body) = get(&srv.url("f"), Some("bytes=0-0"));
        assert_eq!((st, body.len()), (200, 100));
        assert!(!h.contains_key("accept-ranges"));
    }
}
