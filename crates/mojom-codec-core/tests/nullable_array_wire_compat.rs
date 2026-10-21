// Copyright 2026 The Mojo Rust SDK Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Nullable array compatibility uses the legacy scalar AST as an independent
//! encoder, plus literal Mojom tag/value fixtures. Float checks compare bits:
//! OrderedFloat equality intentionally cannot detect lost NaN payloads or -0.

use mojom_codec_core::{
    MojomParse, MojomType, MojomValue, NullablePrimitiveArray, ParsingErrorType, ParsingResult,
    Predicate, deparse_top_level_value, deserialize_exact, pack_mojom_type, serialize,
};
use ordered_float::OrderedFloat;
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;
use std::marker::PhantomData;

const COUNTS: [usize; 11] = [0, 1, 7, 8, 9, 15, 16, 17, 63, 64, 65];

struct Envelope<T, W = T>(T, PhantomData<W>);

impl<C, T: MojomParse<W, C>, W: 'static> MojomParse<Envelope<T, W>, C> for Envelope<T, W> {
    fn mojom_type() -> MojomType {
        MojomType::Struct {
            field_names: vec!["value".into()],
            fields: vec![<T as MojomParse<W, C>>::mojom_type()],
        }
    }

    fn into_mojom_value(self, context: &C) -> MojomValue {
        MojomValue::Struct(vec!["value".into()], vec![self.0.into_mojom_value(context)])
    }

    fn try_from_mojom_value(value: MojomValue, context: &C) -> anyhow::Result<Self> {
        let MojomValue::Struct(_, mut fields) = value else {
            anyhow::bail!("expected Envelope struct")
        };
        anyhow::ensure!(fields.len() == 1, "expected one field");
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
legacy_leaf!(u8, UInt8);
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
impl<T: LegacyValue> LegacyValue for Option<T> {
    fn legacy_value(self) -> MojomValue {
        MojomValue::Nullable(self.map(|value| Box::new(value.legacy_value())))
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
impl<K: LegacyValue, V: LegacyValue> LegacyValue for HashMap<K, V> {
    fn legacy_value(self) -> MojomValue {
        MojomValue::Map(
            self.into_iter()
                .map(|(key, value)| (key.legacy_value(), value.legacy_value()))
                .collect::<BTreeMap<_, _>>(),
        )
    }
}

fn encode_with_context<C, T: MojomParse<W, C>, W: 'static>(value: T, context: &C) -> Vec<u8> {
    let (wire, handles, ids) =
        serialize::<C, Envelope<T, W>>(Envelope(value, PhantomData), context);
    assert!(handles.is_empty());
    assert_eq!(ids, 0);
    wire
}

fn decode_with_context<C, T: MojomParse<W, C>, W: 'static>(
    wire: &[u8],
    context: &C,
) -> ParsingResult<T> {
    deserialize_exact::<C, Envelope<T, W>>(wire, &mut [], 0, context).map(|value| value.0)
}

fn encode<T: MojomParse<W>, W: 'static>(value: T) -> Vec<u8> {
    encode_with_context::<(), T, W>(value, &())
}

fn encode_legacy<T: MojomParse<W> + LegacyValue, W: 'static>(value: T) -> Vec<u8> {
    let ty = pack_mojom_type(&<Envelope<T, W> as MojomParse>::mojom_type());
    let value = MojomValue::Struct(vec!["value".into()], vec![value.legacy_value()]);
    let (wire, handles, ids) = deparse_top_level_value(value, &ty).unwrap();
    assert!(handles.is_empty());
    assert_eq!(ids, 0);
    wire
}

fn decode<T: MojomParse<W>, W: 'static>(wire: &[u8]) -> ParsingResult<T> {
    decode_with_context::<(), T, W>(wire, &())
}

