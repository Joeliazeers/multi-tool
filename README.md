# MultiTool

Aplikasi desktop **GUI** (Rust + Tauri) dan CLI untuk:

1. **Download video** dengan `yt-dlp`: cukup masukkan link. Referer, cookies browser, dan format bersifat opsional.
2. **Transcribe video → SRT**: bahasa ditebak whisper (atau dipilih manual), whisper membuat transkrip, lalu LLM merapikannya menjadi SRT dalam bahasa yang dipilih.
3. **Translate SRT** ke bahasa apa pun dengan LLM, **tanpa sensor** (makian, slang, dan konten eksplisit diterjemahkan apa adanya).
4. **QR Code** dari link/teks, simpan sebagai PNG, JPG, SVG, WebP, atau BMP.

## Kebutuhan

- `yt-dlp`, `ffmpeg`, `ffprobe` di PATH
- Speech-to-text: venv Python dengan `faster-whisper` (default `~/whisper-env/bin/python`), **atau** API whisper (Groq/OpenAI) lewat `--asr api`
- LLM dengan API OpenAI-compatible (default: Ollama di `localhost:11434`)

## Install

Library sistem untuk GUI (Ubuntu/Debian, sekali saja):

```bash
sudo apt install -y libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev librsvg2-dev libxdo-dev libssl-dev build-essential
```

```bash
cargo install --path gui           # GUI  -> multitool-gui
cargo install --path .             # CLI  -> multitool (opsional)
./packaging/install-shortcut.sh    # shortcut GUI di menu aplikasi + Desktop
```

> **Pindah dari vidsub?** Jalankan `cargo uninstall vidsub vidsub-gui`, lalu install ulang di atas.
> Pengaturanmu aman: file konfigurasi tetap `~/.config/vidsub/config.env` dan variabel `VIDSUB_*` tetap berlaku.
> Folder download default sekarang `~/Videos` (sebelumnya `~/Videos/vidsub`).

## Windows

Download **`MultiTool_*_x64-setup.exe`** (installer, tanpa hak admin) atau **`MultiTool-portable.exe`** dari
tab **Releases** repo ini (atau dari *Actions → Windows build → Artifacts*). Windows SmartScreen mungkin
memperingatkan karena aplikasi belum ditandatangani: klik *More info → Run anyway*.

Syarat (sekali pasang, di PowerShell):

```powershell
winget install yt-dlp.yt-dlp Gyan.FFmpeg      # download video + ekstrak audio
```

Tutup & buka ulang aplikasi supaya PATH terbaca; **Pengaturan → Status sistem** menunjukkan apa yang sudah terdeteksi.
WebView2 sudah ada di Windows 11 dan Windows 10 yang ter-update.

Transcribe butuh salah satu dari:

- **API whisper** (paling mudah, tanpa Python): Pengaturan → mesin whisper → API (Groq/OpenAI), isi API key.
- **Lokal**: `py -m venv %USERPROFILE%\whisper-env` lalu `%USERPROFILE%\whisper-env\Scripts\pip install faster-whisper`.
  Default Python-nya `%USERPROFILE%\whisper-env\Scripts\python.exe`.

Konfigurasi disimpan di `%APPDATA%\vidsub\config.env`; folder download default `%USERPROFILE%\Videos`.

