//! 演算量のベンチ。締切は 64 サンプル @ 48 kHz = 1333 µs/block。
//!
//! **最悪ケースを測る。** ダンパーの無い楽器では演奏中ほぼ全弦が鳴っている
//! ので、平均ではなく「全 44 位置を叩いた直後」を基準にする (P3 の完了条件)。
//!
//! # 測るときの注意 (PhyPiano から継承した教訓)
//!
//! criterion は 1 つのベンチを数十万回まわす。叩いたきりの弦は途中で振幅が
//! デノーマル域に入り、**デノーマルのコストを測ることになる**。定期的に
//! 叩き直すこと (`RESTRIKE_BLOCKS`)。

use criterion::{criterion_group, criterion_main, Criterion};
use phydulcimer_core::engine::DulcimerEngine;
use phydulcimer_core::instrument::{Instrument, KEY_MAX, KEY_MIN};

const SR: f64 = 48_000.0;
const BLOCK: usize = 64;
/// このブロック数ごとに全鍵を叩き直す (デノーマル域に落とさないため)。
const RESTRIKE_BLOCKS: usize = 2_000;

fn strike_all(inst: &mut Instrument) {
    for key in KEY_MIN..=KEY_MAX {
        inst.note_on(key, 1.0);
    }
}

fn bench_instrument(c: &mut Criterion) {
    let mut group = c.benchmark_group("instrument");

    // 最悪ケース: 全 44 位置が鳴っている。
    group.bench_function("all44/block64", |b| {
        let mut inst = Instrument::new(SR);
        let mut buf = [0.0f32; BLOCK];
        strike_all(&mut inst);
        let mut n = 0usize;
        b.iter(|| {
            n += 1;
            if n % RESTRIKE_BLOCKS == 0 {
                strike_all(&mut inst);
            }
            std::hint::black_box(inst.process(&mut buf));
        });
    });

    // 本命: エンジン全体 (弦 + 響板 ×2 + 箱 + ROOM + クリップ) の最悪ケース。
    group.bench_function("engine/all44/block64", |b| {
        let mut engine = DulcimerEngine::new(SR, BLOCK);
        let mut l = [0.0f32; BLOCK];
        let mut r = [0.0f32; BLOCK];
        for key in KEY_MIN..=KEY_MAX {
            engine.note_on(key, 1.0);
        }
        let mut n = 0usize;
        b.iter(|| {
            n += 1;
            if n % RESTRIKE_BLOCKS == 0 {
                for key in KEY_MIN..=KEY_MAX {
                    engine.note_on(key, 1.0);
                }
            }
            std::hint::black_box(engine.process_stereo(&mut l, &mut r));
        });
    });

    // 参考: 無音 (全弦が静止) のコスト。active スキップを入れる前の基準。
    group.bench_function("silent/block64", |b| {
        let mut inst = Instrument::new(SR);
        let mut buf = [0.0f32; BLOCK];
        b.iter(|| {
            std::hint::black_box(inst.process(&mut buf));
        });
    });

    // 参考: note_on のコスト (オーディオスレッドで走る)。
    group.bench_function("note_on/lowest", |b| {
        let mut inst = Instrument::new(SR);
        b.iter(|| {
            inst.note_on(KEY_MIN, 1.0);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_instrument);
criterion_main!(benches);
