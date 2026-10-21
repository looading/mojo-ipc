// Copyright 2026 The Mojo Rust SDK Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Public AST names and malformed-pointer diagnostics must survive lazy labels.

use mojom_codec_core::{
    MojomType, MojomValue, MojomWireType, ParsingError, ParsingErrorType, Predicate,
    deparse_single_value_for_testing, pack_mojom_type, parse_single_value_for_testing,
};
use std::collections::BTreeMap;

fn array(element_type: MojomType) -> MojomType {
    MojomType::Array {
        element_type: Box::new(element_type),
        num_elements: None,
    }
}

fn encode(ty: &MojomType, value: MojomValue) -> (MojomWireType, Vec<u8>) {
    let wire_type = pack_mojom_type(ty);
    let (bytes, handles) = deparse_single_value_for_testing(value, &wire_type)
        .unwrap()
        .into_parts();
    assert!(handles.is_empty());
    (wire_type, bytes)
}

fn text(value: &str) -> MojomValue {
    MojomValue::String(value.into())
}

fn advance_pointer(bytes: &mut [u8], offset: usize) {
    let value = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
    bytes[offset..offset + 8].copy_from_slice(&(value + 8).to_le_bytes());
}

fn wrong_pointer(
    error: ParsingError,
    offset: usize,
    name: &str,
    expected_offset: usize,
    actual_offset: usize,
) {
    assert_eq!(error.offset, offset);
    let ParsingErrorType::WrongPointer {
        name: actual_name,
        expected_offset: actual_expected,
        actual_offset: actual_actual,
    } = error.ty
    else {
        panic!("expected WrongPointer, got {error:?}")
    };
    assert_eq!(actual_name, name);
    assert_eq!(actual_expected, expected_offset);
    assert_eq!(actual_actual, actual_offset);
}

#[test]
fn real_nested_struct_names_are_preserved_in_declaration_order() {
    let ty = MojomType::Struct {
        field_names: vec!["number".into(), "text".into(), "child".into()],
        fields: vec![
            MojomType::UInt32,
            MojomType::String,
            MojomType::Struct {
                field_names: vec!["flag".into(), "title".into()],
                fields: vec![MojomType::Bool, MojomType::String],
            },
        ],
    };
    let value = || {
        MojomValue::Struct(
            vec!["number".into(), "text".into(), "child".into()],
            vec![
                MojomValue::UInt32(42),
                text("中文"),
                MojomValue::Struct(
                    vec!["flag".into(), "title".into()],
                    vec![MojomValue::Bool(true), text("nested")],
                ),
            ],
        )
    };
    let (wire_type, bytes) = encode(&ty, value());
    assert_eq!(
        parse_single_value_for_testing(&bytes, &mut [], &wire_type).unwrap(),
        value()
    );
}

#[test]
fn array_labels_keep_the_failing_element_index_and_offset() {
    let value = || MojomValue::Array(vec![text("a"), text("bc")]);
    let (wire_type, mut bytes) = encode(&array(MojomType::String), value());
    assert_eq!(
        parse_single_value_for_testing(&bytes, &mut [], &wire_type).unwrap(),
        value()
    );
    // Array body: 8-byte header, two pointers, then two 16-byte strings.
    advance_pointer(&mut bytes, 16);
    wrong_pointer(
        parse_single_value_for_testing(&bytes, &mut [], &wire_type).unwrap_err(),
        40,
        "Array_Element_1",
        48,
        40,
    );
}

#[test]
fn nested_arrays_retain_inner_pointer_diagnostics() {
    let value = || {
        MojomValue::Array(vec![
            MojomValue::Array(vec![text("a")]),
            MojomValue::Array(vec![text("b")]),
        ])
    };
    let (wire_type, mut bytes) = encode(&array(array(MojomType::String)), value());
    assert_eq!(
        parse_single_value_for_testing(&bytes, &mut [], &wire_type).unwrap(),
        value()
    );
    // Outer body is 24 bytes; each inner array and its string take 32 bytes.
    // The second inner array starts at 56; its string pointer is at 64.
    advance_pointer(&mut bytes, 64);
    wrong_pointer(
        parse_single_value_for_testing(&bytes, &mut [], &wire_type).unwrap_err(),
        72,
        "Array_Element_0",
        24,
        16,
    );
}

#[test]
fn map_labels_survive_without_returning_a_temporary_struct_names_vector() {
    let ty = MojomType::Map {
        key_type: Box::new(MojomType::String),
        value_type: Box::new(MojomType::String),
    };
    let value = || {
        MojomValue::Map(BTreeMap::from([
            (text("a"), text("x")),
            (text("b"), text("y")),
        ]))
    };
    let (wire_type, bytes) = encode(&ty, value());
    assert_eq!(
        parse_single_value_for_testing(&bytes, &mut [], &wire_type).unwrap(),
        value()
    );
    for (slot, offset, name, expected, actual) in
        [(8, 24, "map_keys", 32, 24), (16, 80, "map_values", 88, 80)]
    {
        let mut malformed = bytes.clone();
        advance_pointer(&mut malformed, slot);
        wrong_pointer(
            parse_single_value_for_testing(&malformed, &mut [], &wire_type).unwrap_err(),
            offset,
            name,
            expected,
            actual,
        );
    }
}

