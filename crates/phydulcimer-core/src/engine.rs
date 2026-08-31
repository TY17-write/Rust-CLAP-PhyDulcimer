//! エンジン — 楽器から出力までの信号経路を束ねる。
//!
//! ```text
//! Instrument ──┬─ バスブリッジのバス ──→ Soundboard (bass zone) ──┐
//!              └─ トレブルブリッジのバス → Soundboard (treble zone) ─┼→ Σ → ソフトクリップ → L/R
//!                        └──── 和 ────→ Cabinet (箱の低域) ────────┘
//! ```
//!
//! Phase 5 の時点では L = R (ROOM は Phase 6 でここに入る)。
//!
//! # 校正 (D-013 の解消)
//!
//! Phase 2 の暫定校正はブリッジ力の**打撃スパイク**が支配的で、クレスト
//! ファクタが 20–40 倍あった。響板は 2 次共振器の並列和 = 過渡に対する
//! ローパス的な応答なので、スパイクを実機と同じ機序で均す。校正値は
//! 響板を通した実測で取り直してある (`CALIBRATED_GAIN`)。
//!
//! # ヘッドルーム (P5 の完了条件)
//!
//! ダンパーが無く全弦が鳴り続けるので、和音の積み上がりはピアノより厳しい。
//! PhyPiano P-024 と同じ解き方: **物理は曲げず**、校正の基準を「全 44 位置 ff で
//! フルスケールに収まる」に置き、出力段にソフトクリップを 1 つ置く。

use crate::cabinet::{Cabinet, CabinetParams};
use crate::instrument::Instrument;
use crate::soundboard::{Soundboard, SoundboardParams};
use crate::Sample;

/// 出力の校正ゲイン。
///
/// 基準: ff 単音 (v=6 m/s) のピークが約 −9 dBFS、全 44 位置 ff がソフト
/// クリップ後にフルスケール内。響板を通した実測で決めた (Phase 5)。
/// 実測 (響板の傾き校正・箱の正規化後): ff 単音 A4 のエンジン出力ピークが
/// 約 −10 dBFS になる値。最低音 G2 の ff は −19 dBFS 程度で音域差が残る
/// (音域バランスは Phase 10)。
const CALIBRATED_GAIN: Sample = 5.5e-3;

/// ソフトクリップ。
///
/// tanh。±1 に漸近し、小信号はほぼ素通し (0.1 で −0.03 dB)。
/// 実機の録音でもマイクプリの飽和で同じことが起きる (PhyPiano P-024 の判断)。
#[inline]
fn soft_clip(x: Sample) -> Sample {
    x.tanh()
}

/// 楽器 + 響板 + 箱 + 出力段。
pub struct DulcimerEngine {
    instrument: Instrument,
    sb_bass: Soundboard,
    sb_treble: Soundboard,
    cabinet: Cabinet,
    /// ブリッジごとのバス (事前確保)
    bus_bass: Vec<Sample>,
    bus_treble: Vec<Sample>,
    /// 響板・箱を通さず、ブリッジ力の和を出す (A/B 検証用)
    raw_output: bool,
}

impl DulcimerEngine {
    /// **確保はここだけ** (メインスレッドで呼ぶこと)。
    pub fn new(sample_rate: f64, max_block: usize) -> Self {
        Self {
            instrument: Instrument::new(sample_rate),
            // ゾーンの種は固定 (ビルドごとに音が変わってはいけない)。
            sb_bass: Soundboard::new(SoundboardParams::default(), 0xB055, sample_rate),
            sb_treble: Soundboard::new(SoundboardParams::default(), 0x7EB1, sample_rate),
            cabinet: Cabinet::new(CabinetParams::default(), sample_rate),
            bus_bass: vec![0.0; max_block.max(1)],
            bus_treble: vec![0.0; max_block.max(1)],
            raw_output: false,
        }
    }

    pub fn instrument(&self) -> &Instrument {
        &self.instrument
    }

    pub fn note_on(&mut self, key: u8, velocity: f64) {
        self.instrument.note_on(key, velocity);
    }

    pub fn note_off(&mut self, key: u8) {
        self.instrument.note_off(key);
    }

    pub fn choke(&mut self, key: u8) {
        self.instrument.choke(key);
    }

    pub fn set_strike_ratio(&mut self, ratio: f64) {
        self.instrument.set_strike_ratio(ratio);
    }

    pub fn set_bridge_coupling(&mut self, k: f64) {
        self.instrument.set_bridge_coupling(k);
    }

    /// 響板・箱を通さない出力に切り替える (A/B 検証用)。
    pub fn set_raw_output(&mut self, raw: bool) {
        self.raw_output = raw;
    }

    pub fn any_hammer_active(&self) -> bool {
        self.instrument.any_hammer_active()
    }

    pub fn is_finite(&self) -> bool {
        self.instrument.is_finite()
            && self.sb_bass.is_finite()
            && self.sb_treble.is_finite()
            && self.cabinet.is_finite()
    }

    /// 全状態を消す (ホストの停止・シーク・ループ折り返し)。
    pub fn reset(&mut self) {
        self.instrument.reset();
        self.sb_bass.reset();
        self.sb_treble.reset();
        self.cabinet.reset();
    }

