// Copyright 2026 The Mojo Rust SDK Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Compatibility fixtures for bulk primitive arrays. Expected bytes come from
//! hand-built Mojom frames and the legacy scalar AST, rather than a second
//! invocation of the optimized Rust array conversion.

use mojom_codec_core::{
    MojomParse, MojomType, MojomValue, ParsingErrorType, ParsingResult, PrimitiveArray,
    deparse_top_level_value, deserialize_exact, extract_interface_ids, pack_mojom_type,
    parse_top_level_value, serialize,
};
use ordered_float::OrderedFloat;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;
use std::marker::PhantomData;

struct Envelope<T, W = T>(T, PhantomData<W>);

impl<T: MojomParse<W>, W: 'static> MojomParse for Envelope<T, W> {
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
            anyhow::bail!("expected Envelope struct")
        };
        anyhow::ensure!(fields.len() == 1, "expected one Envelope field");
        Ok(Self(
            T::try_from_mojom_value(fields.pop().unwrap(), context)?,
            PhantomData,
        ))
    }
}

trait LegacyValue {
    fn legacy_value(self) -> MojomValue;
}

macro_rules! legacy_leaf {
    ($ty:ty, $variant:ident) => {
        impl LegacyValue for $ty {
            fn legacy_value(self) -> MojomValue {
                MojomValue::$variant(self)
            }
        }
    };
}
legacy_leaf!(bool, Bool);
legacy_leaf!(i8, Int8);
legacy_leaf!(i16, Int16);
legacy_leaf!(u16, UInt16);
legacy_leaf!(i32, Int32);
legacy_leaf!(u32, UInt32);
legacy_leaf!(i64, Int64);
legacy_leaf!(u64, UInt64);
legacy_leaf!(OrderedFloat<f32>, Float32);
legacy_leaf!(OrderedFloat<f64>, Float64);

impl LegacyValue for f32 {
    fn legacy_value(self) -> MojomValue {
        MojomValue::Float32(OrderedFloat(self))
    }
}

impl LegacyValue for f64 {
    fn legacy_value(self) -> MojomValue {
        MojomValue::Float64(OrderedFloat(self))
    }
}

impl<T: LegacyValue> LegacyValue for Vec<T> {
    fn legacy_value(self) -> MojomValue {
        MojomValue::Array(self.into_iter().map(LegacyValue::legacy_value).collect())
    }
}

impl<T: LegacyValue, const N: usize> LegacyValue for [T; N] {
    fn legacy_value(self) -> MojomValue {
        MojomValue::Array(self.into_iter().map(LegacyValue::legacy_value).collect())
    }
}

impl<T: LegacyValue> LegacyValue for Option<T> {
    fn legacy_value(self) -> MojomValue {
        MojomValue::Nullable(self.map(|value| Box::new(value.legacy_value())))
    }
}

impl<K: LegacyValue, V: LegacyValue> LegacyValue for HashMap<K, V> {
    fn legacy_value(self) -> MojomValue {
        MojomValue::Map(
            self.into_iter()
                .map(|(key, value)| (key.legacy_value(), value.legacy_value()))
                .collect::<BTreeMap<_, _>>(),
        )
    }
}

fn encode<T: MojomParse<W>, W: 'static>(value: T) -> Vec<u8> {
    let (bytes, handles, interface_ids) = serialize(Envelope::<T, W>(value, PhantomData), &());
    assert!(handles.is_empty());
    assert_eq!(interface_ids, 0);
    bytes
}

fn encode_legacy<T: MojomParse<W> + LegacyValue, W: 'static>(value: T) -> Vec<u8> {
    let ty = pack_mojom_type(&Envelope::<T, W>::mojom_type());
    let value = MojomValue::Struct(vec!["value".into()], vec![value.legacy_value()]);
    let (bytes, handles, interface_ids) = deparse_top_level_value(value, &ty).unwrap();
    assert!(handles.is_empty());
    assert_eq!(interface_ids, 0);
    bytes
}

fn decode<T: MojomParse<W>, W: 'static>(wire: &[u8]) -> ParsingResult<T> {
    deserialize_exact::<(), Envelope<T, W>>(wire, &mut [], 0, &()).map(|value| value.0)
}

