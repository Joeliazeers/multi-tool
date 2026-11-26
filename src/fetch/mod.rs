//! Download file langsung secara multi-bagian (ala IDM).
mod engine;
pub mod limit;
pub mod names;
pub mod table;
#[cfg(test)]
pub(crate) mod testserver;

use crate::download::DownloadArgs;
use reqwest::blocking::{Client, RequestBuilder, Response};
use std::time::Duration;

pub use engine::download;

/// Hasil pemeriksaan sebuah link yang ternyata file langsung.
#[derive(Debug, Clone)]
pub struct FileInfo {
    /// URL akhir setelah redirect
    pub url: String,
    pub name: String,
    /// Ukuran total; None bila server tidak memberi tahu
    pub size: Option<u64>,
    /// Server mendukung HTTP Range
    pub ranges: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

pub fn client() -> Client {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert(reqwest::header::ACCEPT_ENCODING, "identity".parse().unwrap());
    Client::builder()
        .user_agent(concat!("MultiTool/", env!("CARGO_PKG_VERSION")))
        .default_headers(h)
        .connect_timeout(Duration::from_secs(15))
        .tcp_keepalive(Duration::from_secs(30))
        .build()
        .expect("klien HTTP")
}

/// Tambahkan referer dan header buatan user ("Nama:Nilai").
pub fn apply_headers(mut req: RequestBuilder, a: &DownloadArgs) -> RequestBuilder {
    if let Some(r) = a.referer.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        req = req.header(reqwest::header::REFERER, r);
    }
    for h in a.headers.iter().filter(|h| !h.trim().is_empty()) {
        if let Some((k, v)) = h.split_once(':') {
            req = req.header(k.trim(), v.trim());
        }
    }
    req
}

fn hdr(r: &Response, name: &str) -> Option<String> {
    r.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
}

/// Periksa link dengan satu GET `Range: bytes=0-0`. `Some` bila itu file langsung
/// (bukan halaman HTML, respons sukses); selain itu `None` dan yt-dlp yang menangani.
pub fn probe(a: &DownloadArgs) -> Option<FileInfo> {
    let url = a.url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    let req = apply_headers(client().get(url).header(reqwest::header::RANGE, "bytes=0-0"), a)
        .timeout(Duration::from_secs(20));
    let resp = req.send().ok()?;
    let status = resp.status().as_u16();
    if status != 200 && status != 206 {
        return None;
    }
    let ctype = hdr(&resp, "content-type").unwrap_or_default().to_ascii_lowercase();
    if ctype.starts_with("text/html") || ctype.starts_with("application/xhtml") {
        return None;
    }
    let (size, ranges) = if status == 206 {
        let total = hdr(&resp, "content-range")
            .and_then(|v| v.rsplit('/').next().and_then(|t| t.trim().parse::<u64>().ok()));
        (total, true)
    } else {
        (hdr(&resp, "content-length").and_then(|v| v.trim().parse::<u64>().ok()), false)
    };
    let ranges = ranges && size.is_some();
    Some(FileInfo {
        name: names::from_response(hdr(&resp, "content-disposition").as_deref(), resp.url().as_str()),
        url: resp.url().to_string(),
        size,
        ranges,
        etag: hdr(&resp, "etag"),
        last_modified: hdr(&resp, "last-modified"),
    })
}

#[cfg(test)]
mod tests {
    use super::testserver::{Opts, TestServer, sample};
    use super::*;

    fn args(url: &str) -> DownloadArgs {
        DownloadArgs { url: url.into(), ..Default::default() }
    }

    #[test]
    fn probe_plain_file() {
        let srv = TestServer::start(
            sample(5000),
            Opts { content_disposition: Some("attachment; filename=\"data.bin\"".into()), ..Default::default() },
        );
        let i = probe(&args(&srv.url("x/y"))).unwrap();
        assert_eq!(i.size, Some(5000));
        assert!(i.ranges);
        assert_eq!(i.name, "data.bin");
        assert_eq!(i.etag.as_deref(), Some("\"v1\""));
    }

    #[test]
    fn probe_name_from_url() {
        let srv = TestServer::start(sample(10), Opts::default());
        assert_eq!(probe(&args(&srv.url("dir/movie%20one.mkv?t=1"))).unwrap().name, "movie one.mkv");
    }

    #[test]
    fn probe_without_range_support() {
        let srv = TestServer::start(sample(777), Opts { ranges: false, ..Default::default() });
        let i = probe(&args(&srv.url("f"))).unwrap();
        assert_eq!((i.size, i.ranges), (Some(777), false));
    }

    #[test]
    fn probe_unknown_size_when_chunked() {
        let srv = TestServer::start(sample(777), Opts { chunked: true, ..Default::default() });
        let i = probe(&args(&srv.url("f"))).unwrap();
        assert_eq!((i.size, i.ranges), (None, false));
    }

    #[test]
    fn probe_empty_file() {
        let srv = TestServer::start(vec![], Opts::default());
        let i = probe(&args(&srv.url("f"))).unwrap();
        assert_eq!(i.size, Some(0));
    }

    #[test]
    fn probe_rejects_html_errors_and_non_http() {
        let srv = TestServer::start(sample(10), Opts { html: true, ..Default::default() });
        assert!(probe(&args(&srv.url("f"))).is_none());
        let srv = TestServer::start(sample(10), Opts { status: Some(404), ..Default::default() });
        assert!(probe(&args(&srv.url("f"))).is_none());
        assert!(probe(&args("ftp://x/y")).is_none());
        assert!(probe(&args("http://127.0.0.1:1/x")).is_none());
    }

    #[test]
    fn probe_sends_referer_and_headers() {
        let srv = TestServer::start(sample(10), Opts::default());
        let a = DownloadArgs {
            url: srv.url("f"),
            referer: Some("https://situs.example/".into()),
            headers: vec!["X-Token: abc123".into()],
            ..Default::default()
        };
        probe(&a).unwrap();
        let heads = srv.heads().join("\n");
        assert!(heads.contains("referer: https://situs.example/"));
        assert!(heads.contains("x-token: abc123"));
        assert!(heads.contains("accept-encoding: identity"));
    }
}
