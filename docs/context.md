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

**Phase 0 (土台) 完了。** モデル本体はまだ無い。

| Phase | 内容 | 状態 |
|---|---|---|
| 0 | ワークスペース骨組み / ドキュメント / render・analyze | ✅ |
| 1 | 1 区間の弦 + 硬いハンマー | 未着手 |
| 2〜10 | (`plan.html` §07) | 未着手 |

テスト 52 件、`cargo clippy` / `cargo fmt` ともにクリーン。

**Phase 0 で作ったのは「測定器が信用できる状態」。** モデルを 1 行も書かずに
先にこれを作ったのは、参照音源を持たない ([D-006](problems.md)) 以上、
測定値そのものが唯一の足場になるため。

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

| 項目 | 設計値 | 実測 |
|---|---|---|
| f0 推定 (自己相関) | 440 Hz | **440.00 Hz** |
| 信号全体の T60 | 2.0 s | **2.000 s** (R² = 0.9999) |
| 部分音 T60 (n = 1) | 3.0 s | **3.000 s** (R² = 1.0000) |
| 部分音 T60 (n = 2) | 1.5 s | **1.500 s** (R² = 1.0000) |
| 部分音 T60 (n = 4) | 0.75 s | **0.750 s** (R² = 1.0000) |
| 部分音位置 (n = 8, B = 4e-4) | +21.9 cent | **+22.2 cent** |
| インハーモニシティ B の推定 | 4.0e-4 | **3.878e-4** (誤差 3%) |
| WAV 往復 (32-bit float) | — | **標本が完全一致** |
| `--partial-t60` の下限 | — | **0.5 s は可 / 0.375 s は不可** |

`B` の 3% 誤差は走査の刻み (1 cent 相当) と中央値の性質によるもので、異常ではない。

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

適切な窓とホップは「測りたい T60 の範囲」で決まり、それは Phase 1 で弦の減衰を
設計してから確定する。**先に決め打ちしない。** → [D-001](problems.md)

### WAV 書き出しの重複を許した

共有すると「レンダラが解析ツールに依存する」筋の悪い依存方向になる。
30 行の重複のほうが安い。→ [D-003](problems.md)

### `CLAUDE.md` をリポジトリに持たせた

PhyPiano は親ディレクトリの `CLAUDE.md` を継承していて自前では持っていない。
本プロジェクトは**独立方針**なので、単独で clone しても鉄則が付いてくる形にした。

---

## 6. 次にやること

**Phase 1 — 1 区間の弦 + 硬いハンマー。**

`plan.html` の P1 にある完了条件のうち、着手前に意識しておくもの:

1. **オーバーサンプル倍率を 4 / 8 / 16 で実測して決める。** 決め打ちしない。
   硬い木のハンマーは接触が 0.1–0.3 ms で、48 kHz では 5–14 サンプルしかない
2. **モード上限を振って測る。** 打弦点が `x/L = 0.05` まで寄るので、第 20 部分音まで
   強く励振される。上限 128 で足りるかは測らないと分からない (計画 §09 の最大の危険)
3. **[D-001](problems.md) に当たる。** 高音側の弦は T60 が 0.5 秒を切る見込み。
   `--partial-t60` が沈黙したら、まず測定器の側を疑う

### 触るときに壊さないでほしいもの

- §4 の実測値の表 — 大きく動いたら測定器が壊れている
- `peak_defaults_to_no_normalisation` テスト — A/B 比較の安全装置
- `mono_is_the_average_not_the_sum` テスト — Phase 6 の X-Y 検証の前提
- `stereo_channels_do_not_swap` テスト — 同上
