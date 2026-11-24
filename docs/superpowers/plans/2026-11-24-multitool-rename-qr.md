# MultiTool Rename + QR Generator Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename the app from vidsub to MultiTool and add a QR code generator (GUI page + CLI command) that saves PNG/JPG/SVG/WebP/BMP.

**Architecture:** QR logic is one new module `src/qr.rs` in the shared library: it builds the module matrix with the `qrcode` crate and draws it itself (RGB image or hand-written SVG), so there is a single rendering path. The Tauri GUI exposes three thin commands over it; the CLI gets a `qr` subcommand. The rename is done first so all new code uses the new crate name `multitool`.

**Tech Stack:** Rust 2024, Tauri 2, clap 4, `qrcode` (matrix only, no image feature), `image` (png/jpeg/webp/bmp features only), `base64`; plain HTML/JS/CSS UI in Indonesian.

**Spec:** `docs/superpowers/specs/2026-11-24-multitool-rename-qr-design.md`

## Global Constraints

- User-visible "vidsub" becomes "MultiTool"; binaries are `multitool` (CLI) and `multitool-gui` (GUI); library crate is `multitool`.
- Do NOT rename: config path `~/.config/vidsub/config.env` (`config::config_file()` stays unchanged, so no fallback code is needed), the `VIDSUB_*` env vars (including `VIDSUB_SELFTEST`, `VIDSUB_GUI_BIN`), the `localStorage` key `vidsub-theme`, and the temp script name `vidsub_whisper.py`.
- The default download folder changes from `<Videos>/vidsub` to the plain system Videos folder (`dirs::video_dir()`, else `~/Videos`) in `Config::output_dir()` (user decision). An explicit `VIDSUB_OUTPUT_DIR`/saved setting still wins. Update README and `.env.example` mentions of `~/Videos/vidsub`.
- QR generation must need no new system packages and no network.
- Supported save formats: PNG, JPG (`.jpg`/`.jpeg`), SVG, WebP, BMP. Default size 512 px, default EC level M, default margin 4 modules, default colors `#000000` on `#ffffff`.
- UI text is Indonesian, styled with the existing card/segmented/btn classes; sidebar order becomes Download, Transcribe, Translate, QR Code (Ctrl 4), Pengaturan (Ctrl 5).
- No git commits unless the user asks (the repo has no commits yet); each task ends at a green build/test instead.
- Never drop existing behavior of download/transcribe/translate.

## Review Focus

- Empty or whitespace-only text: friendly error, no panic (Task 2 test).
- Text too long for any QR code (e.g. 8000 chars): friendly error saying it is too long (Task 2 test).
- Invalid color (`red`, `#12`, `#gggggg`) and identical fg/bg: error naming the problem (Task 2 test).
- Save path with no extension or an unknown one (`.txt`): no extension gets `.png`; unknown errors and lists supported formats (Task 2 test).
- Requested size smaller than the QR itself (e.g. 10 px) or not a multiple of the module count: still scannable output, 1 px per module minimum, dimensions documented (Task 2 test).

---

### Task 1: Rename vidsub → MultiTool

**Files:**
- Modify: `Cargo.toml`, `gui/Cargo.toml`, `gui/tauri.conf.json`, `src/lib.rs`, `src/main.rs`, `src/menu.rs`, `src/config.rs` (only `try_parse_from(["vidsub"])`), `gui/src/main.rs`, `gui/ui/index.html`, `gui/ui/styles.css` (header comment), `packaging/install-shortcut.sh`, `README.md`
- Rename: `packaging/vidsub.svg` → `packaging/multitool.svg`

**Interfaces:**
- Produces: library crate `multitool` (imports become `multitool::...`), binaries `multitool` and `multitool-gui`.

- [ ] **Step 1: Baseline.** Run `cargo build --workspace` and confirm it succeeds before touching anything.

- [ ] **Step 2: Rename packages and imports.** `Cargo.toml` package `name = "multitool"`; `gui/Cargo.toml` package/bin `multitool-gui` with dependency `multitool = { path = ".." }` and a description mentioning MultiTool; replace every `use vidsub::` / `vidsub::` path in `src/main.rs`, `src/menu.rs`, `gui/src/main.rs` with `multitool::`; clap `#[command(name = "multitool", version)]`; menu banner `=== MultiTool — … ===`; `lib.rs` doc comments; `set_title("MultiTool")` and the `expect` message in `gui/src/main.rs`.

- [ ] **Step 3: Rename GUI/Tauri strings.** `gui/tauri.conf.json`: `productName` and window `title` = `MultiTool`, `identifier` = `id.multitool.desktop`, `shortDescription` = "Download video, subtitle, QR code, and more". `gui/ui/index.html`: `<title>` and sidebar brand text = `MultiTool`. `styles.css` header comment.

