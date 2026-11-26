//! Tabel bagian download: dibagi rata di awal, lalu dibelah dinamis saat ada pekerja menganggur.
use serde::{Deserialize, Serialize};

const MIB: u64 = 1024 * 1024;
/// Satu bagian minimal sebesar ini (file kecil memakai lebih sedikit bagian).
const MIN_PART: u64 = MIB;
/// Bagian hanya dibelah bila sisanya lebih dari ini.
const MIN_SPLIT: u64 = 2 * MIB;

/// Rentang byte `[start, end)`; `pos` = byte berikutnya yang belum diunduh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seg {
    pub start: u64,
    pub pos: u64,
    pub end: u64,
}

impl Seg {
    pub fn remaining(&self) -> u64 {
        self.end.saturating_sub(self.pos)
    }
}

#[derive(Debug)]
pub struct Table {
    segs: Vec<Seg>,
    claimed: Vec<bool>,
}

impl Table {
    pub fn new(total: u64, parts: usize) -> Table {
        if total == 0 {
            return Table { segs: vec![], claimed: vec![] };
        }
        let parts = (parts.max(1) as u64).min((total / MIN_PART).max(1));
        let size = total / parts;
        let segs: Vec<Seg> = (0..parts)
            .map(|i| {
                let start = i * size;
                let end = if i + 1 == parts { total } else { start + size };
                Seg { start, pos: start, end }
            })
            .collect();
        Table { claimed: vec![false; segs.len()], segs }
    }

    pub fn from_segs(segs: Vec<Seg>) -> Table {
        Table { claimed: vec![false; segs.len()], segs }
    }

    /// Bagian menutupi `0..total` tanpa celah/tumpang tindih dan `pos` ada di dalam rentangnya.
    pub fn is_valid_for(&self, total: u64) -> bool {
        let mut v: Vec<&Seg> = self.segs.iter().collect();
        v.sort_by_key(|s| s.start);
        let mut at = 0;
        for s in v {
            if s.start != at || s.end < s.start || s.pos < s.start || s.pos > s.end {
                return false;
            }
            at = s.end;
        }
        at == total
    }

    /// Ambil bagian untuk dikerjakan: yang belum diklaim, atau belah bagian terbesar yang tersisa.
    pub fn claim(&mut self) -> Option<usize> {
        if let Some(i) = (0..self.segs.len()).find(|&i| !self.claimed[i] && self.segs[i].remaining() > 0) {
            self.claimed[i] = true;
            return Some(i);
        }
        let (victim, rem) = (0..self.segs.len())
            .filter(|&i| self.claimed[i])
            .map(|i| (i, self.segs[i].remaining()))
            .max_by_key(|&(_, r)| r)?;
        if rem <= MIN_SPLIT {
            return None;
        }
        let old_end = self.segs[victim].end;
        let mid = self.segs[victim].pos + rem / 2;
        self.segs[victim].end = mid;
        self.segs.push(Seg { start: mid, pos: mid, end: old_end });
        self.claimed.push(true);
        Some(self.segs.len() - 1)
    }

    /// Berapa byte (<= `want`) yang masih boleh ditulis bagian `i`; 0 = selesai.
    pub fn room(&self, i: usize, want: u64) -> u64 {
        self.segs[i].remaining().min(want)
    }

    /// `(pos, end)` bagian `i`.
    pub fn bounds(&self, i: usize) -> (u64, u64) {
        (self.segs[i].pos, self.segs[i].end)
    }

    pub fn advance(&mut self, i: usize, n: u64) {
        let s = &mut self.segs[i];
        s.pos = (s.pos + n).min(s.end);
    }

    /// Lepas klaim (pekerja berhenti sebelum bagian selesai).
    pub fn release(&mut self, i: usize) {
        self.claimed[i] = false;
    }

    pub fn done_bytes(&self) -> u64 {
        self.segs.iter().map(|s| s.pos - s.start).sum()
    }

    pub fn is_complete(&self) -> bool {
        self.segs.iter().all(|s| s.remaining() == 0)
    }

    pub fn segs(&self) -> Vec<Seg> {
        self.segs.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covers(t: &Table, total: u64) -> bool {
        t.is_valid_for(total)
    }

    #[test]
    fn even_split_covers_all() {
        let t = Table::new(10 * MIB, 4);
        assert_eq!(t.segs().len(), 4);
        assert!(covers(&t, 10 * MIB));
        let t = Table::new(10 * MIB + 3, 4);
        assert!(covers(&t, 10 * MIB + 3));
    }

    #[test]
    fn small_files_use_fewer_parts() {
        assert_eq!(Table::new(10, 8).segs().len(), 1);
        assert_eq!(Table::new(2 * MIB + 5, 8).segs().len(), 2);
    }

    #[test]
    fn empty_file() {
        let t = Table::new(0, 8);
        assert!(t.is_complete());
        assert!(t.segs().is_empty());
        assert!(t.is_valid_for(0));
    }

    #[test]
    fn claims_then_splits() {
        let mut t = Table::new(8 * MIB, 4);
        let first: Vec<_> = (0..4).map(|_| t.claim().unwrap()).collect();
        assert_eq!(first, vec![0, 1, 2, 3]);
        // semua diklaim, 2 MiB tersisa per bagian: tidak lebih dari MIN_SPLIT -> tidak dibelah
        assert_eq!(t.claim(), None);
        let mut t = Table::new(40 * MIB, 4);
        for _ in 0..4 {
            t.claim().unwrap();
        }
        let n = t.claim().expect("harus membelah bagian terbesar");
        assert_eq!(n, 4);
        assert_eq!(t.segs().len(), 5);
        assert!(covers(&t, 40 * MIB));
        assert_eq!(t.done_bytes(), 0);
    }

    #[test]
    fn split_takes_upper_half_of_remaining() {
        let mut t = Table::new(40 * MIB, 1);
        let a = t.claim().unwrap();
        t.advance(a, 4 * MIB);
        let b = t.claim().unwrap();
        let s = t.segs();
        assert_eq!(s[a].end, 4 * MIB + 18 * MIB);
        assert_eq!(s[b].start, s[a].end);
        assert_eq!(s[b].end, 40 * MIB);
    }

    #[test]
    fn advance_clamps_and_room_reports() {
        let mut t = Table::new(3 * MIB, 1);
        let i = t.claim().unwrap();
        assert_eq!(t.room(i, 64 * 1024), 64 * 1024);
        t.advance(i, 10 * MIB);
        assert_eq!(t.room(i, 1), 0);
        assert!(t.is_complete());
        assert_eq!(t.done_bytes(), 3 * MIB);
    }

    #[test]
    fn release_allows_reclaim() {
        let mut t = Table::new(3 * MIB, 1);
        let i = t.claim().unwrap();
        t.advance(i, 100);
        t.release(i);
        assert_eq!(t.claim(), Some(i));
    }

    #[test]
    fn serde_roundtrip_and_validation() {
        let mut t = Table::new(40 * MIB, 4);
        for _ in 0..5 {
            t.claim();
        }
        let json = serde_json::to_string(&t.segs()).unwrap();
        let back: Vec<Seg> = serde_json::from_str(&json).unwrap();
        let t2 = Table::from_segs(back);
        assert!(t2.is_valid_for(40 * MIB));
        assert!(!t2.is_valid_for(41 * MIB));
        let mut bad = t2.segs();
        bad[0].end -= 1;
        assert!(!Table::from_segs(bad).is_valid_for(40 * MIB));
    }
}
