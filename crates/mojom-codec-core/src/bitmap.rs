// Copyright 2026 The Mojo Rust SDK Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Private LSB-first bitmap operations. No Option or Vec representation is
//! reinterpreted. x86_64 uses only its baseline SSE2 instruction set.

use std::mem::MaybeUninit;

const _: () = assert!(std::mem::size_of::<bool>() == 1);

fn low_mask(count: usize) -> u8 {
    debug_assert!(count <= 8);
    ((1_u16 << count) - 1) as u8
}

fn expand_byte(byte: u8) -> [bool; 8] {
    [
        byte & 1 != 0,
        byte & 2 != 0,
        byte & 4 != 0,
        byte & 8 != 0,
        byte & 16 != 0,
        byte & 32 != 0,
        byte & 64 != 0,
        byte & 128 != 0,
    ]
}

fn pack_scalar(values: &[bool], output: &mut [u8]) {
    debug_assert_eq!(output.len(), values.len().div_ceil(8));
    for (group, byte) in values.chunks(8).zip(output) {
        *byte = group
            .iter()
            .enumerate()
            .fold(0, |bits, (bit, value)| bits | (u8::from(*value) << bit));
    }
}

fn unpack_scalar(input: &[u8], output: &mut [MaybeUninit<bool>]) {
    debug_assert_eq!(input.len(), output.len().div_ceil(8));
    for (byte, group) in input.iter().zip(output.chunks_mut(8)) {
        for (slot, value) in group.iter_mut().zip(expand_byte(*byte)) {
            slot.write(value);
        }
    }
}

pub(crate) fn pack(values: &[bool], output: &mut [u8]) {
    assert_eq!(output.len(), values.len().div_ceil(8));
    #[cfg(target_arch = "x86_64")]
    let processed = if values.len() >= 16 {
        // SAFETY: SSE2 is a baseline requirement of Rust's x86_64 targets.
        // The helper accesses only complete 16-element groups in these slices.
        unsafe { x86_64::pack_full_groups(values, output) }
    } else {
        0
    };
    #[cfg(not(target_arch = "x86_64"))]
    let processed = 0;
    pack_scalar(&values[processed..], &mut output[processed / 8..]);
}

pub(crate) fn unpack(input: &[u8], count: usize) -> Vec<bool> {
    assert_eq!(input.len(), count.div_ceil(8));
    let mut result = Vec::with_capacity(count);
    {
        let output = &mut result.spare_capacity_mut()[..count];
        #[cfg(target_arch = "x86_64")]
        let processed = if count >= 16 {
            // SAFETY: SSE2 is baseline on x86_64. The input contains all
            // ceil(count/8) bytes; output provides count writable bool slots.
            unsafe { x86_64::unpack_full_groups(input, output) }
        } else {
            0
        };
        #[cfg(not(target_arch = "x86_64"))]
        let processed = 0;
        unpack_scalar(&input[processed / 8..], &mut output[processed..]);
    }
    // SAFETY: SIMD initializes exactly its complete groups, and unpack_scalar
    // initializes every remaining slot. Every stored byte is 0 or 1, the only
    // valid bool representations. No uninitialized capacity becomes visible.
    unsafe { result.set_len(count) };
    result
}

/// Test adapter for tag classification and lazy scalar reads. An all-zero bitmap never invokes
/// read_value, so dirty None slots do not undergo scalar loads/conversions.
#[cfg(test)]
#[inline]
pub(crate) fn unpack_nullable<T>(
    tags: &[u8],
    count: usize,
    mut read_value: impl FnMut(usize) -> T,
) -> Vec<Option<T>> {
    assert_eq!(tags.len(), count.div_ceil(8));
    match nullable_tag_pattern(tags, count) {
        Some(false) => return (0..count).map(|_| None).collect(),
        Some(true) => return (0..count).map(|index| Some(read_value(index))).collect(),
        None => (),
    }
    (0..count)
        .map(|index| (tags[index / 8] & (1 << (index % 8)) != 0).then(|| read_value(index)))
        .collect()
}

