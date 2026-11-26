# Download Multi-Bagian ala IDM Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Link file langsung diunduh lewat banyak koneksi paralel dengan jeda/lanjut, batas kecepatan, dan antrean; shortcut bernama MultiTool.

**Architecture:** Modul baru `src/fetch/` (probe HTTP, tabel bagian dinamis, sidecar resume, pembatas kecepatan, mesin pekerja thread) dipanggil dari `download::run`/`download::probe` bila link adalah file langsung; selain itu yt-dlp seperti sekarang. Progres lewat `report` (event `Segments` baru), jeda lewat bendera `keep` di `report::Job`. Antrean dan bar bagian ada di frontend.

**Tech Stack:** Rust 2024, `reqwest` blocking (sudah ada), `std::thread::scope`, `serde_json`, Tauri 2, JS polos.

**Spec:** `docs/superpowers/specs/2026-11-27-segmented-download-design.md`

## Global Constraints
- Tidak ada dependensi sistem/crate baru (test server pakai `std::net`).
- Teks UI, pesan error, dan log berbahasa Indonesia.
- `reqwest` tanpa dekompresi otomatis; kirim `Accept-Encoding: identity`.
- Config lama (`~/.config/vidsub/config.env`, `VIDSUB_*`) tetap; kunci baru juga berawalan `VIDSUB_`.
- Default: 8 koneksi (1-32), antrean 3 download bersamaan, batas kecepatan 0 = tanpa batas; retry 6x, backoff 1,2,4... maks 30 detik; potong bagian hanya bila sisa > 2 MiB; sidecar ditulis tiap ~1 detik.
- Header/cookie user tidak ditulis ke sidecar. Linux + Windows (`write_all_at` / `seek_write`).
- Jangan commit kecuali user meminta.

## Review Focus
- Server tanpa `Content-Length` (chunked): satu aliran, file benar (Task 5, 6).
- Server mengabaikan Range (balas 200): jatuh ke satu koneksi, file benar (Task 6).
- Nama file jahat (`../../x`, `CON.txt`, titik/spasi di ujung, kosong): tetap di dalam folder tujuan (Task 2).
- File 0 byte dan file lebih kecil dari jumlah bagian (mis. 10 byte, 8 bagian) (Task 6).
- Nama tujuan sudah ada: ` (1)`, dan resume tidak tertukar dengan file yang sudah ada (Task 6, 7).

## File Structure
- Create `src/fetch/mod.rs`: API publik `probe`, `download`, tipe `FileInfo`.
- Create `src/fetch/names.rs`: nama file dari header/URL, sanitasi, nama unik.
- Create `src/fetch/limit.rs`: pembatas kecepatan global + `parse_size`.
- Create `src/fetch/table.rs`: tabel bagian dinamis (claim/split/advance), serialisasi.
- Create `src/fetch/engine.rs`: pekerja, retry, throttle, sidecar, progres.
- Create `src/fetch/testserver.rs` (`#[cfg(test)]`): server HTTP lokal.
- Modify `src/lib.rs`, `src/config.rs`, `src/download.rs`, `src/report.rs`, `src/main.rs`, `gui/src/main.rs`, `gui/ui/{index.html,app.js,styles.css}`, `packaging/install-shortcut.sh` (cek), `README.md`.

---

### Task 1: Pembatas kecepatan dan parse ukuran
**Files:** Create `src/fetch/limit.rs`, `src/fetch/mod.rs` (hanya `pub mod limit;` dulu); Modify `src/lib.rs` (`pub mod fetch;`)

**Interfaces:**
- Produces: `limit::parse_size(&str) -> Result<u64, String>` ("2M"=2*1048576, "500K", "1G", angka polos = byte, kosong/"0" = 0), `limit::set_rate(bytes_per_sec: u64)`, `limit::acquire(n: usize, stop: &AtomicBool)`.

