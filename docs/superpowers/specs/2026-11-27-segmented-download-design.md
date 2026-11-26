# Download multi-bagian ala IDM + shortcut MultiTool (desain)

Status: menunggu persetujuan user (disusun 2026-11-27).

## Maksud
User biasanya mendownload **link file langsung** (zip, iso, mp4, dll.), bukan situs video. Sekarang semua download lewat yt-dlp dengan satu koneksi, jadi lambat. Fitur ini membagi satu file jadi beberapa bagian yang diunduh bersamaan, seperti Internet Download Manager (IDM), dan ditambah beberapa perbaikan atas IDM. Semuanya ada di dalam MultiTool; user tidak perlu aplikasi lain.

Sekaligus: shortcut di komputer user masih bernama `vidsub` (menunjuk `vidsub-gui`). Kode sudah bernama MultiTool, tapi belum pernah di-install ulang.

## Tujuan
- Link file langsung diunduh lewat banyak koneksi paralel, jauh lebih cepat dari satu koneksi bila server membatasi kecepatan per koneksi.
- Jeda dan lanjut yang tahan sampai aplikasi ditutup dan dibuka lagi.
- Satu halaman Download: tempel link, aplikasi memilih mesin sendiri (file langsung = mesin baru, situs video = yt-dlp seperti sekarang).
- Shortcut menu aplikasi + Desktop bernama **MultiTool**, yang lama (`vidsub`) hilang.

## Bukan tujuan / anti-tujuan
- Tidak ada dependensi sistem baru (tidak pakai aria2c). Tetap Rust + `reqwest` yang sudah ada.
- Tidak mengubah cara kerja download situs video selain menambah `-N` (fragmen paralel) ke yt-dlp.
- Tahap berikutnya, bukan sekarang: jadwal download, penangkap link dari clipboard/browser, cek checksum, mirror, FTP/torrent, daftar otomatis unduhan yang belum selesai saat aplikasi dibuka.
- Gagal berarti: file rusak tapi dianggap selesai. Lebih baik error jelas daripada file korup.

## Kendala
- Stack: Rust edition 2024, Tauri 2, HTML/JS polos di `gui/ui`, teks UI bahasa Indonesia.
- `reqwest` dipakai dengan `default-features = false` (tidak ada dekompresi otomatis). Itu menguntungkan: kirim `Accept-Encoding: identity` supaya byte range sama dengan byte file.
- Progres dan pembatalan lewat `report` (`Job`, `cancel`, `Event`). Event baru harus ikut dikirim ke GUI lewat sink yang sama dan tetap bisa dicetak di CLI.
- Config lama (`~/.config/vidsub/config.env`, `VIDSUB_*`) tidak boleh dipindah.
- Harus jalan di Linux dan Windows (tulis posisi: `FileExt::write_all_at` di Unix, `seek_write` di Windows).

## Pendekatan yang dipertimbangkan
1. **Mesin sendiri di Rust untuk link langsung, yt-dlp tetap untuk situs video. Dipilih user.** Tanpa dependensi baru, kendali penuh atas progres/jeda/bagian.
2. Membungkus `aria2c`. Ditolak: user harus memasang, bundling di Windows repot, progres per bagian terbatas.
3. Mesin sendiri untuk semuanya, termasuk stream yt-dlp. Ditolak: terlalu besar dan tidak dibutuhkan user.

## Keputusan

### Memilih mesin (`download::run` dan `probe`)
- Satu request **GET dengan `Range: bytes=0-0`** (jangan HEAD, banyak server menolaknya), ikuti redirect, kirim referer/header user.
- Dianggap **file langsung** bila respons sukses dan `Content-Type` bukan `text/html`. Selain itu (HTML, 4xx/5xx, error jaringan) diserahkan ke yt-dlp, yang akan memberi pesan error yang pas.
- Dari respons yang sama diambil: ukuran total (`Content-Range` / `Content-Length`), dukungan Range (206), `ETag`, `Last-Modified`, nama file.
- `cookies_from_browser` hanya berlaku untuk yt-dlp; di mesin baru diabaikan dengan peringatan (cookie bisa dikirim lewat `-H "Cookie: ..."`).

