# 引き継ぎメモ

**別の環境で作業を再開するとき、最初に読む文書。**

他のドキュメントと役割を分けてある:

| 文書 | 内容 |
|---|---|
| [`plan.html`](plan.html) | 全体計画とフェーズ構成 (**Artifact が正**) |
| [`research.md`](research.md) | 楽器の物理と手法選定の根拠 |
| [`problems.md`](problems.md) | 既知の問題・意図的に諦めた近似 (D-001〜) |
| **このファイル** | **現在地、判断の理由、環境、ハマりどころ** |

コードとコミットログから読み取れることは書かない。**読み取れないこと**だけを書く。

---

## 1. 現在地

**Phase 2 (CLAP 化) 完了 — ただし DAW での実機確認が未実施。**

| Phase | 内容 | 状態 |
|---|---|---|
| 0 | ワークスペース骨組み / ドキュメント / render・analyze | ✅ |
| 1 | 1 区間の弦 + 硬い撥 (modal / hammer / segment) | ✅ |
| 2 | CLAP 化 (instrument / plugin / ABI テスト) | ✅ (実機確認待ち) |
| 3 | 楽器全体 (配置表・設計則) | 未着手 |
| 4〜10 | (`plan.html` §07) | 未着手 |

テスト 135 件、`cargo clippy` / `cargo fmt` ともにクリーン。

### DAW の実機確認 (2026-08-31 に 1 回目実施)

1 回目の確認で **D-016 (ループ再生で LUFS +66 まで発散) が見つかり、修正した**。
「実機でしか出ない不具合が必ずある」という想定どおりの成果。

| 項目 | 状態 |
|---|---|
| ロード・発音 | ✅ 確認済み |
| 離鍵しても鳴り続ける | ✅ 確認済み |
| ループで音量が頭打ちになる | **要再確認** (D-016 修正後) |
| Level (CC7) / Strike Position (CC74) | **要確認** (CC 対応を追加した) |
| ロールが頭打ちになる | **要再確認** (修正前は際限なく増えた) |

```bash
cargo build --release -p phydulcimer-plugin
copy target\release\phydulcimer_plugin.dll target\PhyDulcimer.clap
```

Phase 1 で決めたこと (数値の根拠は [problems.md](problems.md)):

- **オーバーサンプルは 16 倍、デシメーションフィルタは不要** ([D-010](problems.md))
- **撥の剛性は接触時間からの校正値**: 木 1e8 / レザー 3e8 / フェルト 4.5e9 ([D-011](problems.md))
- **木の撥は再接触を許す** (チャタリング)。PhyPiano の「最初の離脱で終わり」は
  フェルト専用の仮定だった ([D-012](problems.md))
- **モードは絞らない**。打ち切りはチャタリング経由で低次まで動かす ([D-010](problems.md))
- SIMD は Phase 8 を待たず移植済み ([D-008](problems.md))。Phase 8 の中身は実測とチューニングになる

---

## 2. 環境

- ツールチェイン: Rust (`rust-version = "1.85"`, edition 2021) で確認
- リモート: 未設定 (ローカルのみ)
- git identity — グローバル設定が無い環境なので、新しい PC では設定が必要:

```bash
git config user.name "TY17"
git config user.email "310846857+TY17-write@users.noreply.github.com"
```

### 隣のプロジェクト

- `../Rust-CLAP-PhyPiano` — **DSP と検証ツールの供給元。** ただし
  **コピーして独立させる方針**なのでパス依存は張っていない
- `../egui-clap-host` — Phase 2 で音を出して確かめるときのホスト
- `../trisphere` — clack の書き方の手本。revision `c5975f9` を揃える

---

## 3. 検証のやり方

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all -- --check
```

オフライン検証 (Phase 0 時点):

```bash
# 単一の減衰正弦波
cargo run --release -p phydulcimer-render -- --out out/smoke.wav --freq 440 --t60 2.0 --dur 3.0
cargo run --release -p phydulcimer-analyze -- --in out/smoke.wav --f0 --t60 --freq 440

# 部分音列 (インハーモニシティと減衰の傾きつき)
cargo run --release -p phydulcimer-render -- --out out/tone.wav \
    --freq 220 --partials 8 --t60 3.0 --t60-taper 1.0 --inharmonicity 4e-4 --dur 4.0
cargo run --release -p phydulcimer-analyze -- --in out/tone.wav \
    --partials 220 --count 8 --partial-t60 --window 2.0