fn assert_compatible<T, W>(value: T)
where
    T: MojomParse<W> + LegacyValue + Clone + Debug + PartialEq,
    W: 'static,
{
    let fast = encode::<T, W>(value.clone());
    let legacy = encode_legacy::<T, W>(value.clone());
    assert_eq!(fast, legacy, "legacy nullable array wire changed");
    assert_eq!(decode::<T, W>(&legacy).unwrap(), value);
    assert_eq!(decode::<T, W>(&fast).unwrap(), value);
}

/// The body is supplied by the test, not computed by the production packer.
fn array_wire(body: &[u8], count: usize) -> Vec<u8> {
    let mut wire = vec![16, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0];
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
            for count in COUNTS {
                for mode in 0..3 {
                    let values = (0..count)
                        .map(|index| {
                            if mode == 0 || (mode == 2 && index % 3 == 1) {
                                None
                            } else {
                                Some(samples[index % samples.len()])
                            }
                        })
                        .collect::<Vec<Option<$ty>>>();
                    assert_compatible::<_, Vec<Option<$ty>>>(values.clone());
                    assert!(matches!(
                        <Vec<Option<$ty>> as MojomParse>::into_mojom_value(values.clone(), &()),
                        MojomValue::NullablePrimitiveArray(NullablePrimitiveArray::$variant(_))
                    ));
                    assert_eq!(
                        <Vec<Option<$ty>> as MojomParse>::try_from_mojom_value(
                            values.clone().legacy_value(),
                            &()
                        )
                        .unwrap(),
                        values
                    );
                    assert_eq!(
                        <Vec<Option<$ty>> as MojomParse>::try_from_mojom_value(
                            MojomValue::NullablePrimitiveArray(NullablePrimitiveArray::$variant(
                                values.clone()
                            )),
                            &()
                        )
                        .unwrap(),
                        values
                    );
                }
            }
            assert_compatible::<_, [Option<$ty>; 3]>([Some(<$ty>::MIN), None, Some(<$ty>::MAX)]);
            assert_compatible::<[Option<$ty>; 0], [Option<$ty>; 0]>([]);
            assert!(matches!(
                <[Option<$ty>; 3] as MojomParse>::into_mojom_value(
                    [Some(<$ty>::MIN), None, Some(<$ty>::MAX)],
                    &()
                ),
                MojomValue::NullablePrimitiveArray(NullablePrimitiveArray::$variant(_))
            ));
        }
    };
}

integer_compatibility!(nullable_int8_legacy_wire, i8, Int8);
integer_compatibility!(nullable_uint8_legacy_wire, u8, UInt8);
integer_compatibility!(nullable_int16_legacy_wire, i16, Int16);
integer_compatibility!(nullable_uint16_legacy_wire, u16, UInt16);
integer_compatibility!(nullable_int32_legacy_wire, i32, Int32);
integer_compatibility!(nullable_uint32_legacy_wire, u32, UInt32);
integer_compatibility!(nullable_int64_legacy_wire, i64, Int64);
integer_compatibility!(nullable_uint64_legacy_wire, u64, UInt64);