/// Some(false): all absent; Some(true): all present; None: mixed.
/// Invalid bits beyond count do not influence the result.
pub(crate) fn nullable_tag_pattern(tags: &[u8], count: usize) -> Option<bool> {
    assert_eq!(tags.len(), count.div_ceil(8));
    let Some((&last, full_tags)) = tags.split_last() else {
        return Some(false);
    };
    // A mixed complete byte already proves both states are present. Keep the
    // uniform scan branch-free (and vectorizable), but avoid scanning the
    // whole bitmap for the common alternating/mixed pattern.
    if full_tags
        .first()
        .is_some_and(|&tag| tag != 0 && tag != u8::MAX)
    {
        return None;
    }
    let mut present = 0_u8;
    let mut absent = 0_u8;
    for &tag in full_tags {
        present |= tag;
        absent |= !tag;
    }
    let tail = count % 8;
    let mask = if tail == 0 { u8::MAX } else { low_mask(tail) };
    present |= last & mask;
    absent |= !last & mask;
    if present == 0 {
        Some(false)
    } else if absent == 0 {
        Some(true)
    } else {
        None
    }
}

#[inline]
fn expand_nullable_byte(tag: u8, value: u8) -> [Option<bool>; 8] {
    [
        (tag & 1 != 0).then(|| value & 1 != 0),
        (tag & 2 != 0).then(|| value & 2 != 0),
        (tag & 4 != 0).then(|| value & 4 != 0),
        (tag & 8 != 0).then(|| value & 8 != 0),
        (tag & 16 != 0).then(|| value & 16 != 0),
        (tag & 32 != 0).then(|| value & 32 != 0),
        (tag & 64 != 0).then(|| value & 64 != 0),
        (tag & 128 != 0).then(|| value & 128 != 0),
    ]
}

pub(crate) fn unpack_nullable_bools(tags: &[u8], values: &[u8], count: usize) -> Vec<Option<bool>> {
    assert_eq!(tags.len(), count.div_ceil(8));
    assert_eq!(values.len(), tags.len());
    let mut result = Vec::with_capacity(count);
    let full_groups = count / 8;
    for group in 0..full_groups {
        if tags[group] == 0 {
            result.extend_from_slice(&[None; 8]);
        } else {
            result.extend_from_slice(&expand_nullable_byte(tags[group], values[group]));
        }
    }
    let tail = count % 8;
    if tail != 0 {
        let tag = tags[full_groups] & low_mask(tail);
        if tag == 0 {
            result.extend_from_slice(&[None; 8][..tail]);
        } else {
            result.extend_from_slice(&expand_nullable_byte(tag, values[full_groups])[..tail]);
        }
    }
    result
}

#[cfg(target_arch = "x86_64")]
mod x86_64 {
    use std::arch::x86_64::*;
    use std::mem::MaybeUninit;

    #[target_feature(enable = "sse2")]
    pub(super) unsafe fn pack_full_groups(values: &[bool], output: &mut [u8]) -> usize {
        let groups = values.len() / 16;
        for group in 0..groups {
            // SAFETY: bool has size one, and its initialized representation is
            // exactly 0 or 1. This reads 16 existing bool elements as bytes;
            // it never reinterprets a Vec header or creates a byte-backed bool
            // reference. loadu has no additional alignment requirement.
            let bits = unsafe {
                let bytes = _mm_loadu_si128(values.as_ptr().add(group * 16).cast());
                _mm_movemask_epi8(_mm_cmpeq_epi8(bytes, _mm_set1_epi8(1))) as u16
            };
            output[group * 2..group * 2 + 2].copy_from_slice(&bits.to_le_bytes());
        }
        groups * 16
    }