- [ ] **Step 4: Rename packaging.** `git mv`-free rename of `packaging/vidsub.svg` to `packaging/multitool.svg`. In `install-shortcut.sh`: data dir `~/.local/share/multitool`, desktop file `multitool.desktop` with `Name=MultiTool`, `StartupWMClass=multitool-gui`, `Exec` from `command -v multitool-gui`, message text updated; first remove the old `$app_dir/vidsub.desktop`, `$desk/vidsub.desktop` and `~/.local/share/vidsub/` if present. `VIDSUB_GUI_BIN` stays as the override variable name.

- [ ] **Step 4b: Default download folder.** In `src/config.rs` `Config::output_dir()` drop the trailing `.join("vidsub")`; also fix the doc comment on the `output_dir` field ("default: ~/Videos") and `.env.example`.

- [ ] **Step 5: Update README.** Title and all prose/commands to MultiTool/`multitool`/`multitool-gui`; add a short "Pindah dari vidsub" note: run `cargo uninstall vidsub vidsub-gui`, settings stay in `~/.config/vidsub/config.env` and `VIDSUB_*` variables keep working.

- [ ] **Step 6: Verify.** Run `cargo build --workspace` (expect success) and `cargo run -q -- --version` (expect output starting with `multitool`). Run `grep -rIn -i vidsub --exclude-dir=target --exclude-dir=gen --exclude-dir=docs --exclude=Cargo.lock .` and confirm every remaining hit is on the Global Constraints "do not rename" list or is the README migration note.

---

### Task 2: QR module (`src/qr.rs`)

**Files:**
- Create: `src/qr.rs` (tests in a `#[cfg(test)] mod tests` at the bottom)
- Modify: `Cargo.toml` (deps), `src/lib.rs` (`pub mod qr;`)

**Interfaces:**
- Consumes: nothing from earlier tasks except the crate name.
- Produces (all `pub` in `multitool::qr`):
  - `enum Ec { L, M, Q, H }` — `Clone, Copy, Debug, PartialEq, Deserialize, clap::ValueEnum`, serde `rename_all = "UPPERCASE"`.
  - `struct QrOptions { size: u32, ec: Ec, fg: String, bg: String, margin: u32 }` — `Clone, Debug, Deserialize, clap::Args`, `#[serde(default)]`, `Default` = 512 / M / `#000000` / `#ffffff` / 4. CLI flags `--size`, `--ec`, `--fg`, `--bg`, `--margin`.
  - `enum Format { Png, Jpg, Svg, Webp, Bmp }` with `fn from_path(path: &Path) -> anyhow::Result<Format>` (case-insensitive extension; `jpeg` = Jpg; unknown/missing → error listing `png, jpg, svg, webp, bmp`).
  - `fn encode(text: &str, opts: &QrOptions, fmt: Format) -> anyhow::Result<Vec<u8>>`
  - `fn png_data_url(text: &str, opts: &QrOptions) -> anyhow::Result<String>` (`data:image/png;base64,…`)
  - `fn save(text: &str, opts: &QrOptions, path: &Path) -> anyhow::Result<PathBuf>` (format from extension; a path with no extension gets `.png` appended and that path is returned; creates no parent dirs).

- [ ] **Step 1: Add dependencies.** `cargo add qrcode --no-default-features`, `cargo add image --no-default-features --features png,jpeg,webp,bmp`, `cargo add base64`. Add `pub mod qr;` to `src/lib.rs`.

- [ ] **Step 2: Write the failing tests** in `src/qr.rs` (stub the public items with `todo!()` so it compiles). Tests, each with these assertions:
  - `png_has_signature_and_square_size`: `encode("https://example.com", &Default::default(), Format::Png)` starts with `[0x89, b'P', b'N', b'G']`; decoded via `image::load_from_memory` is square, width `<= 512` and `>= 256`.
  - `size_is_whole_pixels_per_module`: for size 100 the decoded width is a multiple of the module count plus margins (`width % (modules + 2*margin) == 0`; compute `modules` with `qrcode::QrCode::new(...).width()`), and `<= 100`.
  - `tiny_size_falls_back_to_one_px_per_module`: size 10 → width equals `modules + 2*margin`.
  - `svg_contains_colors`: with `fg = "#ff0000"`, output is UTF-8, contains `<svg` and `#ff0000`.
  - `jpg_webp_bmp_decode`: each of the three formats decodes with `image::load_from_memory`.
  - `top_left_is_background_and_has_dark_modules`: decoded PNG pixel (0,0) is `bg`; at least one pixel equals `fg`.
  - `higher_ec_makes_bigger_or_equal_code`: PNG width with `Ec::H` is `>=` width with `Ec::L` at size 1 (1 px/module) for the same 200-char text.
  - `empty_text_errors`: `""` and `"   "` → `Err`, message contains "kosong".
  - `too_long_text_errors`: 8000-char text → `Err`, message contains "terlalu panjang".
  - `bad_colors_error`: `fg = "red"`, `"#12"`, `"#gggggg"` → `Err`; `"#fff"` and `"#FFFFFF"` → `Ok`; `fg == bg` → `Err` containing "sama".
  - `format_from_path`: `a.PNG`→Png, `a.jpeg`→Jpg, `a.jpg`→Jpg, `a.svg`, `a.webp`, `a.bmp`; `a.txt` and `a` → `Err`, the message contains `png, jpg, svg, webp, bmp`.
  - `save_appends_png_when_no_extension`: in a `tempfile::tempdir()`, `save(..., dir/"qr")` returns `dir/qr.png`, file exists and starts with the PNG signature; saving to `dir/x.svg` writes a file starting with `<?xml` or `<svg`.
  - `data_url_prefix`: `png_data_url` starts with `data:image/png;base64,` and decodes back to a PNG.

