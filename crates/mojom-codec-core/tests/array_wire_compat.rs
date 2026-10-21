// Copyright 2026 The Mojo Rust SDK Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Wire compatibility tests for the compact uint8-array representation.
//!
//! The legacy AST encoder is exercised independently of the Rust Vec/array
//! conversions. Small hand-written wire messages additionally check the
//! protocol layout without using either encoder as the expected result.

use mojom_codec_core::{
    MojomParse, MojomType, MojomValue, ParsingErrorType, ParsingResult, deparse_top_level_value,
    deserialize_exact, pack_mojom_type, parse_top_level_value, serialize,
};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;
use std::marker::PhantomData;

/// Mojom messages must have a struct at their root. W permits testing the
/// same field conversion used by a generated typemapped struct.
struct Envelope<T, W = T> {
    value: T,
    wire_as: PhantomData<W>,
}

impl<T, W> Envelope<T, W> {
    fn new(value: T) -> Self {
        Self {
            value,
            wire_as: PhantomData,
        }
    }
}

impl<T: MojomParse<W>, W: 'static> MojomParse for Envelope<T, W> {
    fn mojom_type() -> MojomType {
        MojomType::Struct {
            field_names: vec!["value".into()],
            fields: vec![T::mojom_type()],
        }
    }

    fn into_mojom_value(self, context: &()) -> MojomValue {
        MojomValue::Struct(
            vec!["value".into()],
            vec![self.value.into_mojom_value(context)],
        )
    }

    fn try_from_mojom_value(value: MojomValue, context: &()) -> anyhow::Result<Self> {
        let MojomValue::Struct(_, mut fields) = value else {
            anyhow::bail!("expected an Envelope struct")
        };
        anyhow::ensure!(fields.len() == 1, "expected one Envelope field");
        Ok(Self::new(T::try_from_mojom_value(
            fields.pop().unwrap(),
            context,
        )?))
    }
}

/// Build the pre-optimization AST, including an Array node per byte array.
/// This deliberately does not call the production MojomParse conversions.
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
legacy_leaf!(u8, UInt8);
legacy_leaf!(u16, UInt16);
legacy_leaf!(bool, Bool);

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

impl LegacyValue for HashMap<u8, u8> {
    fn legacy_value(self) -> MojomValue {
        MojomValue::Map(
            self.into_iter()
                .map(|(key, value)| (MojomValue::UInt8(key), MojomValue::UInt8(value)))
                .collect::<BTreeMap<_, _>>(),
        )
    }
}

fn encode<T: MojomParse<W>, W: 'static>(value: T) -> Vec<u8> {
    let (bytes, handles, interface_ids_offset) = serialize(Envelope::<T, W>::new(value), &());
    assert!(handles.is_empty());
    assert_eq!(interface_ids_offset, 0);
    bytes
}

fn encode_legacy<T: MojomParse<W> + LegacyValue, W: 'static>(value: T) -> Vec<u8> {
    let value = MojomValue::Struct(vec!["value".into()], vec![value.legacy_value()]);
    let ty = pack_mojom_type(&Envelope::<T, W>::mojom_type());
    let (bytes, handles, interface_ids_offset) = deparse_top_level_value(value, &ty).unwrap();
    assert!(handles.is_empty());
    assert_eq!(interface_ids_offset, 0);
    bytes
}

fn decode<T: MojomParse<W>, W: 'static>(bytes: &[u8]) -> ParsingResult<T> {
    deserialize_exact::<(), Envelope<T, W>>(bytes, &mut [], 0, &()).map(|value| value.value)
}