    #[target_feature(enable = "sse2")]
    pub(super) unsafe fn unpack_full_groups(
        input: &[u8],
        output: &mut [MaybeUninit<bool>],
    ) -> usize {
        let groups = output.len() / 16;
        assert!(input.len() >= groups * 2);
        // SAFETY: SSE2 is enabled for this function. Each complete group owns
        // 16 writable MaybeUninit<bool> slots, each one byte. AND with `ones`
        // guarantees exactly 0/1 in every lane, hence valid initialized bools.
        // storeu accepts their alignment; input indexing is checked above.
        // Only these initialized groups are counted/returned.
        unsafe {
            let masks = _mm_setr_epi8(1, 2, 4, 8, 16, 32, 64, -128, 1, 2, 4, 8, 16, 32, 64, -128);
            let ones = _mm_set1_epi8(1);
            for group in 0..groups {
                let low = _mm_set1_epi8(input[group * 2] as i8);
                let high = _mm_set1_epi8(input[group * 2 + 1] as i8);
                let packed = _mm_unpacklo_epi64(low, high);
                let selected = _mm_and_si128(packed, masks);
                let bools = _mm_and_si128(_mm_cmpeq_epi8(selected, masks), ones);
                _mm_storeu_si128(output.as_mut_ptr().add(group * 16).cast(), bools);
            }
        }
        groups * 16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scalar_unpack(input: &[u8], count: usize) -> Vec<bool> {
        let mut result = Vec::with_capacity(count);
        unpack_scalar(input, &mut result.spare_capacity_mut()[..count]);
        // SAFETY: unpack_scalar initializes every requested slot with a bool.
        unsafe { result.set_len(count) };
        result
    }

    #[test]
    fn every_byte_and_partial_byte_has_lsb_first_order() {
        for byte in 0..=u8::MAX {
            for count in 0..=8 {
                let input = if count == 0 { vec![] } else { vec![byte] };
                let expected = (0..count)
                    .map(|bit| byte & (1 << bit) != 0)
                    .collect::<Vec<_>>();
                assert_eq!(unpack(&input, count), expected);
                assert_eq!(scalar_unpack(&input, count), expected);
                let mut packed = vec![0xa5; count.div_ceil(8)];
                pack(&expected, &mut packed);
                assert_eq!(
                    packed,
                    if count == 0 {
                        vec![]
                    } else {
                        vec![byte & low_mask(count)]
                    }
                );
            }
        }
    }

    #[test]
    fn every_sixteen_bit_pattern_matches_the_scalar_oracle() {
        for word in 0..=u16::MAX {
            let input = word.to_le_bytes();
            let expected = (0..16)
                .map(|bit| word & (1 << bit) != 0)
                .collect::<Vec<_>>();
            assert_eq!(unpack(&input, 16), expected);
            assert_eq!(scalar_unpack(&input, 16), expected);
            let mut packed = [0; 2];
            pack(&expected, &mut packed);
            assert_eq!(packed, input);
            let mut scalar = [0; 2];
            pack_scalar(&expected, &mut scalar);
            assert_eq!(packed, scalar);
        }
    }

    #[test]
    fn random_lengths_and_dirty_tail_bits_match_scalar() {
        let mut seed = 0x5eed_0123_9876_abcd_u64;
        for count in (0_usize..=129).chain([255, 256, 257, 1023, 1024, 1025]) {
            let mut input = vec![0; count.div_ceil(8)];
            for byte in &mut input {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                *byte = seed as u8;
            }
            let expected = (0..count)
                .map(|bit| input[bit / 8] & (1 << (bit % 8)) != 0)
                .collect::<Vec<_>>();
            assert_eq!(unpack(&input, count), expected);
            assert_eq!(scalar_unpack(&input, count), expected);
            let mut fast = vec![0xa5; count.div_ceil(8)];
            let mut scalar = fast.clone();
            pack(&expected, &mut fast);
            pack_scalar(&expected, &mut scalar);
            assert_eq!(fast, scalar);
        }
    }

    #[test]
    fn nullable_tags_never_read_none_slots_and_ignore_unused_bits() {
        for tag in 0..=u8::MAX {
            for count in 0..=8 {
                let tags = if count == 0 { vec![] } else { vec![tag] };
                let mut read = vec![];
                let actual = unpack_nullable(&tags, count, |index| {
                    read.push(index);
                    index as u16 + 100
                });
                let expected = (0..count)
                    .map(|bit| (tag & (1 << bit) != 0).then_some(bit as u16 + 100))
                    .collect::<Vec<_>>();
                assert_eq!(actual, expected);
                assert_eq!(
                    read,
                    (0..count)
                        .filter(|bit| tag & (1 << bit) != 0)
                        .collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn every_nullable_bool_tag_and_value_bitmap_matches_scalar() {
        for tag in 0..=u8::MAX {
            for value in 0..=u8::MAX {
                let expected = (0..8)
                    .map(|bit| (tag & (1 << bit) != 0).then(|| value & (1 << bit) != 0))
                    .collect::<Vec<_>>();
                assert_eq!(unpack_nullable_bools(&[tag], &[value], 8), expected);
            }
        }
    }
}