- [ ] Tulis test: `parse_size("2M")==Ok(2_097_152)`, `("500k")==Ok(512_000)`, `("0")==Ok(0)`, `("")==Ok(0)`, `("abc")` Err; `acquire` dengan rate 1_000_000 dua kali 500_000 byte memakan >= 400 ms; rate 0 tidak menunggu; `stop=true` membuat `acquire` kembali segera.
- [ ] Jalankan `cargo test -p multitool limit` — harus gagal (belum ada).
- [ ] Implementasi: penjadwal sederhana (`Mutex<(rate, next_free: Instant)>`), tidur dalam irisan <= 100 ms sambil mengecek `stop`.
- [ ] Jalankan test lagi — harus lulus.

### Task 2: Nama file
**Files:** Create `src/fetch/names.rs`

**Interfaces:**
- Produces: `names::from_response(content_disposition: Option<&str>, url: &str) -> String` (sudah disanitasi, tak pernah kosong), `names::sanitize(&str) -> String`, `names::unique(dir: &Path, name: &str) -> PathBuf` (tambah ` (1)`, ` (2)` sebelum ekstensi bila `dir/name` atau `dir/name.part` sudah ada).

- [ ] Test: `filename*=UTF-8''na%20ma.zip` → `na ma.zip`; `filename="a.zip"` → `a.zip`; tanpa header, URL `https://x/y/file%20(1).iso?token=1` → `file (1).iso`; URL `https://x/` → `download`; sanitize: `../../etc/passwd` → `passwd`, `a\b:c*?.txt` → `a_b_c__.txt`, `CON.txt` → `_CON.txt`, `name. ` → `name`, `` → `download`; `unique` dengan file `a.zip` ada → `a (1).zip`.
- [ ] Jalankan, lihat gagal; implementasi; jalankan, lulus.

### Task 3: Tabel bagian dinamis
**Files:** Create `src/fetch/table.rs`

**Interfaces:**
- Produces: `#[derive(Serialize, Deserialize, Clone)] struct Seg { start: u64, pos: u64, end: u64 }` (end eksklusif); `struct Table`; `Table::new(total: u64, parts: usize) -> Table` (bagian rata; `parts` dikurangi bila `total / parts < 1 MiB`, minimal 1; total 0 → tanpa bagian), `Table::from_segs(Vec<Seg>) -> Table`, `claim(&mut self) -> Option<usize>`, `room(&self, i, want: u64) -> u64` (min(want, end-pos), 0 = selesai), `advance(&mut self, i, n: u64)` (clamp ke end), `release(&mut self, i)`, `done_bytes(&self) -> u64`, `is_complete(&self) -> bool`, `segs(&self) -> Vec<Seg>`.
- `claim`: bagian belum diklaim dengan sisa > 0; kalau tak ada, pilih bagian terklaim dengan sisa terbesar bila sisa > 2 MiB, bagi: korban `end = pos + sisa/2`, bagian baru `[end_baru, end_lama)` diklaim pemanggil.

- [ ] Test: `new(10 MiB, 4)` → 4 bagian menutupi 0..10 MiB tanpa celah; `new(10, 8)` → 1 bagian; `new(0, 8)` → kosong dan `is_complete()`; claim berturut-turut menghabiskan bagian awal lalu membelah (setelah semua diklaim, claim ke-5 menghasilkan bagian baru dan jumlah bagian bertambah, tanpa tumpang tindih dan total panjang tetap); `advance` melewati `end` di-clamp; `release` membuat bagian bisa diklaim lagi; round-trip serde.
- [ ] Jalankan (gagal), implementasi, jalankan (lulus).

### Task 4: Server uji
**Files:** Create `src/fetch/testserver.rs`

**Interfaces:**
- Produces (hanya `#[cfg(test)]`): `TestServer::start(data: Vec<u8>, opts: Opts) -> TestServer` dengan `url(&self, path: &str) -> String`, field publik penghitung `requests`, `max_concurrent`. `Opts { ranges: bool, chunked: bool, etag: &'static str, content_disposition: Option<String>, drop_after: Option<usize> (putus koneksi setelah N byte, sekali per permintaan pertama), throttle_above: Option<usize> (balas 429 bila koneksi bersamaan > N), html: bool, status: Option<u16> }` + `Default`.
- Melayani GET (dan HEAD) dengan `Range: bytes=a-b`, `If-Range`, `ETag`, thread per koneksi, berhenti saat di-drop.