fn assert_compatible<T, W>(value: T)
where
    T: MojomParse<W> + LegacyValue + Clone + Debug + PartialEq,
    W: 'static,
{
    let optimized = encode::<T, W>(value.clone());
    let legacy = encode_legacy::<T, W>(value.clone());
    assert_eq!(optimized, legacy, "wire encoding changed");
    assert_eq!(decode::<T, W>(&legacy).unwrap(), value);
    assert_eq!(decode::<T, W>(&optimized).unwrap(), value);
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

/// Independent, intentionally narrow reader for a struct containing one
/// nonnullable uint8 array, using the documented Mojom header/pointer layout.
fn read_legacy_byte_wire(bytes: &[u8]) -> Vec<u8> {
    assert_eq!(read_u32(bytes, 0), 16);
    assert_eq!(read_u32(bytes, 4), 0);
    assert_eq!(read_u64(bytes, 8), 8);
    let declared_size = read_u32(bytes, 16) as usize;
    let count = read_u32(bytes, 20) as usize;
    assert!(declared_size >= 8 + count);
    assert_eq!(bytes.len(), 16 + declared_size.next_multiple_of(8));
    bytes[24..24 + count].to_vec()
}

fn byte_wire(bytes: &[u8]) -> Vec<u8> {
    let mut wire = vec![
        16, 0, 0, 0, 0, 0, 0, 0, // struct size and version
        8, 0, 0, 0, 0, 0, 0, 0, // relative pointer to array
    ];
    wire.extend(u32::try_from(8 + bytes.len()).unwrap().to_le_bytes());
    wire.extend(u32::try_from(bytes.len()).unwrap().to_le_bytes());
    wire.extend(bytes);
    wire.resize(wire.len().next_multiple_of(8), 0);
    wire
}

#[test]
fn known_byte_array_wire_matches_both_representations() {
    let golden = [
        16, 0, 0, 0, 0, 0, 0, 0, // struct header
        8, 0, 0, 0, 0, 0, 0, 0, // array pointer
        11, 0, 0, 0, 3, 0, 0, 0, // unpadded array byte size and count
        0, 128, 255, 0, 0, 0, 0, 0, // elements and alignment padding
    ];
    let bytes = vec![0, 128, 255];
    assert_eq!(encode::<_, Vec<u8>>(bytes.clone()), golden);
    assert_eq!(encode_legacy::<_, Vec<u8>>(bytes.clone()), golden);
    assert_eq!(byte_wire(&bytes), golden);
    assert_eq!(decode::<Vec<u8>, Vec<u8>>(&golden).unwrap(), bytes);
    assert_eq!(read_legacy_byte_wire(&golden), bytes);

    let empty = [
        16, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0,
    ];
    assert_eq!(encode::<_, Vec<u8>>(Vec::<u8>::new()), empty);
    assert_eq!(
        decode::<Vec<u8>, Vec<u8>>(&empty).unwrap(),
        Vec::<u8>::new()
    );
}

#[test]
fn byte_array_boundaries_preserve_legacy_wire_and_decode() {
    for count in [0, 1, 7, 8, 9, 64, 1024 * 1024] {
        let bytes = (0..count)
            .map(|index| (index as u8).wrapping_mul(37).wrapping_add(11))
            .collect::<Vec<_>>();
        let optimized = encode::<_, Vec<u8>>(bytes.clone());
        let legacy = encode_legacy::<_, Vec<u8>>(bytes.clone());
        assert_eq!(optimized, legacy, "count={count}");
        assert_eq!(optimized, byte_wire(&bytes), "count={count}");
        assert_eq!(read_legacy_byte_wire(&optimized), bytes, "count={count}");
        assert_eq!(decode::<Vec<u8>, Vec<u8>>(&legacy).unwrap(), bytes);
    }
}

#[test]
fn byte_array_ast_conversion_accepts_old_and_new_values() {
    let bytes = vec![0, 1, 127, 128, 255];
    assert!(matches!(
        <Vec<u8> as MojomParse>::into_mojom_value(bytes.clone(), &()),
        MojomValue::ByteArray(value) if value == bytes
    ));
    for value in [
        bytes.clone().legacy_value(),
        MojomValue::ByteArray(bytes.clone()),
    ] {
        assert_eq!(
            <Vec<u8> as MojomParse>::try_from_mojom_value(value, &()).unwrap(),
            bytes
        );
    }
    for value in [
        bytes.clone().legacy_value(),
        MojomValue::ByteArray(bytes.clone()),
    ] {
        assert_eq!(
            <[u8; 5] as MojomParse>::try_from_mojom_value(value, &()).unwrap(),
            [0, 1, 127, 128, 255]
        );
    }
    assert!(matches!(
        <[u8; 0] as MojomParse>::into_mojom_value([], &()),
        MojomValue::ByteArray(value) if value.is_empty()
    ));
}

#[test]
fn nested_nullable_and_fixed_arrays_keep_the_same_wire() {
    assert_compatible::<_, Vec<Vec<u8>>>(vec![vec![], vec![1], vec![2; 9], vec![255; 8]]);
    assert_compatible::<_, Option<Vec<u8>>>(Some(vec![0, 128, 255]));
    assert_compatible::<_, Option<Vec<u8>>>(None::<Vec<u8>>);
    assert_compatible::<_, Vec<Option<Vec<u8>>>>(vec![None, Some(vec![]), Some(vec![7; 9])]);
    assert_compatible::<[u8; 0], [u8; 0]>([]);
    assert_compatible::<_, [u8; 1]>([255]);
    assert_compatible::<_, [u8; 7]>([1; 7]);
    assert_compatible::<_, [u8; 8]>([2; 8]);
    assert_compatible::<_, [u8; 9]>([3; 9]);
    assert_compatible::<_, [u8; 64]>([4; 64]);
    assert_compatible::<_, [[u8; 9]; 2]>([[1; 9], [255; 9]]);
    assert_compatible::<_, Option<[u8; 9]>>(Some([7; 9]));
}

#[test]
fn other_primitive_arrays_keep_their_element_layouts() {
    let u16s = vec![0x0102, 0x80ff, 0x0000];
    assert_compatible::<_, Vec<u16>>(u16s.clone());
    let wire = encode::<_, Vec<u16>>(u16s);
    assert_eq!(&wire[16..24], &[14, 0, 0, 0, 3, 0, 0, 0]);
    assert_eq!(&wire[24..32], &[2, 1, 255, 128, 0, 0, 0, 0]);

    let bools = vec![true, false, true, true, false, false, false, true, true];
    assert_compatible::<_, Vec<bool>>(bools.clone());
    let wire = encode::<_, Vec<bool>>(bools);
    assert_eq!(&wire[16..24], &[10, 0, 0, 0, 9, 0, 0, 0]);
    assert_eq!(&wire[24..32], &[0x8d, 0x01, 0, 0, 0, 0, 0, 0]);

    let nullable = vec![Some(0x11_u8), None, Some(0xff), Some(0)];
    assert_compatible::<_, Vec<Option<u8>>>(nullable.clone());
    let wire = encode::<_, Vec<Option<u8>>>(nullable);
    assert_eq!(&wire[16..24], &[13, 0, 0, 0, 4, 0, 0, 0]);
    assert_eq!(&wire[24..32], &[0x0d, 0x11, 0, 0xff, 0, 0, 0, 0]);
}

#[derive(Clone, Debug, PartialEq)]
struct MappedByte(u8);

impl MojomParse<u8> for MappedByte {
    fn mojom_type() -> MojomType {
        MojomType::UInt8
    }

    fn into_mojom_value(self, _: &()) -> MojomValue {
        MojomValue::UInt8(self.0 ^ 0xa5)
    }

    fn try_from_mojom_value(value: MojomValue, _: &()) -> anyhow::Result<Self> {
        let MojomValue::UInt8(value) = value else {
            anyhow::bail!("MappedByte requires one uint8")
        };
        Ok(Self(value ^ 0xa5))
    }
}

impl LegacyValue for MappedByte {
    fn legacy_value(self) -> MojomValue {
        MojomValue::UInt8(self.0 ^ 0xa5)
    }
}

#[test]
fn uint8_typemaps_are_not_bypassed_by_the_byte_fast_path() {
    let values = vec![MappedByte(0), MappedByte(0xa5), MappedByte(255)];
    assert_compatible::<_, Vec<u8>>(values.clone());
    let wire = encode::<_, Vec<u8>>(values.clone());
    assert_eq!(read_legacy_byte_wire(&wire), vec![0xa5, 0, 0x5a]);
    assert!(matches!(
        <Vec<MappedByte> as MojomParse<Vec<u8>>>::into_mojom_value(values.clone(), &()),
        MojomValue::Array(_)
    ));
    assert_eq!(
        <Vec<MappedByte> as MojomParse<Vec<u8>>>::try_from_mojom_value(
            MojomValue::ByteArray(vec![0xa5, 0, 0x5a]),
            &()
        )
        .unwrap(),
        values
    );
    assert_compatible::<_, [u8; 3]>([MappedByte(0), MappedByte(0xa5), MappedByte(255)]);
    assert_eq!(
        <[MappedByte; 3] as MojomParse<[u8; 3]>>::try_from_mojom_value(
            MojomValue::ByteArray(vec![0xa5, 0, 0x5a]),
            &()
        )
        .unwrap(),
        [MappedByte(0), MappedByte(0xa5), MappedByte(255)]
    );
}

#[derive(Debug)]
struct CheckedByte(u8);

impl MojomParse<u8> for CheckedByte {
    fn mojom_type() -> MojomType {
        MojomType::UInt8
    }

    fn into_mojom_value(self, _: &()) -> MojomValue {
        MojomValue::UInt8(self.0)
    }

    fn try_from_mojom_value(value: MojomValue, _: &()) -> anyhow::Result<Self> {
        let MojomValue::UInt8(value) = value else {
            anyhow::bail!("CheckedByte requires one uint8")
        };
        anyhow::ensure!(value != 255, "255 is rejected by this typemap");
        Ok(Self(value))
    }
}

#[test]
fn fallible_uint8_typemaps_return_errors_for_vectors_and_fixed_arrays() {
    fn invalid_values() -> [MojomValue; 2] {
        [
            vec![1_u8, 255, 3].legacy_value(),
            MojomValue::ByteArray(vec![1, 255, 3]),
        ]
    }

    for value in invalid_values() {
        assert!(
            <Vec<CheckedByte> as MojomParse<Vec<u8>>>::try_from_mojom_value(value, &()).is_err()
        );
    }
    for value in invalid_values() {
        assert!(
            <[CheckedByte; 3] as MojomParse<[u8; 3]>>::try_from_mojom_value(value, &()).is_err()
        );
    }
    let wire = byte_wire(&[1, 255, 3]);
    assert!(matches!(
        decode::<Vec<CheckedByte>, Vec<u8>>(&wire).unwrap_err().ty,
        ParsingErrorType::UnconvertibleValue { .. }
    ));
    assert!(matches!(
        decode::<[CheckedByte; 3], [u8; 3]>(&wire).unwrap_err().ty,
        ParsingErrorType::UnconvertibleValue { .. }
    ));
}

#[test]
fn declared_array_padding_remains_accepted() {
    // Unlike a string, an array may declare reserved bytes after its elements.
    // Neither these reserved bytes nor alignment padding must be zero.
    for declared_size in [13_u32, 19] {
        let mut wire = byte_wire(&[10, 20, 30]);
        wire[16..20].copy_from_slice(&declared_size.to_le_bytes());
        wire.resize(16 + (declared_size as usize).next_multiple_of(8), 0xab);
        wire[27..].fill(0xab);
        assert_eq!(decode::<Vec<u8>, Vec<u8>>(&wire).unwrap(), [10, 20, 30]);
        assert_eq!(read_legacy_byte_wire(&wire), [10, 20, 30]);
    }
}

#[test]
fn malformed_byte_arrays_are_rejected_without_panicking() {
    let canonical = byte_wire(&[10, 20, 30]);
    // Every truncated prefix, including missing final alignment padding,
    // must fail instead of silently returning a partial vector.
    for length in 0..canonical.len() {
        assert!(
            decode::<Vec<u8>, Vec<u8>>(&canonical[..length]).is_err(),
            "length={length}"
        );
    }
    for size in [0_u32, 1, 7, 8, 9, 10, 17, u32::MAX] {
        let mut wire = canonical.clone();
        wire[16..20].copy_from_slice(&size.to_le_bytes());
        assert!(decode::<Vec<u8>, Vec<u8>>(&wire).is_err(), "size={size}");
    }
    // A huge element count with a tiny frame must be validated before any
    // per-element AST allocation. The bytes themselves stay a small fixture.
    for count in [4_u32, 9, u32::MAX] {
        let mut wire = canonical.clone();
        wire[20..24].copy_from_slice(&count.to_le_bytes());
        assert!(decode::<Vec<u8>, Vec<u8>>(&wire).is_err(), "count={count}");
    }
    for pointer in [0_u64, 1, 7, 9, 16, u64::MAX] {
        let mut wire = canonical.clone();
        wire[8..16].copy_from_slice(&pointer.to_le_bytes());
        assert!(
            decode::<Vec<u8>, Vec<u8>>(&wire).is_err(),
            "pointer={pointer}"
        );
    }
    let mut trailing = canonical;
    trailing.extend([0; 8]);
    assert!(matches!(
        decode::<Vec<u8>, Vec<u8>>(&trailing).unwrap_err().ty,
        ParsingErrorType::TooMuchData { remaining_bytes: 8 }
    ));
}

#[test]
fn fixed_array_lengths_are_checked_on_wire_and_ast_paths() {
    for count in [0, 2, 4] {
        let wire = byte_wire(&vec![7; count]);
        assert!(matches!(decode::<[u8; 3], [u8; 3]>(&wire).unwrap_err().ty,
            ParsingErrorType::WrongArraySize { expected: 3, actual } if actual == count));
        assert!(
            <[u8; 3] as MojomParse>::try_from_mojom_value(
                MojomValue::ByteArray(vec![7; count]),
                &()
            )
            .is_err()
        );
        assert!(
            <[u8; 3] as MojomParse>::try_from_mojom_value(vec![7_u8; count].legacy_value(), &())
                .is_err()
        );
        assert!(
            <[MappedByte; 3] as MojomParse<[u8; 3]>>::try_from_mojom_value(
                MojomValue::ByteArray(vec![7; count]),
                &()
            )
            .is_err()
        );
    }
    assert_eq!(
        decode::<[u8; 3], [u8; 3]>(&byte_wire(&[1, 2, 3])).unwrap(),
        [1, 2, 3]
    );
}

fn map_wire(keys: &[u8], values: &[u8]) -> Vec<u8> {
    // struct { map<uint8,uint8> value; }, followed by the map's struct of two
    // array pointers. Both arrays use the same standalone array layout as
    // byte_wire(), with the outer Envelope header removed.
    let key_array = byte_wire(keys).split_off(16);
    let value_array = byte_wire(values).split_off(16);
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
fn byte_maps_preserve_wire_and_validate_parallel_arrays() {
    let values = HashMap::from([(0_u8, 255_u8), (1, 128), (255, 0)]);
    assert_compatible::<_, HashMap<u8, u8>>(values.clone());
    let golden = map_wire(&[0, 1, 255], &[255, 128, 0]);
    assert_eq!(encode::<_, HashMap<u8, u8>>(values.clone()), golden);
    assert_eq!(
        decode::<HashMap<u8, u8>, HashMap<u8, u8>>(&golden).unwrap(),
        values
    );
    assert_compatible::<_, HashMap<u8, u8>>(HashMap::<u8, u8>::new());

    let duplicate = map_wire(&[1, 1], &[10, 20]);
    assert!(matches!(
        decode::<HashMap<u8, u8>, HashMap<u8, u8>>(&duplicate)
            .unwrap_err()
            .ty,
        ParsingErrorType::DuplicateMapKey { .. }
    ));
    let mismatched = map_wire(&[1, 2], &[10]);
    assert!(matches!(
        decode::<HashMap<u8, u8>, HashMap<u8, u8>>(&mismatched)
            .unwrap_err()
            .ty,
        ParsingErrorType::MismatchedMap {
            key_len: 2,
            value_len: 1
        }
    ));
}

#[test]
fn parser_emits_compact_bytes_only_for_nonnullable_uint8_elements() {
    let ty = pack_mojom_type(&Envelope::<Vec<u8>>::mojom_type());
    let wire = byte_wire(&[0, 127, 255]);
    let (remaining, value) = parse_top_level_value(&wire, &mut [], 0, &ty).unwrap();
    assert!(remaining.is_empty());
    let MojomValue::Struct(_, mut fields) = value else {
        panic!("expected struct")
    };
    assert!(
        matches!(fields.pop().unwrap(), MojomValue::ByteArray(bytes) if bytes == [0, 127, 255])
    );

    let ty = pack_mojom_type(&Envelope::<Vec<Option<u8>>>::mojom_type());
    let wire = encode::<_, Vec<Option<u8>>>(vec![Some(255), None]);
    let (remaining, value) = parse_top_level_value(&wire, &mut [], 0, &ty).unwrap();
    assert!(remaining.is_empty());
    let MojomValue::Struct(_, mut fields) = value else {
        panic!("expected struct")
    };
    assert!(matches!(
        fields.pop().unwrap(),
        MojomValue::NullablePrimitiveArray(_)
    ));
}