**Build sendiri** (di Windows, perlu [Rust](https://rustup.rs) + Visual Studio Build Tools workload "C++"):

```powershell
.\packaging\build-windows.ps1     # hasil di folder dist\
```

Atau biarkan GitHub membangunnya: push ke repo, lalu lihat *Actions → Windows build*. Push tag `v*`
(mis. `git tag v0.3.0 && git push --tags`) untuk otomatis membuat Release berisi file `.exe`.

## GUI

Klik shortcut **MultiTool**, atau jalankan `multitool-gui`.

- **Download** — tempel link → judul, thumbnail & resolusi tersedia muncul otomatis. Pilih kualitas
  (Terbaik/1080p/720p/480p/360p/Audio saja), referer, folder simpan, cookies browser. Bisa langsung
  lanjut transcribe & translate setelah download.
- **Transcribe** — pilih atau seret video ke jendela. Bahasa otomatis (hasil tebakan LLM ditampilkan
  untuk dikonfirmasi) atau manual. Simpan ke folder lain / "Simpan sebagai…".
- **Translate** — pratinjau SRT, bahasa asal/tujuan, gaya bahasa (gaul, formal, …), lokasi simpan.
- **QR Code** — tempel link/teks, QR muncul langsung. Atur ukuran, level koreksi error, warna, lalu
  **Simpan sebagai…** (PNG, JPG, SVG, WebP, BMP) atau salin gambar.
- **Pengaturan** — model LLM (daftar dari server), mesin whisper, folder default, tema, status sistem.
- Setiap pekerjaan punya kartu progres sendiri (persentase, kecepatan, sisa waktu, **Batal**, log).
  **Download bisa berjalan bersamaan** dengan transcribe/translate; transcribe & translate
  (berbagi CPU dan LLM) dijalankan satu per satu — yang ditekan saat sibuk otomatis masuk antrean.

Desain mengikuti rekomendasi skill UI/UX Pro Max (flat, mode gelap/terang, Inter, ikon Lucide).

## CLI

Menu interaktif di terminal:

```bash
multitool
```

Di menu, setiap fitur menanyakan **lokasi simpan**: pakai default, pilih folder/file lewat
dialog (zenity), atau ketik path. File input (video/SRT) juga bisa dipilih lewat dialog.

Atau lewat perintah langsung:

```bash
# download (referer opsional, -o = folder simpan, -q = kualitas)
multitool download "https://contoh.com/video" -r "https://situs-embed.com/" -o ~/Videos/film -q 720p
multitool download "URL" --cookies-from-browser firefox

# transcribe: bahasa auto (whisper menebak) atau manual
multitool transcribe video.mp4                 # -> video.<lang>.srt
multitool transcribe video.mp4 --lang ja       # manual
multitool transcribe video.mp4 --refine        # + rapikan dengan LLM (default: SRT whisper apa adanya)
multitool transcribe video.mp4 -o ~/Subtitle/  # -o = folder atau file .srt

# translate
multitool translate video.ja.srt --to id       # -> video.id.srt
multitool translate video.ja.srt --to "Indonesian (gaul Jakarta)" -o ~/Subtitle/film-indo.srt

# QR code dari link (format dari ekstensi: png, jpg, svg, webp, bmp)
multitool qr "https://contoh.com" -o ~/Pictures/qr.png
multitool qr "https://contoh.com" -o qr.svg --ec H --fg "#1a1a8a" --bg "#ffffff" --size 800

# semua sekaligus
multitool auto "URL" -r "https://referer/" --to id -o ~/Videos/film -s ~/Subtitle

multitool config                               # lihat konfigurasi aktif
```

## Konfigurasi

Lewat flag, environment variable, atau file `~/.config/vidsub/config.env` / `./.env`.
Lihat `.env.example`.

**Model untuk terjemahan tanpa sensor:** model biasa (mis. `qwen2.5:7b-instruct`) masih suka
menghaluskan kata kasar. Pakai model *abliterated* (penolakan/sensornya dihapus):

```bash
ollama pull huihui_ai/aya-expanse-abliterated:8b   # ~5 GB, multibahasa (termasuk Indonesia)
echo 'VIDSUB_LLM_MODEL=huihui_ai/aya-expanse-abliterated:8b' >> ~/.config/vidsub/config.env
```

Alternatif: `huihui_ai/qwen2.5-abliterate:7b` (4.7 GB), `huihui_ai/qwen3-abliterated:8b` (5 GB, model "thinking", lebih lambat).

## Cara kerja

- **Deteksi bahasa:** deteksi bawaan whisper pada sampel 3 menit (beberapa jendela 30 detik,
  bagian tanpa suara dilewati). Kalau sampel tidak berisi ucapan, bahasa harus dipilih manual.
  Di GUI hasilnya (plus kandidat lain) ditampilkan untuk dikonfirmasi.
- **Subtitle:** whisper (word timestamps) dipecah menjadi baris ≤84 karakter / ≤7 detik, dan dipotong di jeda dan tanda baca.
- **Penjaga sinkron:** LLM wajib menyalin teks sumber tiap baris. Baris yang "bergeser" ditolak dan diulang.
  Baris yang panjangnya janggal diproses ulang satu per satu. Koreksi transkrip yang terlalu jauh
  dari audio dibuang (teks whisper yang dipakai), jadi timing tetap sinkron.