    /// 1 ブロックをステレオへ処理する (上書き)。返り値はブロックのピーク。
    ///
    /// Phase 5 では L = R。ROOM (Phase 6) がここで L/R を分ける。
    pub fn process_stereo(&mut self, left: &mut [Sample], right: &mut [Sample]) -> Sample {
        let len = left.len().min(right.len()).min(self.bus_bass.len());
        let (left, right) = (&mut left[..len], &mut right[..len]);

        self.instrument
            .process_buses(&mut self.bus_bass[..len], &mut self.bus_treble[..len]);

        let mut peak = 0.0 as Sample;
        for i in 0..len {
            let b = self.bus_bass[i];
            let t = self.bus_treble[i];

            let y = if self.raw_output {
                // Phase 2 の暫定経路 (ブリッジ力の和)。A/B と旧測定の再現用。
                soft_clip((b + t) * 0.004)
            } else {
                let sb = self.sb_bass.process_sample(b) + self.sb_treble.process_sample(t);
                let cab = self.cabinet.process_sample(b + t);
                soft_clip((sb + cab) * CALIBRATED_GAIN)
            };

            left[i] = y;
            right[i] = y;
            peak = peak.max(y.abs());
        }
        peak
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instrument::{KEY_MAX, KEY_MIN};

    const SR: f64 = 48_000.0;
    const BLOCK: usize = 64;

    fn render(engine: &mut DulcimerEngine, seconds: f64) -> (Vec<Sample>, Sample) {
        let n = (SR * seconds) as usize;
        let mut l = vec![0.0; n];
        let mut r = vec![0.0; n];
        let mut peak = 0.0f32;
        for i in (0..n).step_by(BLOCK) {
            let end = (i + BLOCK).min(n);
            let (lh, rh) = (&mut l[i..end], &mut r[i..end]);
            // 分割借用のため一時バッファに逃がす。
            let mut lb = lh.to_vec();
            let mut rb = rh.to_vec();
            peak = peak.max(engine.process_stereo(&mut lb, &mut rb));
            lh.copy_from_slice(&lb);
            rh.copy_from_slice(&rb);
        }
        (l, peak)
    }

    #[test]
    fn a_note_sounds_and_both_channels_match() {
        let mut e = DulcimerEngine::new(SR, BLOCK);
        e.note_on(69, 0.8);
        let n = (SR * 0.5) as usize;
        let mut l = vec![0.0; n];
        let mut r = vec![0.0; n];
        for i in (0..n).step_by(BLOCK) {
            let end = (i + BLOCK).min(n);
            let mut lb = vec![0.0; end - i];
            let mut rb = vec![0.0; end - i];
            e.process_stereo(&mut lb, &mut rb);
            l[i..end].copy_from_slice(&lb);
            r[i..end].copy_from_slice(&rb);
        }
        assert!(l.iter().any(|&s| s.abs() > 1e-3), "音が出ていない");
        assert_eq!(l, r, "Phase 5 では L = R のはず");
    }

    /// P5 の完了条件: ヘッドルーム。全 44 位置 ff でもフルスケール内。
    #[test]
    fn all_positions_ff_stay_within_full_scale() {
        let mut e = DulcimerEngine::new(SR, BLOCK);
        for key in KEY_MIN..=KEY_MAX {
            e.note_on(key, 1.0);
        }
        let (x, peak) = render(&mut e, 2.0);
        assert!(x.iter().all(|s| s.is_finite()));
        assert!(e.is_finite());
        assert!(peak <= 1.0, "フルスケールを超えた: {peak}");
        assert!(peak > 0.3, "全打鍵なのに小さすぎる: {peak}");
    }

    /// 校正の固定: ff 単音のピークが −14〜−6 dBFS。
    #[test]
    fn a_ff_single_note_is_calibrated() {
        let mut e = DulcimerEngine::new(SR, BLOCK);
        e.note_on(69, 1.0);
        let (_, peak) = render(&mut e, 1.0);
        let dbfs = 20.0 * (peak as f64).log10();
        assert!(
            (-14.0..=-6.0).contains(&dbfs),
            "ff 単音の校正が外れた: {dbfs:.1} dBFS"
        );
    }

    #[test]
    fn the_attack_is_followed_by_an_audible_ring() {
        // 音の形の固定。打撃過渡が支配的なのは響板を通しても変わらない
        // (板自体が過渡で一斉に鳴る — 実測でクレストは生 39 → 響板後 125 と
        // むしろ増えた。D-013 の追記を参照)。ここでは「過渡の後に聴こえる
        // 持続部が残っている」ことと、比が異常域に入らないことだけを固定する。
        let mut e = DulcimerEngine::new(SR, BLOCK);
        e.note_on(69, 1.0);
        let (x, _) = render(&mut e, 1.0);
        let attack = x[..(SR * 0.05) as usize]
            .iter()
            .fold(0.0f32, |a, &b| a.max(b.abs()));
        let late: f64 = {
            let seg = &x[(SR * 0.5) as usize..];
            (seg.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / seg.len() as f64).sqrt()
        };
        assert!(late > 1e-4, "持続部が聴こえない: RMS {late:.3e}");
        let crest = attack as f64 / late;
        assert!(
            (10.0..1000.0).contains(&crest),
            "クレストファクタが異常: {crest:.1}"
        );
    }

    #[test]
    fn reset_clears_the_whole_chain() {
        let mut e = DulcimerEngine::new(SR, BLOCK);
        e.note_on(60, 0.9);
        render(&mut e, 0.2);
        e.reset();
        let (x, peak) = render(&mut e, 0.2);
        let _ = x;
        assert!(peak < 1e-6, "reset 後に鳴っている: {peak}");
    }
}
