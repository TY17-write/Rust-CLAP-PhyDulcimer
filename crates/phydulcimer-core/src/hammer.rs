//! 非線形ハンマーモデル。
//!
//! ハンマーダルシマーの撥は**木の棒** (メイプル・チェリー・オーク・ウォルナット)。
//! 片面を素の木のまま、もう片面をレザーやピアノフェルトで覆うものが多く、
//! **奏者は面を使い分けて音色を変える**。
//!
//! 奏法は「drop and bounce」— 6–12 inch の高さから落として重力で当て、素早く
//! 跳ね返して接触時間を最小化する。落下高さから逆算すると弦に当たる速度は
//! 1.7–2.4 m/s、強打はもっと速い。
//!
//! 接触の非線形性が**強打ほど倍音が豊かになる**挙動を生むのはピアノと同じで、
//! サンプラーがベロシティレイヤーで階段的に近似している部分が連続的に出る。
//!
//! # ピアノのフェルトとの違い
//!
//! | | ピアノのフェルト | ダルシマーの木 |
//! |---|---|---|
//! | 剛性 `K` | 4.5e9 | **桁で硬い** |
//! | 指数 `p` | 2.2–3.5 (圧縮するほど硬くなる) | **1.5 前後** (ヘルツ接触。木は圧縮硬化しない) |
//! | 接触時間 | 0.5–4 ms | **0.1–1 ms** |
//!
//! `p` が 1 に近いぶん**接触時間のベロシティ依存が弱い**。実機の硬い撥もそう
//! 振る舞う。接触が短いので、PhyPiano が Phase 9 で苦しんだ「接触時間が弦の
//! 1 周期を超えて基音より上を励振できない」問題は起きにくい。
//!
//! # 力の式 (Hunt-Crossley 型)
//!
//! ```text
//! F = K·Δ^p·(1 + λ·Δ̇)     (Δ > 0 のとき)
//! F = 0                     (Δ ≤ 0 のとき、フェルトは引っ張れない)
//! ```
//!
//! `Δ = y_h − y_s` は圧縮量、`K` は剛性、`p ≈ 2.2–3.5` は非線形指数
//! (低音ほど小さい)、`λ` はヒステリシス項の係数。
//!
//! Stulov の緩和関数モデルのほうが実測フェルトには忠実だが、内部状態を
//! 1つ余分に持つ必要がある。Hunt-Crossley は速度に比例する減衰項ひとつで
//! ヒステリシスループを作れて、エネルギー的にも散逸的 (安定) なので、
//! リアルタイム前提の本プロジェクトではこちらを採る。
//!
//! # 数値積分
//!
//! 半陰的 (シンプレクティック) オイラー法で積分する。速度を先に更新して
//! から位置に反映するので、陽的オイラーと同じコストでエネルギーの振る舞いが
//! ずっと素直になる。
//!
//! **接触が短いぶん刻みが要る。** 48 kHz では 0.2 ms の接触が 10 サンプルしか
//! ない。そのため**接触している間だけ**オーバーサンプリングする (接触は 1 ms
//! 未満なので、全体のコストにはほぼ効かない)。
//!
//! **倍率は決め打ちにしない。** 硬い撥に 4 倍で足りるかは測らないと分からないので、
//! [`Segment`](crate::segment::Segment) 側で可変にしてある。Phase 1 の実測結果は
//! `docs/problems.md` の D-010。