- [ ] **Step 3: Run tests to see them fail.** `cargo test -p multitool qr::` → expect FAIL (`not yet implemented`).

- [ ] **Step 4: Implement.** Approach, which the signatures and tests leave open:
  - Parse colors with a private `fn parse_color(s: &str) -> anyhow::Result<[u8; 3]>` (accepts `#rgb`/`#rrggbb`, leading `#` optional).
  - Build `QrCode::with_error_correction_level(text.trim().as_bytes(), level)`; map the library's data-too-long error to "Teks terlalu panjang untuk QR code (maks ±2900 karakter untuk level H…)" style Indonesian message; empty/whitespace → "Teks kosong".
  - `total = modules + 2*margin`; `scale = max(1, size / total)`; canvas side = `total*scale`. Draw dark modules (`code.to_colors()[y*modules+x] == Color::Dark`) as `scale×scale` blocks of `fg` over a `bg` canvas; encode with `image` as RGB8 (JPG via `JpegEncoder`, quality 95). SVG is a hand-built string: `viewBox="0 0 total total"`, `shape-rendering="crispEdges"`, a `bg` rect, and one `path` of `M x y h1v1h-1z` per dark module in `fg`; width/height attributes = canvas side.
  - `png_data_url` = `data:image/png;base64,` + `base64::engine::general_purpose::STANDARD`.

- [ ] **Step 5: Run tests.** `cargo test -p multitool qr::` → all pass. Also `cargo build --workspace` still succeeds.

---

### Task 3: CLI `qr` subcommand

**Files:**
- Modify: `src/main.rs`, `README.md` (CLI section)

**Interfaces:**
- Consumes: `multitool::qr::{QrOptions, save}` from Task 2.
- Produces: `multitool qr <TEXT> [-o <FILE>] [--size N] [--ec L|M|Q|H] [--fg #hex] [--bg #hex] [--margin N]`; default `-o` is `qr.png` in the current directory; prints `✔ QR disimpan: <path>`.

- [ ] **Step 1: Add the variant** `Qr { text: String, #[arg(short='o', long, default_value="qr.png")] out: PathBuf, #[command(flatten)] opts: qr::QrOptions }` with doc comment "Buat QR code dari link/teks, simpan sebagai PNG/JPG/SVG/WebP/BMP", and its arm in `main` calling `qr::save` and printing the saved path.

- [ ] **Step 2: Verify.** From the scratchpad directory run `cargo run -q --manifest-path <repo>/Cargo.toml -- qr "https://example.com" -o <scratchpad>/t.png`, then `-o t.jpg`, `t.svg`, `t.webp`, `t.bmp`; `file <scratchpad>/t.*` must report PNG, JPEG, SVG, WebP, and PC bitmap respectively. `multitool qr "" ` must exit non-zero with the "Teks kosong" message; `-o t.txt` must exit non-zero listing the formats. Decode check if `zbarimg` exists: `zbarimg t.png` prints the URL (skip if not installed, and say so).

- [ ] **Step 3: README.** Add a `qr` example to the CLI section and a GUI bullet for the QR Code page.

---

### Task 4: Tauri commands

**Files:**
- Modify: `gui/src/main.rs`

**Interfaces:**
- Consumes: `multitool::qr::{QrOptions, png_data_url, save}`.
- Produces (registered in `generate_handler!`):
  - `#[tauri::command] fn qr_preview(text: String, opts: qr::QrOptions) -> R<String>` — data URL for the preview.
  - `#[tauri::command] async fn qr_save(text: String, opts: qr::QrOptions, path: String) -> R<String>` — uses `blocking(...)`, returns the saved path.
  - `#[tauri::command] async fn pick_save_qr(app: tauri::AppHandle, suggested: Option<String>) -> Option<String>` — save dialog, default name `qr.png` in `~` (or the suggested dir), filters in this order: PNG `png`, JPEG `jpg`/`jpeg`, SVG `svg`, WebP `webp`, BMP `bmp`; returns the chosen path (extension validation is done by `qr_save`).