```

### 測るときに知っておくこと

- **`--peak` の既定は 0 (正規化しない)。** PhyPiano から意図的に変えた ([D-004](problems.md))。
  聴きたいときだけ `--peak 0.9` を付ける
- **Goertzel は窓の中の平均振幅を返す。** 減衰する信号を全区間で測ると小さく出る
  (3 秒窓で −37.7 dB、先頭 0.2 秒窓で −8.95 dB、設計振幅 0.5)。**窓を切ること**
- **部分音は走査で見つける。** インハーモニシティ B を知らなくてよい。B は結果として
  報告される
- **`--partial-t60` は T60 が 0.4 秒を切ると沈黙する** ([D-001](problems.md))。
  「減衰が無い」ではない

---

## 4. 実測値 (回帰の基準)

**大きく変わったら何かを壊している。**

### Phase 0 — 測定器そのもの

| 項目 | 設計値 | 実測 |
|---|---|---|
| f0 推定 (自己相関) | 440 Hz | **440.00 Hz** |
| 信号全体の T60 | 2.0 s | **2.000 s** (R² = 0.9999) |
| 部分音 T60 (n = 1 / 2 / 4) | 3.0 / 1.5 / 0.75 s | **3.000 / 1.500 / 0.750 s** |
| 部分音位置 (n = 8, B = 4e-4) | +21.9 cent | **+22.2 cent** |
| インハーモニシティ B の推定 | 4.0e-4 | **3.878e-4** (誤差 3%) |
| WAV 往復 (32-bit float) | — | **標本が完全一致** |
| `--partial-t60` の下限 | — | **0.5 s は可 / 0.375 s は不可** ([D-001](problems.md)) |

### Phase 1 — 弦と撥 (treble-long: L=495 mm, D4=293.66 Hz, d=0.5 mm)

| 項目 | 設計値 | 実測 |
|---|---|---|
| 張力 / 応力 | — | **130.3 N / 663 MPa** (music wire の実用域) |
| インハーモニシティ B | 1.897e-4 | **1.925e-4** (誤差 1.5%、アナライザ経由) |
| 部分音の伸び (n=10) | +16 cent | **+16.2 cent** |
| 部分音 T60 (n=1 / 4 / 8、設計アンカー 2.0s/0.5s) | 2.00 / 1.73 / 1.20 s | **2.001 / 1.730 / 1.203 s** (R² = 1.0000) |
| 打弦点 1/8 のノッチ (第 8 部分音) | — | **約 −90 dB** (隣接 −21 dB に対し −111 dB) |
| 壁での接触時間 (木, v=0.5→6) | 0.1–1.0 ms・単調減少 | **0.28 → 0.17 ms** |
| 壁での接触時間 (フェルト, v=2) | ピアノの範囲 | **0.95 ms** |
| os=16 の収束 (対 os=64) | — | 木 **≤1.5 dB** / フェルト **≤0.5 dB** |
| デシメーション drop vs average | — | 部分音 0.01 dB 一致、フロア **−129 dB 以下** |
| モード打ち切りの波及 (60 本 vs 全 75 本) | — | **0.3 dB 以内** (20 本だと低次まで 1–4 dB 動く) |

打弦点 0.09 では第 11 部分音 (1/0.09 ≈ 11) がノッチに埋もれ、アナライザの走査が
隣の部分音を拾う (+174 cent と表示される)。**異常ではなくノッチの傍証。**

### Phase 2 — プラグイン

| 項目 | 値 |
|---|---|
| 生ピーク (撥 0.5 / 2.0 / 6.0 m/s) | **4.7 / 21.5 / 83.3 N** — スパイク支配 ([D-013](problems.md)) |
| クレストファクタ (スパイク / 持続部) | **20–40 倍** |
| `CALIBRATED_GAIN` (暫定) | **0.004** (ff 単音 ≈ −9 dBFS) |
| 鍵域 | MIDI **43–86** (G2–D6、クロマチック 44 鍵の暫定配置) |
| `process` のアロケーション | **0** (`assert_no_alloc` が ABI テスト全体で監視) |

---

## 5. 判断とその理由

**コードを見ても分からない「なぜそうしたか」。**

### 解析器を lib にした (PhyPiano は bin の中の mod)

PhyPiano では `analysis.rs` が bin の中に閉じていて、`tests/` から叩けなかった。
参照音源を持たない本プロジェクトでは**測定器の正しさが唯一の足場**なので、
統合テスト (`tools/analyze/tests/roundtrip.rs`) から固定できる形にした。

### WAV はチャンネルを分けて持つ

Phase 0 ではモノしか書かないのに `Wav { channels: Vec<Vec<f32>> }` にしてある。
Phase 6 の ROOM で **L/R の相互相関そのものを測る**ため。後から直すと
「モノ化された値で調整してしまった」事故が起きうるので、最初から分けた。

`mono()` は**和ではなく平均**。X-Y のモノ互換性を見るとき、和と平均では
レベルが 6 dB 違う。テストで固定してある。

### `--peak` の既定を 0 にした

PhyPiano の頻出事故 (A/B のたびに正規化でレベルが揃い、差が消える) を
注意力ではなく既定値で防ぐ。→ [D-004](problems.md)

### `--partial-t60` の下限を Phase 0 では直さなかった

適切な窓とホップは「測りたい T60 の範囲」で決まり、それは弦の減衰を
設計してから確定する。**先に決め打ちしない。** → [D-001](problems.md)

Phase 1 では当たらなかった (測った T60 は最短 1.2 s)。**Phase 3 の高音コースで
当たる見込み** — 5 kHz アンカー 0.8 s の設計だと高次部分音が 0.4 s を切る。

### 接触時間ではなくスペクトルで収束を判定した (Phase 1)

木の撥はチャタリングするので「接触時間の合計」が倍率に対して収束しない
(0.09 → 1.41 ms と増え続ける)。倍率の選定は**耳に届く観測量 = 部分音レベル**の
収束で行った。→ [D-010](problems.md) / [D-012](problems.md)

### 撥の再接触を許した (PhyPiano からの構造変更)

PhyPiano の「最初の離脱で Released」はフェルト (1 回の接触で運動量を渡しきる)
専用の仮定だった。木の撥は弦に「逃げられて」23 µs で離脱し、まだ弦へ向かって
いるのに力を出せなくなっていた。離脱時に `velocity > 0` なら再接触を許し、
コストの上界として 20 ms で打ち切る。→ [D-012](problems.md)

### WAV 書き出しの重複を許した

共有すると「レンダラが解析ツールに依存する」筋の悪い依存方向になる。
30 行の重複のほうが安い。→ [D-003](problems.md)

### `CLAUDE.md` をリポジトリに持たせた

PhyPiano は親ディレクトリの `CLAUDE.md` を継承していて自前では持っていない。
本プロジェクトは**独立方針**なので、単独で clone しても鉄則が付いてくる形にした。

---

## 6. 次にやること

**まず §1 の DAW 実機確認。** その後 **Phase 3 — 楽器全体** (`plan.html` の P3)。

1. `layout.rs` — **15/14 の配置表** (D/G 全音階)。クロマチック暫定配置
   ([D-014](problems.md)) を置き換える。同じ音高が複数の場所に存在するので
   MIDI からの割当は 1 対多
2. `scaling.rs` — 音域ごとの設計則。低音は短く太い巻線弦。導いた線径が
   文献値の範囲に入ることが完了条件 (参照音源を持たないので、ここが外部基準)
3. **[D-001](problems.md) に先に当たること** — 高音コースの T60 は 0.4 秒を
   切る見込みで、いまの `--partial-t60` では測れない。窓とホップを引数にする
4. 最悪ケース (全 44 音を叩いた直後) の CPU を criterion で測る

clack の API は `~\.cargo\git\checkouts\` を直接読むのが早い。**revision を
間違えないこと** (`Cargo.lock` が固定しているコミットを見る)。

### 触るときに壊さないでほしいもの

- §4 の実測値の表 — 大きく動いたら測定器かモデルが壊れている
- `peak_defaults_to_no_normalisation` テスト — A/B 比較の安全装置
- `mono_is_the_average_not_the_sum` / `stereo_channels_do_not_swap` テスト —
  Phase 6 の X-Y 検証の前提
- `oversampling_is_converged_at_the_default` テスト — os=16 の収束の固定。
  **os=4/8 と比べる形に書き換えないこと** (そちらは収束していない)
- `faces_are_ordered_from_hard_to_soft` テスト — 撥の面の順序
- `note_off_does_not_stop_the_ring` (core と ABI の両方) — この楽器の定義
- `ringing_strings_do_not_survive_a_loop_restart` — ループ折り返しの安全装置
- ABI テストの `assert_no_alloc` — `Cargo.toml` の `default-features = false` を
  外すと **release で検査が消えて素通りになる** (既定機能の `disable_release`)