fn assert_compatible<T, W>(value: T)
where
    T: MojomParse<W> + LegacyValue + Clone + Debug + PartialEq,
    W: 'static,
{
    let optimized = encode::<T, W>(value.clone());
    let legacy = encode_legacy::<T, W>(value.clone());
    assert_eq!(optimized, legacy, "legacy wire changed");
    assert_eq!(decode::<T, W>(&legacy).unwrap(), value);
    assert_eq!(decode::<T, W>(&optimized).unwrap(), value);
}

/// A documented struct header, relative pointer and array header; the caller
/// supplies the raw little-endian/bit-packed payload independently of codec.
fn array_wire(body: &[u8], count: usize) -> Vec<u8> {
    let mut wire = vec![
        16, 0, 0, 0, 0, 0, 0, 0, // struct size / version
        8, 0, 0, 0, 0, 0, 0, 0, // pointer to immediately following array
    ];
    wire.extend(u32::try_from(8 + body.len()).unwrap().to_le_bytes());
    wire.extend(u32::try_from(count).unwrap().to_le_bytes());
    wire.extend(body);
    wire.resize(wire.len().next_multiple_of(8), 0);
    wire
}

macro_rules! integer_compatibility {
    ($name:ident, $ty:ty, $variant:ident) => {
        #[test]
        fn $name() {
            let samples = [<$ty>::MIN, 0, 1, <$ty>::MAX];
            for count in [0, 1, 7, 8, 9, 64] {
                let values = samples
                    .into_iter()
                    .cycle()
                    .take(count)
                    .collect::<Vec<$ty>>();
                assert_compatible::<_, Vec<$ty>>(values.clone());
                let body = values
                    .iter()
                    .flat_map(|value| value.to_le_bytes())
                    .collect::<Vec<_>>();
                let golden = array_wire(&body, values.len());
                assert_eq!(encode::<_, Vec<$ty>>(values.clone()), golden);
                assert_eq!(decode::<Vec<$ty>, Vec<$ty>>(&golden).unwrap(), values);
            }
            assert_compatible::<_, [$ty; 3]>([<$ty>::MIN, 0, <$ty>::MAX]);
            assert_compatible::<[$ty; 0], [$ty; 0]>([]);
            let values = vec![<$ty>::MIN, 1, <$ty>::MAX];
            assert_eq!(
                <Vec<$ty> as MojomParse>::try_from_mojom_value(values.clone().legacy_value(), &())
                    .unwrap(),
                values
            );
            assert_eq!(
                <Vec<$ty> as MojomParse>::try_from_mojom_value(
                    MojomValue::PrimitiveArray(PrimitiveArray::$variant(values.clone())),
                    &()
                )
                .unwrap(),
                values
            );
        }
    };
}

integer_compatibility!(int8_legacy_and_golden_wire, i8, Int8);
integer_compatibility!(int16_legacy_and_golden_wire, i16, Int16);
integer_compatibility!(uint16_legacy_and_golden_wire, u16, UInt16);
integer_compatibility!(int32_legacy_and_golden_wire, i32, Int32);
integer_compatibility!(uint32_legacy_and_golden_wire, u32, UInt32);
integer_compatibility!(int64_legacy_and_golden_wire, i64, Int64);
integer_compatibility!(uint64_legacy_and_golden_wire, u64, UInt64);

#[test]
fn booleans_are_lsb_first_and_cross_byte_boundaries() {
    let pattern = [true, false, true, true, false, false, false, true, true];
    for count in [0, 1, 7, 8, 9, 15, 16, 17, 64, 65] {
        let values = pattern.into_iter().cycle().take(count).collect::<Vec<_>>();
        assert_compatible::<_, Vec<bool>>(values.clone());
        let mut body = vec![0_u8; count.div_ceil(8)];
        for (index, &value) in values.iter().enumerate() {
            if value {
                body[index / 8] |= 1 << (index % 8);
            }
        }
        assert_eq!(
            encode::<_, Vec<bool>>(values.clone()),
            array_wire(&body, count)
        );
    }
    let wire = array_wire(&[0x8d, 0x01], 9);
    assert_eq!(decode::<Vec<bool>, Vec<bool>>(&wire).unwrap(), pattern);
    // High, unused bits in the last bitfield are outside the value and may be set.
    assert_eq!(
        decode::<Vec<bool>, Vec<bool>>(&array_wire(&[0x8d, 0xff], 9)).unwrap(),
        pattern
    );
    assert_compatible::<_, [bool; 9]>(pattern);
    assert_compatible::<[bool; 0], [bool; 0]>([]);
}