- [ ] **Step 1: Implement the three commands** following the style of `save_srt`/`blocking` in the same file, and register them.

- [ ] **Step 2: Verify.** `cargo build -p multitool-gui` succeeds with no new warnings.

---

### Task 5: QR Code page in the GUI

**Files:**
- Modify: `gui/ui/index.html`, `gui/ui/app.js`, `gui/ui/styles.css`

**Interfaces:**
- Consumes: Tauri commands `qr_preview`, `qr_save`, `pick_save_qr`, `open_path` (existing).
- Produces: nav button `data-page="qr"` and section `#page-qr`.

- [ ] **Step 1: Sidebar and shortcuts.** In `index.html` add a QR nav item (new `i-qr-code` Lucide symbol in the sprite, label "QR Code", `<kbd>Ctrl 4</kbd>`) between Translate and the separator, and change Pengaturan to `Ctrl 5`. In `app.js` extend the Ctrl-digit handler to `["1".."5"]` mapping `["download","transcribe","translate","qr","settings"]`.

- [ ] **Step 2: Markup `#page-qr`** (`hidden` by default, same `page-head` pattern: h1 "QR Code", p "Tempel link atau teks, QR code langsung dibuat."): one card with the link input (`#qr-text`, `type="text"`, placeholder `https://…`, reuses `.input-group` with the link icon) and a two-column area: left options, right preview. Options: size (`#qr-size` number 64–2048, default 512), EC level segmented (`name="qr-ec"`, L/M/Q/H, M checked, help text "Makin tinggi, makin tahan rusak tapi makin padat"), margin (`#qr-margin`, 0–16, default 4), two color inputs (`#qr-fg` `#000000`, `#qr-bg` `#ffffff`, `type="color"`). Preview: `<img id="qr-img" alt="Pratinjau QR code">` on a checkerboard-free neutral card, an empty-state text "Ketik link untuk membuat QR", `#qr-error` (`role="alert"`, `.field-error`, hidden), and buttons "Simpan sebagai…" (`#qr-save`, primary, disabled until a QR exists) and "Salin gambar" (`#qr-copy`, hidden when `window.ClipboardItem` is undefined). After saving, show the saved path with a "Buka folder" link calling `open_path` with `reveal: true`.

- [ ] **Step 3: Behavior in `app.js`.** Debounce (250 ms) any input/change on the QR controls → `invoke("qr_preview", { text, opts: { size, ec, fg, bg, margin } })`; ignore stale responses (compare a request counter, like the probe code does with `probedKey`); empty text clears the preview and disables the buttons without showing an error; errors from the backend go to `#qr-error` and clear the image. "Simpan sebagai…" calls `pick_save_qr`, then `qr_save`; success shows a toast/status line "Tersimpan: <path>" using whatever notification helper `app.js` already uses (find it; do not invent a new one). "Salin gambar" fetches the data URL → blob → `navigator.clipboard.write([new ClipboardItem({"image/png": blob})])`; on rejection show "Salin gambar tidak didukung di sistem ini" and hide the button.

- [ ] **Step 4: Styles** in `styles.css`, reusing existing tokens (card, segmented, btn, field-error); preview image `image-rendering: pixelated`, max 100% of its box, `aspect-ratio: 1`; the two-column area collapses to one column below the app's existing narrow breakpoint.

- [ ] **Step 5: Verify statically.** `node --check gui/ui/app.js` passes; `cargo build -p multitool-gui` passes.

- [ ] **Step 6: Verify in the running app.** Launch with `cargo run -p multitool-gui` (use the `run` skill if available). In the QR page: type `https://example.com` → preview appears within about half a second; change EC level and colors → preview updates; clear the text → preview clears with no error; Save as PNG and JPG, then check with `file`; press Ctrl 4 and Ctrl 5 to confirm the shortcuts; confirm the window title and sidebar read "MultiTool". Report any item that could not be checked (for example if no display is available) instead of claiming it passed.

---

## Self-review notes

- Spec coverage: rename (Task 1), formats and options (Task 2), CLI (Task 3), Tauri commands (Task 4), GUI page/copy/save/shortcuts (Task 5), config fallback (resolved: path unchanged, so no code), shortcut cleanup (Task 1 step 4).
- Names match across tasks: `QrOptions`, `Ec`, `Format`, `encode`, `png_data_url`, `save`; Tauri commands `qr_preview`/`qr_save`/`pick_save_qr`; page id `page-qr`.
- Clarification vs. the spec: the output side length is `modules+2·margin` times an integer scale, so it can be slightly under the requested size (never over).
