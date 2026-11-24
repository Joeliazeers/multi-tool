# Helper faster-whisper yang dipanggil oleh MultiTool (Rust). Ditulis ke temp lalu dijalankan
# dengan python dari venv whisper. Hasil: JSON {language, language_probability, duration, segments}.
import argparse, json, sys


def ts(t):
    h, r = divmod(t, 3600)
    m, s = divmod(r, 60)
    return f"{int(h):02}:{int(m):02}:{s:05.2f}"


p = argparse.ArgumentParser()
p.add_argument("--input", required=True)
p.add_argument("--out", required=True)
p.add_argument("--model", default="large-v3")
p.add_argument("--device", default="cpu")
p.add_argument("--compute-type", default="int8")
p.add_argument("--threads", type=int, default=0)
p.add_argument("--language", default=None)
p.add_argument("--prompt", default=None)
p.add_argument("--quiet", action="store_true")
p.add_argument("--detect", action="store_true", help="hanya deteksi bahasa")
p.add_argument("--vad", action="store_true", help="lewati bagian hening dengan VAD (lebih cepat, bisa melewatkan suara pelan)")
a = p.parse_args()

from faster_whisper import WhisperModel, decode_audio

print(f"[whisper] memuat model {a.model} ({a.device}/{a.compute_type})...", file=sys.stderr, flush=True)
model = WhisperModel(a.model, device=a.device, compute_type=a.compute_type, cpu_threads=a.threads)

if a.detect:
    # Deteksi bahasa bawaan Whisper: beberapa jendela 30 detik, bagian tanpa suara dilewati (VAD).
    from faster_whisper.vad import VadOptions, get_speech_timestamps

    audio = decode_audio(a.input, sampling_rate=16000)
    speech = sum(t["end"] - t["start"] for t in get_speech_timestamps(audio, VadOptions(threshold=0.25))) / 16000
    print(f"[whisper] ucapan di sampel: {speech:.0f} detik", file=sys.stderr, flush=True)
    if speech < 2:
        # Whisper selalu menebak sesuatu, bahkan untuk audio hening -> jangan dipercaya.
        with open(a.out, "w", encoding="utf-8") as f:
            json.dump({"language": "", "language_probability": 0.0, "candidates": [], "speech_seconds": speech}, f)
        sys.exit(0)
    lang, prob, all_probs = model.detect_language(
        audio, vad_filter=True, language_detection_segments=4, language_detection_threshold=0.9
    )
    top = sorted(all_probs, key=lambda x: -x[1])[:5]
    print(f"[whisper] bahasa: {lang} (p={prob:.2f})", file=sys.stderr, flush=True)
    with open(a.out, "w", encoding="utf-8") as f:
        json.dump({"language": lang, "language_probability": prob, "candidates": top, "speech_seconds": speech}, f)
    sys.exit(0)
segments, info = model.transcribe(
    a.input,
    task="transcribe",
    language=a.language or None,
    initial_prompt=a.prompt or None,
    # VAD (Silero) membuang dialog pelan/berbisik. Default: tanpa VAD, dengan
    # hallucination_silence_threshold untuk membuang teks karangan di bagian hening.
    vad_filter=a.vad,
    vad_parameters=dict(threshold=0.25, min_silence_duration_ms=400, speech_pad_ms=300) if a.vad else None,
    hallucination_silence_threshold=None if a.vad else 2.0,
    condition_on_previous_text=False,
    no_speech_threshold=0.6,
    compression_ratio_threshold=2.4,
    beam_size=5,
    word_timestamps=True,
)
print(
    f"[whisper] bahasa: {info.language} (p={info.language_probability:.2f}), durasi {info.duration:.0f}s",
    file=sys.stderr,
    flush=True,
)
# Bahasa tanpa spasi butuh baris lebih pendek (dalam karakter).
CJK = {"zh", "ja", "yue", "th", "lo", "my", "km"}
cjk = info.language in CJK
MAX_CHARS = 18 if cjk else 84
MAX_DUR = 5.0 if cjk else 7.0
MAX_GAP = 0.6 if cjk else 0.8
END = tuple(".?!。？！…")
SOFT = tuple(",;:、，")
ABBREV = {"mr.", "mrs.", "ms.", "dr.", "st.", "jr.", "sr.", "vs.", "prof.", "no.", "mt."}


class W:
    def __init__(self, start, end, word):
        self.start, self.end, self.word = start, end, word


def text_of(ws):
    return "".join(x.word for x in ws).strip()


out = []
cur = []


def norm(t):
    return "".join(ch for ch in t.lower() if ch.isalnum())


def emit():
    global cur
    if cur:
        t = text_of(cur)
        # Whisper kadang mengulang baris yang sama berturut-turut: buang duplikatnya.
        if out and norm(out[-1]["text"]) == norm(t) and cur[0].start - out[-1]["end"] < 4.0:
            out[-1]["end"] = cur[-1].end
            t = ""
        if t:
            out.append({"start": cur[0].start, "end": cur[-1].end, "text": t})
            if not a.quiet:
                print(f"  [{ts(cur[0].start)} / {ts(info.duration)}] {t}", file=sys.stderr, flush=True)
    cur = []


def feed(w):
    """Gabungkan kata lintas segmen whisper jadi potongan subtitle yang enak dibaca."""
    if cur and (
        w.start - cur[-1].end > MAX_GAP
        or len(text_of(cur + [w])) > MAX_CHARS
        or w.end - cur[0].start > MAX_DUR
    ):
        emit()
    cur.append(w)
    t = text_of(cur)
    tail = w.word.strip()
    if (tail.endswith(END) and len(t) >= 15 and tail.lower() not in ABBREV) or (
        tail.endswith(SOFT) and len(t) >= MAX_CHARS * 0.6
    ):
        emit()


for s in segments:
    # Timestamp kata bisa "melebar" melewati bagian hening (mis. satu kata 14 detik):
    # batasi durasi tiap kata supaya baris subtitle tidak terpecah/terlalu panjang.
    cap = 1.2 if cjk else 2.0
    words = [W(w.start, min(w.end, w.start + cap), w.word) for w in (s.words or []) if w.word.strip()]
    if not words:
        words = [W(s.start, s.end, " " + s.text.strip())]
    for w in words:
        feed(w)
    if cjk:
        # Teks Mandarin/Jepang dari whisper jarang bertanda baca: batas segmen whisper
        # adalah satu-satunya penanda akhir ucapan, jadi jangan digabung dengan segmen berikutnya.
        emit()
emit()

with open(a.out, "w", encoding="utf-8") as f:
    json.dump(
        {
            "language": info.language,
            "language_probability": info.language_probability,
            "duration": info.duration,
            "segments": out,
        },
        f,
        ensure_ascii=False,
    )