#[test]
fn float32_raw_and_ordered_preserve_all_bits() {
    let bits = [
        0x00000000_u32,
        0x80000000,
        0x3f800000,
        0xbf800000,
        0x7f800000,
        0xff800000,
        0x7fc00001,
        0xffc12345,
        0x7f800001,
    ];
    let values = bits.map(f32::from_bits);
    let golden = array_wire(
        &bits
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>(),
        bits.len(),
    );
    for wire in [
        encode::<_, Vec<f32>>(values.to_vec()),
        encode_legacy::<_, Vec<f32>>(values.to_vec()),
        encode::<_, [f32; 9]>(values),
    ] {
        assert_eq!(wire, golden);
        assert_eq!(
            decode::<Vec<f32>, Vec<f32>>(&wire)
                .unwrap()
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            bits
        );
        assert_eq!(
            decode::<[f32; 9], [f32; 9]>(&wire)
                .unwrap()
                .map(f32::to_bits),
            bits
        );
    }
    let ordered = values.map(OrderedFloat);
    for wire in [
        encode::<_, Vec<OrderedFloat<f32>>>(ordered.to_vec()),
        encode_legacy::<_, Vec<OrderedFloat<f32>>>(ordered.to_vec()),
        encode::<_, [OrderedFloat<f32>; 9]>(ordered),
    ] {
        assert_eq!(wire, golden);
        assert_eq!(
            decode::<Vec<OrderedFloat<f32>>, Vec<OrderedFloat<f32>>>(&wire)
                .unwrap()
                .iter()
                .map(|value| value.0.to_bits())
                .collect::<Vec<_>>(),
            bits
        );
        assert_eq!(
            decode::<[OrderedFloat<f32>; 9], [OrderedFloat<f32>; 9]>(&wire)
                .unwrap()
                .map(|value| value.0.to_bits()),
            bits
        );
    }
    // Fixed literal checks the endian convention without using to_le_bytes.
    assert_eq!(
        encode::<_, Vec<f32>>(vec![1.0, -0.0]),
        array_wire(&[0, 0, 128, 63, 0, 0, 0, 128], 2)
    );
}

#[test]
fn float64_raw_and_ordered_preserve_all_bits() {
    let bits = [
        0x0000000000000000_u64,
        0x8000000000000000,
        0x3ff0000000000000,
        0xbff0000000000000,
        0x7ff0000000000000,
        0xfff0000000000000,
        0x7ff8000000000001,
        0xfff8123456789abc,
        0x7ff0000000000001,
    ];
    let values = bits.map(f64::from_bits);
    let golden = array_wire(
        &bits
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>(),
        bits.len(),
    );
    for wire in [
        encode::<_, Vec<f64>>(values.to_vec()),
        encode_legacy::<_, Vec<f64>>(values.to_vec()),
        encode::<_, [f64; 9]>(values),
    ] {
        assert_eq!(wire, golden);
        assert_eq!(
            decode::<Vec<f64>, Vec<f64>>(&wire)
                .unwrap()
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            bits
        );
        assert_eq!(
            decode::<[f64; 9], [f64; 9]>(&wire)
                .unwrap()
                .map(f64::to_bits),
            bits
        );
    }
    let ordered = values.map(OrderedFloat);
    for wire in [
        encode::<_, Vec<OrderedFloat<f64>>>(ordered.to_vec()),
        encode_legacy::<_, Vec<OrderedFloat<f64>>>(ordered.to_vec()),
        encode::<_, [OrderedFloat<f64>; 9]>(ordered),
    ] {
        assert_eq!(wire, golden);
        assert_eq!(
            decode::<Vec<OrderedFloat<f64>>, Vec<OrderedFloat<f64>>>(&wire)
                .unwrap()
                .iter()
                .map(|value| value.0.to_bits())
                .collect::<Vec<_>>(),
            bits
        );
        assert_eq!(
            decode::<[OrderedFloat<f64>; 9], [OrderedFloat<f64>; 9]>(&wire)
                .unwrap()
                .map(|value| value.0.to_bits()),
            bits
        );
    }
    assert_eq!(
        encode::<_, Vec<f64>>(vec![1.0, -0.0]),
        array_wire(&[0, 0, 0, 0, 0, 0, 240, 63, 0, 0, 0, 0, 0, 0, 0, 128], 2)
    );
}

