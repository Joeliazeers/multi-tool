"use strict";
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

/* ------------------------------------------------------------ helpers */
const $ = (s) => document.querySelector(s);
const $$ = (s) => Array.from(document.querySelectorAll(s));

function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k === "class") el.className = v;
    else if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else el.setAttribute(k, v === true ? "" : v);
  }
  for (const c of children.flat()) {
    if (c == null || c === false) continue;
    el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return el;
}

function icon(name, cls = "icon") {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("class", cls);
  svg.setAttribute("aria-hidden", "true");
  const use = document.createElementNS(ns, "use");
  use.setAttribute("href", `#i-${name}`);
  svg.append(use);
  return svg;
}

function fmtDur(sec) {
  if (sec == null || !isFinite(sec)) return "";
  sec = Math.round(sec);
  const hh = Math.floor(sec / 3600), mm = Math.floor((sec % 3600) / 60), ss = sec % 60;
  return hh ? `${hh}:${String(mm).padStart(2, "0")}:${String(ss).padStart(2, "0")}` : `${mm}:${String(ss).padStart(2, "0")}`;
}
function fmtSize(b) {
  if (!b) return "";
  const u = ["B", "KB", "MB", "GB"]; let i = 0;
  while (b >= 1024 && i < u.length - 1) { b /= 1024; i++; }
  return `${b.toFixed(i ? 1 : 0)} ${u[i]}`;
}
// Path Linux ("/") maupun Windows (C:\dir\file atau C:/dir/file).
const basename = (p) => p.split(/[\\/]/).filter(Boolean).pop() || p;
function dirname(p) {
  const i = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
  if (i < 0) return p;
  if (i === 0) return p[0];
  const head = p.slice(0, i);
  return /^[A-Za-z]:$/.test(head) ? p.slice(0, i + 1) : head; // C:\file -> C:\
}
const joinPath = (d, f) => `${d.replace(/[\\/]$/, "")}${d.includes("\\") && !d.includes("/") ? "\\" : "/"}${f}`;
/** Tampilkan path panjang terpotong di depan ("…/folder/file.srt"). */
function setPath(el, text) {
  el.replaceChildren(h("bdi", {}, text));
  el.title = text;
}
const isSrt = (p) => /\.srt$/i.test(p);
const isMedia = (p) => /\.(mp4|mkv|webm|mov|avi|flv|ts|m4v|mp3|m4a|wav|ogg|opus|flac|aac)$/i.test(p);

let toastTimer;
function toast(msg) {
  const t = $("#toast");
  t.textContent = msg;
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.hidden = true), 3200);
}

/* ------------------------------------------------------------ state */
const S = {
  settings: null,
  langs: [],
  running: false,
  dl: { dir: "", probedKey: "", info: null },
  tr: { file: null, out: null }, // out: null | {dir} | {file}
  tl: { file: null, info: null, out: null },
};

/* ------------------------------------------------------------ theme */
function applyTheme() {
  const pref = localStorage.getItem("vidsub-theme") || "system";
  const light = pref === "light" || (pref === "system" && matchMedia("(prefers-color-scheme: light)").matches);
  document.documentElement.dataset.theme = light ? "light" : "dark";
  $("#set-theme").value = pref;
}
matchMedia("(prefers-color-scheme: light)").addEventListener("change", applyTheme);

/* ------------------------------------------------------------ navigation */
function go(page) {
  $$(".page").forEach((p) => (p.hidden = p.id !== `page-${page}`));
  $$(".nav-item").forEach((b) => {
    const on = b.dataset.page === page;
    b.classList.toggle("active", on);
    if (on) b.setAttribute("aria-current", "page"); else b.removeAttribute("aria-current");
  });
  $("#scroll").scrollTop = 0;
  if (page === "settings") checkDeps();
}
$$("[data-page]").forEach((b) => b.addEventListener("click", () => go(b.dataset.page)));
document.addEventListener("keydown", (e) => {
  if (e.ctrlKey && !e.shiftKey && !e.altKey && ["1", "2", "3", "4", "5"].includes(e.key)) {
    e.preventDefault();
    go(["download", "transcribe", "translate", "qr", "settings"][Number(e.key) - 1]);
  }
});

/* ------------------------------------------------------------ languages */
function fillLangSelect(sel, def) {
  const auto = sel.dataset.auto === "1";
  sel.replaceChildren();
  if (auto) sel.append(h("option", { value: "auto" }, sel.id === "tl-from" ? "Auto (dari nama file / dideteksi LLM)" : "Auto — whisper menebak bahasa"));
  for (const l of S.langs) sel.append(h("option", { value: l.code }, `${l.name} · ${l.local}`));
  if (sel.id === "tr-lang" || sel.id === "tl-to") sel.append(h("option", { value: "__custom" }, "Lainnya…"));
  sel.value = def || (auto ? "auto" : "id");
}
function langName(code) {
  const l = S.langs.find((x) => x.code === code);
  return l ? l.name : code;
}
function ensureLangOption(sel, lang) {
  if (![...sel.options].some((o) => o.value === lang.code)) {
    sel.insertBefore(h("option", { value: lang.code }, `${lang.name} (${lang.code})`), sel.options[sel.dataset.auto === "1" ? 1 : 0] || null);
  }
  sel.value = lang.code;
}

/* ------------------------------------------------------------ pekerjaan (kartu aktivitas) */
let jobSeq = Date.now() % 1e6;            // ID unik per sesi
const jobsById = new Map();               // job id backend -> JobCard