- [ ] Tulis satu test yang memastikan server menjawab 206 + `Content-Range` yang benar, 200 saat `ranges:false`, dan `Accept-Ranges`.
- [ ] Jalankan (gagal), implementasi, jalankan (lulus).

### Task 5: Probe
**Files:** Modify `src/fetch/mod.rs`

**Interfaces:**
- Consumes: `names::from_response`, `testserver`, `DownloadArgs` (`referer`, `headers`).
- Produces: `struct FileInfo { url: String, name: String, size: Option<u64>, ranges: bool, etag: Option<String>, last_modified: Option<String> }`; `fetch::client() -> reqwest::blocking::Client` (redirect default, `Accept-Encoding: identity`, connect timeout 15 s, read timeout 30 s — cek API `read_timeout` pada versi reqwest yang terpasang, kalau tidak ada pakai timeout per permintaan di `engine`); `fetch::apply_headers(req, &DownloadArgs) -> RequestBuilder`; `fetch::probe(a: &DownloadArgs) -> Option<FileInfo>` (GET `Range: bytes=0-0`; `None` bila error, status bukan 200/206, atau `Content-Type` `text/html`).

- [ ] Test (pakai TestServer): server biasa → `Some` dengan `size==Some(len)`, `ranges==true`, nama dari `Content-Disposition`; `ranges:false` → `ranges==false`, `size` dari `Content-Length`; `chunked:true` → `size==None`; `html:true` → `None`; `status:Some(404)` → `None`; referer & header user tampil di permintaan (server mencatat header).
- [ ] Jalankan (gagal), implementasi, jalankan (lulus).

### Task 6: Mesin download (tanpa jeda)
**Files:** Create `src/fetch/engine.rs`; Modify `src/fetch/mod.rs`, `src/report.rs`

**Interfaces:**
- Consumes: `Table`, `limit`, `FileInfo`, `client()`, `apply_headers`.
- Produces: `report::Event::Segments { parts: Vec<SegPart> }` dengan `SegPart { from: f64, to: f64, pos: f64 }` (0..1), `report::segments(Vec<SegPart>)`; `fetch::download(a: &DownloadArgs, info: &FileInfo, out_dir: &Path, parts: usize, limit: u64) -> anyhow::Result<PathBuf>`.
- Alur: tujuan = `names::unique(out_dir, info.name)`; `.part` diprealokasi (`set_len`); `std::thread::scope` menjalankan `parts` pekerja (satu bila `!ranges` atau `size` tak diketahui, tanpa Table, tulis berurutan); thread utama memantau tiap 100 ms: `report::check()` → set `stop`; emit `report::progress` (fraksi, "X MB · Y MB/s · sisa Z") dan `report::segments`; pekerja tidak memakai `report`.
- Pekerja: `claim` → GET `Range: bytes=pos-(end-1)`; wajib 206 + `Content-Range` cocok (200 → sinyal "tanpa Range": hentikan semua, ulang satu koneksi dari nol, log peringatan); baca potongan <= 64 KiB, `limit::acquire`, tulis di offset (`write_all_at` / `seek_write`), `advance`; baca `room` tiap potongan. Retry per bagian 6x dengan backoff, hitungan direset saat ada kemajuan. Status 429/403/503: turunkan `cap` (koneksi aktif maksimal) jadi `max(1, cap/2)` sekali per kejadian (compare_exchange), pekerja yang `active > cap` melepas bagian dan berhenti. Selesai: ukuran `.part` harus == total, lalu rename ke tujuan.
- Error akhir menghapus nothing otomatis (sidecar di Task 7 yang mengatur penyimpanan/penghapusan).

