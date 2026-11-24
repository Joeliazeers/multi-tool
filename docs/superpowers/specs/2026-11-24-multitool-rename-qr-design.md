# MultiTool rename + QR code generator (short design)

Status: approved by the user on 2026-11-24.

## Intent
The app is growing past video/subtitle work, so it gets a general name, **MultiTool**. It also gains a QR code generator: paste a link, see the QR code, save it as an image.

## Goals
- Every user-visible "vidsub" becomes "MultiTool": window title, sidebar brand, product name, shortcut/menu entry, README, CLI name and help text.
- Binaries become `multitool` (CLI) and `multitool-gui` (GUI).
- New "QR Code" page in the GUI: live preview, size, error-correction level, foreground/background color, Save as (PNG, JPG, SVG, WebP, BMP), copy image.
- CLI: `multitool qr "URL" -o qr.png` (format chosen from the file extension).

## Non-goals / anti-goals
- No renaming of the config folder `~/.config/vidsub/` or the `VIDSUB_*` env vars. Existing users must not lose their settings.
- No new system dependencies and no network use for QR generation.
- No change to download/transcribe/translate behavior.

## Constraints
- Existing stack: Rust (edition 2024), Tauri 2 GUI with plain HTML/JS in `gui/ui`, UI text in Indonesian.
- The CSP already allows `img-src data:`, so the preview can be a data URL.

## Approaches considered
1. **Pure-Rust (`qrcode` + `image`) in the shared lib, exposed to GUI and CLI.** Chosen: no system deps, offline, one implementation for both front ends, easy to unit-test.
2. JS QR library in the frontend. Rejected: needs vendoring, duplicates logic, the CLI can't use it, and saving still needs Rust.
3. Shell out to `qrencode`. Rejected: new system dependency, limited formats.

## Decisions
- New module `src/qr.rs`: `render(text, opts) -> image/svg bytes`, `save(path, ...)`; format from extension. Options: size px, EC level (L/M/Q/H), fg/bg hex colors, quiet-zone margin.
- Tauri commands: `qr_preview` (returns a PNG data URL), `qr_save` (writes the chosen file), `pick_save_qr` (save dialog with format filters). Copy-to-clipboard is done in the webview with the preview PNG if the API allows it, otherwise dropped and reported.
- Rename touches: `Cargo.toml` (package, bin names), `gui/Cargo.toml`, `gui/tauri.conf.json` (productName, title, identifier), `gui/ui/*`, `packaging/*`, README, CLI `#[command(name)]`, docs strings. Rust crate name stays importable as `multitool`.
- Config fallback: if `~/.config/multitool/config.env` does not exist, keep reading `~/.config/vidsub/config.env`. New saves keep going to the old path, so nothing moves.
- The installed shortcut script removes the old `vidsub.desktop` entries when installing the new one.

## Left to the builder
Exact layout and styling of the QR page (follow the existing card/segmented-control style), icon choice, default size (suggest 512 px), error handling copy.
