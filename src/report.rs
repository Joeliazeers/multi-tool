//! Pelaporan progres & pembatalan, dipakai bersama oleh CLI dan GUI.
//! CLI: dicetak ke terminal. GUI: `set_sink` meneruskan event ke jendela.
use anyhow::{Result, bail};
use serde::Serialize;
use std::cell::RefCell;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Event {
    /// Tahap baru (judul besar)
    Stage { text: String },
    /// Progres tahap saat ini. `fraction` None = tidak tentu.
    Progress { fraction: Option<f64>, detail: String },
    /// Baris log biasa
    Log { text: String },
    /// Peringatan (bukan fatal)
    Warn { text: String },
}

/// Sink menerima ID pekerjaan (None di luar pekerjaan, mis. CLI) + event.
type Sink = Box<dyn Fn(Option<u64>, Event) + Send + Sync>;
static SINK: RwLock<Option<Sink>> = RwLock::new(None);
/// Pembatalan global (CLI, satu proses = satu pekerjaan).
static CANCEL: AtomicBool = AtomicBool::new(false);

/// Konteks pekerjaan per-thread: GUI bisa menjalankan beberapa pekerjaan bersamaan
/// (mis. download sambil transcribe), masing-masing dengan event & tombol batal sendiri.
#[derive(Clone)]
pub struct Job {
    pub id: u64,
    pub cancel: Arc<AtomicBool>,
}

thread_local! {
    static JOB: RefCell<Option<Job>> = const { RefCell::new(None) };
}

struct JobGuard;
impl Drop for JobGuard {
    fn drop(&mut self) {
        JOB.with(|j| *j.borrow_mut() = None);
    }
}

/// Jalankan `f` di thread ini sebagai pekerjaan `job`.
pub fn with_job<T>(job: Job, f: impl FnOnce() -> T) -> T {
    JOB.with(|j| *j.borrow_mut() = Some(job));
    let _guard = JobGuard; // reset walau panik (thread pool dipakai ulang)
    f()
}

fn current_job() -> Option<u64> {
    JOB.with(|j| j.borrow().as_ref().map(|j| j.id))
}

pub fn set_sink(f: impl Fn(Option<u64>, Event) + Send + Sync + 'static) {
    *SINK.write().unwrap() = Some(Box::new(f));
}

pub fn emit(e: Event) {
    if let Some(s) = SINK.read().unwrap().as_ref() {
        s(current_job(), e);
        return;
    }
    match e {
        Event::Stage { text } => println!("{text}"),
        Event::Log { text } => println!("{text}"),
        Event::Warn { text } => eprintln!("  ! {text}"),
        Event::Progress { fraction, detail } => {
            let pct = fraction.map(|f| format!("{:5.1}% ", f * 100.0)).unwrap_or_default();
            let line: String = format!("{pct}{detail}").chars().take(110).collect();
            eprint!("\r\x1b[2K  {line}");
            let _ = std::io::stderr().flush();
            if fraction.is_some_and(|f| f >= 1.0) {
                eprintln!();
            }
        }
    }
}

pub fn stage(text: impl Into<String>) {
    emit(Event::Stage { text: text.into() });
}
pub fn log(text: impl Into<String>) {
    emit(Event::Log { text: text.into() });
}
pub fn warn(text: impl Into<String>) {
    emit(Event::Warn { text: text.into() });
}
pub fn progress(fraction: Option<f64>, detail: impl Into<String>) {
    emit(Event::Progress { fraction: fraction.map(|f| f.clamp(0.0, 1.0)), detail: detail.into() });
}

/// Batalkan (mode CLI / tanpa konteks pekerjaan).
pub fn cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}
pub fn reset_cancel() {
    CANCEL.store(false, Ordering::SeqCst);
}
/// Apakah pekerjaan di thread ini dibatalkan?
pub fn cancelled() -> bool {
    JOB.with(|j| j.borrow().as_ref().map(|j| j.cancel.load(Ordering::SeqCst)))
        .unwrap_or_else(|| CANCEL.load(Ordering::SeqCst))
}
/// Error kalau user menekan batal.
pub fn check() -> Result<()> {
    if cancelled() {
        bail!("Dibatalkan");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_thread_cancel() {
        let a = Job { id: 1, cancel: Arc::new(AtomicBool::new(false)) };
        let b = Job { id: 2, cancel: Arc::new(AtomicBool::new(false)) };
        let a2 = a.clone();
        let ta = std::thread::spawn(move || with_job(a2, || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            (current_job(), cancelled())
        }));
        let tb = std::thread::spawn(move || with_job(b, || (current_job(), cancelled())));
        a.cancel.store(true, Ordering::SeqCst);
        assert_eq!(ta.join().unwrap(), (Some(1), true));
        assert_eq!(tb.join().unwrap(), (Some(2), false)); // pekerjaan lain tidak ikut batal
        assert_eq!(current_job(), None);
    }
}
