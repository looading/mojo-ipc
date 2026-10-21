// Copyright 2026 The Mojo Rust SDK Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The optional candidate validator must retain standard owned UTF-8 errors.

use mojom_codec_core::{
    MojomType, MojomValue, MojomWireType, ParsingErrorType, pack_mojom_type,
    parse_single_value_for_testing,
};

fn wire(raw: &[u8], pad: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend(u32::try_from(raw.len() + 8).unwrap().to_le_bytes());
    bytes.extend(u32::try_from(raw.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(raw);
    if pad {
        bytes.resize(bytes.len().next_multiple_of(8), 0xa5);
    }
    bytes
}

fn string_type() -> MojomWireType {
    pack_mojom_type(&MojomType::String)
}

fn assert_standard_error(raw: &[u8], pad: bool) {
    let standard = String::from_utf8(raw.to_vec()).unwrap_err();
    let error =
        parse_single_value_for_testing(&wire(raw, pad), &mut [], &string_type()).unwrap_err();
    assert_eq!(error.offset, raw.len() + 8);
    let ParsingErrorType::NonUTF8String { err } = error.ty else {
        panic!("expected the standard UTF-8 error, got {error:?}")
    };
    assert_eq!(err.as_bytes(), raw);
    assert_eq!(err.utf8_error(), standard.utf8_error());
    assert_eq!(err.to_string(), standard.to_string());
}

#[test]
fn valid_ascii_unicode_nul_and_simd_boundary_lengths_are_unchanged() {
    for length in [0, 1, 7, 63, 64, 65, 127, 128, 129, 256, 131_072] {
        let pattern = "中🙂\0é".as_bytes();
        let mut mixed = pattern.repeat(length / pattern.len());
        mixed.resize(length, b'x');
        for raw in [vec![b'a'; length], vec![0; length], mixed] {
            let actual =
                parse_single_value_for_testing(&wire(&raw, true), &mut [], &string_type()).unwrap();
            assert_eq!(actual, MojomValue::String(String::from_utf8(raw).unwrap()));
        }
    }
    // Every ASCII control is valid UTF-8, including NUL and DEL.
    let raw: Vec<u8> = (0..=127).collect();
    assert_eq!(
        parse_single_value_for_testing(&wire(&raw, true), &mut [], &string_type()).unwrap(),
        MojomValue::String(String::from_utf8(raw).unwrap())
    );
}

#[test]
fn invalid_sequences_keep_the_exact_standard_error_before_and_after_boundaries() {
    let invalid: &[&[u8]] = &[
        &[0x80],                   // unexpected continuation
        &[0xc0, 0xaf],             // overlong two-byte encoding
        &[0xe0, 0x80, 0xaf],       // overlong three-byte encoding
        &[0xed, 0xa0, 0x80],       // UTF-16 surrogate
        &[0xf4, 0x90, 0x80, 0x80], // above U+10FFFF
        &[0xf5, 0x80, 0x80, 0x80], // invalid leading byte
        &[0xff],                   // invalid leading byte
        &[0xc2],                   // incomplete two-byte sequence
        &[0xe2, 0x82],             // incomplete three-byte sequence
        &[0xf0, 0x9f, 0x99],       // incomplete four-byte sequence
        &[0xe2, 0x28, 0xa1],       // non-continuation inside a sequence
    ];
    for prefix in [0, 1, 62, 63, 64, 65, 127, 128] {
        for sequence in invalid {
            for suffix in [b"".as_slice(), b"valid-after".as_slice()] {
                let mut raw = vec![b'a'; prefix];
                raw.extend_from_slice(sequence);
                raw.extend_from_slice(suffix);
                assert_standard_error(&raw, true);
            }
        }
    }
}

#[test]
fn utf8_validation_still_precedes_final_padding_validation() {
    let mut invalid = vec![b'a'; 65];
    invalid[64] = 0xff;
    assert_standard_error(&invalid, false);

    let valid = wire(&vec![b'a'; 65], false);
    let error = parse_single_value_for_testing(&valid, &mut [], &string_type()).unwrap_err();
    assert_eq!(error.offset, 73);
    assert!(matches!(
        error.ty,
        ParsingErrorType::NotEnoughData {
            tried_to_parse,
            expected_size: 7,
            remaining_bytes: 0,
        } if tried_to_parse == "padding"
    ));
}

#[test]
fn string_size_validation_still_precedes_utf8_validation() {
    let mut bytes = wire(&vec![0xff; 64], true);
    bytes[0..4].copy_from_slice(&73_u32.to_le_bytes());
    let error = parse_single_value_for_testing(&bytes, &mut [], &string_type()).unwrap_err();
    assert_eq!(error.offset, 8);
    assert!(matches!(
        error.ty,
        ParsingErrorType::WrongSize {
            expected_size: 73,
            actual_size: 72,
        }
    ));
}

#[test]
fn nested_string_keeps_the_global_parser_offset_and_owned_error_bytes() {
    let ty = MojomType::Struct {
        field_names: vec!["text".into()],
        fields: vec![MojomType::String],
    };
    for length in [63, 64, 65, 129] {
        let mut raw = vec![b'x'; length];
        raw[length - 1] = 0xff;
        let mut bytes = Vec::new();
        bytes.extend(16_u32.to_le_bytes());
        bytes.extend(0_u32.to_le_bytes());
        bytes.extend(8_u64.to_le_bytes());
        bytes.extend(wire(&raw, false));
        let error =
            parse_single_value_for_testing(&bytes, &mut [], &pack_mojom_type(&ty)).unwrap_err();
        assert_eq!(error.offset, 24 + length);
        let ParsingErrorType::NonUTF8String { err } = error.ty else {
            panic!("expected UTF-8 error before missing padding")
        };
        assert_eq!(err.as_bytes(), raw);
        assert_eq!(
            err.utf8_error(),
            String::from_utf8(raw).unwrap_err().utf8_error()
        );
    }
}