- [ ] Test integrasi (TestServer, 8 MiB data acak deterministik): 4 bagian → isi sama byte-per-byte; file 10 byte dengan 8 bagian → benar; 0 byte → file 0 byte dibuat; `ranges:false` → benar, hanya 1 permintaan data; `chunked:true` → benar; `drop_after:Some(1 MiB)` → benar lewat retry; `throttle_above:Some(2)` dengan 8 bagian → benar dan `max_concurrent` turun; tujuan sudah ada → hasil ` (1)` dan file lama tak berubah; progres dilaporkan (pakai `report::set_sink` tidak dipakai di test; cukup pastikan tidak panik tanpa konteks Job).
- [ ] Jalankan (gagal), implementasi, jalankan (lulus).

### Task 7: Jeda, lanjut, sidecar
**Files:** Modify `src/fetch/engine.rs`, `src/fetch/mod.rs`, `src/report.rs`

**Interfaces:**
- Produces: `report::Job { id, cancel, keep: Arc<AtomicBool> }`; `report::keep_partial() -> bool` (true bila cancel + keep); pembatalan tanpa `keep` menghapus `.part` + `<nama>.part.mtdl`; dengan `keep` atau error jaringan fatal menyimpannya dan `download` mengembalikan `Err` berisi "Dijeda" (jeda) — error fatal biasa menyimpan sidecar juga.
- Sidecar JSON: `{ url, total, etag, last_modified, segs: Vec<Seg> }` (tanpa header). Ditulis tiap ~1 s dan saat berhenti. `download` memeriksa sidecar untuk `info.name` di `out_dir` (nama `<name>.part` + sidecar ada dan url sama) → lanjut dari `segs`, kirim `If-Range`; bila server membalas 200 / `etag`/`last_modified`/`total` beda → mulai dari nol dengan `report::warn`. Resume memakai nama yang sama (tidak lewat `names::unique`) selama `.part`+sidecar ada.

- [ ] Test: jalankan `download` dengan `drop` yang selalu gagal setelah N byte dan batas retry habis? (lebih sederhana: pakai flag `report::Job` dengan `cancel+keep` diset dari thread lain setelah sebagian terunduh) → `Err` memuat "Dijeda", `.part` dan sidecar ada; panggil `download` lagi → isi sama byte-per-byte dan sidecar hilang; resume dengan ETag berubah → file akhir sesuai data baru; batal tanpa `keep` → `.part` dan sidecar terhapus; sidecar tidak memuat nilai header/referer (cari string rahasia di file).
- [ ] Jalankan (gagal), implementasi, jalankan (lulus).

### Task 8: Config, dispatch, CLI
**Files:** Modify `src/config.rs`, `src/download.rs`, `src/main.rs`

**Interfaces:**
- Config baru (`global = true`): `download_segments: usize` (`--dl-segments`, `VIDSUB_DL_SEGMENTS`, 8), `speed_limit: u64` (`--dl-limit`, `VIDSUB_SPEED_LIMIT`, 0, value_parser `limit::parse_size`), `download_parallel: usize` (`--dl-parallel`, `VIDSUB_DL_PARALLEL`, 3). Tambah ke `unsafe_default`, `save`, `print`, serde `#[serde(default = ...)]` agar config lama aman.
- `DownloadArgs` baru: `segments: Option<usize>` (`--segments`), `limit: Option<u64>` (`--limit`, value_parser `parse_size`), keduanya `#[serde(default)]`.
- `download::run`: jika `format` kosong dan bukan `playlist` dan `fetch::probe(a)` → `Some(info)` → `report::stage("Download file")`, `fetch::download(...)` (parts = `a.segments` atau config, dijepit 1..=32; limit = `a.limit` atau config), `Ok(vec![path])`; selain itu yt-dlp dengan tambahan `-N <parts>`. `cookies_from_browser` terisi + native → `report::warn`.
- `VideoInfo` + `kind: String` ("video"/"file"), `size: Option<u64>`, `filename: Option<String>`, `ranges: bool`; `download::probe` memanggil `fetch::probe` dulu.

