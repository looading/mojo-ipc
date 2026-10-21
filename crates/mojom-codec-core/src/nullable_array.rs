// Copyright 2026 The Mojo Rust SDK Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Nullable scalar arrays: a leading tag bitmap and fixed-width value slots.
//! Null slots are retained on the wire; bool values use a second bitmap.

use crate::{MojomValue, PackedLeafType, primitive_array::move_identical_vec};
use ordered_float::OrderedFloat;
use std::any::{Any, TypeId};

#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub values_offset: usize,
    pub body_size: usize,
}

/// Storage after the eight-byte array header, excluding final alignment.
/// Enums and endpoints are deliberately excluded from this numeric path.
pub(crate) fn layout(leaf: &PackedLeafType, count: usize) -> Option<Layout> {
    let tags = count.div_ceil(8);
    if *leaf == PackedLeafType::Bool {
        return Some(Layout {
            values_offset: tags,
            body_size: tags.checked_mul(2)?,
        });
    }
    let width = match leaf {
        PackedLeafType::Int8 | PackedLeafType::UInt8 => 1,
        PackedLeafType::Int16 | PackedLeafType::UInt16 => 2,
        PackedLeafType::Int32 | PackedLeafType::UInt32 | PackedLeafType::Float32 => 4,
        PackedLeafType::Int64 | PackedLeafType::UInt64 | PackedLeafType::Float64 => 8,
        _ => return None,
    };
    let values_offset = tags.checked_add(width - 1)? / width * width;
    Some(Layout {
        values_offset,
        body_size: values_offset.checked_add(count.checked_mul(width)?)?,
    })
}

macro_rules! define_arrays {
    ($(($variant:ident, $ty:ty)),* $(,)?) => {
        /// Compact arrays of nullable numeric/bool elements. Float slots retain
        /// their IEEE bits; this AST representation does not change Mojom wire.
        #[derive(Debug, PartialEq, PartialOrd, Ord, Eq, Hash)]
        pub enum NullablePrimitiveArray { $($variant(Vec<Option<$ty>>),)* }

        pub(crate) fn is_native<T: 'static, W: 'static>() -> bool {
            TypeId::of::<T>() == TypeId::of::<W>() &&
                [$(TypeId::of::<Option<$ty>>(),)* TypeId::of::<Option<f32>>(), TypeId::of::<Option<f64>>()]
                    .contains(&TypeId::of::<T>())
        }

        impl NullablePrimitiveArray {
            pub fn len(&self) -> usize { match self { $(Self::$variant(v) => v.len(),)* } }
            pub fn is_empty(&self) -> bool { self.len() == 0 }
            pub(crate) fn leaf_type(&self) -> PackedLeafType {
                match self { $(Self::$variant(_) => PackedLeafType::$variant,)* }
            }

            pub(crate) fn from_vec<T: 'static, W: 'static>(value: Vec<T>) -> Result<Self, Vec<T>> {
                if TypeId::of::<T>() != TypeId::of::<W>() { return Err(value); }
                $(if TypeId::of::<T>() == TypeId::of::<Option<$ty>>() {
                    return Ok(Self::$variant(move_identical_vec(value)));
                })*
                if TypeId::of::<T>() == TypeId::of::<Option<f32>>() {
                    return Ok(Self::Float32(move_identical_vec::<T, Option<f32>>(value)
                        .into_iter().map(|v| v.map(OrderedFloat)).collect()));
                }
                if TypeId::of::<T>() == TypeId::of::<Option<f64>>() {
                    return Ok(Self::Float64(move_identical_vec::<T, Option<f64>>(value)
                        .into_iter().map(|v| v.map(OrderedFloat)).collect()));
                }
                Err(value)
            }

            pub(crate) fn into_vec<T: 'static, W: 'static>(mut self) -> Result<Vec<T>, Self> {
                if TypeId::of::<T>() != TypeId::of::<W>() { return Err(self); }
                let erased: &mut dyn Any = match &mut self { $(Self::$variant(v) => v,)* };
                if let Some(values) = erased.downcast_mut::<Vec<T>>() {
                    return Ok(std::mem::take(values));
                }
                match &mut self {
                    Self::Float32(v) if TypeId::of::<T>() == TypeId::of::<Option<f32>>() => {
                        let values: Vec<Option<f32>> = std::mem::take(v).into_iter().map(|v| v.map(|x| x.0)).collect();
                        return Ok(move_identical_vec(values));
                    }
                    Self::Float64(v) if TypeId::of::<T>() == TypeId::of::<Option<f64>>() => {
                        let values: Vec<Option<f64>> = std::mem::take(v).into_iter().map(|v| v.map(|x| x.0)).collect();
                        return Ok(move_identical_vec(values));
                    }
                    _ => (),
                }
                Err(self)
            }

            pub(crate) fn into_values(self) -> Box<dyn ExactSizeIterator<Item = MojomValue>> {
                match self { $(Self::$variant(v) => Box::new(v.into_iter().map(|value|
                    MojomValue::Nullable(value.map(|value| Box::new(MojomValue::$variant(value)))))),)* }
            }
        }
    };
}

define_arrays!(
    (Bool, bool),
    (Int8, i8),
    (UInt8, u8),
    (Int16, i16),
    (UInt16, u16),
    (Int32, i32),
    (UInt32, u32),
    (Int64, i64),
    (UInt64, u64),
    (Float32, OrderedFloat<f32>),
    (Float64, OrderedFloat<f64>),
);

