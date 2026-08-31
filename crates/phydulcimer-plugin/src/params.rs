//! パラメータ定義とスレッド間共有。
//!
//! パラメータはメインスレッド (ホストの UI・オートメーション) とオーディオ
//! スレッドの両方から触られる。ロックを取れないので、アトミックに読み書きする。
//!
//! Phase 2 は最小の 2 つだけ。撥の面 (Phase 7)・ROOM (Phase 6) などは
//! そのフェーズで足す。

use std::sync::atomic::{AtomicU32, Ordering};

/// パラメータの識別子。
///
/// **公開後は変更しないこと。** ホストのオートメーションとプリセットがこの ID に
/// 紐づいているため、変えると既存プロジェクトの設定が壊れる。
pub mod id {
    pub const LEVEL: u32 = 1;
    pub const STRIKE_POSITION: u32 = 2;
}

/// 1つのパラメータの仕様。
pub struct ParamSpec {
    pub id: u32,
    pub name: &'static [u8],
    pub min: f64,
    pub max: f64,
    pub default: f64,
    /// 表示に使う単位 (空文字なら無単位)
    pub unit: &'static str,
    /// 表示の小数点以下桁数
    pub decimals: usize,
}

/// 全パラメータ。ホストへ列挙する順序でもある。
pub const PARAMS: &[ParamSpec] = &[
    ParamSpec {
        id: id::LEVEL,
        name: b"Level",
        min: 0.0,
        max: 1.0,
        default: 0.7,
        unit: "",
        decimals: 2,
    },
    ParamSpec {
        id: id::STRIKE_POSITION,
        name: b"Strike Position",
        // 実機の奏者はブリッジから 25–50 mm を叩く (x/L = 0.05–0.15)。
        // 音色変化を確かめられるよう広めに取る。次の打撃から効く。
        min: 0.03,
        max: 0.30,
        default: 0.09,
        unit: "",
        decimals: 3,
    },
];

/// `id` に対応する仕様を返す。
pub fn spec(id: u32) -> Option<&'static ParamSpec> {
    PARAMS.iter().find(|p| p.id == id)
}

/// f32 をアトミックに読み書きするヘルパー。
#[derive(Debug)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn new(value: f32) -> Self {
        Self(AtomicU32::new(value.to_bits()))
    }

    #[inline]
    pub fn store(&self, value: f32) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }

    #[inline]
    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// 全パラメータの現在値。メインスレッドとオーディオスレッドで共有する。
#[derive(Debug)]
pub struct ParamValues {
    pub level: AtomicF32,
    pub strike_position: AtomicF32,
}

impl Default for ParamValues {
    fn default() -> Self {
        Self::new()
    }
}

impl ParamValues {
    pub fn new() -> Self {
        let default_of = |id: u32| spec(id).map(|s| s.default as f32).unwrap_or(0.0);
        Self {
            level: AtomicF32::new(default_of(id::LEVEL)),
            strike_position: AtomicF32::new(default_of(id::STRIKE_POSITION)),
        }
    }

    /// ホストからの値を該当パラメータへ書き込む。範囲外はクランプする。
    pub fn set(&self, id: u32, value: f64) {
        let Some(spec) = spec(id) else { return };
        let v = value.clamp(spec.min, spec.max) as f32;
        match id {
            id::LEVEL => self.level.store(v),
            id::STRIKE_POSITION => self.strike_position.store(v),
            _ => {}
        }
    }

    /// 現在値を取り出す。未知の ID なら `None`。
    pub fn get(&self, id: u32) -> Option<f64> {
        Some(match id {
            id::LEVEL => self.level.load() as f64,
            id::STRIKE_POSITION => self.strike_position.load() as f64,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_param_has_a_spec_and_a_sane_default() {
        for p in PARAMS {
            assert!(p.min < p.max, "{:?}: min >= max", p.name);
            assert!(
                (p.min..=p.max).contains(&p.default),
                "{:?}: default が範囲外",
                p.name
            );
            assert!(spec(p.id).is_some());
        }
    }

    #[test]
    fn param_ids_are_unique() {
        for (i, a) in PARAMS.iter().enumerate() {
            for b in &PARAMS[i + 1..] {
                assert_ne!(a.id, b.id, "パラメータ ID が重複している: {}", a.id);
            }
        }
    }

    #[test]
    fn defaults_round_trip() {
        let values = ParamValues::new();
        for p in PARAMS {
            let got = values.get(p.id).expect("既知の ID");
            assert!(
                (got - p.default).abs() < 1e-6,
                "{:?}: {got} != {}",
                p.name,
                p.default
            );
        }
    }

    #[test]
    fn set_clamps_to_range() {
        let values = ParamValues::new();
        values.set(id::LEVEL, 99.0);
        assert_eq!(values.get(id::LEVEL), Some(1.0));
        values.set(id::LEVEL, -5.0);
        assert_eq!(values.get(id::LEVEL), Some(0.0));

        values.set(id::STRIKE_POSITION, 0.0);
        // 値は f32 で保持されるので、min (0.03) との比較は丸め誤差を許す。
        let clamped = values.get(id::STRIKE_POSITION).unwrap();
        assert!((clamped - 0.03).abs() < 1e-6, "clamped = {clamped}");
    }

    #[test]
    fn unknown_id_is_ignored() {
        let values = ParamValues::new();
        values.set(9999, 0.5);
        assert_eq!(values.get(9999), None);
        assert!(spec(9999).is_none());
    }
}