/** Satu kartu = satu rangkaian pekerjaan (mis. download -> transcribe -> translate). */
class JobCard {
  static all = [];
  constructor(title, steps) {
    this.el = $("#job-tpl").content.firstElementChild.cloneNode(true);
    this.q = (c) => this.el.querySelector(c);
    this.q(".job-title").textContent = title;
    this.q(".act-steps").replaceChildren(...steps.map((s) => h("span", { class: "step" }, s)));
    this.q(".act-cancel").addEventListener("click", () => this.cancel());
    this.q(".act-pause").addEventListener("click", () => this.pause());
    this.q(".act-close").addEventListener("click", () => this.close());
    this.running = true; this.cancelled = false; this.currentId = null; this.waiter = null;
    this.warns = 0; this.frac = null; this.raf = 0; this.pending = null; this.t0 = Date.now();
    this.setIcon("loader", "running");
    this.setBar(null);
    this.q(".act-stage").textContent = "Menyiapkan…";
    $("#jobs").prepend(this.el);
    $("#activity").hidden = false;
    JobCard.all.push(this);
    // buang kartu selesai yang lama supaya panel tidak memanjang
    JobCard.all.filter((c) => !c.running).slice(0, -4).forEach((c) => c.close());
    this.timer = setInterval(() => this.tick(), 1000);
    this.tick();
    updateStartButtons();
  }
  /** ID baru untuk satu panggilan backend; event dengan ID ini masuk ke kartu ini. */
  newId() {
    const id = ++jobSeq;
    jobsById.set(id, this);
    this.currentId = id;
    return id;
  }
  step(i) {
    this.el.querySelectorAll(".step").forEach((el, k) => {
      el.classList.toggle("done", k < i);
      el.classList.toggle("now", k === i);
    });
  }
  setIcon(name, cls) {
    const el = this.q(".act-icon");
    el.className = `act-icon ${cls}`;
    el.replaceChildren(icon(name));
  }
  setBar(frac, state) {
    this.frac = frac;
    const bar = this.q(".bar");
    bar.className = `bar ${state || (frac == null ? "indeterminate" : "")}`;
    const pct = frac == null ? null : Math.round(frac * 100);
    bar.firstElementChild.style.width = frac == null || state ? "" : `${pct}%`;
    if (pct == null) bar.removeAttribute("aria-valuenow"); else bar.setAttribute("aria-valuenow", pct);
    this.tick();
  }
  tick() {
    if (!this.running) return;
    const elapsed = fmtDur((Date.now() - this.t0) / 1000);
    this.q(".act-pct").textContent = this.frac == null ? elapsed : `${Math.round(this.frac * 100)}% · ${elapsed}`;
  }
  stage(text, detail = "") {
    this.q(".act-stage").textContent = text;
    this.q(".act-detail").textContent = detail;
  }
  log(line, warn) {
    const pre = this.q("pre");
    pre.textContent += (warn ? "! " : "") + line + "\n";
    if (pre.textContent.length > 60000) pre.textContent = pre.textContent.slice(-50000);
    pre.scrollTop = pre.scrollHeight;
    if (warn) this.q(".act-warn-count").textContent = `· ${++this.warns} peringatan`;
  }
  onEvent(e) {
    if (e.kind === "stage") { this.stage(e.text); this.setBar(null); this.log(`== ${e.text}`); }
    else if (e.kind === "progress") {
      this.pending = e; // batasi update DOM 1x per frame
      if (!this.raf) this.raf = requestAnimationFrame(() => {
        this.raf = 0;
        this.q(".act-detail").textContent = this.pending.detail;
        this.setBar(this.pending.fraction);
      });
    } else if (e.kind === "segments") {
      this.segParts = e.parts;
      if (!this.segRaf) this.segRaf = requestAnimationFrame(() => { this.segRaf = 0; this.renderSegs(this.segParts); });
    } else if (e.kind === "log") this.log(e.text);
    else if (e.kind === "warn") this.log(e.text, true);
  }
  renderSegs(parts) {
    const bar = this.q(".segbar");
    bar.hidden = false;
    if (bar.children.length !== parts.length) {
      bar.replaceChildren(...parts.map(() => h("div", { class: "seg" }, h("i"))));
    }
    parts.forEach((p, k) => {
      const el = bar.children[k];
      const w = Math.max(p.to - p.from, 1e-9);
      el.style.left = `${p.from * 100}%`;
      el.style.width = `${(p.to - p.from) * 100}%`;
      el.firstElementChild.style.width = `${Math.min(1, (p.pos - p.from) / w) * 100}%`;
    });
  }
  setPausable(on) { this.q(".act-pause").hidden = !on; }
  async pause() {
    this.q(".act-pause").disabled = true;
    this.stage("Menjeda…");
    if (this.currentId) await invoke("pause", { jobId: this.currentId });
  }
  waiting(text, detail = "Download tetap bisa berjalan bersamaan.") {
    this.stage(text, detail);
    this.setIcon("clock", "waiting");
    this.setBar(null);
  }
  resumed() { this.setIcon("loader", "running"); }
  async cancel() {
    this.cancelled = true;
    this.q(".act-cancel").disabled = true;
    this.stage("Membatalkan…");
    if (this.waiter) this.waiter.reject(new Error("Dibatalkan"));
    else if (this.currentId) await invoke("cancel", { jobId: this.currentId });
  }
  finish(ok, title, detail) {
    this.running = false;
    clearInterval(this.timer);
    this.q(".act-cancel").hidden = true;
    this.setPausable(false);
    this.q(".segbar").hidden = true;
    this.q(".act-close").hidden = false;
    this.stage(title, detail || "");
    this.q(".act-pct").textContent = fmtDur((Date.now() - this.t0) / 1000);
    if (ok === true) { this.setIcon("check-circle", "ok"); this.setBar(1, "ok"); this.step(99); }
    else if (ok === false) { this.setIcon("alert", "fail"); this.setBar(1, "fail"); }
    else { this.setIcon("x", ""); this.setBar(0, "stopped"); }
    updateStartButtons();
  }
  result(...rows) {
    const box = this.q(".act-result");
    box.append(...rows);
    box.hidden = false;
  }
  close() {
    if (this.running) return;
    this.el.remove();
    JobCard.all = JobCard.all.filter((c) => c !== this);
    for (const [id, c] of jobsById) if (c === this) jobsById.delete(id);
    if (!JobCard.all.length) $("#activity").hidden = true;
  }
}