impl NullablePrimitiveArray {
    /// Full declared storage and tail alignment have been checked by the caller.
    pub(crate) fn from_wire(
        leaf: &PackedLeafType,
        body: &[u8],
        count: usize,
        layout: Layout,
    ) -> Self {
        let tags = &body[..count.div_ceil(8)];
        let values = &body[layout.values_offset..];
        let pattern = if *leaf == PackedLeafType::Bool {
            None
        } else {
            crate::bitmap::nullable_tag_pattern(tags, count)
        };
        macro_rules! read {
            ($ty:ty, $variant:ident) => {
                read!($ty, $variant, |value| value)
            };
            ($ty:ty, $variant:ident, $wrap:expr) => {
                Self::$variant(match pattern {
                    Some(false) => (0..count).map(|_| None).collect(),
                    Some(true) => values
                        .chunks_exact(std::mem::size_of::<$ty>())
                        .map(|chunk| Some($wrap(<$ty>::from_le_bytes(chunk.try_into().unwrap()))))
                        .collect(),
                    None => values
                        .chunks_exact(std::mem::size_of::<$ty>())
                        .enumerate()
                        .map(|(i, chunk)| {
                            (tags[i / 8] & (1 << (i % 8)) != 0)
                                .then(|| $wrap(<$ty>::from_le_bytes(chunk.try_into().unwrap())))
                        })
                        .collect(),
                })
            };
        }

        match leaf {
            PackedLeafType::Bool => {
                Self::Bool(crate::bitmap::unpack_nullable_bools(tags, values, count))
            }
            PackedLeafType::Int8 => read!(i8, Int8),
            PackedLeafType::UInt8 => read!(u8, UInt8),
            PackedLeafType::Int16 => read!(i16, Int16),
            PackedLeafType::UInt16 => read!(u16, UInt16),
            PackedLeafType::Int32 => read!(i32, Int32),
            PackedLeafType::UInt32 => read!(u32, UInt32),
            PackedLeafType::Int64 => read!(i64, Int64),
            PackedLeafType::UInt64 => read!(u64, UInt64),
            PackedLeafType::Float32 => read!(f32, Float32, OrderedFloat),
            PackedLeafType::Float64 => read!(f64, Float64, OrderedFloat),
            _ => unreachable!("nullable scalar layout was validated"),
        }
    }

    pub(crate) fn append_payload(&self, bytes: &mut Vec<u8>, layout: Layout) {
        let start = bytes.len();
        bytes.resize(
            start
                .checked_add(layout.body_size)
                .expect("message storage overflow"),
            0,
        );
        // The zeroed body already encodes all None tags, value slots and
        // padding. Inspect only Option tags, never a None's inner storage.
        if match self {
            Self::Bool(v) => v.iter().all(Option::is_none),
            Self::Int8(v) => v.iter().all(Option::is_none),
            Self::UInt8(v) => v.iter().all(Option::is_none),
            Self::Int16(v) => v.iter().all(Option::is_none),
            Self::UInt16(v) => v.iter().all(Option::is_none),
            Self::Int32(v) => v.iter().all(Option::is_none),
            Self::UInt32(v) => v.iter().all(Option::is_none),
            Self::Int64(v) => v.iter().all(Option::is_none),
            Self::UInt64(v) => v.iter().all(Option::is_none),
            Self::Float32(v) => v.iter().all(Option::is_none),
            Self::Float64(v) => v.iter().all(Option::is_none),
        } {
            return;
        }
        let output = &mut bytes[start..];
        // Zero initialization preserves all None value slots and internal
        // alignment padding. Every Some sets its tag and exact scalar bits.
        macro_rules! write {
            ($values:expr, $width:literal, $to_bytes:expr) => {
                for (group, values) in $values.chunks(8).enumerate() {
                    let mut tags = 0_u8;
                    for (bit, value) in values.iter().enumerate() {
                        if let Some(value) = value {
                            tags |= 1 << bit;
                            let offset = layout.values_offset + (group * 8 + bit) * $width;
                            output[offset..offset + $width].copy_from_slice(&$to_bytes(value));
                        }
                    }
                    output[group] = tags;
                }
            };
        }
        match self {
            Self::Bool(values) => {
                for (group, values) in values.chunks(8).enumerate() {
                    let mut tags = 0_u8;
                    let mut bits = 0_u8;
                    for (bit, value) in values.iter().enumerate() {
                        if let Some(value) = value {
                            tags |= 1 << bit;
                            bits |= u8::from(*value) << bit;
                        }
                    }
                    output[group] = tags;
                    output[layout.values_offset + group] = bits;
                }
            }
            Self::Int8(v) => write!(v, 1, |v: &i8| v.to_le_bytes()),
            Self::UInt8(v) => write!(v, 1, |v: &u8| v.to_le_bytes()),
            Self::Int16(v) => write!(v, 2, |v: &i16| v.to_le_bytes()),
            Self::UInt16(v) => write!(v, 2, |v: &u16| v.to_le_bytes()),
            Self::Int32(v) => write!(v, 4, |v: &i32| v.to_le_bytes()),
            Self::UInt32(v) => write!(v, 4, |v: &u32| v.to_le_bytes()),
            Self::Int64(v) => write!(v, 8, |v: &i64| v.to_le_bytes()),
            Self::UInt64(v) => write!(v, 8, |v: &u64| v.to_le_bytes()),
            Self::Float32(v) => write!(v, 4, |v: &OrderedFloat<f32>| v.0.to_le_bytes()),
            Self::Float64(v) => write!(v, 8, |v: &OrderedFloat<f64>| v.0.to_le_bytes()),
        }
    }
}
