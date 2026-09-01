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
    pub const ROOM: u32 = 3;
    pub const MIC_DISTANCE: u32 = 4;
    pub const XY_ANGLE: u32 = 5;
    pub const ROOM_SIZE: u32 = 6;
    pub const ABSORPTION: u32 = 7;
    pub const HAMMER_FACE: u32 = 8;
    pub const MUTE: u32 = 9;
    pub const TEMPERAMENT: u32 = 10;
    pub const LAYOUT: u32 = 11;
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
    ParamSpec {
        id: id::ROOM,
        // X-Y ステレオの部屋。DAW 側で空間を作りたいときのために切れる。
        // **音質の測定・調整は必ず off で行うこと** (部屋は粗を隠す)。
        name: b"Room",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        unit: "",
        decimals: 0,
    },
    ParamSpec {
        id: id::MIC_DISTANCE,
        // これ 1 本でタイト ⇔ アンビエントが出る (直接音だけが 1/d で落ちる)。
        name: b"Mic Distance",
        min: 0.3,
        max: 3.0,
        default: 1.2,
        unit: " m",
        decimals: 2,
    },
    ParamSpec {
        id: id::XY_ANGLE,
        // X-Y の開き角。実物どおりは 90°。幅が控えめなのは方式の音。
        name: b"X-Y Angle",
        min: 60.0,
        max: 135.0,
        default: 90.0,
        unit: " deg",
        decimals: 0,
    },
    ParamSpec {
        id: id::ROOM_SIZE,
        // 0 = Small, 1 = Medium, 2 = Large。
        name: b"Room Size",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        unit: "",
        decimals: 0,
    },
    ParamSpec {
        id: id::ABSORPTION,
        // 壁の吸音率。RT60 と高域の落ち方を決める。
        name: b"Wall Absorption",
        min: 0.0,
        max: 0.9,
        default: 0.35,
        unit: "",
        decimals: 2,
    },
    ParamSpec {
        id: id::HAMMER_FACE,
        // 0 = Wood, 1 = Leather, 2 = Felt (Phase 7)。実機の撥は裏表で面が
        // 違い、奏者が持ち替える。次の打撃から効く。
        name: b"Hammer Face",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        unit: "",
        decimals: 0,
    },
    ParamSpec {
        id: id::MUTE,
        // パームミュート (Phase 7)。0 = 開放、1 = 手のひらで押さえ切る。
        // 鳴っている弦に即座に効く (高い部分音から先に止まる)。
        name: b"Mute",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        decimals: 2,
    },
    ParamSpec {
        id: id::TEMPERAMENT,
        // 0 = Pure Fifth (2:3 ブリッジ、左が +2 cent)、1 = Equal (平均律の5度)。
        // ブリッジ位置 = 弦の設計が変わるので **activate 時に適用** (Phase 7)。
        name: b"Temperament",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        decimals: 0,
    },
    ParamSpec {
        id: id::LAYOUT,
        // 0 = Diatonic 15/14 (G2-D6, 27 音)、1 = Chromatic D#2-E6 (50 音、
        // ブロンズ巻低音弦ブロック込み)。
        // 弦バンクの再構築を伴うので **activate 時に適用** (Phase 7)。
        name: b"Layout",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        decimals: 0,
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
    pub room: AtomicF32,
    pub mic_distance: AtomicF32,
    pub xy_angle: AtomicF32,
    pub room_size: AtomicF32,
    pub absorption: AtomicF32,
    pub hammer_face: AtomicF32,
    pub mute: AtomicF32,
    pub temperament: AtomicF32,
    pub layout: AtomicF32,
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
            room: AtomicF32::new(default_of(id::ROOM)),
            mic_distance: AtomicF32::new(default_of(id::MIC_DISTANCE)),
            xy_angle: AtomicF32::new(default_of(id::XY_ANGLE)),
            room_size: AtomicF32::new(default_of(id::ROOM_SIZE)),
            absorption: AtomicF32::new(default_of(id::ABSORPTION)),
            hammer_face: AtomicF32::new(default_of(id::HAMMER_FACE)),
            mute: AtomicF32::new(default_of(id::MUTE)),
            temperament: AtomicF32::new(default_of(id::TEMPERAMENT)),
            layout: AtomicF32::new(default_of(id::LAYOUT)),
        }
    }

    /// ホストからの値を該当パラメータへ書き込む。範囲外はクランプする。
    pub fn set(&self, id: u32, value: f64) {
        let Some(spec) = spec(id) else { return };
        let v = value.clamp(spec.min, spec.max) as f32;
        match id {
            id::LEVEL => self.level.store(v),
            id::STRIKE_POSITION => self.strike_position.store(v),
            id::ROOM => self.room.store(v),
            id::MIC_DISTANCE => self.mic_distance.store(v),
            id::XY_ANGLE => self.xy_angle.store(v),
            id::ROOM_SIZE => self.room_size.store(v),
            id::ABSORPTION => self.absorption.store(v),
            id::HAMMER_FACE => self.hammer_face.store(v),
            id::MUTE => self.mute.store(v),
            id::TEMPERAMENT => self.temperament.store(v),
            id::LAYOUT => self.layout.store(v),
            _ => {}
        }
    }

    /// 現在値を取り出す。未知の ID なら `None`。
    pub fn get(&self, id: u32) -> Option<f64> {
        Some(match id {
            id::LEVEL => self.level.load() as f64,
            id::STRIKE_POSITION => self.strike_position.load() as f64,
            id::ROOM => self.room.load() as f64,
            id::MIC_DISTANCE => self.mic_distance.load() as f64,
            id::XY_ANGLE => self.xy_angle.load() as f64,
            id::ROOM_SIZE => self.room_size.load() as f64,
            id::ABSORPTION => self.absorption.load() as f64,
            id::HAMMER_FACE => self.hammer_face.load() as f64,
            id::MUTE => self.mute.load() as f64,
            id::TEMPERAMENT => self.temperament.load() as f64,
            id::LAYOUT => self.layout.load() as f64,
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
