//! Guarantees of `odr-corrupt`, checked on synthetic images in both byte orders.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::many_single_char_names
)]

use odr_core::ByteOrder;
use odr_core::block::{BlockStatus, classify};
use odr_corrupt::{Op, Spec, apply, sha256_hex};
use odr_testkit::synth_image;
use proptest::prelude::*;

const BS: usize = 8192;

fn img(n: usize, ord: ByteOrder) -> Vec<u8> {
    synth_image(n, BS, ord, 99)
}

/// Every differing byte lies inside some manifest range (and truncation is recorded).
fn diff_covered(input: &[u8], out: &[u8], m: &odr_corrupt::Manifest) -> bool {
    let inside = |o: usize| {
        m.changes
            .iter()
            .any(|c| (c.offset as usize..(c.offset + c.len) as usize).contains(&o))
    };
    let common = input.len().min(out.len());
    let diffs_ok = (0..common).all(|o| input[o] == out[o] || inside(o));
    let trunc_ok = out.len() == input.len()
        || m.changes
            .iter()
            .any(|c| c.what == "truncate" && c.offset as usize == out.len());
    diffs_ok && trunc_ok
}

fn all_ops() -> Vec<Op> {
    vec![
        Op::Bitrot { count: 3 },
        Op::Torn { count: 2 },
        Op::Zero { count: 2 },
        Op::Ransom {
            lead_bytes: 3 * BS as u64 + 100,
            chunk: 4096,
            every: 9,
        },
        Op::WipeHeader,
        Op::Swap { count: 2 },
        Op::Holes { count: 3, len: 777 },
        Op::Forge { count: 2 },
        Op::Truncate {
            keep_bytes: 40 * BS as u64 + 5,
        },
    ]
}

#[test]
fn deterministic_and_input_untouched() {
    for ord in [ByteOrder::Little, ByteOrder::Big] {
        let input = img(64, ord);
        let before = sha256_hex(&input);
        let spec = Spec {
            block_size: BS as u64,
            seed: 7,
            ops: all_ops(),
            blocks: None,
        };
        let (a, ma) = apply(&input, &spec).unwrap();
        let (b, mb) = apply(&input, &spec).unwrap();
        assert_eq!(a, b);
        assert_eq!(ma, mb);
        assert_eq!(sha256_hex(&input), before);
        assert_eq!(ma.input_sha256, before);
        assert_eq!(ma.output_sha256, sha256_hex(&a));
        assert!(diff_covered(&input, &a, &ma));
        let other = Spec { seed: 8, ..spec };
        assert_ne!(apply(&input, &other).unwrap().0, a, "seed must matter");
    }
}

#[test]
fn each_pattern_has_the_expected_effect() {
    let ord = ByteOrder::Little;
    let input = img(32, ord);
    let run = |ops: Vec<Op>| {
        apply(
            &input,
            &Spec {
                block_size: BS as u64,
                seed: 3,
                ops,
                blocks: None,
            },
        )
        .unwrap()
    };

    let (o, m) = run(vec![Op::WipeHeader]);
    assert!(o[..2 * BS].iter().all(|b| *b == 0));
    assert_eq!(o[2 * BS..], input[2 * BS..]);
    assert_eq!((m.changes[0].offset, m.changes[0].len), (0, 2 * BS as u64));

    let (o, _) = run(vec![Op::Truncate {
        keep_bytes: 10 * BS as u64 + 1,
    }]);
    assert_eq!(o.len(), 10 * BS + 1);

    let (o, m) = run(vec![Op::Zero { count: 1 }]);
    let b = m.changes[0].block as usize;
    assert_eq!(
        classify(&o[b * BS..(b + 1) * BS], BS, ord),
        BlockStatus::Zeroed
    );

    let (o, m) = run(vec![Op::Torn { count: 1 }]);
    let b = m.changes[0].block as usize;
    assert_ne!(
        classify(&o[b * BS..(b + 1) * BS], BS, ord),
        BlockStatus::Valid
    );

    let (o, m) = run(vec![Op::Swap { count: 1 }]);
    let (x, y) = (m.changes[0].block as usize, m.changes[1].block as usize);
    assert_ne!(x, y);
    assert_eq!(o[x * BS..(x + 1) * BS], input[y * BS..(y + 1) * BS]);
    assert_eq!(o[y * BS..(y + 1) * BS], input[x * BS..(x + 1) * BS]);

    let (o, m) = run(vec![Op::Ransom {
        lead_bytes: 5 * BS as u64,
        chunk: 0,
        every: 0,
    }]);
    assert_eq!(m.changes.len(), 1);
    assert_eq!(o[5 * BS..], input[5 * BS..]);
}

/// The point of `forge`: content changed, yet every block still validates.
#[test]
fn forged_blocks_still_pass_checksum() {
    for ord in [ByteOrder::Little, ByteOrder::Big] {
        let input = img(16, ord);
        let (o, m) = apply(
            &input,
            &Spec {
                block_size: BS as u64,
                seed: 5,
                ops: vec![Op::Forge { count: 4 }],
                blocks: None,
            },
        )
        .unwrap();
        assert!(m.changes.iter().any(|c| c.what == "forge_value"));
        assert_ne!(o, input);
        for b in 0..16 {
            assert_eq!(
                classify(&o[b * BS..(b + 1) * BS], BS, ord),
                BlockStatus::Valid,
                "block {b}"
            );
        }
    }
}

#[test]
fn bad_specs_are_rejected() {
    let input = img(4, ByteOrder::Little);
    let spec = |block_size, ops| Spec {
        block_size,
        seed: 1,
        ops,
        blocks: None,
    };
    assert!(apply(&input, &spec(1000, vec![Op::Zero { count: 1 }])).is_err());
    assert!(apply(&input[..100], &spec(8192, vec![Op::Zero { count: 1 }])).is_err());
    assert!(apply(&input[..BS], &spec(8192, vec![Op::Swap { count: 1 }])).is_err());
    assert!(apply(&input, &spec(8192, vec![Op::Holes { count: 1, len: 0 }])).is_err());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    /// For any seed and any subset/order of ops: deterministic, manifest covers the diff.
    #[test]
    fn manifest_covers_diff(seed in any::<u64>(), mask in 1u16..512, big in any::<bool>()) {
        let ord = if big { ByteOrder::Big } else { ByteOrder::Little };
        let input = img(48, ord);
        let ops: Vec<Op> = all_ops().into_iter().enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0).map(|(_, o)| o).collect();
        let spec = Spec {
            block_size: BS as u64,
            seed,
            ops,
            blocks: None,
        };
        let (o, m) = apply(&input, &spec).unwrap();
        prop_assert!(diff_covered(&input, &o, &m));
        prop_assert_eq!(apply(&input, &spec).unwrap().0, o);
    }
}
