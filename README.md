# Rust-CLAP-PhyDulcimer

Rust で書くハンマーダルシマー物理モデリング音源 (CLAP プラグイン)。

サンプル再生ではなく、**ハンマー・弦・ブリッジ・響板の物理を実時間で解く**。
対象はアメリカ式ハンマーダルシマー (標準 15/14)。

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

**Phase 6 (ROOM — X-Y ステレオ) まで完了。** 弦 2 本のコース・ブリッジ結合・
響板と箱・X-Y の部屋が揃い、アンビエント込みのステレオで鳴る。

| Phase | 内容 | 状態 |
|---|---|---|
| 0 | ワークスペース骨組み / ドキュメント / オフラインレンダラ・解析ツール | ✅ 完了 |
| 1 | 1 区間の弦 + 硬い撥 | ✅ 完了 |
| 2 | CLAP 化 (clack) — 弾ける状態にする | ✅ 完了 |
| 3 | 楽器全体 (15/14 配置表・設計則) | ✅ 完了 |
| 4 | ブリッジ結合 (この音源の中身) | ✅ 完了 |
| 5 | 響板と箱 | ✅ 完了 |
| 6 | ROOM — X-Y ステレオ | ✅ 完了 |
| 7 | 演奏表現 (打弦点・ハンマー面・ミュート) | 未着手 |
| 8 | 最適化 (SIMD) | 未着手 |
| 9 | GUI とプリセット | 未着手 |
| 10 | 音色の追い込み | 未着手 |

テスト 183 件、`cargo clippy` / `cargo fmt` ともにクリーン。
全体計画とフェーズ構成は [`docs/plan.html`](docs/plan.html)。

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
- パラメータ: `Level` / `Strike Position` (打弦点 x/L、次の打撃から効く) /
  `Room` (on/off) / `Mic Distance` (0.3–3 m — これ 1 本でタイト ⇔ アンビエント) /
  `X-Y Angle` (60–135°) / `Room Size` (S/M/L) / `Wall Absorption`
- **ROOM は X-Y ステレオの模倣**: L/R に時間差を作らない (定位はレベル差だけ)。
  DAW 側で空間を作るときは `Room = Off`。**音質の判断も必ず Off で**

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

自動テストは CLAP の ABI までを検証する。以下はホスト実機でのみ確かめられる:

1. ロードできて音が出るか (鍵域は G2–D6 の全音階。外と半音の欠落は無音が正しい)
2. 離鍵しても鳴り続けるか (切れたらバグ)
3. 再生停止で音が止まるか / ループ再生で音量が頭打ちになるか
   (連打で膨らむのはロールの物理として正しいが、際限なく増えたらバグ)
4. パラメータと CC (CC7 / CC74) がオートメーションに乗るか
5. L/R の定位 — 低音がやや左、高音がやや右 (楽器の幾何から出る)

## 検証ツール (render / analyze)

オフラインのレンダラと解析ツールで、モデルを数値で確かめられる。

```bash
# 楽器全体をステレオで鳴らす (和音、X-Y の部屋込み)
cargo run --release -p phydulcimer-render -- --instrument \
    --key 55 --key 62 --key 67 --key 74 --vel 0.9 --dur 4.0 --out out/chord.wav --peak 0.9

# 部屋を切って測定用に (音質の判断は必ずこちらで)
cargo run --release -p phydulcimer-render -- --instrument --key 69 --no-room --out out/a4.wav

# 部分音・T60・インハーモニシティ・うなり
cargo run --release -p phydulcimer-analyze -- --in out/a4.wav --partials 440 --count 12 --partial-t60
cargo run --release -p phydulcimer-analyze -- --in out/a4.wav --partials 440 --count 4 --modulation

# X-Y の検証 (相互相関のピークは lag 0 に立つ)
cargo run --release -p phydulcimer-analyze -- --in out/chord.wav --correlation

# 44 発音位置の設計表 / 響板+箱の IR とバンドレベル / 撥の接触時間
cargo run --release -p phydulcimer-render -- --table
cargo run --release -p phydulcimer-render -- --soundboard --out out/ir.wav --dur 1.0
cargo run --release -p phydulcimer-render -- --contact-table --os 64
```

`-h` で全オプションが出る。

**`--peak` の既定は 0 (正規化しない)。** A/B 比較でレベルが揃って差が消える事故を
防ぐため。聴くときだけ `--peak 0.9` を付ける。

### 主要な実測値

| 項目 | 設計値 | 実測 |
|---|---|---|
| インハーモニシティ B (トレブル最低コース) | 1.897e-4 | **1.925e-4** (誤差 1.5%) |
| 部分音ごとの T60 | 設計どおり | **誤差 0.1% 以内** (R² = 1.0000) |
| 打弦点 1/8 のノッチ (第 8 部分音) | — | **約 −90 dB** |
| ユニゾンのうなり (包絡リップル) | — | **32–36 dB** |
| L/R 相互相関のピーク位置 | lag 0 (X-Y の定義) | **lag 0** (係数 0.95) |
| 全 44 位置 ff の最悪ケース CPU | < 50% | **484 µs / 1333 µs = 36%** |

全実測値と経緯は [`docs/context.md`](docs/context.md) §4。

## ドキュメント

| 文書 | 内容 |
|---|---|
| [`docs/plan.html`](docs/plan.html) | 全体計画とフェーズ構成 |
| [`docs/research.md`](docs/research.md) | 楽器の物理と手法選定の根拠、論文サーベイ |
| [`docs/problems.md`](docs/problems.md) | 既知の問題・意図的に諦めた近似 (D-001〜) |
| [`docs/context.md`](docs/context.md) | 現在地・判断の理由・実測値・ハマりどころ |
| [`CLAUDE.md`](CLAUDE.md) | 作業の決まりごと |

## 既知の制約

- **参照音源を持たない** ([D-006](docs/problems.md))。物理量が文献値の範囲に入ることが
  唯一の外部基準になる
- 撥の剛性は物理定数ではなく、接触時間からの校正値 ([D-011](docs/problems.md))
- 木の撥はチャタリングし、「接触時間の合計」は収束した観測量ではない
  ([D-012](docs/problems.md))
- 全音階配置なので楽器に無い半音は鳴らない。トレブル左側の音は純正5度の帰結で
  +2 cent ([D-017](docs/problems.md))
- ブリッジ結合は撥の接触力の順方向注入。打撃後の持続的な弦間交換 (真の双方向) は
  未実装 ([D-018](docs/problems.md))
- 響板は板のシミュレーションではなくフィルタ近似で、箱と合わせてパラメータは
  校正値。打撃過渡が支配的な音になる ([D-020](docs/problems.md))
- 音域バランス・音色の追い込みは未着手 (Phase 10)

解消済みの問題も含め、全記録は [`docs/problems.md`](docs/problems.md) (D-001〜)。

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
- [clack](https://github.com/prokopyl/clack) — Rust の CLAP バインディング
- [hound](https://github.com/ruuda/hound) — WAV の読み書き

## ライセンス

MIT OR Apache-2.0