/* Antrean transcribe/translate: satu per satu (berbagi CPU whisper & LLM). Download tidak ikut antre. */
const proc = { busy: false, waiters: [] };
function processBusy() { return proc.busy; }
async function withProcess(card, label, fn) {
  if (card.cancelled) throw new Error("Dibatalkan");
  if (proc.busy) {
    card.waiting(`Menunggu giliran ${label}… (${proc.waiters.length + 1} dalam antrean)`);
    await new Promise((resolve, reject) => {
      const w = { resolve, reject: (e) => { proc.waiters = proc.waiters.filter((x) => x !== w); card.waiter = null; reject(e); } };
      card.waiter = w;
      proc.waiters.push(w);
    });
    card.waiter = null;
    card.resumed();
  } else proc.busy = true;
  updateStartButtons();
  try {
    return await fn(card.newId());
  } finally {
    const next = proc.waiters.shift();
    if (next) next.resolve(); else proc.busy = false;
    updateStartButtons();
  }
}

/* Antrean download: maksimal `download_parallel` sekaligus (pengaturan); sisanya menunggu giliran. */
const dlq = { running: 0, waiters: [] };
async function withDownloadSlot(card, fn) {
  const max = S.settings?.cfg.download_parallel || 3;
  if (card.cancelled) throw new Error("Dibatalkan");
  if (dlq.running >= max) {
    card.waiting(`Menunggu giliran download… (${dlq.waiters.length + 1} dalam antrean)`, "Dimulai otomatis saat ada download yang selesai.");
    await new Promise((resolve, reject) => {
      const w = { resolve, reject: (e) => { dlq.waiters = dlq.waiters.filter((x) => x !== w); card.waiter = null; reject(e); } };
      card.waiter = w;
      dlq.waiters.push(w);
    });
    card.waiter = null;
    card.resumed();
  } else dlq.running++;
  try {
    return await fn();
  } finally {
    const next = dlq.waiters.shift();
    if (next) next.resolve(); else dlq.running--;
  }
}

function resultRow(path, { openFile = true, extra = [] } = {}) {
  return h("div", { class: "result-row" },
    h("span", { class: "result-path" }, path),
    openFile ? h("button", { class: "btn ghost sm", type: "button", onclick: () => invoke("open_path", { path }).catch(toast) }, icon("file-text"), "Buka") : null,
    h("button", { class: "btn ghost sm", type: "button", onclick: () => invoke("open_path", { path, reveal: true }).catch(toast) }, icon("folder-open"), "Folder"),
    ...extra,
  );
}
function nextBtn(label, onclick) {
  return h("button", { class: "btn ghost sm", type: "button", onclick }, label, icon("arrow-right"));
}

/** Jalankan rangkaian pekerjaan dalam kartu baru. */
async function runPipeline(title, steps, fn) {
  const card = new JobCard(title, steps);
  try {
    await fn(card);
  } catch (err) {
    const msg = String(err);
    if (/Dijeda/.test(msg) && !card.cancelled) {
      card.finish(null, "Dijeda", "Bagian yang sudah diunduh disimpan.");
      if (card.resumeFn) card.result(h("button", { class: "btn primary sm", type: "button", onclick: () => { card.close(); card.resumeFn(); } }, icon("play"), "Lanjutkan"));
    } else if (/Dibatalkan/.test(msg) || card.cancelled) card.finish(null, "Dibatalkan", "Proses dihentikan.");
    else {
      card.finish(false, "Gagal", "");
      card.result(h("div", { class: "act-error" }, msg));
      card.log(msg, true);
    }
  }
}

function updateStartButtons() {
  const busy = processBusy();
  $("#tr-start").disabled = !S.tr.file;
  $("#tl-start").disabled = !S.tl.file;
  $("#tr-start-label").textContent = busy ? "Antrekan transcribe" : "Mulai transcribe";
  $("#tl-start-label").textContent = busy ? "Antrekan translate" : "Translate";
}

/* ------------------------------------------------------------ DOWNLOAD */
function dlArgs() {
  return {
    url: $("#dl-url").value.trim(),
    referer: $("#dl-referer").value.trim() || null,
    quality: document.querySelector('input[name="quality"]:checked').value,
    cookies_from_browser: $("#dl-cookies").value || null,
    format: $("#dl-format").value.trim() || null,
    output_dir: S.dl.dir || null,
    playlist: $("#dl-playlist").checked,
    headers: [],
    segments: parseInt($("#dl-segments").value, 10) || null,
    limit: $("#dl-limit").value.trim() === "" ? null : Math.max(0, Math.round(parseFloat($("#dl-limit").value) * 1048576)) || 0,
  };
}
function validUrl(u) {
  try { const x = new URL(u); return x.protocol === "http:" || x.protocol === "https:"; } catch { return false; }
}