#[test]
fn nullable_bool_tag_and_value_bitmaps_are_separate_lsb_first_sequences() {
    for count in COUNTS {
        for mode in 0..3 {
            let values = (0..count)
                .map(|index| {
                    if mode == 0 || (mode == 2 && index % 3 == 1) {
                        None
                    } else {
                        Some(index % 2 == 0)
                    }
                })
                .collect::<Vec<_>>();
            assert_compatible::<_, Vec<Option<bool>>>(values.clone());
            assert!(matches!(
                <Vec<Option<bool>> as MojomParse>::into_mojom_value(values, &()),
                MojomValue::NullablePrimitiveArray(NullablePrimitiveArray::Bool(_))
            ));
        }
    }
    let values = [
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
    let golden = array_wire(&[0xad, 0x01, 0x89, 0x01], 9);
    assert_eq!(encode::<_, Vec<Option<bool>>>(values.to_vec()), golden);
    assert_eq!(
        encode_legacy::<_, Vec<Option<bool>>>(values.to_vec()),
        golden
    );
    assert_eq!(
        decode::<Vec<Option<bool>>, Vec<Option<bool>>>(&golden).unwrap(),
        values
    );
    assert_compatible::<_, [Option<bool>; 9]>(values);
    assert_compatible::<[Option<bool>; 0], [Option<bool>; 0]>([]);
    // Null value bits and unused bits after the last element are ignored.
    assert_eq!(
        decode::<Vec<Option<bool>>, Vec<Option<bool>>>(&array_wire(&[0xad, 0xff, 0xdb, 0xff], 9))
            .unwrap(),
        values
    );
    assert_eq!(
        encode::<_, Vec<Option<bool>>>(vec![None::<bool>; 9]),
        array_wire(&[0, 0, 0, 0], 9)
    );
}

#[test]
fn nullable_integer_golden_alignment_and_null_slots() {
    assert_eq!(
        encode::<_, Vec<Option<u8>>>(vec![Some(1), None, Some(0xfe)]),
        array_wire(&[5, 1, 0, 0xfe], 3)
    );
    let u16_wire = array_wire(&[5, 0, 1, 0, 0, 0, 0x34, 0x12], 3);
    assert_eq!(
        encode::<_, Vec<Option<u16>>>(vec![Some(1), None, Some(0x1234)]),
        u16_wire
    );
    let u32_wire = array_wire(
        &[5, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0xef, 0xcd, 0xab, 0x89],
        3,
    );
    assert_eq!(
        encode::<_, Vec<Option<u32>>>(vec![Some(1), None, Some(0x89abcdef)]),
        u32_wire
    );
    let u64_wire = array_wire(
        &[
            5, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 7, 6, 5, 4,
            3, 2, 1,
        ],
        3,
    );
    assert_eq!(
        encode::<_, Vec<Option<u64>>>(vec![Some(1), None, Some(0x0102030405060708)]),
        u64_wire
    );
    let mut dirty = u16_wire;
    dirty[25] = 0xab; // alignment byte
    dirty[28..30].fill(0xff); // None's uint16 slot
    assert_eq!(
        decode::<Vec<Option<u16>>, Vec<Option<u16>>>(&dirty).unwrap(),
        [Some(1), None, Some(0x1234)]
    );
    let mut dirty = u32_wire;
    dirty[25..28].fill(0xab);
    dirty[32..36].fill(0xff);
    assert_eq!(
        decode::<Vec<Option<u32>>, Vec<Option<u32>>>(&dirty).unwrap(),
        [Some(1), None, Some(0x89abcdef)]
    );
    let mut dirty = u64_wire;
    dirty[25..32].fill(0xab);
    dirty[40..48].fill(0xff);
    assert_eq!(
        decode::<Vec<Option<u64>>, Vec<Option<u64>>>(&dirty).unwrap(),
        [Some(1), None, Some(0x0102030405060708)]
    );
}

macro_rules! float_compatibility {
    ($name:ident, $ty:ty, $bits_ty:ty, $bits:expr) => {
        #[test]
        fn $name() {
            let samples: [$bits_ty; 8] = $bits;
            for count in COUNTS {
                for mode in 0..3 {
                    let values = (0..count)
                        .map(|index| {
                            if mode == 0 || (mode == 2 && index % 3 == 1) {
                                None
                            } else {
                                Some(<$ty>::from_bits(samples[index % samples.len()]))
                            }
                        })
                        .collect::<Vec<Option<$ty>>>();
                    let expected_bits = values
                        .iter()
                        .map(|value| value.map(<$ty>::to_bits))
                        .collect::<Vec<_>>();
                    let wire = encode::<_, Vec<Option<$ty>>>(values.clone());
                    assert!(matches!(
                        <Vec<Option<$ty>> as MojomParse>::into_mojom_value(values.clone(), &()),
                        MojomValue::NullablePrimitiveArray(_)
                    ));
                    assert_eq!(wire, encode_legacy::<_, Vec<Option<$ty>>>(values.clone()));
                    assert_eq!(
                        decode::<Vec<Option<$ty>>, Vec<Option<$ty>>>(&wire)
                            .unwrap()
                            .into_iter()
                            .map(|value| value.map(<$ty>::to_bits))
                            .collect::<Vec<_>>(),
                        expected_bits
                    );
                    let ordered = values
                        .iter()
                        .map(|value| value.map(OrderedFloat))
                        .collect::<Vec<_>>();
                    assert_eq!(
                        wire,
                        encode::<_, Vec<Option<OrderedFloat<$ty>>>>(ordered.clone())
                    );
                    assert_eq!(
                        wire,
                        encode_legacy::<_, Vec<Option<OrderedFloat<$ty>>>>(ordered)
                    );
                    assert_eq!(
                        decode::<Vec<Option<OrderedFloat<$ty>>>, Vec<Option<OrderedFloat<$ty>>>>(
                            &wire
                        )
                        .unwrap()
                        .into_iter()
                        .map(|value| value.map(|value| value.0.to_bits()))
                        .collect::<Vec<_>>(),
                        expected_bits
                    );
                }
            }
            let fixed = [
                Some(<$ty>::from_bits(samples[1])),
                None,
                Some(<$ty>::from_bits(samples[5])),
            ];
            let wire = encode::<_, [Option<$ty>; 3]>(fixed);
            assert_eq!(wire, encode_legacy::<_, [Option<$ty>; 3]>(fixed));
            assert_eq!(
                decode::<[Option<$ty>; 3], [Option<$ty>; 3]>(&wire)
                    .unwrap()
                    .map(|value| value.map(<$ty>::to_bits)),
                fixed.map(|value| value.map(<$ty>::to_bits))
            );
            let ordered = fixed.map(|value| value.map(OrderedFloat));
            assert_eq!(wire, encode::<_, [Option<OrderedFloat<$ty>>; 3]>(ordered));
            assert_eq!(
                decode::<[Option<OrderedFloat<$ty>>; 3], [Option<OrderedFloat<$ty>>; 3]>(&wire)
                    .unwrap()
                    .map(|value| value.map(|value| value.0.to_bits())),
                fixed.map(|value| value.map(<$ty>::to_bits))
            );
        }
    };
}

float_compatibility!(
    nullable_float32_preserves_nan_payload_and_signed_zero,
    f32,
    u32,
    [
        0, 0x80000000, 0x7f800000, 0xff800000, 0x7fc00001, 0x7fc01234, 0xffc12345, 0x7f800001
    ]
);
float_compatibility!(
    nullable_float64_preserves_nan_payload_and_signed_zero,
    f64,
    u64,
    [
        0,
        0x8000000000000000,
        0x7ff0000000000000,
        0xfff0000000000000,
        0x7ff8000000000001,
        0x7ff8123456789abc,
        0xfff8123456789abc,
        0x7ff0000000000001
    ]
);

#[test]
fn nullable_float_literal_golden_and_dirty_none_slot() {
    let f32_wire = array_wire(
        &[5, 0, 0, 0, 0, 0, 0, 128, 0, 0, 0, 0, 0x34, 0x12, 0xc0, 0x7f],
        3,
    );
    assert_eq!(
        encode::<_, Vec<Option<f32>>>(vec![Some(-0.0), None, Some(f32::from_bits(0x7fc01234))]),
        f32_wire
    );
    let mut dirty = f32_wire;
    dirty[32..36].copy_from_slice(&0x7f800001_u32.to_le_bytes());
    assert_eq!(
        decode::<Vec<Option<f32>>, Vec<Option<f32>>>(&dirty)
            .unwrap()
            .into_iter()
            .map(|value| value.map(f32::to_bits))
            .collect::<Vec<_>>(),
        [Some(0x80000000), None, Some(0x7fc01234)]
    );
    let f64_wire = array_wire(
        &[
            5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 0, 0, 0, 0, 0, 0, 0, 0, 0xbc, 0x9a,
            0x78, 0x56, 0x34, 0x12, 0xf8, 0x7f,
        ],
        3,
    );
    assert_eq!(
        encode::<_, Vec<Option<f64>>>(vec![
            Some(-0.0),
            None,
            Some(f64::from_bits(0x7ff8123456789abc))
        ]),
        f64_wire
    );
}

#[test]
fn nullable_arrays_nested_fixed_outer_nullable_and_map_values() {
    assert_compatible::<_, Vec<Vec<Option<u16>>>>(vec![
        vec![],
        vec![Some(1), None],
        vec![None; 17],
    ]);
    assert_compatible::<_, [[Option<u32>; 3]; 2]>([
        [Some(1), None, Some(u32::MAX)],
        [None, Some(7), None],
    ]);
    assert_compatible::<_, Option<Vec<Option<i64>>>>(Some(vec![
        Some(i64::MIN),
        None,
        Some(i64::MAX),
    ]));
    assert_compatible::<_, Option<Vec<Option<i64>>>>(None::<Vec<Option<i64>>>);
    assert_compatible::<_, HashMap<u32, Option<u64>>>(HashMap::from([
        (1_u32, None),
        (2, Some(u64::MAX)),
    ]));
    assert_compatible::<_, HashMap<u32, Vec<Option<bool>>>>(HashMap::from([
        (1_u32, vec![]),
        (2, vec![Some(true), None, Some(false)]),
    ]));
    assert_compatible::<_, Vec<Option<Vec<Option<i8>>>>>(vec![
        None,
        Some(vec![]),
        Some(vec![None, Some(-1)]),
    ]);
}

struct ConversionContext {
    mask: u32,
    encode_calls: Cell<usize>,
    decode_calls: Cell<usize>,
    reject: Option<u32>,
}

impl ConversionContext {
    fn new(reject: Option<u32>) -> Self {
        Self {
            mask: 0xa55a1234,
            encode_calls: Cell::new(0),
            decode_calls: Cell::new(0),
            reject,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ContextValue(u32);

impl MojomParse<u32, ConversionContext> for ContextValue {
    fn mojom_type() -> MojomType {
        MojomType::UInt32
    }
    fn into_mojom_value(self, context: &ConversionContext) -> MojomValue {
        context.encode_calls.set(context.encode_calls.get() + 1);
        MojomValue::UInt32(self.0 ^ context.mask)
    }
    fn try_from_mojom_value(
        value: MojomValue,
        context: &ConversionContext,
    ) -> anyhow::Result<Self> {
        context.decode_calls.set(context.decode_calls.get() + 1);
        let MojomValue::UInt32(value) = value else {
            anyhow::bail!("expected uint32")
        };
        let value = value ^ context.mask;
        anyhow::ensure!(context.reject != Some(value), "value rejected by context");
        Ok(Self(value))
    }
}

#[test]
fn contextual_typemap_converts_some_once_and_never_converts_none() {
    let context = ConversionContext::new(None);
    let values = vec![Some(ContextValue(1)), None, Some(ContextValue(7))];
    let wire = encode_with_context::<_, _, Vec<Option<u32>>>(values.clone(), &context);
    assert_eq!(context.encode_calls.get(), 2);
    // Literal slots are 1^mask, zero for None, 7^mask in little endian.
    assert_eq!(
        wire,
        array_wire(
            &[
                5, 0, 0, 0, 0x35, 0x12, 0x5a, 0xa5, 0, 0, 0, 0, 0x33, 0x12, 0x5a, 0xa5
            ],
            3
        )
    );
    assert_eq!(
        decode_with_context::<_, Vec<Option<ContextValue>>, Vec<Option<u32>>>(&wire, &context)
            .unwrap(),
        values
    );
    assert_eq!(context.decode_calls.get(), 2);
    for value in [
        vec![Some(context.mask ^ 1), None, Some(context.mask ^ 7)].legacy_value(),
        MojomValue::NullablePrimitiveArray(NullablePrimitiveArray::UInt32(vec![
            Some(context.mask ^ 1),
            None,
            Some(context.mask ^ 7),
        ])),
    ] {
        context.decode_calls.set(0);
        let actual = <[Option<ContextValue>; 3] as MojomParse<
            [Option<u32>; 3],
            ConversionContext,
        >>::try_from_mojom_value(value, &context)
        .unwrap();
        assert_eq!(actual, [Some(ContextValue(1)), None, Some(ContextValue(7))]);
        assert_eq!(context.decode_calls.get(), 2);
    }
    let rejecting_zero = ConversionContext::new(Some(0));
    let none = encode_with_context::<_, _, [Option<u32>; 3]>(
        [None::<ContextValue>, None, None],
        &rejecting_zero,
    );
    assert_eq!(rejecting_zero.encode_calls.get(), 0);
    assert_eq!(
        decode_with_context::<_, [Option<ContextValue>; 3], [Option<u32>; 3]>(
            &none,
            &rejecting_zero
        )
        .unwrap(),
        [None, None, None]
    );
    assert_eq!(rejecting_zero.decode_calls.get(), 0);
}

#[test]
fn fallible_contextual_typemap_propagates_errors_without_panics() {
    let context = ConversionContext::new(Some(7));
    fn invalid(context: &ConversionContext) -> [MojomValue; 2] {
        let values = vec![Some(context.mask ^ 1), None, Some(context.mask ^ 7)];
        [
            values.clone().legacy_value(),
            MojomValue::NullablePrimitiveArray(NullablePrimitiveArray::UInt32(values)),
        ]
    }
    for value in invalid(&context) {
        context.decode_calls.set(0);
        assert!(<Vec<Option<ContextValue>> as MojomParse<Vec<Option<u32>>, ConversionContext>>::try_from_mojom_value(value, &context).is_err());
        assert_eq!(context.decode_calls.get(), 2);
    }
    for value in invalid(&context) {
        context.decode_calls.set(0);
        assert!(<[Option<ContextValue>; 3] as MojomParse<[Option<u32>; 3], ConversionContext>>::try_from_mojom_value(value, &context).is_err());
        assert_eq!(context.decode_calls.get(), 2);
    }
    let wire = encode_with_context::<_, _, Vec<Option<u32>>>(
        vec![Some(ContextValue(1)), None, Some(ContextValue(7))],
        &context,
    );
    assert!(matches!(
        decode_with_context::<_, Vec<Option<ContextValue>>, Vec<Option<u32>>>(&wire, &context)
            .unwrap_err()
            .ty,
        ParsingErrorType::UnconvertibleValue { .. }
    ));
    assert!(matches!(
        decode_with_context::<_, [Option<ContextValue>; 3], [Option<u32>; 3]>(&wire, &context)
            .unwrap_err()
            .ty,
        ParsingErrorType::UnconvertibleValue { .. }
    ));
}

#[derive(Debug, PartialEq)]
struct StrictEnum(i32);
static ENUM_VALID: fn(i32) -> bool = |value| matches!(value, 0 | 1);

impl MojomParse for StrictEnum {
    fn mojom_type() -> MojomType {
        MojomType::Enum {
            is_valid: Predicate::new::<Self>(&ENUM_VALID),
        }
    }
    fn into_mojom_value(self, _: &()) -> MojomValue {
        MojomValue::Enum(self.0)
    }
    fn try_from_mojom_value(value: MojomValue, _: &()) -> anyhow::Result<Self> {
        let MojomValue::Enum(value) = value else {
            anyhow::bail!("expected enum")
        };
        anyhow::ensure!(ENUM_VALID(value), "invalid enum");
        Ok(Self(value))
    }
}

#[test]
fn nullable_enum_still_validates_the_discriminant_even_for_none() {
    // Legacy parses/validates the scalar before applying the nullable tag.
    for tag in [0_u8, 1] {
        let wire = array_wire(&[tag, 0, 0, 0, 2, 0, 0, 0], 1);
        assert!(matches!(
            decode::<Vec<Option<StrictEnum>>, Vec<Option<StrictEnum>>>(&wire)
                .unwrap_err()
                .ty,
            ParsingErrorType::InvalidDiscriminant { value: 2 }
        ));
    }
    assert_eq!(
        decode::<Vec<Option<StrictEnum>>, Vec<Option<StrictEnum>>>(&array_wire(
            &[0, 0, 0, 0, 1, 0, 0, 0],
            1
        ))
        .unwrap(),
        [None]
    );
    assert_eq!(
        decode::<Vec<Option<StrictEnum>>, Vec<Option<StrictEnum>>>(&array_wire(
            &[1, 0, 0, 0, 1, 0, 0, 0],
            1
        ))
        .unwrap(),
        [Some(StrictEnum(1))]
    );
}

#[test]
fn nullable_array_headers_truncation_padding_and_fixed_lengths() {
    let canonical = array_wire(&[5, 0, 1, 0, 0, 0, 0x34, 0x12], 3);
    for length in 0..canonical.len() {
        assert!(
            decode::<Vec<Option<u16>>, Vec<Option<u16>>>(&canonical[..length]).is_err(),
            "length={length}"
        );
    }
    for size in [0_u32, 1, 7, 8, 9, 15, 17, u32::MAX] {
        let mut wire = canonical.clone();
        wire[16..20].copy_from_slice(&size.to_le_bytes());
        assert!(
            decode::<Vec<Option<u16>>, Vec<Option<u16>>>(&wire).is_err(),
            "size={size}"
        );
    }
    for count in [4_u32, 9, u32::MAX] {
        let mut wire = canonical.clone();
        wire[20..24].copy_from_slice(&count.to_le_bytes());
        assert!(
            decode::<Vec<Option<u16>>, Vec<Option<u16>>>(&wire).is_err(),
            "count={count}"
        );
    }
    // Both declared reserved body bytes and final alignment padding are accepted.
    for size in [17_u32, 19, 24] {
        let mut wire = canonical.clone();
        wire[16..20].copy_from_slice(&size.to_le_bytes());
        wire.resize(16 + (size as usize).next_multiple_of(8), 0xab);
        assert_eq!(
            decode::<Vec<Option<u16>>, Vec<Option<u16>>>(&wire).unwrap(),
            [Some(1), None, Some(0x1234)]
        );
        wire.pop();
        assert!(decode::<Vec<Option<u16>>, Vec<Option<u16>>>(&wire).is_err());
    }
    for count in [0, 2, 4] {
        let wire = encode::<_, Vec<Option<u16>>>(vec![None::<u16>; count]);
        assert!(
            matches!(decode::<[Option<u16>; 3], [Option<u16>; 3]>(&wire).unwrap_err().ty, ParsingErrorType::WrongArraySize { expected: 3, actual } if actual == count)
        );
        assert!(
            <[Option<u16>; 3] as MojomParse>::try_from_mojom_value(
                MojomValue::NullablePrimitiveArray(NullablePrimitiveArray::UInt16(vec![
                    None;
                    count
                ])),
                &()
            )
            .is_err()
        );
        assert!(
            <[Option<u16>; 3] as MojomParse>::try_from_mojom_value(
                vec![None::<u16>; count].legacy_value(),
                &()
            )
            .is_err()
        );
    }
    let mut extra = canonical;
    extra.extend([0; 8]);
    assert!(matches!(
        decode::<Vec<Option<u16>>, Vec<Option<u16>>>(&extra)
            .unwrap_err()
            .ty,
        ParsingErrorType::TooMuchData { remaining_bytes: 8 }
    ));
    let mut huge_bools = array_wire(&[], 0);
    huge_bools[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode::<Vec<Option<bool>>, Vec<Option<bool>>>(&huge_bools).is_err());
    assert!(
        decode::<Vec<Option<bool>>, Vec<Option<bool>>>(&array_wire(&[0xff, 1, 0xff], 9)).is_err()
    );
    // The last case contains tag/value bytes but lacks the final alignment.
    let bools = array_wire(&[0xff, 1, 0xff, 1], 9);
    assert!(decode::<Vec<Option<bool>>, Vec<Option<bool>>>(&bools[..bools.len() - 1]).is_err());
}