### Mesin baru: modul `src/fetch.rs`
- File sasaran di-`set_len` (prealokasi) sebagai `<nama>.part`; tiap pekerja menulis di offset sendiri. Setelah selesai dan ukuran cocok, `.part` di-rename ke nama akhir. Kalau nama akhir sudah ada, tambah ` (1)`, ` (2)`, dst.
- **Bagian dinamis**: daftar bagian `{start, pos, end}` di dalam satu `Mutex`. Awalnya dibagi rata ke N koneksi. Pekerja yang bagiannya habis mengambil bagian lain dengan sisa terbesar (bila sisa lebih dari 2 MiB) dan membelah sisanya jadi dua, lalu mengerjakan separuh atas. Pekerja pemilik mengecek `end` tiap potongan data, jadi ia berhenti di batas baru. Hasilnya koneksi tidak menganggur di akhir download.
- Jumlah koneksi: default 8, rentang 1-32, bisa diatur di Pengaturan, per download, dan CLI `--segments`. Ukuran file kecil (< 1 MiB per bagian) memakai lebih sedikit bagian.
- **Retry**: tiap bagian maksimal 6 percobaan, jeda eksponensial (1, 2, 4, ... detik, dibatasi 30). Setiap percobaan lanjut dari `pos` terakhir. Kalau server membalas 429/403/503 pada banyak koneksi, jumlah koneksi aktif dipangkas separuh dan download dilanjutkan.
- **Tanpa dukungan Range** (respons 200, atau ukuran tidak diketahui): satu koneksi, tanpa resume, dengan pesan jelas di log. Bila sebuah bagian dijawab 200 padahal minta Range, berhenti paralel dan ulang dengan satu koneksi.
- **Validasi**: respons 206 harus punya `Content-Range` yang cocok dengan yang diminta. Ukuran akhir harus sama dengan total. Beda satu byte pun dianggap gagal, bukan selesai.
- **Jeda dan lanjut**: jeda menghentikan pekerjaan dan **menyimpan** `.part` plus sidecar `<nama>.part.mtdl` (JSON: url tanpa rahasia, total, etag, last_modified, daftar bagian dengan `pos`). Sidecar ditulis tiap ~1 detik dan saat jeda. Header/cookie yang dimasukkan user tidak disimpan ke disk; user mengisinya lagi bila perlu. Menempel link yang sama lagi (nama file yang sama) otomatis menemukan sidecar dan melanjutkan. Sebelum lanjut, kirim `If-Range` dengan ETag/Last-Modified; kalau file berubah di server, mulai dari nol dengan peringatan. **Batal** menghentikan dan menghapus `.part` dan sidecar.
- **Batas kecepatan** global untuk semua download mesin baru dalam proses (token bucket), 0 = tanpa batas. Pengaturan + `--limit` (mis. `2M`, `500K`).
- **Nama file**: `Content-Disposition` (`filename*` lalu `filename`), kalau tidak ada segmen terakhir URL (percent-decode), kalau kosong `download`. Dibersihkan dari pemisah folder, karakter kontrol, karakter terlarang Windows, dan nama terlarang Windows (`CON`, `NUL`, dst.).

### Pelaporan
- Event baru `report::Event::Segments { total, done, parts: Vec<{from, to, pos}> }` (rentang dinormalkan 0..1) selain `Progress` yang sudah ada (`fraction` + teks kecepatan/ETA). Event dibatasi sekitar 10 kali per detik.
- Tombol Jeda memakai satu bendera tambahan pada `report::Job` ("simpan sisa") di samping `cancel`; Batal tetap seperti sekarang dan menghapus sisa.

### Antrean
- Antrean ada di frontend: kartu pekerjaan yang sudah ada menunggu giliran. Maksimal download bersamaan default **3**, diatur di Pengaturan. Sisanya berstatus "Menunggu".
- Backend tetap mengizinkan banyak `Kind::Download` bersamaan; batasnya hanya di frontend.

### GUI (halaman Download)
- Label "Link video" jadi "Link (video atau file)". "Cek video" juga memeriksa file langsung: preview menampilkan nama file, ukuran, dan "multi-bagian: ya/tidak" (tanpa thumbnail).
- Untuk file langsung: bagian Kualitas disembunyikan; "Opsi lanjutan" berisi jumlah koneksi dan batas kecepatan. "Setelah download" (transcribe/translate otomatis) hanya dijalankan bila hasilnya file video/audio; selain itu dilewati diam-diam dengan satu baris log.
- Kartu pekerjaan: bar bagian (kotak-kotak per bagian, mengikuti event `Segments`), kecepatan, sisa waktu, tombol **Jeda / Lanjut / Batal**.
- Pengaturan baru: jumlah koneksi default, batas kecepatan, maksimal download bersamaan. Simpan lewat pola `config.rs` yang ada.

### CLI
- `multitool download URL [--segments N] [--limit 2M]` (opsi panjang saja, karena `-s` sudah dipakai `auto --srt-dir`). Output: progres satu baris yang sudah ada. Ctrl+C = jeda (sisa disimpan).

### yt-dlp
- Tambah `-N <segments>` pada pemanggilan yt-dlp untuk mempercepat HLS/DASH. Tidak ada perubahan lain.

### Shortcut
- Jalankan `cargo uninstall vidsub vidsub-gui` (bila terpasang), `cargo install --path gui`, `cargo install --path .`, lalu `packaging/install-shortcut.sh`. Skrip sudah menghapus `vidsub.desktop` di menu dan Desktop, dan memasang `multitool.desktop` dengan `Name=MultiTool`. (Pilihan saya: "MultiTool" sesuai nama aplikasi; user menulis "multi tool", mudah diganti ke "Multi Tool" bila mau.)
- README dan `--help` diperbarui untuk fitur baru.

## Pengujian
- Server HTTP lokal sederhana di dalam test (hanya `std::net`, tanpa dependensi dev baru) yang bisa: melayani Range, tidak mendukung Range, memutus koneksi di tengah, mengganti ETag, menolak dengan 429.
- Unit: pembagian awal dan pembelahan bagian dinamis, parsing nama file (`filename*`, nama berbahaya), pembatas kecepatan, parsing ukuran `2M`/`500K`, round-trip sidecar.
- Integrasi: download 8 MiB dengan 4 bagian harus identik byte-per-byte; jeda lalu lanjut menghasilkan file identik; server tanpa Range tetap menghasilkan file benar; ETag berubah memulai ulang; kegagalan di tengah di-retry.
- GUI: periksa manual lewat build debug (selftest yang ada, `gui/ui/selftest.js`, bila memungkinkan).

## Diserahkan ke pembuat
Ukuran potongan baca/tulis, tata letak persisnya bar bagian dan gaya kartu (ikuti gaya kartu yang ada), nama field konfigurasi (ikuti pola `config.rs`), teks pesan error bahasa Indonesia, dan apakah bar bagian digambar dengan elemen div atau canvas.