async function probe(force) {
  const a = dlArgs();
  const err = $("#dl-probe-error");
  if (!a.url) { $("#dl-preview").hidden = true; err.hidden = true; return; }
  if (!validUrl(a.url)) { err.textContent = "Link harus diawali http:// atau https://"; err.hidden = false; return; }
  const key = `${a.url}|${a.referer}|${a.cookies_from_browser}|${a.playlist}`;
  if (!force && key === S.dl.probedKey) return;
  S.dl.probedKey = key;
  err.hidden = true;
  const pv = $("#dl-preview");
  pv.hidden = false;
  $("#dl-thumb").className = "thumb skeleton";
  $("#dl-thumb").replaceChildren();
  $("#dl-title").replaceChildren(h("div", { class: "skeleton-line" }));
  $("#dl-meta").replaceChildren(h("div", { class: "skeleton-line short" }));
  const btn = $("#dl-check");
  btn.classList.add("loading"); btn.disabled = true;
  try {
    const info = await invoke("probe", { args: a });
    if (S.dl.probedKey !== key) return; // sudah ganti link
    S.dl.info = info;
    applyKind(info.kind);
    $("#dl-thumb").className = "thumb";
    if (info.kind === "file") {
      $("#dl-thumb").classList.add("file-thumb");
      $("#dl-thumb").append(icon("file-text"));
      $("#dl-title").textContent = info.filename || info.title;
      $("#dl-meta").replaceChildren(
        h("span", {}, info.size != null ? fmtSize(info.size) : "ukuran tidak diketahui"),
        h("span", {}, info.ranges ? "Multi-bagian: ya (bisa dijeda & dilanjutkan)" : "Multi-bagian: tidak (server hanya mengizinkan satu koneksi)"),
      );
      applyHeights([]);
      return;
    }
    if (info.thumbnail && /^https?:\/\//.test(info.thumbnail)) {
      $("#dl-thumb").append(h("img", { src: info.thumbnail, alt: "", referrerpolicy: "no-referrer", loading: "lazy" }));
    }
    $("#dl-title").textContent = info.title;
    const meta = [];
    if (info.uploader) meta.push(h("span", {}, info.uploader));
    if (info.duration) meta.push(h("span", {}, fmtDur(info.duration)));
    if (info.extractor) meta.push(h("span", {}, info.extractor));
    if (info.is_playlist) meta.push(h("span", {}, `Playlist · ${info.entries} video`));
    if (info.heights.length) meta.push(h("span", {}, `Maks ${info.heights[0]}p`));
    $("#dl-meta").replaceChildren(...meta);
    applyHeights(info.heights);
  } catch (e) {
    if (S.dl.probedKey !== key) return;
    pv.hidden = true;
    S.dl.info = null;
    applyKind("video");
    applyHeights([]);
    err.textContent = `Info video tidak bisa diambil: ${e}. Download tetap bisa dicoba.`;
    err.hidden = false;
  } finally {
    btn.classList.remove("loading"); btn.disabled = false;
  }
}
/** File langsung: sembunyikan opsi video (kualitas, yt-dlp), tampilkan koneksi & batas kecepatan. */
function applyKind(kind) {
  const file = kind === "file";
  $("#dl-quality-field").hidden = file;
  $("#dl-ytdlp-opts").hidden = file;
  $("#dl-file-opts").hidden = !file;
}
function applyHeights(heights) {
  const max = heights.length ? heights[0] : null;
  for (const r of $$('input[name="quality"]')) {
    const h = parseInt(r.value, 10);
    r.disabled = !!(max && h && h > max && !heights.some((x) => x >= h));
    r.parentElement.title = r.disabled ? `Tidak tersedia (maks ${max}p)` : "";
    if (r.disabled && r.checked) $('input[name="quality"][value="best"]').checked = true;
  }
  $("#dl-quality-help").textContent = heights.length
    ? `Tersedia: ${heights.filter((x) => x >= 144).slice(0, 8).map((x) => x + "p").join(", ")}.`
    : "Resolusi maksimal; kalau tidak ada, diambil yang terdekat di bawahnya.";
}

let probeTimer;
$("#dl-url").addEventListener("input", () => { clearTimeout(probeTimer); probeTimer = setTimeout(() => probe(false), 700); });
$("#dl-url").addEventListener("keydown", (e) => { if (e.key === "Enter") { clearTimeout(probeTimer); probe(true); } });
$("#dl-referer").addEventListener("change", () => S.dl.info || $("#dl-probe-error").hidden === false ? probe(false) : null);
$("#dl-check").addEventListener("click", () => probe(true));
$("#dl-dir-pick").addEventListener("click", async () => {
  const p = await invoke("pick_folder", { start: S.dl.dir });
  if (p) { S.dl.dir = p; setPath($("#dl-dir"), p); }
});
$("#dl-auto-tr").addEventListener("change", () => {
  const on = $("#dl-auto-tr").checked;
  $("#dl-auto-tl").disabled = !on;
  $("#dl-auto-tl-lang").disabled = !on || !$("#dl-auto-tl").checked;
});
$("#dl-auto-tl").addEventListener("change", () => ($("#dl-auto-tl-lang").disabled = !$("#dl-auto-tl").checked));

$("#dl-start").addEventListener("click", () => {
  const a = dlArgs();
  if (!validUrl(a.url)) {
    $("#dl-probe-error").textContent = a.url ? "Link harus diawali http:// atau https://" : "Masukkan link dulu.";
    $("#dl-probe-error").hidden = false;
    $("#dl-url").focus();
    return;
  }
  startDownload(a, {
    autoTr: $("#dl-auto-tr").checked,
    autoTl: $("#dl-auto-tr").checked && $("#dl-auto-tl").checked,
    tlCode: $("#dl-auto-tl-lang").value,
    info: S.dl.info && S.dl.probedKey.startsWith(`${a.url}|`) ? S.dl.info : null,
  });
  toast("Download dimulai");
});

