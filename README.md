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

**Phase 3 (楽器全体) 完了。** 15/14 の実機配置と設計則が入った。

| Phase | 内容 | 状態 |
|---|---|---|
| 0 | ワークスペース骨組み / ドキュメント / オフラインレンダラ・解析ツール | ✅ 完了 |
| 1 | 1 区間の弦 + 硬い撥 | ✅ 完了 |
| 2 | CLAP 化 (clack) — 弾ける状態にする | ✅ 完了 (実機確認済み) |
| 3 | 楽器全体 (15/14 配置表・設計則) | ✅ 完了 |
| 4 | ブリッジ結合 (この音源の中身) | 未着手 |
| 5 | 響板と箱 | 未着手 |
| 6 | ROOM — X-Y ステレオ | 未着手 |
| 7 | 演奏表現 (打弦点・ハンマー面・ミュート) | 未着手 |
| 8 | 最適化 (SIMD) | 未着手 |
| 9 | GUI とプリセット | 未着手 |
| 10 | 音色の追い込み | 未着手 |

テスト 135 件、`cargo clippy` / `cargo fmt` ともにクリーン。

**計画の正は Artifact**: https://claude.ai/code/artifact/a650768b-6e46-4ba6-a022-0a3ab186990d
([`docs/plan.html`](docs/plan.html) はそのコピー)

## ビルド

```bash
cargo build --release --workspace
cargo test --workspace
```

## CLAP プラグインとして使う

```bash
cargo build --release -p phydulcimer-plugin
copy target\release\phydulcimer_plugin.dll target\PhyDulcimer.clap
```

`.clap` は **`target\` 直下**に置く (`target\release\` だと `cargo clean -p` で消える)。
これを DAW の CLAP フォルダへ入れる。

- 鍵域は **G2–D6 (MIDI 43–86)**、ただし**全音階配置** (G / D メジャー)。
  実機どおり半音の欠落があり、**G#・Bb・D#・F などの鍵は鳴らない**
  ([D-017](docs/problems.md))。C#5 / C#6 だけは存在する
- 同じ音高が複数の位置にあるときは最も長い区間 (バス → トレブル右 → 左) を叩く
- トレブル左にしか無い音 (C#5, A5, B5, C#6, D6) は純正5度の帰結で **+2 cent**
- **離鍵しても鳴り続けるのが仕様** (ダンパーが無い)。止まるのはホストの停止時だけ
- パラメータ: `Level` / `Strike Position` (打弦点 x/L、次の打撃から効く)

設計表 (44 位置の長さ・線径・巻線・張力・応力・B) は:

```bash
cargo run --release -p phydulcimer-render -- --table
```

### MIDI CC (GUI が入る Phase 9 までの操作手段)

| CC | パラメータ | 値の意味 |
|---|---|---|
| **CC7** | Level | 0–127 → 0–1 |
| **CC74** | Strike Position | 0 = ブリッジ寄り x/L 0.03 (明るい) / 127 = 中央寄り 0.30 (丸い) |

CC とホストのオートメーションは同じ値を動かす (後勝ち)。

### 実機確認チェックリスト (DAW でしか確認できない)

1. ロードできて音が出るか — **確認済み** (2026-08-31)
2. **離鍵しても鳴り続けるか** — **確認済み**
3. 再生停止で音が止まるか / ループ折り返しで音量が増え続けないか —
   **要再確認**。初回の確認で LUFS +66 まで発散する不具合が見つかり
   ([D-016](docs/problems.md))、修正済み。ループしても音量が頭打ちに
   なることを確認してほしい
4. Level (CC7) / Strike Position (CC74) が効くか — CC 対応を足したので確認可能に
5. 連打 (ロール) が自然に重なるか — **確認済み** (大きくなっていくのは仕様。
   ただし D-016 の修正前は際限なく増えた。頭打ちになることを再確認してほしい)

## 使い方 (Phase 1)

弦の 1 区間 (トレブル最低コース、D4) を木・レザー・フェルトの撥で叩ける。

```bash
# 弦を鳴らす (木の撥、ブリッジ寄り x/L=0.09)
cargo run --release -p phydulcimer-render -- --string --out out/d4.wav --dur 3.0 --peak 0.9

# 打弦点と撥の面を変える (この楽器の主要な音色操作)
cargo run --release -p phydulcimer-render -- --string --out out/soft.wav \
    --strike 0.20 --face felt --vel 1.0 --dur 3.0 --peak 0.9

# 部分音・T60・インハーモニシティを測る
cargo run --release -p phydulcimer-analyze -- --in out/d4.wav --partials 293.66 --count 12 --partial-t60

# 撥の接触時間の表 (剛体壁、音は出ない)
cargo run --release -p phydulcimer-render -- --contact-table --os 64

# 疎通確認用の減衰正弦波 (Phase 0 の経路検証)
cargo run --release -p phydulcimer-render -- --out out/smoke.wav --freq 440 --t60 2.0 --dur 3.0
cargo run --release -p phydulcimer-analyze -- --in out/smoke.wav --f0 --t60
```

`-h` で全オプションが出る。

**`--peak` の既定は 0 (正規化しない)。** A/B 比較でレベルが揃って差が消える事故を
防ぐため、PhyPiano から意図的に変えている。聴くときだけ `--peak 0.9` を付ける。

### Phase 1 の実測値 (アナライザ経由の完了条件)

| 項目 | 設計値 | 実測 |
|---|---|---|
| インハーモニシティ B (treble-long) | 1.897e-4 | **1.925e-4** (誤差 1.5%) |
| 部分音 T60 (n = 1 / 4 / 8) | 2.00 / 1.73 / 1.20 s | **2.001 / 1.730 / 1.203 s** (R² = 1.0000) |
| 打弦点 1/8 のノッチ (第 8 部分音) | — | **約 −90 dB** |
| 壁での接触時間 (木, v=0.5→6 m/s) | 0.1–1.0 ms・単調減少 | **0.28 → 0.17 ms** |
| オーバーサンプル 16x の収束 (対 64x) | — | 木 **≤1.5 dB** / フェルト **≤0.5 dB** |
| エイリアスフロア (デシメーションフィルタ無し) | — | **−129 dB 以下** (フィルタ不要と判定) |

全実測値と Phase 0 のぶんは [`docs/context.md`](docs/context.md) §4。

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
- 撥の剛性は物理定数ではなく、接触時間からの校正値 ([D-011](docs/problems.md))
- 木の撥はチャタリングし、「接触時間の合計」は収束した観測量ではない
  ([D-012](docs/problems.md))
- 出力の校正は暫定。打撃スパイクが支配的で、持続部は小さめに出る
  ([D-013](docs/problems.md))
- 弦バンクは暫定形 (クロマチック 44 鍵・コース 1 本・ブリッジ結合なし)
  ([D-014](docs/problems.md))
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
