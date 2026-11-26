//! Mesin download multi-bagian: pekerja paralel, retry, throttle, jeda/lanjut lewat sidecar.
use super::table::{Seg, Table};
use super::{FileInfo, apply_headers, client, limit, names};
use crate::download::DownloadArgs;
use crate::report::{self, SegPart};
use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CHUNK: usize = 64 * 1024;
const MAX_RETRIES: u32 = 6;
const BACKOFF_CAP_SECS: u64 = 30;
const SIDECAR_EVERY: Duration = Duration::from_secs(1);
const REPORT_EVERY: Duration = Duration::from_millis(100);
const THROTTLE_DEBOUNCE: Duration = Duration::from_millis(500);

/// Isi `<nama>.part.mtdl`. Sengaja tanpa header/cookie user.
#[derive(Serialize, Deserialize)]
struct Sidecar {
    url: String,
    total: u64,
    etag: Option<String>,
    last_modified: Option<String>,
    segs: Vec<Seg>,
}

fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    dest.with_file_name(PathBuf::from(s).file_name().unwrap())
}

fn sidecar_path(dest: &Path) -> PathBuf {
    let mut s = part_path(dest).into_os_string();
    s.push(".mtdl");
    PathBuf::from(s)
}

fn write_sidecar(dest: &Path, sc: &Sidecar) {
    let path = sidecar_path(dest);
    let tmp = path.with_extension("mtdl.tmp");
    if let Ok(json) = serde_json::to_vec(sc)
        && std::fs::write(&tmp, json).is_ok()
    {
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn remove_partial(dest: &Path) {
    let _ = std::fs::remove_file(part_path(dest));
    let _ = std::fs::remove_file(sidecar_path(dest));
}

#[cfg(unix)]
fn write_at(f: &File, buf: &[u8], off: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    f.write_all_at(buf, off)
}
#[cfg(windows)]
fn write_at(f: &File, buf: &[u8], off: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut done = 0;
    while done < buf.len() {
        done += f.seek_write(&buf[done..], off + done as u64)?;
    }
    Ok(())
}

fn fmt_bytes(b: f64) -> String {
    if b >= 1_073_741_824.0 {
        format!("{:.2} GB", b / 1_073_741_824.0)
    } else {
        format!("{:.1} MB", b / 1_048_576.0)
    }
}

fn fmt_eta(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Penghitung kecepatan geser (jendela 5 detik).
struct Speed(VecDeque<(Instant, u64)>);
impl Speed {
    fn new() -> Speed {
        Speed(VecDeque::new())
    }
    fn push(&mut self, done: u64) -> f64 {
        let now = Instant::now();
        self.0.push_back((now, done));
        while self.0.len() > 2 && now.duration_since(self.0[0].0) > Duration::from_secs(5) {
            self.0.pop_front();
        }
        let (t0, d0) = self.0[0];
        let dt = now.duration_since(t0).as_secs_f64();
        if dt < 0.3 { 0.0 } else { done.saturating_sub(d0) as f64 / dt }
    }
}

fn report_progress(done: u64, total: Option<u64>, speed: f64) {
    let size = match total {
        Some(t) => format!("{} / {}", fmt_bytes(done as f64), fmt_bytes(t as f64)),
        None => fmt_bytes(done as f64),
    };
    let eta = match total {
        Some(t) if speed > 1.0 => format!(" · sisa {}", fmt_eta((t.saturating_sub(done)) as f64 / speed)),
        _ => String::new(),
    };
    let fraction = total.filter(|t| *t > 0).map(|t| done as f64 / t as f64);
    report::progress(fraction, format!("{size} · {}/s{eta}", fmt_bytes(speed)));
}

enum SegErr {
    Retry(String),
    Fatal(String),
    Throttled(u16),
    NoRange,
    Stopped,
}

struct Shared {
    table: Mutex<Table>,
    file: File,
    stop: AtomicBool,
    fatal: Mutex<Option<String>>,
    no_range: AtomicBool,
    cap: AtomicUsize,
    last_throttle: Mutex<Option<Instant>>,
    args: DownloadArgs,
    http: Client,
    if_range: Option<String>,
    active: Arc<AtomicUsize>,
}

impl Shared {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn set_fatal(&self, msg: String) {
        self.fatal.lock().unwrap().get_or_insert(msg);
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Tidur sambil memantau `stop`.
    fn sleep(&self, d: Duration) {
        let end = Instant::now() + d;
        while !self.stopped() {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            std::thread::sleep(left.min(Duration::from_millis(50)));
        }
    }

    /// True bila pekerja ini harus pensiun karena koneksi aktif melebihi `cap`.
    fn retire(&self) -> bool {
        loop {
            let a = self.active.load(Ordering::SeqCst);
            if a <= self.cap.load(Ordering::SeqCst).max(1) {
                return false;
            }
            if self.active.compare_exchange(a, a - 1, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                return true;
            }
        }
    }

    /// Server membatasi koneksi: turunkan `cap` separuh (maks. sekali per `THROTTLE_DEBOUNCE`).
    fn on_throttle(&self) {
        let mut last = self.last_throttle.lock().unwrap();
        if last.is_some_and(|t| t.elapsed() < THROTTLE_DEBOUNCE) {
            return;
        }
        *last = Some(Instant::now());
        let c = self.cap.load(Ordering::SeqCst);
        self.cap.store((c / 2).max(1), Ordering::SeqCst);
    }
}

fn try_segment(sh: &Shared, i: usize, buf: &mut [u8]) -> Result<(), SegErr> {
    let (pos, end) = sh.table.lock().unwrap().bounds(i);
    if pos >= end {
        return Ok(());
    }
    let mut req = apply_headers(sh.http.get(sh.args.url.trim()), &sh.args)
        .header(reqwest::header::RANGE, format!("bytes={pos}-{}", end - 1));
    if let Some(v) = &sh.if_range {
        req = req.header(reqwest::header::IF_RANGE, v);
    }
    let mut resp = req.send().map_err(|e| SegErr::Retry(format!("koneksi gagal: {e}")))?;
    match resp.status().as_u16() {
        206 => {}
        200 => return Err(SegErr::NoRange),
        s @ (429 | 403 | 503) => return Err(SegErr::Throttled(s)),
        s if s >= 500 => return Err(SegErr::Retry(format!("server error {s}"))),
        s => return Err(SegErr::Fatal(format!("server menolak (HTTP {s})"))),
    }
    let ok = resp
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().starts_with(&format!("bytes {pos}-")));
    if !ok {
        return Err(SegErr::Retry("Content-Range tidak sesuai permintaan".into()));
    }
    let mut at = pos;
    loop {
        if sh.stopped() {
            return Err(SegErr::Stopped);
        }
        let want = sh.table.lock().unwrap().room(i, buf.len() as u64) as usize;
        if want == 0 {
            return Ok(()); // selesai, atau sisanya dibelah pekerja lain
        }
        limit::GLOBAL.acquire(want, &sh.stop);
        let n = resp.read(&mut buf[..want]).map_err(|e| SegErr::Retry(format!("koneksi terputus: {e}")))?;
        if n == 0 {
            return Err(SegErr::Retry("koneksi terputus sebelum bagian selesai".into()));
        }
        write_at(&sh.file, &buf[..n], at).map_err(|e| SegErr::Fatal(format!("gagal menulis file: {e}")))?;
        at += n as u64;
        sh.table.lock().unwrap().advance(i, n as u64);
    }
}

/// Kerjakan satu bagian sampai selesai, dengan retry + backoff.
fn run_segment(sh: &Shared, i: usize, buf: &mut [u8]) -> Result<(), SegErr> {
    let mut fails = 0u32;
    loop {
        let before = sh.table.lock().unwrap().bounds(i).0;
        match try_segment(sh, i, buf) {
            Ok(()) => return Ok(()),
            Err(SegErr::Throttled(s)) => {
                if sh.cap.load(Ordering::SeqCst) <= 1 {
                    // sudah satu koneksi dan masih ditolak: hitung sebagai kegagalan biasa
                    fails += 1;
                    if fails > MAX_RETRIES {
                        return Err(SegErr::Fatal(format!("server membatasi/menolak (HTTP {s})")));
                    }
                    sh.sleep(Duration::from_secs((1u64 << (fails - 1)).min(BACKOFF_CAP_SECS)));
                    continue;
                }
                sh.on_throttle();
                return Err(SegErr::Throttled(s));
            }
            Err(SegErr::Retry(msg)) => {
                if sh.table.lock().unwrap().bounds(i).0 > before {
                    fails = 0; // ada kemajuan
                }
                fails += 1;
                if fails > MAX_RETRIES {
                    return Err(SegErr::Fatal(format!("gagal setelah {MAX_RETRIES}x percobaan: {msg}")));
                }
                sh.sleep(Duration::from_secs((1u64 << (fails - 1)).min(BACKOFF_CAP_SECS)));
                if sh.stopped() {
                    return Err(SegErr::Stopped);
                }
            }
            Err(e) => return Err(e),
        }
    }
}

/// Satu pekerja. Mengembalikan true bila pensiun (sudah mengurangi `active` sendiri).
fn worker(sh: &Shared) -> bool {
    let mut buf = vec![0u8; CHUNK];
    loop {
        if sh.stopped() {
            return false;
        }
        if sh.retire() {
            return true;
        }
        let Some(i) = sh.table.lock().unwrap().claim() else { return false };
        match run_segment(sh, i, &mut buf) {
            Ok(()) => {}
            Err(SegErr::Throttled(_)) => {
                sh.table.lock().unwrap().release(i);
                sh.sleep(THROTTLE_DEBOUNCE);
            }
            Err(SegErr::NoRange) => {
                sh.no_range.store(true, Ordering::SeqCst);
                sh.stop.store(true, Ordering::SeqCst);
                return false;
            }
            Err(SegErr::Stopped) => {
                sh.table.lock().unwrap().release(i);
                return false;
            }
            Err(SegErr::Fatal(m)) | Err(SegErr::Retry(m)) => {
                sh.set_fatal(m);
                return false;
            }
        }
    }
}

/// Jalankan satu pekerja di thread sendiri (pemanggil sudah menambah `active`).
fn spawn_worker(sh: &Arc<Shared>) {
    let sh = sh.clone();
    std::thread::spawn(move || {
        let active = sh.active.clone();
        let retired = worker(&sh);
        drop(sh); // lepas handle file sebelum dihitung selesai
        if !retired {
            active.fetch_sub(1, Ordering::SeqCst);
        }
    });
}

/// Cari sisa download sebelumnya untuk URL ini (nama `name`, `name (1)`, ...).
fn find_resume(dir: &Path, name: &str, url: &str, info: &FileInfo) -> Option<(PathBuf, Sidecar)> {
    let total = info.size?;
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for k in 0..50 {
        let cand = if k == 0 { name.to_string() } else { format!("{stem} ({k}){ext}") };
        let dest = dir.join(&cand);
        let Ok(raw) = std::fs::read(sidecar_path(&dest)) else { continue };
        let Ok(sc) = serde_json::from_slice::<Sidecar>(&raw) else { continue };
        if sc.url != url {
            continue;
        }
        let part_len = std::fs::metadata(part_path(&dest)).map(|m| m.len()).ok();
        let same = sc.total == total
            && sc.etag == info.etag
            && (sc.etag.is_some() || sc.last_modified == info.last_modified)
            && part_len == Some(total)
            && Table::from_segs(sc.segs.clone()).is_valid_for(total);
        if same {
            return Some((dest, sc));
        }
        report::warn(format!("File di server berubah sejak sebelumnya: mulai ulang {cand}"));
        remove_partial(&dest);
    }
    None
}

/// Download `info` ke `out_dir` dan kembalikan path hasil.
/// `parts` = jumlah koneksi, `limit` = batas byte/detik (0 = bebas).
pub fn download(a: &DownloadArgs, info: &FileInfo, out_dir: &Path, parts: usize, limit: u64) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir).with_context(|| format!("gagal membuat folder {}", out_dir.display()))?;
    limit::GLOBAL.set_rate(limit);
    let url = a.url.trim().to_string();

    if info.size == Some(0) {
        let dest = names::unique(out_dir, &info.name);
        File::create(&dest)?;
        return Ok(dest);
    }
    let parallel = info.ranges && info.size.is_some();
    if !parallel {
        let dest = names::unique(out_dir, &info.name);
        return single(a, info, &dest);
    }
    let total = info.size.unwrap();

    let (dest, table, resumed) = match find_resume(out_dir, &info.name, &url, info) {
        Some((dest, sc)) => {
            report::log(format!("Melanjutkan {}", dest.display()));
            (dest, Table::from_segs(sc.segs), true)
        }
        None => (names::unique(out_dir, &info.name), Table::new(total, parts), false),
    };
    report::log(format!("Tujuan: {}", dest.display()));
    let part = part_path(&dest);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&part)
        .with_context(|| format!("gagal membuat {}", part.display()))?;
    if !resumed {
        file.set_len(total).context("gagal mengalokasikan file (disk penuh?)")?;
    }
    let workers = table.segs().len().clamp(1, parts.clamp(1, 32));
    let active = Arc::new(AtomicUsize::new(workers));
    let sh = Arc::new(Shared {
        table: Mutex::new(table),
        file,
        stop: AtomicBool::new(false),
        fatal: Mutex::new(None),
        no_range: AtomicBool::new(false),
        cap: AtomicUsize::new(workers),
        last_throttle: Mutex::new(None),
        args: a.clone(),
        http: client(),
        if_range: info.etag.clone().or_else(|| info.last_modified.clone()),
        active: active.clone(),
    });
    let make_sidecar = |sh: &Shared| Sidecar {
        url: url.clone(),
        total,
        etag: info.etag.clone(),
        last_modified: info.last_modified.clone(),
        segs: sh.table.lock().unwrap().segs(),
    };
    write_sidecar(&dest, &make_sidecar(&sh));
    report::stage(format!("Download {} ({} koneksi)", info.name, workers));

    for _ in 0..workers {
        spawn_worker(&sh);
    }
    let mut respawns = 0;

    // ---- pemantau (thread pemanggil): progres, sidecar, batal
    let mut speed = Speed::new();
    let (mut last_report, mut last_side) = (Instant::now() - REPORT_EVERY, Instant::now());
    let mut cancelled = false;
    loop {
        if !cancelled && report::cancelled() {
            cancelled = true;
            sh.stop.store(true, Ordering::SeqCst);
        }
        let (done, complete) = {
            let t = sh.table.lock().unwrap();
            (t.done_bytes(), t.is_complete())
        };
        if last_report.elapsed() >= REPORT_EVERY {
            last_report = Instant::now();
            report_progress(done, Some(total), speed.push(done));
            let parts_view: Vec<SegPart> = sh
                .table
                .lock()
                .unwrap()
                .segs()
                .iter()
                .map(|s| SegPart {
                    from: s.start as f64 / total as f64,
                    to: s.end as f64 / total as f64,
                    pos: s.pos as f64 / total as f64,
                })
                .collect();
            report::segments(parts_view);
        }
        if last_side.elapsed() >= SIDECAR_EVERY {
            last_side = Instant::now();
            write_sidecar(&dest, &make_sidecar(&sh));
        }
        if complete || sh.stopped() {
            break;
        }
        if active.load(Ordering::SeqCst) == 0 {
            // Semua pekerja habis padahal belum selesai (balapan langka: bagian yang dilepas
            // tepat saat pekerja lain keluar). Jalankan satu pekerja lagi.
            if respawns >= 5 {
                break;
            }
            respawns += 1;
            active.fetch_add(1, Ordering::SeqCst);
            spawn_worker(&sh);
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    sh.stop.store(true, Ordering::SeqCst);
    // beri pekerja waktu singkat untuk berhenti (koneksi macet tidak boleh menahan jeda/batal)
    let deadline = Instant::now() + Duration::from_secs(3);
    while active.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }

    let complete = sh.table.lock().unwrap().is_complete();
    if complete {
        report_progress(total, Some(total), 0.0);
        let len = sh.file.metadata().map(|m| m.len()).unwrap_or(0);
        if len != total {
            remove_partial(&dest);
            bail!("ukuran file tidak cocok ({len} dari {total} byte)");
        }
        let _ = sh.file.sync_all();
        drop(sh);
        std::fs::rename(&part, &dest).with_context(|| format!("gagal memindahkan ke {}", dest.display()))?;
        let _ = std::fs::remove_file(sidecar_path(&dest));
        return Ok(dest);
    }
    if sh.no_range.load(Ordering::SeqCst) {
        report::warn("Server tidak mendukung download multi-bagian: memakai satu koneksi.".to_string());
        drop(sh);
        remove_partial(&dest);
        return single(a, info, &dest);
    }
    if cancelled {
        if report::keep_partial() {
            write_sidecar(&dest, &make_sidecar(&sh));
            bail!("Dijeda");
        }
        drop(sh);
        remove_partial(&dest);
        bail!("Dibatalkan");
    }
    write_sidecar(&dest, &make_sidecar(&sh));
    let why = sh.fatal.lock().unwrap().clone().unwrap_or_else(|| "download berhenti tak terduga".into());
    bail!("{why} (bagian yang sudah diunduh disimpan; ulangi untuk melanjutkan)")
}

/// Satu koneksi, tanpa resume: server tanpa Range atau ukuran tidak diketahui.
fn single(a: &DownloadArgs, info: &FileInfo, dest: &Path) -> Result<PathBuf> {
    let http = client();
    let part = part_path(dest);
    report::stage(format!("Download {} (1 koneksi)", info.name));
    if !info.ranges {
        report::log("Server tidak mendukung Range: tidak bisa dilanjutkan bila terputus.".to_string());
    }
    let stop = AtomicBool::new(false);
    let mut last_err = String::new();
    for attempt in 1..=3 {
        let resp = apply_headers(http.get(a.url.trim()), a).send();
        let mut resp = match resp {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => bail!("server menolak (HTTP {})", r.status().as_u16()),
            Err(e) => {
                last_err = format!("koneksi gagal: {e}");
                continue;
            }
        };
        let expect = resp.content_length();
        let mut out = File::create(&part)?;
        let mut buf = vec![0u8; CHUNK];
        let (mut done, mut speed) = (0u64, Speed::new());
        let mut last = Instant::now() - REPORT_EVERY;
        let res: Result<()> = loop {
            if report::cancelled() {
                drop(out);
                let _ = std::fs::remove_file(&part);
                if report::keep_partial() {
                    bail!("Dijeda (server tidak mendukung resume: akan mulai dari awal)");
                }
                bail!("Dibatalkan");
            }
            limit::GLOBAL.acquire(CHUNK, &stop);
            match resp.read(&mut buf) {
                Ok(0) => break Ok(()),
                Ok(n) => {
                    use std::io::Write;
                    if let Err(e) = out.write_all(&buf[..n]) {
                        bail!("gagal menulis file: {e}");
                    }
                    done += n as u64;
                    if last.elapsed() >= REPORT_EVERY {
                        last = Instant::now();
                        report_progress(done, expect.or(info.size), speed.push(done));
                    }
                }
                Err(e) => break Err(anyhow::anyhow!("koneksi terputus: {e}")),
            }
        };
        match res {
            Ok(()) if expect.is_none_or(|e| e == done) => {
                drop(out);
                std::fs::rename(&part, dest)?;
                report_progress(done, Some(done), 0.0);
                return Ok(dest.to_path_buf());
            }
            Ok(()) => last_err = format!("ukuran tidak cocok ({done} dari {} byte)", expect.unwrap_or(0)),
            Err(e) => last_err = e.to_string(),
        }
        report::warn(format!("Percobaan {attempt}/3 gagal: {last_err}"));
    }
    let _ = std::fs::remove_file(&part);
    bail!("download gagal: {last_err}")
}

#[cfg(test)]
mod tests {
    use super::super::probe;
    use super::super::testserver::{Opts, TestServer, sample};
    use super::*;
    use std::sync::atomic::AtomicBool;

    const MIB: usize = 1024 * 1024;

    fn args(url: &str) -> DownloadArgs {
        DownloadArgs { url: url.into(), ..Default::default() }
    }

    fn fetch(srv: &TestServer, dir: &Path, parts: usize) -> Result<PathBuf> {
        let a = args(&srv.url("data.bin"));
        let info = probe(&a).expect("probe");
        download(&a, &info, dir, parts, 0)
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".part"))
            .collect()
    }

    #[test]
    fn parallel_download_is_byte_identical() {
        let data = sample(8 * MIB);
        let srv = TestServer::start(data.clone(), Opts::default());
        let dir = tempfile::tempdir().unwrap();
        let p = fetch(&srv, dir.path(), 4).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), data);
        assert_eq!(p.file_name().unwrap(), "data.bin");
        assert!(leftovers(dir.path()).is_empty());
        assert!(srv.requests() >= 5); // 1 probe + >= 4 bagian
    }

    #[test]
    fn tiny_file_with_many_parts() {
        let data = sample(10);
        let srv = TestServer::start(data.clone(), Opts::default());
        let dir = tempfile::tempdir().unwrap();
        let p = fetch(&srv, dir.path(), 8).unwrap();
        assert_eq!(std::fs::read(p).unwrap(), data);
    }

    #[test]
    fn empty_file() {
        let srv = TestServer::start(vec![], Opts::default());
        let dir = tempfile::tempdir().unwrap();
        let p = fetch(&srv, dir.path(), 8).unwrap();
        assert_eq!(std::fs::metadata(p).unwrap().len(), 0);
    }

    #[test]
    fn server_without_range_falls_back_to_single_connection() {
        let data = sample(3 * MIB);
        let srv = TestServer::start(data.clone(), Opts { ranges: false, ..Default::default() });
        let dir = tempfile::tempdir().unwrap();
        let p = fetch(&srv, dir.path(), 8).unwrap();
        assert_eq!(std::fs::read(p).unwrap(), data);
        assert_eq!(srv.requests(), 2); // probe + satu aliran penuh
    }

    #[test]
    fn range_ignored_midway_falls_back() {
        // probe mengira Range didukung (206), tapi permintaan bagian dijawab 200
        let data = sample(3 * MIB);
        let srv = TestServer::start(data.clone(), Opts::default());
        let a = args(&srv.url("data.bin"));
        let mut info = probe(&a).unwrap();
        info.ranges = true;
        srv.set_ignore_ranges_after_first();
        let dir = tempfile::tempdir().unwrap();
        let p = download(&a, &info, dir.path(), 4, 0).unwrap();
        assert_eq!(std::fs::read(p).unwrap(), data);
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn chunked_without_length() {
        let data = sample(2 * MIB + 17);
        let srv = TestServer::start(data.clone(), Opts { chunked: true, ..Default::default() });
        let dir = tempfile::tempdir().unwrap();
        let p = fetch(&srv, dir.path(), 8).unwrap();
        assert_eq!(std::fs::read(p).unwrap(), data);
    }

    #[test]
    fn retries_dropped_connections() {
        let data = sample(6 * MIB);
        let srv = TestServer::start(data.clone(), Opts { drop_after: Some(MIB / 2), ..Default::default() });
        let dir = tempfile::tempdir().unwrap();
        let p = fetch(&srv, dir.path(), 3).unwrap();
        assert_eq!(std::fs::read(p).unwrap(), data);
    }

    #[test]
    fn reduces_connections_when_throttled() {
        let data = sample(8 * MIB);
        let srv = TestServer::start(data.clone(), Opts { throttle_above: Some(2), chunk_delay_ms: 1, ..Default::default() });
        let dir = tempfile::tempdir().unwrap();
        let p = fetch(&srv, dir.path(), 8).unwrap();
        assert_eq!(std::fs::read(p).unwrap(), data);
        assert!(srv.rejected() > 0, "server tidak pernah membalas 429, tes tidak berarti");
    }

    #[test]
    fn existing_destination_gets_suffix() {
        let data = sample(2 * MIB);
        let srv = TestServer::start(data.clone(), Opts::default());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("data.bin"), b"lama").unwrap();
        let p = fetch(&srv, dir.path(), 2).unwrap();
        assert_eq!(p.file_name().unwrap(), "data (1).bin");
        assert_eq!(std::fs::read(dir.path().join("data.bin")).unwrap(), b"lama");
        assert_eq!(std::fs::read(p).unwrap(), data);
    }

    #[test]
    fn hostile_name_stays_in_folder() {
        let srv = TestServer::start(
            sample(100),
            Opts { content_disposition: Some("attachment; filename=\"../../evil.sh\"".into()), ..Default::default() },
        );
        let dir = tempfile::tempdir().unwrap();
        let p = fetch(&srv, dir.path(), 2).unwrap();
        assert_eq!(p.parent().unwrap(), dir.path());
        assert_eq!(p.file_name().unwrap(), "evil.sh");
    }

    fn job() -> (report::Job, Arc<AtomicBool>, Arc<AtomicBool>) {
        let cancel = Arc::new(AtomicBool::new(false));
        let keep = Arc::new(AtomicBool::new(false));
        (report::Job { id: 1, cancel: cancel.clone(), keep: keep.clone() }, cancel, keep)
    }

    /// Jalankan download di thread, hentikan setelah `after`; `keep` true = jeda.
    fn interrupted(srv: &TestServer, dir: &Path, parts: usize, keep: bool, a: DownloadArgs) -> String {
        let info = probe(&a).unwrap();
        let (j, cancel, keepf) = job();
        let dir = dir.to_path_buf();
        let h = std::thread::spawn(move || report::with_job(j, || download(&a, &info, &dir, parts, 0)));
        std::thread::sleep(Duration::from_millis(400));
        keepf.store(keep, Ordering::SeqCst);
        cancel.store(true, Ordering::SeqCst);
        let _ = srv;
        format!("{:#}", h.join().unwrap().unwrap_err())
    }

    #[test]
    fn pause_then_resume_is_identical() {
        let data = sample(12 * MIB);
        let srv = TestServer::start(data.clone(), Opts { chunk_delay_ms: 4, ..Default::default() });
        let dir = tempfile::tempdir().unwrap();
        let a = DownloadArgs {
            url: srv.url("data.bin"),
            referer: Some("https://rahasia.example/".into()),
            headers: vec!["X-Secret: topsecret123".into()],
            ..Default::default()
        };
        let err = interrupted(&srv, dir.path(), 4, true, a.clone());
        assert!(err.contains("Dijeda"), "{err}");
        let side = std::fs::read_to_string(dir.path().join("data.bin.part.mtdl")).unwrap();
        assert!(dir.path().join("data.bin.part").exists());
        assert!(!side.contains("topsecret123") && !side.contains("rahasia.example"));
        let sc: Sidecar = serde_json::from_str(&side).unwrap();
        let done: u64 = sc.segs.iter().map(|s| s.pos - s.start).sum();
        assert!(done > 0 && done < 12 * MIB as u64, "done={done}");

        let before = srv.requests();
        let sent_before = srv.bytes_sent();
        let info = probe(&a).unwrap();
        let p = download(&a, &info, dir.path(), 4, 0).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), data);
        assert!(leftovers(dir.path()).is_empty());
        // setelah jeda hanya byte yang belum diunduh yang diminta lagi (bukan mulai dari 0)
        let sent_resume = srv.bytes_sent() - sent_before;
        assert!(sent_resume <= 12 * MIB as u64 - done + MIB as u64, "terkirim {sent_resume}, sudah {done}");
        let heads = srv.heads();
        let started_from_zero = heads[before + 1..].iter().filter(|h| h.contains("range: bytes=0-")).count();
        assert!(started_from_zero <= 1, "{started_from_zero}");
    }

    #[test]
    fn cancel_removes_partial_files() {
        let srv = TestServer::start(sample(12 * MIB), Opts { chunk_delay_ms: 4, ..Default::default() });
        let dir = tempfile::tempdir().unwrap();
        let err = interrupted(&srv, dir.path(), 4, false, args(&srv.url("data.bin")));
        assert!(err.contains("Dibatalkan"), "{err}");
        assert!(leftovers(dir.path()).is_empty());
        assert!(!dir.path().join("data.bin").exists());
    }

    #[test]
    fn resume_restarts_when_file_changed_on_server() {
        let old = sample(10 * MIB);
        let srv = TestServer::start(old, Opts { chunk_delay_ms: 4, ..Default::default() });
        let dir = tempfile::tempdir().unwrap();
        let a = args(&srv.url("data.bin"));
        interrupted(&srv, dir.path(), 4, true, a.clone());
        let mut new = sample(10 * MIB);
        new.reverse();
        srv.set_data(new.clone(), "\"v2\"");
        let p = fetch(&srv, dir.path(), 4).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), new);
        assert_eq!(p.file_name().unwrap(), "data.bin");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn fatal_http_error_keeps_nothing_misleading() {
        let srv = TestServer::start(sample(2 * MIB), Opts::default());
        let a = args(&srv.url("data.bin"));
        let info = probe(&a).unwrap();
        srv.set_status(Some(404));
        let dir = tempfile::tempdir().unwrap();
        let err = download(&a, &info, dir.path(), 2, 0).unwrap_err();
        assert!(format!("{err:#}").contains("404"));
        assert!(!dir.path().join("data.bin").exists());
    }
}