function startDownload(a, o) {
  const { autoTr, autoTl, tlCode, info } = o;
  const steps = ["Download", ...(autoTr ? ["Transcribe"] : []), ...(autoTl ? ["Translate"] : [])];
  const title = info?.title ? `Download · ${info.title}` : `Download · ${a.url}`;
  const canPause = info?.kind === "file" && info.ranges;
  runPipeline(title, steps, async (card) => {
    card.resumeFn = () => { startDownload(a, o); toast("Melanjutkan download"); };
    card.step(0);
    const files = await withDownloadSlot(card, () => {
      card.resumed();
      card.setPausable(canPause);
      return invoke("download", { jobId: card.newId(), args: a });
    });
    card.setPausable(false);
    const media = files.filter(isMedia);
    const rows = files.map((f) => resultRow(f, { extra: isMedia(f) ? [nextBtn("Transcribe", () => { setTrFile(f); go("transcribe"); })] : [] }));
    if (!autoTr || !media.length) {
      if (autoTr) card.log("Bukan file video/audio: transcribe dilewati.");
      card.finish(true, files.length > 1 ? `${files.length} file terdownload` : "Download selesai", basename(files[0]));
      return card.result(...rows);
    }
    const outs = [...rows];
    for (const f of media) {
      card.step(1);
      const tr = await withProcess(card, "transcribe", (jobId) => invoke("transcribe", { jobId, input: f, lang: "auto", useLlm: $("#tr-llm").checked, out: null }));
      outs.push(resultRow(tr.srt));
      if (autoTl) {
        card.step(2);
        const tl = await withProcess(card, "translate", (jobId) => invoke("translate_srt", { jobId, input: tr.srt, to: langName(tlCode), from: tr.lang.code, out: null }));
        outs.push(resultRow(tl));
      }
    }
    card.finish(true, "Semua selesai", `${media.length} video diproses`);
    card.result(...outs);
  });
}

/* ------------------------------------------------------------ TRANSCRIBE */
async function setTrFile(path) {
  try {
    const m = await invoke("media_info", { path });
    S.tr.file = m;
    S.tr.out = null;
    $("#tr-drop").hidden = true;
    $("#tr-file").hidden = false;
    $("#tr-file-name").textContent = m.name;
    $("#tr-file-meta").textContent = [m.dir, fmtSize(m.size), m.duration ? `durasi ${fmtDur(m.duration)}` : "bukan media yang dikenali ffmpeg"].filter(Boolean).join(" · ");
    updateTrOut();
    updateStartButtons();
  } catch (e) { toast(String(e)); }
}
function trLangCode() {
  const v = $("#tr-lang").value;
  return v === "__custom" ? $("#tr-lang-custom").value.trim() : v;
}
function updateTrOut() {
  const f = S.tr.file;
  $("#tr-out-reset").hidden = !S.tr.out;
  if (!f) { $("#tr-out").textContent = "Pilih video dulu"; return; }
  const code = trLangCode();
  const name = `${f.stem}.${code === "auto" || !code ? "<bahasa>" : code}.srt`;
  setPath($("#tr-out"), S.tr.out?.file || joinPath(S.tr.out?.dir || f.dir, name));
}
async function pickTrFile() {
  const p = await invoke("pick_file", { kind: "video", start: S.tr.file?.dir || S.dl.dir });
  if (p) setTrFile(p);
}
$("#tr-drop").addEventListener("click", pickTrFile);
$("#tr-file-change").addEventListener("click", pickTrFile);
$("#tr-lang").addEventListener("change", () => {
  const v = $("#tr-lang").value;
  $("#tr-lang-custom").hidden = v !== "__custom";
  $("#tr-confirm-wrap").hidden = v !== "auto";
  if (v === "__custom") $("#tr-lang-custom").focus();
  updateTrOut();
});
$("#tr-lang-custom").addEventListener("input", updateTrOut);
$("#tr-out-dir").addEventListener("click", async () => {
  const p = await invoke("pick_folder", { start: S.tr.out?.dir || S.tr.file?.dir });
  if (p) { S.tr.out = { dir: p }; updateTrOut(); }
});
$("#tr-out-file").addEventListener("click", async () => {
  if (!S.tr.file) return toast("Pilih video dulu.");
  const code = trLangCode();
  const suggested = joinPath(S.tr.out?.dir || S.tr.file.dir, `${S.tr.file.stem}.${code === "auto" ? "srt" : code + ".srt"}`);
  const p = await invoke("save_srt", { suggested });
  if (p) { S.tr.out = { file: p }; updateTrOut(); }
});
$("#tr-out-reset").addEventListener("click", () => { S.tr.out = null; updateTrOut(); });

function askLanguage(det, fileName) {
  return new Promise((resolve) => {
    const dlg = $("#lang-dialog");
    $("#lang-dialog-file").textContent = fileName;
    $("#lang-dialog-name").textContent = `${det.lang.name} (${det.lang.code})`;
    const conf = det.confidence != null ? `Keyakinan whisper ${Math.round(det.confidence * 100)}%` : "";
    const others = det.candidates.slice(1, 4).map(([l, p]) => `${l.name} ${Math.max(1, Math.round(p * 100))}%`);
    $("#lang-dialog-reason").textContent = [conf, others.length ? `lainnya: ${others.join(", ")}` : ""].filter(Boolean).join(" · ");
    $("#lang-dialog-reason").classList.toggle("low", det.confidence != null && det.confidence < 0.6);
    const sel = $("#lang-dialog-select");
    fillLangSelect(sel);
    ensureLangOption(sel, det.lang);
    dlg.returnValue = "";
    dlg.addEventListener("close", () => {
      if (dlg.returnValue !== "ok") return resolve(null);
      resolve({ code: sel.value });
    }, { once: true });
    dlg.showModal();
  });
}