#[test]
fn pointer_union_retains_its_label_after_ordinal_is_adjusted_to_the_parent() {
    for nullable in [false, true] {
        let union_type = MojomType::Union {
            variants: BTreeMap::from([(7, MojomType::String)]),
        };
        let choice_type = if nullable {
            MojomType::Nullable {
                inner_type: Box::new(union_type),
            }
        } else {
            union_type
        };
        let ty = MojomType::Struct {
            field_names: vec!["prefix".into(), "choice".into(), "tail".into()],
            fields: vec![MojomType::UInt64, choice_type, MojomType::String],
        };
        let value = || {
            let choice = MojomValue::Union(7, Box::new(text("u")));
            let choice = if nullable {
                MojomValue::Nullable(Some(Box::new(choice)))
            } else {
                choice
            };
            MojomValue::Struct(
                vec!["prefix".into(), "choice".into(), "tail".into()],
                vec![MojomValue::UInt64(9), choice, text("t")],
            )
        };
        let (wire_type, mut bytes) = encode(&ty, value());
        let mut expected = value();
        let MojomValue::Struct(names, _) = &mut expected else {
            unreachable!()
        };
        // This existing public AST behavior is intentional to preserve: the
        // deferred union payload supplies its union label, not "choice".
        names[1] = "Union_Field_7".into();
        assert_eq!(
            parse_single_value_for_testing(&bytes, &mut [], &wire_type).unwrap(),
            expected
        );
        advance_pointer(&mut bytes, 24);
        wrong_pointer(
            parse_single_value_for_testing(&bytes, &mut [], &wire_type).unwrap_err(),
            40,
            "Union_Field_7",
            48,
            40,
        );
    }
}

#[test]
fn standalone_negative_union_label_and_unknown_tag_errors_are_unchanged() {
    let ty = MojomType::Union {
        variants: BTreeMap::from([(-7, MojomType::String)]),
    };
    let value = || MojomValue::Union(-7, Box::new(text("x")));
    let (wire_type, bytes) = encode(&ty, value());
    assert_eq!(
        parse_single_value_for_testing(&bytes, &mut [], &wire_type).unwrap(),
        value()
    );
    let mut malformed = bytes.clone();
    advance_pointer(&mut malformed, 8);
    wrong_pointer(
        parse_single_value_for_testing(&malformed, &mut [], &wire_type).unwrap_err(),
        16,
        "Union_Field_-7",
        24,
        16,
    );
    let mut unknown_tag = bytes;
    unknown_tag[4..8].copy_from_slice(&99_i32.to_le_bytes());
    let error = parse_single_value_for_testing(&unknown_tag, &mut [], &wire_type).unwrap_err();
    assert_eq!(error.offset, 4);
    assert!(matches!(
        error.ty,
        ParsingErrorType::InvalidDiscriminant { value: 99 }
    ));
}

#[test]
fn generic_nullable_enum_array_keeps_tag_bitmap_boundaries_and_validation() {
    static VALID: fn(i32) -> bool = |value| matches!(value, 0 | 7);
    let ty = array(MojomType::Nullable {
        inner_type: Box::new(MojomType::Enum {
            is_valid: Predicate::new::<()>(&VALID),
        }),
    });
    let value = || {
        MojomValue::Array(
            (0..9)
                .map(|idx| {
                    MojomValue::Nullable((idx % 2 == 1).then(|| Box::new(MojomValue::Enum(7))))
                })
                .collect(),
        )
    };
    let (wire_type, mut bytes) = encode(&ty, value());
    assert_eq!(
        parse_single_value_for_testing(&bytes, &mut [], &wire_type).unwrap(),
        value()
    );
    // Two tag bytes precede alignment and nine enum i32 slots. Even the first
    // None slot must still reject an invalid enum before wrapping its value.
    bytes[12..16].copy_from_slice(&99_i32.to_le_bytes());
    let error = parse_single_value_for_testing(&bytes, &mut [], &wire_type).unwrap_err();
    assert_eq!(error.offset, 12);
    assert!(matches!(
        error.ty,
        ParsingErrorType::InvalidDiscriminant { value: 99 }
    ));
}

