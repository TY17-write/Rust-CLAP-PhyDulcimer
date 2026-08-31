# Rust-CLAP-PhyDulcimer — 作業の決まりごと

- 回答はすべて日本語ベースで行うこと

## ファイル編集の鉄則

- ファイルの編集は **Edit / Write ツールだけ**を使うこと。シェル (PowerShell / Bash) でファイルを書き換えてはいけない。
  - 禁止: `Set-Content` / `Out-File` / `Add-Content` / `sed -i` / `>` `>>` によるソースへの書き込み
  - 理由: Windows PowerShell 5.1 の `Get-Content` は UTF-8 を ANSI (CP932) として読むため、読んだ時点で日本語が不可逆に壊れる。さらに一部の文字が直後の改行や引用符を飲み込み、ソースが構文レベルで破壊される
  - `cargo fmt` は使用可 (UTF-8 を正しく扱う。日本語コメントも数式記号も無傷であることを確認済み)
  - `cp` によるファイル**複製**は可 (バイト列をそのまま移すだけで、内容を読み書きしない)。ただし複製後の修正は Edit で行う
- どうしてもスクリプトで一括処理が必要なときは、**着手前にユーザーへ理由と影響範囲を伝えて許可を得る**
- 作業を始める前に、対象リポジトリがコミット済みか確認すること
- 画面操作が必要なタスクを自動実行しない。**GUI の確認はユーザーに依頼する**

## このプロジェクト固有の決まり

### 測ってから触る

このプロジェクトは**参照音源を持たない** (→ [`docs/problems.md`](docs/problems.md) D-006)。
「参照より何 dB」で判断できないので、次を守らないと寄る辺が無くなる。

- **物理量が文献値の範囲に入ることが唯一の外部基準。** 線径・張力・接触時間・
  インハーモニシティ B の完了条件を緩めない
- 耳で気づいたことを**そのままパラメータに反映しない**。まず何が動いているかを測る
- 指標を先に作ってからモデルを触る

### 測るときの落とし穴

- **`--peak` の既定は 0 (正規化しない)。** 明示的に指定したときだけ正規化される
- **測定器が「測れない」と言ったのを「値が無い」と読まない** (D-001 / D-002)
- 信号全体の `--t60` はダンパーの無いこの楽器では意味を持たない。`--partial-t60` を使う
- Phase 6 以降、**音質の判断は必ず ROOM を off にして行う**。部屋は粗を隠す

### コード

- `phydulcimer-core` はオーディオスレッドから呼ばれる。**ヒープ確保・ロック・
  ファイル I/O・panic しうる操作をしない**
- UI の文字は **ASCII のみ** (egui の既定フォントに日本語グリフが無い)
- 各フェーズごとにコミットする

## 検証の入り口

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all -- --check

# 疎通確認 (Phase 0)
cargo run --release -p phydulcimer-render -- --out out/smoke.wav --freq 440 --t60 2.0 --dur 3.0
cargo run --release -p phydulcimer-analyze -- --in out/smoke.wav --f0 --t60
```

現在地・実測値・判断の理由は [`docs/context.md`](docs/context.md)。
計画は [`docs/plan.html`](docs/plan.html) (Artifact が正)。