$("#tr-start").addEventListener("click", () => {
  const f = S.tr.file;
  if (!f) return;
  let code = trLangCode();
  if (!code) { toast("Isi kode/nama bahasa."); return $("#tr-lang-custom").focus(); }
  const auto = code === "auto";
  const confirm = auto && $("#tr-confirm").checked;
  const useLlm = $("#tr-llm").checked;
  const autoTl = $("#tr-auto-tl").checked;
  const tlCode = $("#tr-auto-tl-lang").value;
  const out = S.tr.out?.file || S.tr.out?.dir || null;
  const outDir = S.tr.out?.dir || null;
  const steps = [...(confirm ? ["Deteksi bahasa"] : []), "Transcribe", ...(autoTl ? ["Translate"] : [])];
  runPipeline(`Transcribe · ${f.name}`, steps, async (card) => {
    let i = 0;
    if (confirm) {
      card.step(i++);
      const det = await withProcess(card, "deteksi bahasa", (jobId) => invoke("detect", { jobId, input: f.path }));
      card.stage("Menunggu konfirmasi bahasa…");
      const pick = await askLanguage(det, f.name);
      if (!pick) throw new Error("Dibatalkan");
      code = pick.code;
    }
    card.step(i++);
    const tr = await withProcess(card, "transcribe", (jobId) => invoke("transcribe", { jobId, input: f.path, lang: code, useLlm, out }));
    const rows = [resultRow(tr.srt, { extra: [nextBtn("Translate", () => { setTlFile(tr.srt, tr.lang.code); go("translate"); })] })];
    if (autoTl) {
      card.step(i++);
      const tl = await withProcess(card, "translate", (jobId) => invoke("translate_srt", { jobId, input: tr.srt, to: langName(tlCode), from: tr.lang.code, out: outDir }));
      rows.push(resultRow(tl));
    }
    card.finish(true, "Transcribe selesai", `${tr.lines} baris · ${tr.lang.name}`);
    card.result(...rows);
  });
});

/* ------------------------------------------------------------ TRANSLATE */
async function setTlFile(path, fromCode) {
  try {
    const info = await invoke("srt_info", { path });
    S.tl.file = { path, dir: dirname(path), name: basename(path), stem: basename(path).replace(/(\.[a-z]{2,3})?\.srt$/i, "") };
    S.tl.info = info;
    S.tl.out = null;
    $("#tl-drop").hidden = true;
    $("#tl-file").hidden = false;
    $("#tl-file-name").textContent = S.tl.file.name;
    $("#tl-file-meta").textContent = [S.tl.file.dir, `${info.lines} baris`, `durasi ${fmtDur(info.duration)}`, info.lang ? `bahasa: ${info.lang.name}` : null].filter(Boolean).join(" · ");
    $("#tl-preview").replaceChildren(...info.preview.map((l) => h("li", {}, l)));
    const code = fromCode || info.lang?.code;
    if (code) ensureLangOption($("#tl-from"), { code, name: langName(code) });
    else $("#tl-from").value = "auto";
    updateTlOut();
    updateStartButtons();
  } catch (e) { toast(String(e)); }
}
function tlTarget() {
  const v = $("#tl-to").value;
  const style = $("#tl-style").value.trim();
  const base = v === "__custom" ? $("#tl-to-custom").value.trim() : langName(v);
  const code = v === "__custom" ? null : v;
  return { name: base && style ? `${base} (${style})` : base, code };
}
function updateTlOut() {
  const f = S.tl.file;
  $("#tl-out-reset").hidden = !S.tl.out;
  if (!f) { $("#tl-out").textContent = "Pilih file SRT dulu"; return; }
  const t = tlTarget();
  const code = t.code || "<bahasa>";
  setPath($("#tl-out"), S.tl.out?.file || joinPath(S.tl.out?.dir || f.dir, `${f.stem}.${code}.srt`));
}
async function pickTlFile() {
  const p = await invoke("pick_file", { kind: "srt", start: S.tl.file?.dir || S.tr.file?.dir || S.dl.dir });
  if (p) setTlFile(p);
}
$("#tl-drop").addEventListener("click", pickTlFile);
$("#tl-file-change").addEventListener("click", pickTlFile);
$("#tl-to").addEventListener("change", () => {
  $("#tl-to-custom").hidden = $("#tl-to").value !== "__custom";
  if (!$("#tl-to-custom").hidden) $("#tl-to-custom").focus();
  updateTlOut();
});
$("#tl-to-custom").addEventListener("input", updateTlOut);
$("#tl-style").addEventListener("input", () => {
  const v = $("#tl-style").value.trim();
  $$(".chip").forEach((c) => c.classList.toggle("active", c.dataset.style === v));
});
$$(".chip").forEach((c) => c.addEventListener("click", () => {
  $("#tl-style").value = c.dataset.style;
  $("#tl-style").dispatchEvent(new Event("input"));
}));
$("#tl-out-dir").addEventListener("click", async () => {
  const p = await invoke("pick_folder", { start: S.tl.out?.dir || S.tl.file?.dir });
  if (p) { S.tl.out = { dir: p }; updateTlOut(); }
});
$("#tl-out-file").addEventListener("click", async () => {
  if (!S.tl.file) return toast("Pilih file SRT dulu.");
  const t = tlTarget();
  const p = await invoke("save_srt", { suggested: joinPath(S.tl.out?.dir || S.tl.file.dir, `${S.tl.file.stem}.${t.code || "terjemahan"}.srt`) });
  if (p) { S.tl.out = { file: p }; updateTlOut(); }
});
$("#tl-out-reset").addEventListener("click", () => { S.tl.out = null; updateTlOut(); });

