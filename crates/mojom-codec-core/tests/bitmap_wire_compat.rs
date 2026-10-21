// Copyright 2026 The Mojo Rust SDK Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! End-to-end bitmap fixtures complement the private exhaustive bitmap tests.
//! The generic legacy AST remains an independent encoder oracle.

use mojom_codec_core::{
    MojomParse, MojomType, MojomValue, deparse_top_level_value, deserialize_exact, pack_mojom_type,
    serialize,
};

struct Envelope<T>(T);
impl<T: MojomParse> MojomParse for Envelope<T> {
    fn mojom_type() -> MojomType {
        MojomType::Struct {
            field_names: vec!["value".into()],
            fields: vec![T::mojom_type()],
        }
    }
    fn into_mojom_value(self, context: &()) -> MojomValue {
        MojomValue::Struct(vec!["value".into()], vec![self.0.into_mojom_value(context)])
    }
    fn try_from_mojom_value(value: MojomValue, context: &()) -> anyhow::Result<Self> {
        let MojomValue::Struct(_, mut fields) = value else {
            anyhow::bail!("expected struct")
        };
        anyhow::ensure!(fields.len() == 1, "expected one field");
        Ok(Self(T::try_from_mojom_value(
            fields.pop().unwrap(),
            context,
        )?))
    }
}

fn encode<T: MojomParse>(value: T) -> Vec<u8> {
    let (wire, handles, ids) = serialize(Envelope(value), &());
    assert!(handles.is_empty());
    assert_eq!(ids, 0);
    wire
}
fn legacy<T: MojomParse>(field: MojomValue) -> Vec<u8> {
    let ty = pack_mojom_type(&Envelope::<T>::mojom_type());
    let (wire, handles, ids) =
        deparse_top_level_value(MojomValue::Struct(vec!["value".into()], vec![field]), &ty)
            .unwrap();
    assert!(handles.is_empty());
    assert_eq!(ids, 0);
    wire
}
fn decode<T: MojomParse>(wire: &[u8]) -> T {
    deserialize_exact::<(), Envelope<T>>(wire, &mut [], 0, &())
        .unwrap()
        .0
}
fn array_wire(body: &[u8], count: usize) -> Vec<u8> {
    let mut wire = vec![16, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0];
    wire.extend(u32::try_from(body.len() + 8).unwrap().to_le_bytes());
    wire.extend(u32::try_from(count).unwrap().to_le_bytes());
    wire.extend(body);
    wire.resize(wire.len().next_multiple_of(8), 0);
    wire
}
fn random(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

#[test]
fn random_boolean_and_nullable_boolean_wire_matches_legacy() {
    let mut seed = 0x789a_bcd1_5eed_4321;
    for count in (0_usize..=129).chain([255, 256, 257, 1023, 1024, 1025]) {
        let values = (0..count)
            .map(|_| random(&mut seed) & 1 != 0)
            .collect::<Vec<_>>();
        let expected = MojomValue::Array(values.iter().copied().map(MojomValue::Bool).collect());
        let wire = encode(values.clone());
        assert_eq!(wire, legacy::<Vec<bool>>(expected), "bool count={count}");
        assert_eq!(decode::<Vec<bool>>(&wire), values);

        let nullable = values
            .into_iter()
            .map(|value| (random(&mut seed) % 3 != 0).then_some(value))
            .collect::<Vec<_>>();
        let expected = MojomValue::Array(
            nullable
                .iter()
                .map(|value| {
                    MojomValue::Nullable(value.map(|value| Box::new(MojomValue::Bool(value))))
                })
                .collect(),
        );
        let wire = encode(nullable.clone());
        assert_eq!(
            wire,
            legacy::<Vec<Option<bool>>>(expected),
            "nullable bool count={count}"
        );
        assert_eq!(decode::<Vec<Option<bool>>>(&wire), nullable);
    }
}

#[test]
fn random_nullable_wide_values_ignore_dirty_none_slots() {
    let mut seed = 0x0123_4abc_5eed_6789;
    for count in (0_usize..=129).chain([255, 256, 257]) {
        let values = (0..count)
            .map(|_| {
                let value = random(&mut seed);
                (value & 3 != 0).then_some(value)
            })
            .collect::<Vec<Option<u64>>>();
        let expected = MojomValue::Array(
            values
                .iter()
                .map(|value| {
                    MojomValue::Nullable(value.map(|value| Box::new(MojomValue::UInt64(value))))
                })
                .collect(),
        );
        let mut wire = encode(values.clone());
        assert_eq!(wire, legacy::<Vec<Option<u64>>>(expected), "count={count}");
        let value_start = 24 + count.div_ceil(8).next_multiple_of(8);
        for (index, value) in values.iter().enumerate() {
            if value.is_none() {
                let slot = &mut wire[value_start + index * 8..value_start + index * 8 + 8];
                assert_eq!(slot, &[0; 8]);
                slot.copy_from_slice(&random(&mut seed).to_le_bytes());
            }
        }
        assert_eq!(decode::<Vec<Option<u64>>>(&wire), values, "count={count}");
    }
}

#[test]
fn literal_bitmap_wire_keeps_order_and_ignores_unused_tail_bits() {
    let wire = array_wire(&[0xa5, 0x5a, 0xff], 17);
    let expected = vec![
        true, false, true, false, false, true, false, true, false, true, false, true, true, false,
        true, false, true,
    ];
    assert_eq!(decode::<Vec<bool>>(&wire), expected);
    assert_eq!(encode(expected), array_wire(&[0xa5, 0x5a, 1], 17));
    let expected = vec![
        Some(true),
        None,
        Some(false),
        Some(true),
        None,
        Some(false),
        None,
        Some(true),
        Some(true),
    ];
    // Null value bits and high tail bits may be dirty on incoming wire.
    assert_eq!(
        decode::<Vec<Option<bool>>>(&array_wire(&[0xad, 0xff, 0xdb, 0xff], 9)),
        expected
    );
    assert_eq!(encode(expected), array_wire(&[0xad, 1, 0x89, 1], 9));
}
