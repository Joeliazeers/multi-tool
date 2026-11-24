"use strict";
// Self-test end-to-end: hanya berjalan di build debug bila VIDSUB_SELFTEST di-set.
// Menjalankan alur GUI yang sama dengan klik user, lalu menulis hasil ke stderr.
(async () => {
  const spec = await window.__TAURI__.core.invoke("selftest_info").catch(() => null);
  if (!spec) return;
  const t = JSON.parse(spec);
  const log = (m) => window.__TAURI__.core.invoke("selftest_log", { msg: String(m) });
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const T0 = Date.now();
  const ts = () => `${((Date.now() - T0) / 1000).toFixed(1)}s`;
  const cardState = (c) => `${c.q(".job-title").textContent} | ${c.q(".act-stage").textContent} | ${c.q(".act-detail").textContent}`;
  const waitFor = async (cond, maxMs = 900000) => { const t0 = Date.now(); while (!cond() && Date.now() - t0 < maxMs) await sleep(200); };

  // Dialog konfirmasi bahasa: catat isinya lalu terima.
  new MutationObserver(() => {
    const d = $("#lang-dialog");
    if (d.open) setTimeout(() => { log(`[${ts()}] dialog: ${$("#lang-dialog-name").textContent} | ${$("#lang-dialog-reason").textContent}`); d.close("ok"); }, 300);
  }).observe($("#lang-dialog"), { attributes: true, attributeFilter: ["open"] });

  await sleep(1500);

  // Skenario QR (tanpa job berat): {"qr": "<folder simpan>"}
  if (t.qr) {
    try {
      const wait = () => sleep(700);
      const state = () => `img.hidden=${$("#qr-img").hidden} src=${($("#qr-img").src || "").slice(0, 30)} err="${$("#qr-error").textContent}" save.disabled=${$("#qr-save").disabled} copy.hidden=${$("#qr-copy").hidden}`;
      const type = async (v) => { $("#qr-text").value = v; $("#qr-text").dispatchEvent(new Event("input")); await wait(); };
      go("qr");
      await log(`[qr] awal: ${state()} | nav aktif=${$(".nav-item[aria-current=page]").dataset.page}`);
      await type("https://example.com");
      await log(`[qr] setelah ketik: ${state()}`);
      const before = $("#qr-img").src;
      document.querySelector('input[name="qr-ec"][value="H"]').click(); await wait();
      await log(`[qr] ganti EC=H: gambar berubah=${$("#qr-img").src !== before}`);
      $("#qr-fg").value = "#1a1a8a"; $("#qr-fg").dispatchEvent(new Event("input")); await wait();
      await log(`[qr] ganti warna: ${state()}`);
      await type("");
      await log(`[qr] teks dikosongkan: ${state()}`);
      await type("a".repeat(8000));
      await log(`[qr] teks 8000 karakter: ${state()}`);
      await type("https://example.com/halo");
      await log(`[qr] pulih: ${state()}`);
      for (const ext of ["png", "jpg", "svg", "webp", "bmp"]) {
        const r = await window.__TAURI__.core.invoke("qr_save", { text: "https://example.com", opts: { size: 512, ec: "M", fg: "#000000", bg: "#ffffff", margin: 4 }, path: `${t.qr}/gui.${ext}` }).catch((e) => `ERR ${e}`);
        await log(`[qr] qr_save ${ext}: ${r}`);
      }
      // ukuran besar tidak boleh membekukan UI: ukur waktu & pastikan pratinjau tetap muncul
      $("#qr-size").value = "8192"; $("#qr-size").dispatchEvent(new Event("input"));
      const t0 = Date.now(); let ticks = 0;
      const iv = setInterval(() => ticks++, 50);
      await sleep(1500); clearInterval(iv);
      await log(`[qr] size 8192: ${state()} | UI tick dalam 1.5s=${ticks} (≈30 = tidak beku)`);
      $("#qr-size").value = "512"; $("#qr-size").dispatchEvent(new Event("input")); await wait();
      // salin gambar: klik sungguhan (mungkin ditolak WebKit tanpa gestur user; yang dicek: bukan karena fetch/CSP)
      $("#qr-copy").click(); await sleep(800);
      await log(`[qr] setelah klik salin: copy.hidden=${$("#qr-copy").hidden} toast="${$("#toast").textContent}"`);
      try {
        const bin = atob(qrState.dataUrl.split(",")[1]);
        const blob = new Blob([Uint8Array.from(bin, (c) => c.charCodeAt(0))], { type: "image/png" });
        await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
        await log("[qr] clipboard.write langsung: OK");
      } catch (e) { await log(`[qr] clipboard.write langsung: ${e.name}: ${e.message}`); }
      await log("[qr] siap screenshot");
      await sleep(25000);
      await log("SELESAI");
    } catch (e) { await log(`ERROR ${e}`); }
    return;
  }

  try {
    // 1) Transcribe (deteksi whisper + konfirmasi)
    go("transcribe");
    await setTrFile(t.transcribe);
    $("#tr-lang").value = "auto"; $("#tr-lang").dispatchEvent(new Event("change"));
    $("#tr-confirm").checked = true;
    $("#tr-llm").checked = false;
    $("#tr-start").click();
    await sleep(500);
    const A = JobCard.all.at(-1);
    await log(`[${ts()}] A dimulai: ${cardState(A)}`);

    // 2) Download saat transcribe berjalan -> harus jalan paralel
    go("download");
    $("#dl-url").value = t.download; S.dl.probedKey = ""; S.dl.info = null;
    $("#dl-start").click();
    await sleep(300);
    const B = JobCard.all.at(-1);
    await waitFor(() => !B.running);
    await log(`[${ts()}] B selesai: ${cardState(B)} | A masih jalan=${A.running} | ${A.running ? "PARALEL OK" : "TIDAK PARALEL"}`);

    // 3) Translate saat transcribe berjalan -> harus antre
    go("translate");
    await setTlFile(t.translate);
    $("#tl-to").value = "id"; $("#tl-to").dispatchEvent(new Event("change"));
    $("#tl-start").click();
    await sleep(500);
    const C = JobCard.all.at(-1);
    await log(`[${ts()}] C (translate) setelah klik: ${cardState(C)} | tombol="${$("#tl-start-label").textContent}"`);

    // 4) Translate kedua, dibatalkan saat masih antre
    $("#tl-start").click();
    await sleep(500);
    const D = JobCard.all.at(-1);
    await log(`[${ts()}] D sebelum batal: ${cardState(D)}`);
    D.q(".act-cancel").click();
    await waitFor(() => !D.running, 5000);
    await log(`[${ts()}] D setelah batal: ${cardState(D)} | A masih jalan=${A.running} | C masih antre/jalan=${C.running}`);

    // 5) Tunggu semua selesai
    for (let k = 0; k < 60 && !JobCard.all.every((c) => !c.running); k++) {
      await sleep(20000);
      for (const c of JobCard.all.filter((c) => c.running)) await log(`[${ts()}] masih jalan: ${cardState(c)} | pct=${c.q(".act-pct").textContent} | log: ${JSON.stringify(c.q("pre").textContent.trim().split("\n").slice(-2))}`);
    }
    for (const [n, c] of [["A", A], ["C", C]]) {
      const res = [...c.el.querySelectorAll(".result-path, .act-error")].map((e) => e.textContent);
      await log(`[${ts()}] ${n} akhir: ${cardState(c)} | ${JSON.stringify(res)}`);
    }
    await log(`[${ts()}] urutan: A selesai sebelum C mulai = ${C.q("pre").textContent.indexOf("==") >= 0}`);
    await log("SELESAI");
  } catch (e) {
    await log(`ERROR ${e}`);
  }
})();