$("#tl-start").addEventListener("click", () => {
  const f = S.tl.file;
  if (!f) return;
  const t = tlTarget();
  if (!t.name) { toast("Isi bahasa tujuan."); return $("#tl-to-custom").focus(); }
  const from = $("#tl-from").value === "auto" ? null : $("#tl-from").value;
  const out = S.tl.out?.file || S.tl.out?.dir || null;
  const lines = S.tl.info.lines;
  runPipeline(`Translate · ${f.name}`, ["Translate"], async (card) => {
    card.step(0);
    const p = await withProcess(card, "translate", (jobId) => invoke("translate_srt", { jobId, input: f.path, to: t.name, from, out }));
    card.finish(true, "Terjemahan selesai", `${lines} baris → ${t.name}`);
    card.result(resultRow(p));
  });
});

/* ------------------------------------------------------------ drag & drop */
listen("file-drop", ({ payload: paths }) => {
  const p = paths?.[0];
  if (!p) return;
  const page = $$(".page").find((x) => !x.hidden)?.id;
  if (isSrt(p)) { setTlFile(p); if (page !== "page-translate") go("translate"); }
  else if (isMedia(p) || page === "page-transcribe") { setTrFile(p); if (page !== "page-transcribe") go("transcribe"); }
  else toast("Seret file video/audio atau .srt");
});
for (const id of ["#tr-drop", "#tl-drop"]) {
  const z = $(id);
  z.addEventListener("dragenter", () => z.classList.add("over"));
  z.addEventListener("dragleave", () => z.classList.remove("over"));
  z.addEventListener("drop", () => z.classList.remove("over"));
}

/* ------------------------------------------------------------ SETTINGS */
function fillSettings() {
  const { cfg } = S.settings;
  $("#set-path").textContent = S.settings.config_path;
  $("#set-llm-url").value = cfg.llm_url;
  $("#set-llm-key").value = cfg.llm_key || "";
  $("#set-llm-model").value = cfg.llm_model;
  $("#set-batch").value = cfg.batch_size;
  $("#set-temp").value = cfg.llm_temperature;
  $$('input[name="asr"]').forEach((r) => (r.checked = r.value === cfg.asr));
  $("#set-py").value = S.settings.whisper_python;
  $("#set-wmodel").value = cfg.whisper_model;
  $("#set-device").value = cfg.whisper_device;
  $("#set-threads").value = cfg.whisper_threads;
  $("#set-vad").checked = !!cfg.whisper_vad;
  $("#set-asr-url").value = cfg.asr_api_url;
  $("#set-asr-key").value = cfg.asr_api_key || "";
  $("#set-asr-model").value = cfg.asr_api_model;
  setPath($("#set-out"), S.settings.output_dir);
  $("#set-dl-segments").value = cfg.download_segments;
  $("#set-dl-limit").value = cfg.speed_limit ? +(cfg.speed_limit / 1048576).toFixed(2) : 0;
  $("#set-dl-parallel").value = cfg.download_parallel;
  $("#dl-segments").placeholder = String(cfg.download_segments);
  toggleAsr();
  $("#model-name").textContent = cfg.llm_model;
  $("#tl-model").textContent = cfg.llm_model;
}
function toggleAsr() {
  const api = document.querySelector('input[name="asr"]:checked')?.value === "api";
  $("#set-asr-local").hidden = api;
  $("#set-asr-api").hidden = !api;
}
$$('input[name="asr"]').forEach((r) => r.addEventListener("change", toggleAsr));
$("#set-llm-key-show").addEventListener("click", () => {
  const i = $("#set-llm-key");
  i.type = i.type === "password" ? "text" : "password";
  $("#set-llm-key-show").setAttribute("aria-label", i.type === "password" ? "Tampilkan API key" : "Sembunyikan API key");
});
$("#set-out-pick").addEventListener("click", async () => {
  const p = await invoke("pick_folder", { start: $("#set-out").textContent });
  if (p) setPath($("#set-out"), p);
});
$("#set-theme").addEventListener("change", () => {
  localStorage.setItem("vidsub-theme", $("#set-theme").value);
  applyTheme();
});
$("#set-models-refresh").addEventListener("click", loadModels);
async function loadModels() {
  const btn = $("#set-models-refresh");
  btn.classList.add("loading");
  try {
    const models = await invoke("list_models", { url: $("#set-llm-url").value.trim(), key: $("#set-llm-key").value.trim() || null });
    $("#set-models").replaceChildren(...models.map((m) => h("option", { value: m })));
    toast(`${models.length} model ditemukan di server`);
  } catch (e) { toast(String(e)); }
  finally { btn.classList.remove("loading"); }
}

$("#set-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const old = S.settings;
  const py = $("#set-py").value.trim();
  const outDir = $("#set-out").textContent.trim();
  const cfg = {
    ...old.cfg,
    llm_url: $("#set-llm-url").value.trim(),
    llm_key: $("#set-llm-key").value.trim() || null,
    llm_model: $("#set-llm-model").value.trim(),
    batch_size: parseInt($("#set-batch").value, 10) || 25,
    llm_temperature: parseFloat($("#set-temp").value) || 0,
    asr: document.querySelector('input[name="asr"]:checked').value,
    whisper_python: py && py !== old.whisper_python ? py : old.cfg.whisper_python,
    whisper_model: $("#set-wmodel").value.trim() || "large-v3",
    whisper_device: $("#set-device").value,
    whisper_threads: parseInt($("#set-threads").value, 10) || 0,
    whisper_vad: $("#set-vad").checked,
    asr_api_url: $("#set-asr-url").value.trim(),
    asr_api_key: $("#set-asr-key").value.trim() || null,
    asr_api_model: $("#set-asr-model").value.trim(),
    output_dir: outDir && outDir !== old.output_dir ? outDir : old.cfg.output_dir,
    download_segments: Math.min(32, Math.max(1, parseInt($("#set-dl-segments").value, 10) || 8)),
    speed_limit: Math.max(0, Math.round((parseFloat($("#set-dl-limit").value) || 0) * 1048576)),
    download_parallel: Math.min(10, Math.max(1, parseInt($("#set-dl-parallel").value, 10) || 3)),
  };
  try {
    await invoke("save_settings", { cfg });
    S.settings = await invoke("get_settings");
    fillSettings();
    if (!S.dl.dir || S.dl.dir === old.output_dir) { S.dl.dir = S.settings.output_dir; setPath($("#dl-dir"), S.dl.dir); }
    $("#set-saved").textContent = "Tersimpan";
    setTimeout(() => ($("#set-saved").textContent = ""), 2500);
    checkDeps();
  } catch (err) { toast(String(err)); }
});