fn literal_words(words: &[u64]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

#[test]
fn literal_nullable_pointer_tables_keep_none_slots_padding_and_empty_arrays() {
    let ty = array(MojomType::Nullable {
        inner_type: Box::new(MojomType::String),
    });
    let wire_type = pack_mojom_type(&ty);
    // Three slots ["a", None, "b"]. The 32-byte inline body is followed by
    // two 16-byte string objects; both relative pointers happen to be 24.
    let bytes = literal_words(&[
        (3 << 32) | 32,
        24,
        0,
        24,
        (1 << 32) | 9,
        0x61,
        (1 << 32) | 9,
        0x62,
    ]);
    let expected = || {
        MojomValue::Array(vec![
            MojomValue::Nullable(Some(Box::new(text("a")))),
            MojomValue::Nullable(None),
            MojomValue::Nullable(Some(Box::new(text("b")))),
        ])
    };
    assert_eq!(
        parse_single_value_for_testing(&bytes, &mut [], &wire_type).unwrap(),
        expected()
    );
    let (encoded, _) = deparse_single_value_for_testing(expected(), &wire_type)
        .unwrap()
        .into_parts();
    assert_eq!(encoded, bytes);

    // A null pointer has no nested object. Extra declared padding and final
    // alignment are both consumed even when none of the slots need parsing.
    let all_none = literal_words(&[(1 << 32) | 17, 0, u64::MAX]);
    assert_eq!(
        parse_single_value_for_testing(&all_none, &mut [], &wire_type).unwrap(),
        MojomValue::Array(vec![MojomValue::Nullable(None)])
    );
    for bytes in [
        literal_words(&[8]),
        literal_words(&[17, u64::MAX, u64::MAX]),
    ] {
        assert_eq!(
            parse_single_value_for_testing(&bytes, &mut [], &wire_type).unwrap(),
            MojomValue::Array(vec![])
        );
    }
}

#[test]
fn pointer_table_validation_precedes_inline_size_and_nested_data_errors() {
    let wire_type = pack_mojom_type(&array(MojomType::String));
    for (words, expected_error) in [
        (
            vec![(2 << 32) | 24, 16],
            "ParsingError { offset: 16, ty: NotEnoughData { tried_to_parse: \"u64\", expected_size: 8, remaining_bytes: 0 } }",
        ),
        (
            // Bad first target is diagnosed only after the numeric slots;
            // therefore the second, unaligned pointer must be reported first.
            vec![(2 << 32) | 24, 32, 3],
            "ParsingError { offset: 16, ty: InvalidPointer { value: 3 } }",
        ),
        (
            vec![(2 << 32) | 16, 16, 24],
            "ParsingError { offset: 24, ty: WrongSize { expected_size: 16, actual_size: 24 } }",
        ),
        (
            vec![(1 << 32) | 40, 8, 0],
            "ParsingError { offset: 16, ty: NotEnoughData { tried_to_parse: \"padding\", expected_size: 24, remaining_bytes: 8 } }",
        ),
    ] {
        let bytes = literal_words(&words);
        let error = parse_single_value_for_testing(&bytes, &mut [], &wire_type).unwrap_err();
        assert_eq!(format!("{error:?}"), expected_error);
    }

    // Declared inline padding precedes the first nested object and is allowed
    // to contain nonzero bytes. Relative pointers must include that padding.
    let padded = literal_words(&[
        (2 << 32) | 32,
        24,
        32,
        u64::MAX,
        (1 << 32) | 9,
        0x61,
        (1 << 32) | 9,
        0x62,
    ]);
    assert_eq!(
        parse_single_value_for_testing(&padded, &mut [], &wire_type).unwrap(),
        MojomValue::Array(vec![text("a"), text("b")])
    );
}

#[test]
fn pointer_array_dispatch_preserves_actual_struct_names_and_nested_maps() {
    let child = MojomType::Struct {
        field_names: vec!["number".into(), "label".into()],
        fields: vec![MojomType::UInt32, MojomType::String],
    };
    let value = || {
        MojomValue::Array(
            [1, 2]
                .into_iter()
                .map(|number| {
                    MojomValue::Struct(
                        vec!["number".into(), "label".into()],
                        vec![MojomValue::UInt32(number), text("child")],
                    )
                })
                .collect(),
        )
    };
    let (wire_type, bytes) = encode(&array(child), value());
    assert_eq!(
        parse_single_value_for_testing(&bytes, &mut [], &wire_type).unwrap(),
        value()
    );

    let map = MojomType::Map {
        key_type: Box::new(MojomType::String),
        value_type: Box::new(MojomType::String),
    };
    let value = || {
        MojomValue::Array(vec![
            MojomValue::Map(BTreeMap::new()),
            MojomValue::Map(BTreeMap::from([(text("key"), text("value"))])),
        ])
    };
    let (wire_type, bytes) = encode(&array(map), value());
    assert_eq!(
        parse_single_value_for_testing(&bytes, &mut [], &wire_type).unwrap(),
        value()
    );
}

#[test]
fn overflowing_pointer_target_is_rejected_without_wrapping_or_panicking() {
    let wire_type = pack_mojom_type(&array(MojomType::String));
    let bytes = literal_words(&[(1 << 32) | 16, u64::MAX - 7]);
    let error = parse_single_value_for_testing(&bytes, &mut [], &wire_type).unwrap_err();
    assert_eq!(error.offset, 8);
    assert!(matches!(
        error.ty,
        ParsingErrorType::InvalidPointer { value } if value == u64::MAX - 7
    ));
}
