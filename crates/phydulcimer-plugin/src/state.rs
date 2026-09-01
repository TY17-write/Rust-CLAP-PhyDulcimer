//! プロジェクト / プリセットへの保存・復元 (CLAP `state` 拡張、Phase 9 後半)。
//!
//! ホストはプロジェクトを保存するときにプラグインの状態をバイト列で受け取り、
//! 読み込むときに返してくる。DAW のプリセット機能も同じ経路を使う。
//! **プラグイン自身はファイルを書かない** — 置き場所はホストが決める
//! (プロジェクトファイルの内部、または DAW のプリセットフォルダ)。
//! 独自プリセットブラウザは持たない。
//!
//! # なぜテキスト形式か
//!
//! バイナリのほうが小さいが、**壊れたときに直せない**。ここでは 1 行 1 項目の
//! テキストにしてある。数百バイトにしかならないので、大きさは問題にならない。
//!
//! ```text
//! phydulcimer 1
//! param 1 0.700000
//! param 2 0.200000
//! ```
//!
//! # 互換性の方針
//!
//! - **知らない行は読み飛ばす。** 将来項目が増えても古い版が壊れない
//! - **知らないパラメータ ID も読み飛ばす。** 逆に、保存されていない
//!   パラメータは既定値のままになる
//! - 先頭の `phydulcimer <版>` だけは必須。**別のプラグインの状態を渡された
//!   ときに黙って受け入れない**ため
//!
//! パラメータ ID は公開後に変えない約束なので ([`params::id`](crate::params::id))、
//! ID をキーにしておけば並び順が変わっても読める。
//!
//! # Layout / Temperament の適用タイミング
//!
//! 復元された値はアトミックに書かれるだけで、弦バンクの再構築を伴う
//! Layout / Temperament は**次の activate で**反映される (Phase 7 の規則の
//! まま)。プロジェクトを開く流れでは load → activate の順なので自然に合う。
//! 演奏中のプリセット読み込みでは GUI に "pending / Reload" チップが出て、
//! Reload かホストの再 activate で反映される。

use crate::params::{self, ParamValues};

/// 状態の形式の版。**増やすのは互換性を壊すときだけ。**
pub const STATE_VERSION: u32 = 1;

/// 先頭行の目印。
const MAGIC: &str = "phydulcimer";

/// 保存する状態。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    /// `(パラメータ ID, 値)`
    pub params: Vec<(u32, f64)>,
}

impl State {
    /// 現在のパラメータ値から状態を組み立てる。
    pub fn capture(values: &ParamValues) -> Self {
        Self {
            params: params::PARAMS
                .iter()
                .filter_map(|spec| values.get(spec.id).map(|v| (spec.id, v)))
                .collect(),
        }
    }

    /// パラメータを書き戻す。**知らない ID は無視される** (`ParamValues::set`)。
    pub fn apply_params(&self, values: &ParamValues) {
        for &(id, value) in &self.params {
            values.set(id, value);
        }
    }

    /// テキストへ書き出す。
    pub fn serialize(&self) -> String {
        let mut out = format!("{MAGIC} {STATE_VERSION}\n");
        for &(id, value) in &self.params {
            // 既定の表示だと桁が落ちることがあるので、桁数を明示する。
            out.push_str(&format!("param {id} {value:.6}\n"));
        }
        out
    }