async function checkDeps() {
  const ul = $("#set-deps");
  const btn = $("#set-deps-refresh");
  btn.classList.add("loading");
  try {
    const deps = await invoke("check_deps");
    ul.replaceChildren(...deps.map((d) => h("li", {},
      h("span", { class: d.ok ? "ok" : "bad" }, icon(d.ok ? "check-circle" : "alert")),
      h("span", { class: "name" }, d.name),
      h("span", { class: "detail", title: d.detail }, d.detail),
    )));
    const llm = deps.find((d) => d.name === "Server LLM");
    $("#llm-dot").className = `dot ${llm?.ok ? "ok" : "bad"}`;
    $("#model-chip").title = llm?.ok ? `Model LLM aktif: ${S.settings.cfg.llm_model}` : `LLM bermasalah: ${llm?.detail}`;
  } catch (e) {
    ul.replaceChildren(h("li", { class: "bad" }, String(e)));
  } finally { btn.classList.remove("loading"); }
}
$("#set-deps-refresh").addEventListener("click", checkDeps);

/* ------------------------------------------------------------ QR code */
const qrState = { seq: 0, timer: null, text: "", opts: null, dataUrl: "", ok: false };

function qrOpts() {
  return {
    size: Math.min(8192, Math.max(64, Number($("#qr-size").value) || 512)),
    ec: document.querySelector('input[name="qr-ec"]:checked').value,
    fg: $("#qr-fg").value,
    bg: $("#qr-bg").value,
    margin: Math.min(64, Math.max(0, Math.round(Number($("#qr-margin").value) || 0))),
  };
}
function qrSetState({ img, error }) {
  const ok = Boolean(img);
  qrState.ok = ok;
  qrState.dataUrl = img || "";
  $("#qr-img").hidden = !ok;
  if (ok) $("#qr-img").src = img; else $("#qr-img").removeAttribute("src");
  $("#qr-empty").hidden = ok;
  $("#qr-error").hidden = !error;
  $("#qr-error").textContent = error || "";
  $("#qr-save").disabled = !ok;
  $("#qr-copy").disabled = !ok;
  if (!ok) $("#qr-saved").hidden = true;
}
async function qrRender() {
  const text = $("#qr-text").value.trim();
  const seq = ++qrState.seq;
  if (!text) return qrSetState({});
  const opts = qrOpts();
  try {
    const img = await invoke("qr_preview", { text, opts });
    if (seq !== qrState.seq) return; // sudah ada input yang lebih baru
    qrState.text = text; qrState.opts = opts;
    qrSetState({ img });
  } catch (e) {
    if (seq === qrState.seq) qrSetState({ error: String(e) });
  }
}
function qrSchedule() {
  clearTimeout(qrState.timer);
  qrState.timer = setTimeout(qrRender, 250);
}
for (const sel of ["#qr-text", "#qr-size", "#qr-margin", "#qr-fg", "#qr-bg"]) $(sel).addEventListener("input", qrSchedule);
$$('input[name="qr-ec"]').forEach((r) => r.addEventListener("change", qrSchedule));

let qrLastSaved = "";
$("#qr-save").addEventListener("click", async () => {
  clearTimeout(qrState.timer);
  await qrRender(); // pastikan file sama dengan isi kotak, bukan pratinjau lama
  if (!qrState.ok) return;
  try {
    const dir = qrLastSaved ? dirname(qrLastSaved) : S.settings.output_dir;
    const path = await invoke("pick_save_qr", { suggested: dir });
    if (!path) return;
    const saved = await invoke("qr_save", { text: qrState.text, opts: qrState.opts, path });
    qrLastSaved = saved;
    setPath($("#qr-saved-path"), saved);
    $("#qr-saved").hidden = false;
    toast("QR code tersimpan");
  } catch (e) { toast(String(e)); }
});
$("#qr-open").addEventListener("click", () => qrLastSaved && invoke("open_path", { path: qrLastSaved, reveal: true }).catch(toast));

if (window.ClipboardItem && navigator.clipboard?.write) {
  $("#qr-copy").hidden = false;
  $("#qr-copy").addEventListener("click", async () => {
    if (!qrState.ok) return;
    try {
      // tanpa fetch(): CSP tidak mengizinkan data: di connect-src
      const bin = atob(qrState.dataUrl.split(",")[1]);
      const blob = new Blob([Uint8Array.from(bin, (c) => c.charCodeAt(0))], { type: "image/png" });
      await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
      toast("Gambar QR disalin");
    } catch {
      $("#qr-copy").hidden = true;
      toast("Salin gambar tidak didukung di sistem ini. Pakai Simpan sebagai…");
    }
  });
}

/* ------------------------------------------------------------ init */
(async function init() {
  applyTheme();
  listen("job", ({ payload }) => jobsById.get(payload.job)?.onEvent(payload));
  [S.settings, S.langs] = await Promise.all([invoke("get_settings"), invoke("languages")]);
  for (const sel of $$(".lang-select")) fillLangSelect(sel);
  $("#tl-to").value = "id";
  fillSettings();
  S.dl.dir = S.settings.output_dir;
  setPath($("#dl-dir"), S.dl.dir);
  updateStartButtons();
  checkDeps();
  $("#dl-url").focus();
})();
