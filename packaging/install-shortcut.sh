#!/usr/bin/env bash
# Pasang shortcut MultiTool (GUI) di menu aplikasi + Desktop. Linux/GNOME.
# Jalankan setelah: cargo install --path gui   (dan opsional: cargo install --path .  untuk CLI)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
gui="${VIDSUB_GUI_BIN:-$(command -v multitool-gui || echo "$HOME/.cargo/bin/multitool-gui")}"
[ -x "$gui" ] || { echo "multitool-gui tidak ditemukan ($gui). Jalankan: cargo install --path gui"; exit 1; }

app_dir="$HOME/.local/share/applications"
data_dir="$HOME/.local/share/multitool"
desk="$(xdg-user-dir DESKTOP 2>/dev/null || echo "$HOME/Desktop")"
mkdir -p "$app_dir" "$data_dir"

# Bersihkan shortcut & data versi lama (vidsub)
rm -f "$app_dir/vidsub.desktop" "$desk/vidsub.desktop"
rm -rf "$HOME/.local/share/vidsub"

cp "$here/multitool.svg" "$data_dir/multitool.svg"
icon="$data_dir/multitool.svg"

cat > "$app_dir/multitool.desktop" <<DESK
[Desktop Entry]
Type=Application
Name=MultiTool
GenericName=Video Downloader, Subtitle & QR Code
Comment=Download video, transcribe ke SRT, translate subtitle dengan LLM, dan buat QR code
Exec=$gui
Icon=$icon
Terminal=false
StartupWMClass=multitool-gui
Categories=AudioVideo;Video;Utility;
Keywords=youtube;download;subtitle;srt;transcribe;translate;whisper;qr;qrcode;
DESK
chmod +x "$app_dir/multitool.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$app_dir" 2>/dev/null || true

if [ -d "$desk" ]; then
  cp "$app_dir/multitool.desktop" "$desk/multitool.desktop"
  chmod +x "$desk/multitool.desktop"
  gio set "$desk/multitool.desktop" metadata::trusted true 2>/dev/null || true
fi
echo "✔ Shortcut terpasang (GUI: $gui)"
echo "  - Menu aplikasi : $app_dir/multitool.desktop (cari 'MultiTool')"
[ -d "$desk" ] && echo "  - Desktop       : $desk/multitool.desktop"