    /// テキストから読み込む。
    ///
    /// 先頭行が合わなければ `None`。それ以外は**できるだけ読む** (壊れた行は
    /// 読み飛ばす)。プロジェクトを開けなくなるより、一部が既定値に戻るほうが
    /// 被害が小さい。
    pub fn deserialize(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        let header = lines.next()?;
        let mut header = header.split_whitespace();
        if header.next()? != MAGIC {
            return None;
        }
        // 版は読むが、今は分岐しない。将来の互換処理のために形だけ確かめる。
        let _version: u32 = header.next()?.parse().ok()?;

        let mut state = Self::default();
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let (key, rest) = match line.split_once(' ') {
                Some(pair) => pair,
                None => continue,
            };
            let rest = rest.trim();
            // "param" 以外の項目は読み飛ばす (前方互換)。
            if key == "param" {
                if let Some((id, value)) = rest.split_once(' ') {
                    if let (Ok(id), Ok(value)) = (id.trim().parse(), value.trim().parse()) {
                        state.params.push((id, value));
                    }
                }
            }
        }
        Some(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::id;

    #[test]
    fn round_trips() {
        let state = State {
            params: vec![(id::LEVEL, 0.5), (id::STRIKE_POSITION, 0.25)],
        };
        let text = state.serialize();
        let back = State::deserialize(&text).expect("読めるはず");
        assert_eq!(back, state);
    }

    #[test]
    fn header_is_required() {
        // 別のプラグインの状態を渡されたときに黙って受け入れない。
        assert!(State::deserialize("").is_none());
        assert!(State::deserialize("someotherplugin 1\nparam 1 0.5\n").is_none());
        assert!(State::deserialize("someothersynth 1\nparam 1 0.5\n").is_none());
        assert!(State::deserialize("phydulcimer\n").is_none());
        assert!(State::deserialize("phydulcimer abc\n").is_none());
        assert!(State::deserialize("phydulcimer 1\n").is_some());
    }

    #[test]
    fn unknown_lines_are_skipped() {
        // 将来項目が増えても古い版が壊れない。
        let text = "phydulcimer 1\n\
                    param 1 0.25\n\
                    somethingnew whatever 1 2 3\n\
                    param 12 0.75\n";
        let state = State::deserialize(text).expect("読めるはず");
        assert_eq!(state.params, vec![(id::LEVEL, 0.25), (id::COMP, 0.75)]);
    }

    #[test]
    fn broken_lines_do_not_abort_the_load() {
        // 一部が既定値に戻るほうが、プロジェクトを開けなくなるよりまし。
        let text = "phydulcimer 1\n\
                    param notanumber 0.5\n\
                    param 1\n\
                    param 1 0.75\n";
        let state = State::deserialize(text).expect("読めるはず");
        assert_eq!(state.params, vec![(id::LEVEL, 0.75)]);
    }

    #[test]
    fn capture_and_apply_round_trip_through_params() {
        let values = ParamValues::new();
        values.set(id::LEVEL, 0.25);
        values.set(id::LAYOUT, 0.0); // 既定 (1.0) から動かす
        values.set(id::MIC_DISTANCE, 2.5);

        let state = State::capture(&values);
        let text = state.serialize();

        // 別のインスタンスへ流し込む。
        let restored = ParamValues::new();
        State::deserialize(&text)
            .expect("読めるはず")
            .apply_params(&restored);

        for spec in params::PARAMS {
            assert_eq!(
                restored.get(spec.id),
                values.get(spec.id),
                "param {} が復元されていない",
                spec.id
            );
        }
    }

    #[test]
    fn unknown_param_ids_are_ignored_on_load() {
        // 将来削除されたパラメータの ID が残っていても落ちない。
        //
        // 値に 0.25 を使うのは、`ParamValues` が `f32` で保持しているため
        // (2 進で割り切れない値は往復で誤差が出る)。
        let text = "phydulcimer 1\nparam 99999 0.5\nparam 1 0.25\n";
        let values = ParamValues::new();
        State::deserialize(text)
            .expect("読めるはず")
            .apply_params(&values);
        assert_eq!(values.get(id::LEVEL), Some(0.25));
        assert_eq!(values.get(99999), None);
    }

    #[test]
    fn values_are_clamped_when_applied() {
        // 手で書き換えられた状態ファイルでも範囲外にならない。
        let text = "phydulcimer 1\nparam 1 999.0\nparam 2 0.0\n";
        let values = ParamValues::new();
        State::deserialize(text)
            .expect("読めるはず")
            .apply_params(&values);
        assert_eq!(values.get(id::LEVEL), Some(1.0));
        // Strike Position は min 0.03 でクランプ (f32 の丸めを許す)。
        let sp = values.get(id::STRIKE_POSITION).unwrap();
        assert!((sp - 0.03).abs() < 1e-6, "clamped = {sp}");
    }

    #[test]
    fn every_param_is_captured() {
        // 全 12 パラメータが漏れなく保存対象に入っている。
        let state = State::capture(&ParamValues::new());
        assert_eq!(state.params.len(), params::PARAMS.len());
        for spec in params::PARAMS {
            assert!(
                state.params.iter().any(|&(id, _)| id == spec.id),
                "param {} が保存されない",
                spec.id
            );
        }
    }
}
