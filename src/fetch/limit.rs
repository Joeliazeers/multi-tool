//! Pembatas kecepatan global dan parser ukuran ("2M", "500K").
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

struct State {
    rate: u64,
    next_free: Option<Instant>,
}

/// Penjadwal kecepatan: tiap `acquire` memesan slot waktu sebesar n/rate.
pub struct Limiter {
    st: Mutex<State>,
}

impl Limiter {
    pub const fn new() -> Limiter {
        Limiter { st: Mutex::new(State { rate: 0, next_free: None }) }
    }

    /// Atur batas (byte/detik); 0 = tanpa batas.
    pub fn set_rate(&self, bytes_per_sec: u64) {
        let mut g = self.st.lock().unwrap();
        g.rate = bytes_per_sec;
        g.next_free = None;
    }

    /// Tunggu sampai `n` byte boleh dibaca. Kembali segera bila `stop` menyala.
    pub fn acquire(&self, n: usize, stop: &AtomicBool) {
        let wait = {
            let mut g = self.st.lock().unwrap();
            if g.rate == 0 {
                return;
            }
            let now = Instant::now();
            let start = g.next_free.map_or(now, |t| t.max(now));
            g.next_free = Some(start + Duration::from_secs_f64(n as f64 / g.rate as f64));
            start.saturating_duration_since(now)
        };
        let end = Instant::now() + wait;
        while !stop.load(Ordering::Relaxed) {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            std::thread::sleep(left.min(Duration::from_millis(100)));
        }
    }
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new()
    }
}

/// Batas bersama untuk semua download di proses ini.
pub static GLOBAL: Limiter = Limiter::new();

/// "2M" -> 2097152, "500k" -> 512000, "1G", angka polos = byte, kosong/"0" = 0 (tanpa batas).
pub fn parse_size(s: &str) -> Result<u64, String> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(0);
    }
    let (num, mult) = match t.chars().last().map(|c| c.to_ascii_lowercase()) {
        Some('k') => (&t[..t.len() - 1], 1024u64),
        Some('m') => (&t[..t.len() - 1], 1024 * 1024),
        Some('g') => (&t[..t.len() - 1], 1024 * 1024 * 1024),
        _ => (t, 1),
    };
    let n: f64 = num.trim().parse().map_err(|_| format!("ukuran tidak valid: {s}"))?;
    if !n.is_finite() || n < 0.0 {
        return Err(format!("ukuran tidak valid: {s}"));
    }
    Ok((n * mult as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sizes() {
        assert_eq!(parse_size("2M"), Ok(2_097_152));
        assert_eq!(parse_size("500k"), Ok(512_000));
        assert_eq!(parse_size("1G"), Ok(1_073_741_824));
        assert_eq!(parse_size("0"), Ok(0));
        assert_eq!(parse_size(""), Ok(0));
        assert_eq!(parse_size("1234"), Ok(1234));
        assert!(parse_size("abc").is_err());
        assert!(parse_size("-5M").is_err());
    }

    #[test]
    fn acquire_throttles() {
        let l = Limiter::new();
        l.set_rate(1_000_000);
        let stop = AtomicBool::new(false);
        let t = Instant::now();
        l.acquire(500_000, &stop);
        l.acquire(500_000, &stop);
        assert!(t.elapsed() >= Duration::from_millis(400), "{:?}", t.elapsed());
    }

    #[test]
    fn unlimited_does_not_wait() {
        let l = Limiter::new();
        let stop = AtomicBool::new(false);
        let t = Instant::now();
        l.acquire(100_000_000, &stop);
        assert!(t.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn stop_interrupts() {
        let l = Limiter::new();
        l.set_rate(1000);
        let stop = AtomicBool::new(false);
        l.acquire(1000, &stop); // memesan 1 detik pertama
        stop.store(true, Ordering::Relaxed);
        let t = Instant::now();
        l.acquire(1_000_000, &stop);
        assert!(t.elapsed() < Duration::from_millis(200));
    }
}