#[test]
fn nested_fixed_and_outer_nullable_primitive_arrays_are_compatible() {
    assert_compatible::<_, Vec<Vec<i16>>>(vec![vec![], vec![-1, i16::MAX], vec![7; 9]]);
    assert_compatible::<_, Vec<[u32; 3]>>(vec![[1, 2, 3], [0, u32::MAX, 7]]);
    assert_compatible::<_, [[i64; 3]; 2]>([[i64::MIN, -1, 0], [1, 7, i64::MAX]]);
    assert_compatible::<_, Option<Vec<u64>>>(Some(vec![0, u64::MAX]));
    assert_compatible::<_, Option<Vec<u64>>>(None::<Vec<u64>>);
    assert_compatible::<_, Vec<Option<Vec<bool>>>>(vec![None, Some(vec![]), Some(vec![true; 9])]);
    assert_compatible::<_, Option<[i8; 9]>>(Some([-1; 9]));

    let nan = f64::from_bits(0x7ff8123456789abc);
    let nested = vec![vec![-0.0_f64, nan], vec![], vec![f64::NEG_INFINITY]];
    let fast = encode::<_, Vec<Vec<f64>>>(nested.clone());
    assert_eq!(fast, encode_legacy::<_, Vec<Vec<f64>>>(nested.clone()));
    let actual = decode::<Vec<Vec<f64>>, Vec<Vec<f64>>>(&fast).unwrap();
    for (actual, expected) in actual.iter().zip(&nested) {
        assert_eq!(
            actual
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn nullable_primitive_elements_keep_tag_bits_and_compact_ast() {
    let values = vec![Some(0x1234_u16), None, Some(0xffff), Some(0)];
    assert_compatible::<_, Vec<Option<u16>>>(values.clone());
    let wire = encode::<_, Vec<Option<u16>>>(values);
    // Four tag bits, then one alignment byte before the uint16 elements.
    assert_eq!(
        wire,
        array_wire(&[0x0d, 0, 0x34, 0x12, 0, 0, 0xff, 0xff, 0, 0], 4)
    );
    let ty = pack_mojom_type(&Envelope::<Vec<Option<u16>>>::mojom_type());
    let (remaining, value) = parse_top_level_value(&wire, &mut [], 0, &ty).unwrap();
    assert!(remaining.is_empty());
    let MojomValue::Struct(_, mut fields) = value else {
        panic!("expected struct")
    };
    assert!(matches!(
        fields.pop().unwrap(),
        MojomValue::NullablePrimitiveArray(_)
    ));
    assert!(matches!(
        <Vec<Option<u16>> as MojomParse>::into_mojom_value(vec![Some(7), None], &()),
        MojomValue::NullablePrimitiveArray(_)
    ));
    assert_compatible::<_, Vec<Option<bool>>>(vec![
        Some(true),
        None,
        Some(false),
        Some(true),
        None,
        Some(false),
        Some(true),
        None,
        Some(true),
    ]);

    let floats = vec![Some(f32::from_bits(0x7fc01234)), None, Some(-0.0_f32)];
    let wire = encode::<_, Vec<Option<f32>>>(floats.clone());
    assert_eq!(wire, encode_legacy::<_, Vec<Option<f32>>>(floats.clone()));
    let bits = |values: Vec<Option<f32>>| {
        values
            .into_iter()
            .map(|value| value.map(f32::to_bits))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        bits(decode::<Vec<Option<f32>>, Vec<Option<f32>>>(&wire).unwrap()),
        bits(floats)
    );
}

#[derive(Clone, Debug, PartialEq)]
struct MappedU16(u16);

impl MojomParse<u16> for MappedU16 {
    fn mojom_type() -> MojomType {
        MojomType::UInt16
    }
    fn into_mojom_value(self, _: &()) -> MojomValue {
        MojomValue::UInt16(self.0 ^ 0xa55a)
    }
    fn try_from_mojom_value(value: MojomValue, _: &()) -> anyhow::Result<Self> {
        let MojomValue::UInt16(value) = value else {
            anyhow::bail!("expected uint16")
        };
        Ok(Self(value ^ 0xa55a))
    }
}

impl LegacyValue for MappedU16 {
    fn legacy_value(self) -> MojomValue {
        MojomValue::UInt16(self.0 ^ 0xa55a)
    }
}

#[derive(Debug)]
struct CheckedI32(i32);

impl MojomParse<i32> for CheckedI32 {
    fn mojom_type() -> MojomType {
        MojomType::Int32
    }
    fn into_mojom_value(self, _: &()) -> MojomValue {
        MojomValue::Int32(self.0)
    }
    fn try_from_mojom_value(value: MojomValue, _: &()) -> anyhow::Result<Self> {
        let MojomValue::Int32(value) = value else {
            anyhow::bail!("expected int32")
        };
        anyhow::ensure!(value != -1, "-1 is forbidden by this typemap");
        Ok(Self(value))
    }
}

#[test]
fn custom_nonidentity_and_fallible_typemaps_are_never_bypassed() {
    let values = vec![MappedU16(0), MappedU16(0xa55a), MappedU16(u16::MAX)];
    assert_compatible::<_, Vec<u16>>(values.clone());
    assert_compatible::<_, [u16; 3]>([MappedU16(0), MappedU16(0xa55a), MappedU16(u16::MAX)]);
    assert_eq!(
        encode::<_, Vec<u16>>(values.clone()),
        array_wire(&[0x5a, 0xa5, 0, 0, 0xa5, 0x5a], 3)
    );
    assert!(matches!(
        <Vec<MappedU16> as MojomParse<Vec<u16>>>::into_mojom_value(values.clone(), &()),
        MojomValue::Array(_)
    ));
    assert_eq!(
        <Vec<MappedU16> as MojomParse<Vec<u16>>>::try_from_mojom_value(
            MojomValue::PrimitiveArray(PrimitiveArray::UInt16(vec![0xa55a, 0, 0x5aa5])),
            &()
        )
        .unwrap(),
        values
    );
    assert_eq!(
        <[MappedU16; 3] as MojomParse<[u16; 3]>>::try_from_mojom_value(
            MojomValue::PrimitiveArray(PrimitiveArray::UInt16(vec![0xa55a, 0, 0x5aa5])),
            &()
        )
        .unwrap(),
        [MappedU16(0), MappedU16(0xa55a), MappedU16(u16::MAX)]
    );

    fn invalid() -> [MojomValue; 2] {
        [
            vec![1_i32, -1, 3].legacy_value(),
            MojomValue::PrimitiveArray(PrimitiveArray::Int32(vec![1, -1, 3])),
        ]
    }
    for value in invalid() {
        assert!(
            <Vec<CheckedI32> as MojomParse<Vec<i32>>>::try_from_mojom_value(value, &()).is_err()
        );
    }
    for value in invalid() {
        assert!(
            <[CheckedI32; 3] as MojomParse<[i32; 3]>>::try_from_mojom_value(value, &()).is_err()
        );
    }
    let wire = array_wire(&[1, 0, 0, 0, 255, 255, 255, 255, 3, 0, 0, 0], 3);
    assert!(matches!(
        decode::<Vec<CheckedI32>, Vec<i32>>(&wire).unwrap_err().ty,
        ParsingErrorType::UnconvertibleValue { .. }
    ));
    assert!(matches!(
        decode::<[CheckedI32; 3], [i32; 3]>(&wire).unwrap_err().ty,
        ParsingErrorType::UnconvertibleValue { .. }
    ));
}

fn map_wire(keys: &[u32], values: &[u32]) -> Vec<u8> {
    let keys = keys
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect::<Vec<_>>();
    let values = values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect::<Vec<_>>();
    let key_array = array_wire(&keys, keys.len() / 4).split_off(16);
    let value_array = array_wire(&values, values.len() / 4).split_off(16);
    let mut wire = vec![
        16, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 24, 0, 0, 0, 0, 0, 0, 0, 16, 0, 0, 0, 0,
        0, 0, 0,
    ];
    wire.extend(u64::try_from(8 + key_array.len()).unwrap().to_le_bytes());
    wire.extend(key_array);
    wire.extend(value_array);
    wire
}

#[test]
fn primitive_map_columns_preserve_validation_and_nested_values() {
    let values = HashMap::from([(0_u32, u32::MAX), (1, 0x12345678), (u32::MAX, 0)]);
    assert_compatible::<_, HashMap<u32, u32>>(values.clone());
    let golden = map_wire(&[0, 1, u32::MAX], &[u32::MAX, 0x12345678, 0]);
    assert_eq!(encode::<_, HashMap<u32, u32>>(values.clone()), golden);
    assert_eq!(
        decode::<HashMap<u32, u32>, HashMap<u32, u32>>(&golden).unwrap(),
        values
    );
    assert_compatible::<_, HashMap<u32, u64>>(HashMap::from([(1_u32, u64::MAX), (7, 0)]));
    assert_compatible::<_, HashMap<u32, Vec<u16>>>(HashMap::from([
        (1_u32, vec![]),
        (7, vec![0, 255, u16::MAX]),
    ]));
    let duplicate = map_wire(&[1, 1], &[10, 20]);
    assert!(matches!(
        decode::<HashMap<u32, u32>, HashMap<u32, u32>>(&duplicate)
            .unwrap_err()
            .ty,
        ParsingErrorType::DuplicateMapKey { .. }
    ));
    let mismatched = map_wire(&[1, 2], &[10]);
    assert!(matches!(
        decode::<HashMap<u32, u32>, HashMap<u32, u32>>(&mismatched)
            .unwrap_err()
            .ty,
        ParsingErrorType::MismatchedMap {
            key_len: 2,
            value_len: 1
        }
    ));
}

#[test]
fn primitive_array_headers_padding_and_truncation_are_validated() {
    let body = [0x34_u8, 0x12, 0xcd, 0xab, 0xff, 0xff];
    let canonical = array_wire(&body, 3);
    for length in 0..canonical.len() {
        assert!(
            decode::<Vec<u16>, Vec<u16>>(&canonical[..length]).is_err(),
            "length={length}"
        );
    }
    for size in [0_u32, 1, 7, 8, 13, 17, u32::MAX] {
        let mut wire = canonical.clone();
        wire[16..20].copy_from_slice(&size.to_le_bytes());
        assert!(decode::<Vec<u16>, Vec<u16>>(&wire).is_err(), "size={size}");
    }
    for count in [4_u32, 9, u32::MAX] {
        let mut wire = canonical.clone();
        wire[20..24].copy_from_slice(&count.to_le_bytes());
        assert!(
            decode::<Vec<u16>, Vec<u16>>(&wire).is_err(),
            "count={count}"
        );
    }
    for size in [15_u32, 19] {
        let mut wire = canonical.clone();
        wire[16..20].copy_from_slice(&size.to_le_bytes());
        wire.resize(16 + (size as usize).next_multiple_of(8), 0xab);
        wire[30..].fill(0xab);
        assert_eq!(
            decode::<Vec<u16>, Vec<u16>>(&wire).unwrap(),
            [0x1234, 0xabcd, 0xffff]
        );
    }
    let mut extra = canonical.clone();
    extra.extend([0; 8]);
    assert!(matches!(
        decode::<Vec<u16>, Vec<u16>>(&extra).unwrap_err().ty,
        ParsingErrorType::TooMuchData { remaining_bytes: 8 }
    ));
    for count in [0, 2, 4] {
        let wire = array_wire(&vec![0; count * 2], count);
        assert!(
            matches!(decode::<[u16; 3], [u16; 3]>(&wire).unwrap_err().ty, ParsingErrorType::WrongArraySize { expected: 3, actual } if actual == count)
        );
        assert!(
            <[u16; 3] as MojomParse>::try_from_mojom_value(
                MojomValue::PrimitiveArray(PrimitiveArray::UInt16(vec![7; count])),
                &()
            )
            .is_err()
        );
        assert!(
            <[u16; 3] as MojomParse>::try_from_mojom_value(vec![7_u16; count].legacy_value(), &())
                .is_err()
        );
    }

    // Bool uses ceil(count / 8), not one byte per element.
    assert!(decode::<Vec<bool>, Vec<bool>>(&array_wire(&[0xff], 9)).is_err());
    let mut enormous_bools = array_wire(&[], 0);
    enormous_bools[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode::<Vec<bool>, Vec<bool>>(&enormous_bools).is_err());
}

#[test]
fn associated_interface_id_array_still_decodes_uint32_values() {
    let mut frame = vec![0xa5; 16];
    frame.extend(array_wire(&[1, 0, 0, 0, 0xfe, 0xff, 0xff, 0xff], 2).split_off(16));
    let (remaining_body, ids) = extract_interface_ids(&frame, 16).unwrap();
    assert_eq!(remaining_body, &[0xa5; 16]);
    assert_eq!(
        ids.into_iter().map(|id| id.get()).collect::<Vec<_>>(),
        [1, 0xfffffffe]
    );
    for invalid in [0_u32, u32::MAX] {
        let mut frame = vec![0; 16];
        frame.extend(array_wire(&invalid.to_le_bytes(), 1).split_off(16));
        assert!(
            matches!(extract_interface_ids(&frame, 16).unwrap_err().ty, ParsingErrorType::InvalidInterfaceId { value, .. } if value == invalid)
        );
    }
}
