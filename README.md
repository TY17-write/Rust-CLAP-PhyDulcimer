# Rust-CLAP-PhyDulcimer

Rust で書くハンマーダルシマー物理モデリング音源 (CLAP プラグイン)。

サンプル再生ではなく、**ハンマー・弦・ブリッジ・響板の物理を実時間で解く**。
対象はアメリカ式ハンマーダルシマー (標準 15/14)。

この楽器はピアノ音源のパラメータを差し替えたものにはならない。構造が 4 か所で違う。

- **ダンパーが無い** — ボイスプールを持たず、楽器の全弦を常時走らせる。
  打撃はボイスの確保ではなく、すでに振動している弦への力の注入
- **ブリッジが弦を 2:3 に分ける** — 1 本の弦が完全 5 度の関係にある 2 つの音高を持つ。
  ブリッジは接着されず張力だけで支えられているので、左右の区間は結合している
- **打弦点が固定されない** — 奏者がブリッジからの距離を選ぶ。主要な音色操作
- **X-Y ステレオの部屋を内蔵する** — 後段の汎用リバーブではない。楽器の幾何と
  部屋を同じ座標系に置き、定位をパンではなく幾何から出す

---

> ## 本プロジェクトについて
>
> **このプロジェクトは Anthropic の Claude Opus 5 によって作成されています。**
>
> 論文調査、アルゴリズム選定、アーキテクチャ設計、実装、テストのすべてが Opus 5 に
> よるものです。楽器の物理と手法選定の根拠は [`docs/research.md`](docs/research.md) に、
> 設計判断と意図的に諦めた近似は [`docs/problems.md`](docs/problems.md) に記録しています。
>
> 本プロジェクトが依拠したすべての参照元は、下記「[参照元](#参照元)」に列挙しています。

---

## 現在の状態

**Phase 0 (土台) 完了。** モデル本体はまだ無く、**測定器が信用できる状態**まで作った。

| Phase | 内容 | 状態 |
|---|---|---|
| 0 | ワークスペース骨組み / ドキュメント / オフラインレンダラ・解析ツール | ✅ 完了 |
| 1 | 1 区間の弦 + 硬いハンマー | 未着手 |
| 2 | CLAP 化 (clack) — 弾ける状態にする | 未着手 |
| 3 | 楽器全体 (配置表・全弦常時・ロール) | 未着手 |
| 4 | ブリッジ結合 (この音源の中身) | 未着手 |
| 5 | 響板と箱 | 未着手 |
| 6 | ROOM — X-Y ステレオ | 未着手 |
| 7 | 演奏表現 (打弦点・ハンマー面・ミュート) | 未着手 |
| 8 | 最適化 (SIMD) | 未着手 |
| 9 | GUI とプリセット | 未着手 |
| 10 | 音色の追い込み | 未着手 |

テスト 52 件、`cargo clippy` / `cargo fmt` ともにクリーン。

**計画の正は Artifact**: https://claude.ai/code/artifact/a650768b-6e46-4ba6-a022-0a3ab186990d
([`docs/plan.html`](docs/plan.html) はそのコピー)

## ビルド

```bash
cargo build --release --workspace
cargo test --workspace
```

## 使い方 (Phase 0)

現時点で鳴るのは疎通確認用の減衰正弦波だけ。`render` が書いた WAV を `analyze` が
**設計値どおりに読み取れるか**を確かめるためのもの。

```bash
# 単一の減衰正弦波
cargo run --release -p phydulcimer-render -- --out out/smoke.wav --freq 440 --t60 2.0 --dur 3.0
cargo run --release -p phydulcimer-analyze -- --in out/smoke.wav --f0 --t60

# 部分音列 (インハーモニシティと減衰の傾きつき)
cargo run --release -p phydulcimer-render -- --out out/tone.wav \
    --freq 220 --partials 8 --t60 3.0 --t60-taper 1.0 --inharmonicity 4e-4 --dur 4.0
cargo run --release -p phydulcimer-analyze -- --in out/tone.wav \
    --partials 220 --count 8 --partial-t60 --window 2.0
```

`-h` で全オプションが出る。

**`--peak` の既定は 0 (正規化しない)。** A/B 比較でレベルが揃って差が消える事故を
防ぐため、PhyPiano から意図的に変えている。聴くときだけ `--peak 0.9` を付ける。

### Phase 0 の実測値

| 項目 | 設計値 | 実測 |
|---|---|---|
| f0 推定 (自己相関) | 440 Hz | **440.00 Hz** |
| 信号全体の T60 | 2.0 s | **2.000 s** (R² = 0.9999) |
| 部分音 T60 (n = 1 / 2 / 4) | 3.0 / 1.5 / 0.75 s | **3.000 / 1.500 / 0.750 s** |
| 部分音位置 (n = 8, B = 4e-4) | +21.9 cent | **+22.2 cent** |
| インハーモニシティ B の推定 | 4.0e-4 | **3.878e-4** |
| WAV 往復 (32-bit float) | — | **標本が完全一致** |

## ドキュメント

| 文書 | 内容 |
|---|---|
| [`docs/plan.html`](docs/plan.html) | 全体計画とフェーズ構成 (Artifact が正) |
| [`docs/research.md`](docs/research.md) | 楽器の物理と手法選定の根拠、論文サーベイ |
| [`docs/problems.md`](docs/problems.md) | 既知の問題・意図的に諦めた近似 (D-001〜) |
| [`docs/context.md`](docs/context.md) | 現在地・判断の理由・実測値・ハマりどころ |
| [`CLAUDE.md`](CLAUDE.md) | 作業の決まりごと |

## 既知の制約

- **参照音源を持たない** ([D-006](docs/problems.md))。物理量が文献値の範囲に入ることが
  唯一の外部基準になる
- `--partial-t60` は T60 が 0.4 秒を切ると測れない ([D-001](docs/problems.md))
- 部分音が混在すると、上記の下限より長い T60 でも落ちることがある。原因未特定
  ([D-002](docs/problems.md))
- 響板は板のシミュレーションではなくフィルタ近似 (Phase 5 で実装予定)

## 参照元

### ハンマーダルシマーの音響

- D. Peterson, "The acoustics of the hammered dulcimer and similar instruments,"
  *JASA*, vol. 120, no. 5 (Suppl.), 2006.
- D. Peterson, "Acoustics of the hammered dulcimer, its history, and recent developments,"
  *JASA*, vol. 95, no. 5 (Suppl.), 1994.
- C. T. Vongsawad et al., "Use of the hammered dulcimer to demonstrate physical
  acoustics principles," *JASA*, vol. 135, no. 4 (Suppl.), 2014.
- "Modal response and sound radiation from a hammered dulcimer,"
  *Proceedings of Meetings on Acoustics*, vol. 14, 035001.

### 打弦弦の合成手法

- B. Bank, S. Zambon, F. Fontana, "A Modal-Based Real-Time Piano Synthesizer,"
  *IEEE TASLP*, vol. 18, no. 4, 2010.
- B. Bank and J. Chabassier, "Model-Based Digital Pianos: From Physics to Sound
  Synthesis," *IEEE Signal Processing Magazine*, vol. 36, no. 1, 2019.
- A. Stulov, "Hysteretic model of the grand piano hammer felt," *JASA*, 1995.
- G. Weinreich, "Coupled piano strings," *JASA*, 1977.

### 箱・音孔・ステレオ録音

- O. Christensen and B. B. Vistisen, "Simple model for low-frequency guitar function,"
  *JASA*, vol. 68, 1980.
- A. D. Blumlein, British Patent 394,325 (1931).
- M. Williams, "Unified theory of microphone systems for stereophonic sound recording,"
  *AES 82nd Convention*, 1987.
- J. Dattorro, "Effect design, Part 1: Reverberator and other filters," *JAES*, vol. 45, no. 9, 1997.

**全文献と、そこから何を採ったかは [`docs/research.md`](docs/research.md) にある。**

### 実装の供給元

- [`Rust-CLAP-PhyPiano`](../Rust-CLAP-PhyPiano) — モーダル共振器・ハンマー・響板・
  検証ツールの供給元。**コピーして独立させている** (パス依存は張らない)
- [clack](https://github.com/prokopyl/clack) — Rust の CLAP バインディング (Phase 2 で使用)
- [hound](https://github.com/ruuda/hound) — WAV の読み書き

## ライセンス

MIT OR Apache-2.0