/// ハンマーの物理パラメータ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HammerParams {
    /// ハンマー質量 [kg]。実機で 4–12 g、低音ほど重い
    pub mass_kg: f64,
    /// フェルト剛性 K [N/m^p]
    pub stiffness: f64,
    /// 非線形指数 p。低音で 2.2 前後、高音で 3.0 前後
    pub exponent: f64,
    /// ヒステリシス係数 λ [s/m]
    pub hysteresis: f64,
    /// 弦に当たる撥の面の幅 [m]
    ///
    /// # なぜ要るか
    ///
    /// **撥は点ではなく面で弦を押す。** 幅より短い波長の部分音は接触面の中で
    /// 打ち消し合い、励振されない。効き始めるのは第 `2L/(πw)` 部分音あたり。
    ///
    /// **ダルシマーの区間は短い** (330–495 mm) ので、同じ幅でもピアノより低い
    /// 部分音から効く。木の面 8 mm・区間 495 mm なら第 39 部分音 (11.5 kHz)、
    /// フェルト面 12 mm なら第 26 部分音 (7.7 kHz)。
    ///
    /// 面を持ち替えると幅も変わる。**「木は明るい」の一部はここから出る** —
    /// 剛性だけではない。
    ///
    /// 重みは [`ModalBank`](crate::modal::ModalBank) の励振重みに掛かるだけなので
    /// **実行時のコストは増えない** (打弦時に 1 回計算する)。
    pub hammer_width_m: f64,
    /// フェルトの記憶の深さ `h_r` (0–1)。**この楽器では既定 0 (無効)**
    ///
    /// # 何のためにあるか
    ///
    /// `F = K·Δ^p·(1 + λ·Δ̇)` は圧縮量の**瞬時の**関数なので、力の波形が
    /// 滑らかな山になる。実際のフェルトは圧縮の履歴を引きずり、**押し込むときと
    /// 戻るときで力が違う** (ヒステリシスループを描く)。
    ///
    /// Stulov (JASA 1995) の緩和モデル:
    ///
    /// ```text
    /// F(t) = K·[ Δ(t)^p − (h_r/τ)·∫₀ᵗ exp(−(t−ξ)/τ)·Δ(ξ)^p dξ ]
    /// ```
    ///
    /// 畳み込みは 1 次の状態変数 1 個で書けるので、実行時のコストはほぼ増えない。
    ///
    /// # なぜ既定で切ってあるか
    ///
    /// PhyPiano ではこれが**必須**だった。市販音源と比べて 1–4 kHz が足りず、
    /// フェルトを硬くすれば数値は合うが接触時間が実機から外れる — 足りないのは
    /// 接触時間の短さではなく**励振波形の鋭さ**で、それを作るのがこの項だった。
    ///
    /// **こちらは素の木の撥が最初から鋭い。** 同じ問題が起きるとは限らないので、
    /// 入れずに始める。必要になるとすればフェルト面を作るときで、そのとき
    /// 測ってから入れる。
    pub felt_memory: f64,
    /// 緩和時間 τ [s]。文献値は接触時間と同程度のオーダー
    pub relaxation_sec: f64,
}

/// 撥の当たり面。奏者が持ち替えて音色を変える。
///
/// 実機の撥は裏表で面が違うことが多く、**演奏中に返して使う**。
/// 音源としてはこれをパラメータに出す (Phase 7)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HammerFace {
    /// 素の木。硬く、接触が短く、明るい
    #[default]
    Wood,
    /// レザーを巻いた面
    Leather,
    /// ピアノフェルトを貼った面。最も柔らかい
    Felt,
}

impl HammerParams {
    /// 素の木の面。**この音源の既定。**
    ///
    /// 質量はピアノのハンマー (4–12 g) より軽く見積もっている。ダルシマーの撥は
    /// 細い木の棒で、打点での実効質量は棒全体の重さではない。
    ///
    /// **`stiffness` は Phase 1 で接触時間を測って決めた値。** 物理定数として
    /// 独立に測ったものではない (→ `docs/problems.md` の D-011)。
    pub fn wood() -> Self {
        Self {
            mass_kg: 5.0e-3,
            // 剛体壁で 0.212 ms / ピーク 173 N (v = 2 m/s)。ピーク力が弦の張力
            // (130 N) と同オーダーに収まる。→ D-011
            stiffness: 1.0e8,
            // ヘルツ接触。木は圧縮しても硬くならないので p は 1.5 に近い。
            exponent: 1.5,
            // 木は散逸が少ない。フェルトの 1/10 のオーダー。
            hysteresis: 1.0e-5,
            hammer_width_m: 0.008,
            felt_memory: 0.0,
            relaxation_sec: 3.0e-4,
        }
    }

