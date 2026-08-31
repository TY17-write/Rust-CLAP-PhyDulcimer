//! GUI → オーディオのノートイベントリング (単一生産者・単一消費者)。
//!
//! GUI の鍵盤クリックをオーディオスレッドへ運ぶ。ロックフリーで確保なし、
//! `&self` だけで両側から触れるので clack の Shared にそのまま置ける
//! (rtrb 型の Producer/Consumer 分割は clack の「Shared は不変参照、
//! AudioProcessor は activate ごとに作り直し」と噛み合わない)。
//!
//! # 不変条件
//!
//! - **push を呼ぶのはエディタのウィンドウスレッドだけ** (単一生産者)。
//!   ウィンドウは常に 1 枚 (`EditorState` が管理、destroy → create は逐次)
//! - **pop を呼ぶのはオーディオスレッドだけ** (単一消費者)
//!
//! # 溢れ方針
//!
//! 満杯なら push は `false` を返して**新しいイベントを捨てる**。生産者は
//! マウスクリックで低レートなので、2 ブロックの間に 64 発は現実に起きない。
//! drop-oldest は生産者が tail を動かすことになり SPSC の単純さが壊れる。

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// スロット数。2 の冪 (添字は `& (LEN-1)`)。
const LEN: usize = 64;

/// ノートイベントのリング。
pub struct NoteRing {
    /// `(key << 32) | velocity.to_bits()` を積む
    slots: [AtomicU64; LEN],
    /// 書き込み位置。**生産者だけが進める** (wrapping)
    head: AtomicU32,
    /// 読み出し位置。**消費者だけが進める** (wrapping)
    tail: AtomicU32,
}

impl Default for NoteRing {
    fn default() -> Self {
        Self::new()
    }
}

impl NoteRing {
    pub fn new() -> Self {
        Self {
            slots: [const { AtomicU64::new(0) }; LEN],
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
        }
    }

    /// テスト用: カウンタを折り返し近くから始める。
    #[cfg(test)]
    fn with_counters(start: u32) -> Self {
        let ring = Self::new();
        ring.head.store(start, Ordering::Relaxed);
        ring.tail.store(start, Ordering::Relaxed);
        ring
    }

    /// ノートを積む (GUI スレッド専用)。満杯なら `false` (捨てられた)。
    pub fn push(&self, key: u8, velocity: f32) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        // Acquire: 消費者がスロットを読み終えた (tail を進めた) のを見てから
        // 同じスロットを上書きする。
        let tail = self.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) as usize >= LEN {
            return false;
        }
        let packed = (u64::from(key) << 32) | u64::from(velocity.to_bits());
        self.slots[head as usize & (LEN - 1)].store(packed, Ordering::Relaxed);
        // Release: スロットの中身を消費者へ公開してから head を進める。
        self.head.store(head.wrapping_add(1), Ordering::Release);
        true
    }

    /// ノートを取り出す (オーディオスレッド専用)。空なら `None`。
    pub fn pop(&self) -> Option<(u8, f32)> {
        let tail = self.tail.load(Ordering::Relaxed);
        // Acquire: 生産者の Release (スロット書き込みの公開) と対。
        let head = self.head.load(Ordering::Acquire);
        if head == tail {
            return None;
        }
        let packed = self.slots[tail as usize & (LEN - 1)].load(Ordering::Relaxed);
        // Release: スロットを読み終えたことを生産者へ公開してから tail を進める。
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        let key = (packed >> 32) as u8;
        let velocity = f32::from_bits(packed as u32);
        Some((key, velocity))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_round_trip_in_order() {
        let ring = NoteRing::new();
        assert!(ring.push(60, 0.5));
        assert!(ring.push(69, 1.0));
        assert_eq!(ring.pop(), Some((60, 0.5)));
        assert_eq!(ring.pop(), Some((69, 1.0)));
        assert_eq!(ring.pop(), None);
    }

    #[test]
    fn velocity_bits_survive_the_packing() {
        let ring = NoteRing::new();
        for v in [0.0f32, 0.2, 0.123_456_79, 1.0] {
            assert!(ring.push(43, v));
            let (_, back) = ring.pop().unwrap();
            assert_eq!(back.to_bits(), v.to_bits());
        }
    }

    #[test]
    fn a_full_ring_rejects_the_new_event() {
        let ring = NoteRing::new();
        for i in 0..64 {
            assert!(ring.push(i as u8, 0.5), "{i} 発目で満杯になった");
        }
        // 65 発目は捨てられる (reject-when-full)。
        assert!(!ring.push(99, 0.5));
        // 既存の 64 発は無傷。
        for i in 0..64 {
            assert_eq!(ring.pop(), Some((i as u8, 0.5)));
        }
        assert_eq!(ring.pop(), None);
        // 空になれば再び積める。
        assert!(ring.push(99, 0.5));
        assert_eq!(ring.pop(), Some((99, 0.5)));
    }

    #[test]
    fn counters_survive_wrapping() {
        // u32 の折り返しをまたいでも順序と満杯判定が壊れない。
        let ring = NoteRing::with_counters(u32::MAX - 3);
        for i in 0..8u8 {
            assert!(ring.push(i, 0.5));
        }
        for i in 0..8u8 {
            assert_eq!(ring.pop(), Some((i, 0.5)));
        }
        assert_eq!(ring.pop(), None);
    }
}