- [ ] Test: config round-trip `save` memuat tiga kunci baru dan nilai lama tanpa kunci baru memakai default; `clap` mengurai `download URL --segments 4 --limit 2M`; `VideoInfo` untuk server file langsung (TestServer) bertipe "file".
- [ ] Jalankan (gagal), implementasi, jalankan `cargo test -p multitool` (semua lulus).

### Task 9: Backend GUI
**Files:** Modify `gui/src/main.rs`

**Interfaces:**
- `jobs: HashMap<u64, (Kind, Arc<AtomicBool> cancel, Arc<AtomicBool> keep)>`; `job()` membuat `report::Job` dengan `keep`; command `pause(job_id)` (set `keep` lalu `cancel`); `Settings`/`save_settings` sudah membawa `Config` jadi field baru ikut; validasi: segments 1..=32, parallel 1..=10.
- `pause` didaftarkan di `generate_handler!`.

- [ ] Implementasi, jalankan `cargo build -p multitool-gui` — harus sukses tanpa warning baru.

### Task 10: Frontend
**Files:** Modify `gui/ui/index.html`, `gui/ui/app.js`, `gui/ui/styles.css`

- [ ] Halaman Download: label jadi "Link (video atau file)"; hasil probe `kind=="file"` → preview teks (nama, ukuran, "multi-bagian: ya/tidak"), sembunyikan kartu Kualitas dan opsi yt-dlp; Opsi lanjutan menambah "Jumlah koneksi" (1-32, default dari pengaturan) dan "Batas kecepatan (KB/s, 0 = bebas)"; `dlArgs()` mengirim `segments` dan `limit` (byte).
- [ ] Pengaturan (kartu "Download & tampilan"): jumlah koneksi default, batas kecepatan default (KB/s), maks download bersamaan. Ikuti pola `loadSettings/collect` yang ada.
- [ ] `JobCard`: bar bagian (`.segbar` dengan div per `SegPart`, event `segments`), tombol Jeda (tampil hanya untuk download file) memanggil `pause`; saat download menolak dengan `Dijeda`, kartu selesai berstatus berhenti dan menampilkan tombol "Lanjutkan" yang memulai download yang sama lagi.
- [ ] Antrean: `withDownloadSlot(card, fn)` seperti `withProcess`, batas dari `S.settings.cfg.download_parallel`; kartu menunggu menampilkan "Menunggu giliran download…".
- [ ] Setelah download bila "Langsung transcribe" aktif tapi file bukan video/audio (ekstensi tidak dikenal): lewati dengan satu baris log.
- [ ] Verifikasi: `cargo build -p multitool-gui`; jalankan app debug bila ada display; kalau tidak bisa dijalankan di sini, laporkan itu apa adanya.

### Task 11: Dokumentasi dan shortcut
**Files:** Modify `README.md`; run installer

- [ ] README: bagian Download menjelaskan file langsung multi-bagian, jeda/lanjut, opsi CLI `--segments`/`--limit`, pengaturan baru.
- [ ] `cargo uninstall vidsub vidsub-gui` (abaikan bila tak ada), `cargo install --path gui`, `cargo install --path .`, `./packaging/install-shortcut.sh`.
- [ ] Verifikasi: `ls ~/.local/share/applications | grep -i -E "vidsub|multitool"` hanya menampilkan `multitool.desktop`; Desktop juga; `grep ^Name= ~/.local/share/applications/multitool.desktop` = `Name=MultiTool`.

## Self-review
Spec coverage: pemilihan mesin (T5, T8), bagian dinamis (T3, T6), retry/throttle/tanpa Range/validasi (T6), jeda-lanjut-sidecar (T7), batas kecepatan (T1), nama file (T2), pelaporan (T6, T7), antrean + GUI + pengaturan (T9, T10), CLI + `-N` (T8), shortcut + README (T11), pengujian (tiap task). Nama tipe konsisten antar task (`Seg`, `Table`, `FileInfo`, `SegPart`, `report::keep_partial`).