    /// レザーを巻いた面。木とフェルトの中間。
    pub fn leather() -> Self {
        Self {
            mass_kg: 5.5e-3,
            // 剛体壁で 0.672 ms / ピーク 69 N (v = 2 m/s)。
            stiffness: 3.0e8,
            exponent: 2.0,
            hysteresis: 6.0e-5,
            hammer_width_m: 0.010,
            felt_memory: 0.0,
            relaxation_sec: 5.0e-4,
        }
    }

    /// ピアノフェルトを貼った面。
    ///
    /// 係数は PhyPiano の中音域の値そのまま (Chaigne & Askenfelt / Stulov 系の
    /// 文献で報告されているオーダー)。実機でもピアノ用のフェルトを貼る。
    pub fn felt() -> Self {
        Self {
            mass_kg: 6.0e-3,
            stiffness: 4.5e9,
            exponent: 2.5,
            hysteresis: 1.0e-4,
            hammer_width_m: 0.012,
            felt_memory: 0.0,
            relaxation_sec: 1.0e-3,
        }
    }

    /// 面から係数を引く。
    pub fn for_face(face: HammerFace) -> Self {
        match face {
            HammerFace::Wood => Self::wood(),
            HammerFace::Leather => Self::leather(),
            HammerFace::Felt => Self::felt(),
        }
    }
}

impl Default for HammerParams {
    fn default() -> Self {
        Self::wood()
    }
}

/// ハンマーの状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HammerState {
    /// まだ打弦されていない
    Idle,
    /// 弦へ向かって飛行中 (接触していない)。**再接触の待機もここ**
    Approaching,
    /// 弦に接触中
    InContact,
    /// 弦から遠ざかる向きで離れた。以降この打撃では力を出さない
    Released,
}

/// 打弦から撥が去るまでの安全上限 [s]。
///
/// 再接触を許すので (下記 `step` 参照)、理論上は撥が弦の近くに留まり続けられる。
/// 接触中はオーバーサンプルが走るため、その窓が無限に続かないようここで打ち切る。
/// 実測では遠端からの反射 (数 ms) が撥を跳ね返すので、この上限に達するのは異常系だけ。
const MAX_ACTIVE_SEC: f64 = 0.02;

/// 非線形ハンマー。
#[derive(Debug, Clone)]
pub struct Hammer {
    params: HammerParams,
    /// ハンマー位置 [m]。弦の静止位置を 0 とし、弦を押し込む向きを正とする
    position: f64,
    /// ハンマー速度 [m/s]
    velocity: f64,
    state: HammerState,
    /// 接触していた時間の合計 [s]。再接触があるので、連続とは限らない
    contact_time: f64,
    /// 打弦してからの経過時間 [s]。[`MAX_ACTIVE_SEC`] の打ち切りに使う
    active_time: f64,
    /// Stulov の緩和項の状態 (過去の圧縮の名残)。深さが 0 なら使わない
    felt_state: f64,
}

impl Hammer {
    pub fn new(params: HammerParams) -> Self {
        Self {
            params,
            position: 0.0,
            velocity: 0.0,
            felt_state: 0.0,
            state: HammerState::Idle,
            contact_time: 0.0,
            active_time: 0.0,
        }
    }

    pub fn params(&self) -> &HammerParams {
        &self.params
    }

    pub fn set_params(&mut self, params: HammerParams) {
        self.params = params;
    }

    pub fn state(&self) -> HammerState {
        self.state
    }

    /// 接触中か。呼び出し側はこれを見てオーバーサンプリングを切り替える。
    #[inline]
    pub fn is_active(&self) -> bool {
        matches!(
            self.state,
            HammerState::Approaching | HammerState::InContact
        )
    }

    /// 接触が続いている時間 [s]。離れた後は最終的な接触時間が残る。
    pub fn contact_duration(&self) -> f64 {
        self.contact_time
    }

    /// 現在の撥の速度 [m/s]。接触後は負 (跳ね返っている)。
    ///
    /// **反発係数の確認に使う。** 木の撥は散逸が少ないので、剛体壁なら
    /// 打撃速度の 8 割以上で跳ね返るはず。ここが極端に小さいなら、
    /// ヒステリシス項が効きすぎているか、刻みが粗くて接触を解像できていない。
    pub fn velocity(&self) -> f64 {
        self.velocity
    }

    /// 打鍵。`velocity_mps` は弦に当たる直前のハンマー速度 [m/s]。
    ///
    /// 実機では pp で 0.5 m/s 程度、ff で 6 m/s 程度。
    pub fn strike(&mut self, velocity_mps: f64) {
        self.position = 0.0;
        self.velocity = velocity_mps.max(0.0);
        self.state = if self.velocity > 0.0 {
            HammerState::Approaching
        } else {
            HammerState::Idle
        };
        self.contact_time = 0.0;
        self.active_time = 0.0;
        self.felt_state = 0.0;
    }

    /// 打弦前の状態に戻す。
    pub fn reset(&mut self) {
        self.position = 0.0;
        self.velocity = 0.0;
        self.state = HammerState::Idle;
        self.contact_time = 0.0;
        self.active_time = 0.0;
        self.felt_state = 0.0;
    }

    /// 1ステップ進め、弦に加わる力 [N] を返す。
    ///
    /// # 引数
    /// - `string_displacement`: 打弦点での弦の変位 [m]
    /// - `string_velocity`: 打弦点での弦の速度 [m/s]
    /// - `dt`: ステップ幅 [s] (オーバーサンプル中は細かい値)
    ///
    /// 返す力は常に非負。フェルトは押すことしかできない。
    pub fn step(&mut self, string_displacement: f64, string_velocity: f64, dt: f64) -> f64 {
        if !self.is_active() {
            return 0.0;
        }

        // 再接触を許すぶん、活動窓が閉じる保証が撥自身には無い。オーバーサンプルの
        // コストを有界にするため、時間で打ち切る。実測ではとうに跳ね返っている。
        self.active_time += dt;
        if self.active_time >= MAX_ACTIVE_SEC {
            self.state = HammerState::Released;
            return 0.0;
        }

        let compression = self.position - string_displacement;
        let force = if compression > 0.0 {
            self.state = HammerState::InContact;
            self.contact_time += dt;

            let rate = self.velocity - string_velocity;
            // powf は f64 で計算する。p が非整数なので整数冪には落とせない。
            let compressed = compression.powf(self.params.exponent);

            // フェルトの記憶 (Stulov)。`compressed` を時定数 τ で 1 次ローパスに
            // 通したものが「過去の圧縮の名残」で、それを差し引く。
            //
            // 押し込む間は名残が実際の圧縮に追いつかないので差が大きく残り、
            // **力の立ち上がりが鋭くなる**。戻るときは名残のほうが大きくなって
            // 力を早めに削るので、波形が前傾する。接触時間はほとんど変わらない
            // まま高域だけが増える。
            let elastic = if self.params.felt_memory > 0.0 {
                let tau = self.params.relaxation_sec.max(1e-9);
                // 指数移動平均。dt/τ が 1 を超えても発散しないようクランプする。
                let alpha = (dt / tau).min(1.0);
                self.felt_state += (compressed - self.felt_state) * alpha;
                let memory = self.params.felt_memory.clamp(0.0, 1.0);
                // 記憶を引きすぎて力が負になると「フェルトが弦を引っ張る」ことに
                // なるので、0 で止める。
                self.params.stiffness * (compressed - memory * self.felt_state).max(0.0)
            } else {
                self.params.stiffness * compressed
            };

            // ヒステリシス項が −1 を下回ると力が負になる (フェルトが引っ張る) ので
            // クランプする。復元中に力が 0 に達したら、そこで接触は終わっている。
            (elastic * (1.0 + self.params.hysteresis * rate)).max(0.0)
        } else {
            if self.state == HammerState::InContact {
                // 離れた。**まだ弦へ向かっているなら再接触を許す。**
                //
                // ピアノ (PhyPiano) は最初の離脱で Released に落としていた。
                // フェルトは接触が 1–4 ms あり 1 回で運動量を渡しきるので、
                // それで正しかった。木の撥は違う: 弦のほうが軽くて速く
                // 「逃げられる」ため、最初の接触は 20 µs 程度で切れ、撥は
                // ほぼ減速しないまま弦に向かって進み続ける。実機の撥は
                // 弦の戻りに叩き返されるまで複数回触れる (→ D-012)。
                //
                // 弦から遠ざかる向き (velocity < 0) で離れたときだけ終わる。
                self.state = if self.velocity > 0.0 {
                    HammerState::Approaching
                } else {
                    HammerState::Released
                };
            }
            0.0
        };

        // 半陰的オイラー: 速度を先に更新し、新しい速度で位置を進める。
        // 弦からの反作用 −F がハンマーを減速させ、最終的に押し返す。
        self.velocity -= force / self.params.mass_kg * dt;
        self.position += self.velocity * dt;

        force
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;
    /// 接触中のオーバーサンプル倍率。
    ///
    /// 木の撥は接触が 0.2 ms 前後しかなく、48 kHz では 10 サンプル。
    /// ここでの倍率はテストが接触を解像できる値にしてある。実際に使う倍率は
    /// [`Segment`](crate::segment::Segment) 側で決める。
    const OS: f64 = 8.0;
    const DT: f64 = 1.0 / (SR * OS);

    /// 弦を「動かない壁」として扱い、接触の様子だけを観測する。
    /// 返り値は (接触時間 [s], 最大力 [N], 跳ね返り速度 [m/s])。
    fn strike_rigid_wall(velocity: f64) -> (f64, f64, f64) {
        let mut h = Hammer::new(HammerParams::wood());
        h.strike(velocity);

        let mut peak_force = 0.0f64;
        // 接触が終わるまで最大 20 ms 回す。
        for _ in 0..(0.020 / DT) as usize {
            let f = h.step(0.0, 0.0, DT);
            peak_force = peak_force.max(f);
            if h.state() == HammerState::Released {
                break;
            }
        }
        (h.contact_duration(), peak_force, h.velocity)
    }

    #[test]
    fn hammer_bounces_off_a_rigid_wall() {
        let (duration, peak, rebound) = strike_rigid_wall(2.0);
        assert!(duration > 0.0, "接触が起きていない");
        assert!(peak > 0.0, "力が発生していない");
        assert!(rebound < 0.0, "跳ね返っていない (v = {rebound})");
    }

    #[test]
    fn contact_time_is_physically_plausible() {
        // 木の撥の接触時間は 0.1–1.0 ms (Phase 1 の完了条件)。
        for v in [0.5, 1.0, 2.0, 4.0, 6.0] {
            let (duration, _, _) = strike_rigid_wall(v);
            let ms = duration * 1000.0;
            assert!(
                (0.1..=1.0).contains(&ms),
                "v={v} m/s で接触時間 {ms:.3} ms は木の撥として不自然"
            );
        }
    }

    /// 面ごとの接触時間 [ms] を測る。
    fn contact_ms(params: HammerParams, velocity: f64) -> f64 {
        let mut h = Hammer::new(params);
        h.strike(velocity);
        for _ in 0..(0.020 / DT) as usize {
            h.step(0.0, 0.0, DT);
            if h.state() == HammerState::Released {
                break;
            }
        }
        h.contact_duration() * 1000.0
    }

    #[test]
    fn faces_are_ordered_from_hard_to_soft() {
        // 木 < レザー < フェルト の順に接触時間が伸びる。奏者が面を返して
        // 音色を変えるという操作が、この順序として出る。
        let wood = contact_ms(HammerParams::wood(), 2.0);
        let leather = contact_ms(HammerParams::leather(), 2.0);
        let felt = contact_ms(HammerParams::felt(), 2.0);

        assert!(
            wood < leather && leather < felt,
            "接触時間の順序が壊れている: wood={wood:.3} leather={leather:.3} felt={felt:.3}"
        );
        // フェルト面はピアノと同じ係数なので、ピアノの範囲 (0.5–4 ms) に入る。
        assert!(
            (0.5..=4.0).contains(&felt),
            "フェルト面の接触時間 {felt:.3} ms がピアノの範囲から外れている"
        );
    }

    #[test]
    fn for_face_matches_the_constructors() {
        assert_eq!(
            HammerParams::for_face(HammerFace::Wood),
            HammerParams::wood()
        );
        assert_eq!(
            HammerParams::for_face(HammerFace::Leather),
            HammerParams::leather()
        );
        assert_eq!(
            HammerParams::for_face(HammerFace::Felt),
            HammerParams::felt()
        );
        // 既定は木。
        assert_eq!(HammerParams::default(), HammerParams::wood());
        assert_eq!(HammerFace::default(), HammerFace::Wood);
    }

    #[test]
    fn harder_strikes_have_shorter_contact() {
        // 強打ほど接触時間が短くなる = 高次倍音が励振される。ピアノの音色変化の源。
        let velocities = [0.5, 1.0, 2.0, 4.0, 6.0];
        let durations: Vec<f64> = velocities.iter().map(|&v| strike_rigid_wall(v).0).collect();

        for w in durations.windows(2) {
            assert!(w[1] < w[0], "接触時間が単調減少していない: {durations:?}");
        }
    }

    #[test]
    fn harder_strikes_produce_more_force() {
        let peaks: Vec<f64> = [0.5, 1.0, 2.0, 4.0, 6.0]
            .iter()
            .map(|&v| strike_rigid_wall(v).1)
            .collect();
        for w in peaks.windows(2) {
            assert!(w[1] > w[0], "力が単調増加していない: {peaks:?}");
        }
    }

    #[test]
    fn force_is_never_negative() {
        let mut h = Hammer::new(HammerParams::wood());
        h.strike(3.0);
        // 弦が激しく動いている状況を模す。ヒステリシス項が効いても力は負にならない。
        for k in 0..2_000 {
            let y = 1e-4 * (k as f64 * 0.3).sin();
            let v = 1e-4 * 0.3 / DT * (k as f64 * 0.3).cos();
            let f = h.step(y, v, DT);
            assert!(f >= 0.0, "step {k} で負の力 {f}");
            assert!(f.is_finite(), "step {k} で非有限の力 {f}");
        }
    }

    #[test]
    fn released_hammer_stays_silent() {
        let mut h = Hammer::new(HammerParams::wood());
        h.strike(2.0);
        for _ in 0..(0.020 / DT) as usize {
            h.step(0.0, 0.0, DT);
        }
        assert_eq!(h.state(), HammerState::Released);
        // 離れた後は弦がどう動こうと力ゼロ。
        for _ in 0..1_000 {
            assert_eq!(h.step(-1e-3, 1.0, DT), 0.0);
        }
    }

    #[test]
    fn zero_velocity_strike_does_nothing() {
        let mut h = Hammer::new(HammerParams::wood());
        h.strike(0.0);
        assert_eq!(h.state(), HammerState::Idle);
        assert_eq!(h.step(0.0, 0.0, DT), 0.0);
    }

    #[test]
    fn idle_hammer_produces_no_force() {
        let mut h = Hammer::new(HammerParams::wood());
        assert_eq!(h.state(), HammerState::Idle);
        assert_eq!(h.step(0.0, 0.0, DT), 0.0);
    }

    #[test]
    fn reset_returns_to_idle() {
        let mut h = Hammer::new(HammerParams::wood());
        h.strike(2.0);
        h.step(0.0, 0.0, DT);
        h.reset();
        assert_eq!(h.state(), HammerState::Idle);
        assert_eq!(h.contact_duration(), 0.0);
    }

    #[test]
    fn softer_felt_gives_longer_contact() {
        // 剛性を下げると接触時間が伸びる = より暗い音になる。ウナコルダの原理。
        let stiff = {
            let mut h = Hammer::new(HammerParams::wood());
            h.strike(2.0);
            for _ in 0..(0.020 / DT) as usize {
                h.step(0.0, 0.0, DT);
                if h.state() == HammerState::Released {
                    break;
                }
            }
            h.contact_duration()
        };

        let soft = {
            let mut p = HammerParams::wood();
            p.stiffness *= 0.25;
            let mut h = Hammer::new(p);
            h.strike(2.0);
            for _ in 0..(0.020 / DT) as usize {
                h.step(0.0, 0.0, DT);
                if h.state() == HammerState::Released {
                    break;
                }
            }
            h.contact_duration()
        };

        assert!(
            soft > stiff,
            "柔らかいフェルトのほうが接触時間が長いはず: soft={soft}, stiff={stiff}"
        );
    }
}
