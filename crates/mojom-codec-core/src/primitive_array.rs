// Copyright 2026 The Mojo Rust SDK Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Compact scalar arrays. The representation changes only the Rust AST;
//! storage sizes, little-endian numbers and LSB-first bools follow Mojom.

use crate::{MojomValue, PackedLeafType};
use ordered_float::OrderedFloat;
use std::any::{Any, TypeId};

/// Compact non-nullable scalar arrays. Uint8 arrays use `MojomValue::ByteArray`.
/// Float wrappers preserve their underlying IEEE bits on the wire.
#[derive(Debug, PartialEq, PartialOrd, Ord, Eq, Hash)]
pub enum PrimitiveArray {
    Bool(Vec<bool>),
    Int8(Vec<i8>),
    Int16(Vec<i16>),
    UInt16(Vec<u16>),
    Int32(Vec<i32>),
    UInt32(Vec<u32>),
    Int64(Vec<i64>),
    UInt64(Vec<u64>),
    Float32(Vec<OrderedFloat<f32>>),
    Float64(Vec<OrderedFloat<f64>>),
}

pub(crate) fn move_identical_vec<T: 'static, U: 'static>(mut value: Vec<T>) -> Vec<U> {
    let erased: &mut dyn Any = &mut value;
    std::mem::take(
        erased
            .downcast_mut::<Vec<U>>()
            .unwrap_or_else(|| unreachable!("element types were checked before moving the Vec")),
    )
}

pub(crate) fn is_native<T: 'static, W: 'static>() -> bool {
    if TypeId::of::<T>() != TypeId::of::<W>() {
        return false;
    }
    let ty = TypeId::of::<T>();
    [
        TypeId::of::<bool>(),
        TypeId::of::<i8>(),
        TypeId::of::<i16>(),
        TypeId::of::<u16>(),
        TypeId::of::<i32>(),
        TypeId::of::<u32>(),
        TypeId::of::<i64>(),
        TypeId::of::<u64>(),
        TypeId::of::<f32>(),
        TypeId::of::<f64>(),
        TypeId::of::<OrderedFloat<f32>>(),
        TypeId::of::<OrderedFloat<f64>>(),
    ]
    .contains(&ty)
}

/// None excludes enums (which require validation), handles and endpoints.
pub(crate) fn payload_size(leaf: &PackedLeafType, count: usize) -> Option<usize> {
    match leaf {
        PackedLeafType::Bool => Some(count.div_ceil(8)),
        PackedLeafType::Int8 => Some(count),
        PackedLeafType::Int16 | PackedLeafType::UInt16 => count.checked_mul(2),
        PackedLeafType::Int32 | PackedLeafType::UInt32 | PackedLeafType::Float32 => {
            count.checked_mul(4)
        }
        PackedLeafType::Int64 | PackedLeafType::UInt64 | PackedLeafType::Float64 => {
            count.checked_mul(8)
        }
        _ => None,
    }
}

impl PrimitiveArray {
    pub fn len(&self) -> usize {
        match self {
            Self::Bool(v) => v.len(),
            Self::Int8(v) => v.len(),
            Self::Int16(v) => v.len(),
            Self::UInt16(v) => v.len(),
            Self::Int32(v) => v.len(),
            Self::UInt32(v) => v.len(),
            Self::Int64(v) => v.len(),
            Self::UInt64(v) => v.len(),
            Self::Float32(v) => v.len(),
            Self::Float64(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn leaf_type(&self) -> PackedLeafType {
        match self {
            Self::Bool(_) => PackedLeafType::Bool,
            Self::Int8(_) => PackedLeafType::Int8,
            Self::Int16(_) => PackedLeafType::Int16,
            Self::UInt16(_) => PackedLeafType::UInt16,
            Self::Int32(_) => PackedLeafType::Int32,
            Self::UInt32(_) => PackedLeafType::UInt32,
            Self::Int64(_) => PackedLeafType::Int64,
            Self::UInt64(_) => PackedLeafType::UInt64,
            Self::Float32(_) => PackedLeafType::Float32,
            Self::Float64(_) => PackedLeafType::Float64,
        }
    }

    pub(crate) fn from_vec<T: 'static, W: 'static>(value: Vec<T>) -> Result<Self, Vec<T>> {
        // Merely having the same wire type is insufficient: a custom typemap
        // must retain every scalar conversion and its context/error behavior.
        if TypeId::of::<T>() != TypeId::of::<W>() {
            return Err(value);
        }
        macro_rules! native {
            ($ty:ty, $variant:ident) => {
                if TypeId::of::<T>() == TypeId::of::<$ty>() {
                    return Ok(Self::$variant(move_identical_vec(value)));
                }
            };
        }
        native!(bool, Bool);
        native!(i8, Int8);
        native!(i16, Int16);
        native!(u16, UInt16);
        native!(i32, Int32);
        native!(u32, UInt32);
        native!(i64, Int64);
        native!(u64, UInt64);
        native!(OrderedFloat<f32>, Float32);
        native!(OrderedFloat<f64>, Float64);
        if TypeId::of::<T>() == TypeId::of::<f32>() {
            return Ok(Self::Float32(
                move_identical_vec::<T, f32>(value)
                    .into_iter()
                    .map(OrderedFloat)
                    .collect(),
            ));
        }
        if TypeId::of::<T>() == TypeId::of::<f64>() {
            return Ok(Self::Float64(
                move_identical_vec::<T, f64>(value)
                    .into_iter()
                    .map(OrderedFloat)
                    .collect(),
            ));
        }
        Err(value)
    }

    pub(crate) fn into_vec<T: 'static, W: 'static>(mut self) -> Result<Vec<T>, Self> {
        if TypeId::of::<T>() != TypeId::of::<W>() {
            return Err(self);
        }
        let erased: &mut dyn Any = match &mut self {
            Self::Bool(v) => v,
            Self::Int8(v) => v,
            Self::Int16(v) => v,
            Self::UInt16(v) => v,
            Self::Int32(v) => v,
            Self::UInt32(v) => v,
            Self::Int64(v) => v,
            Self::UInt64(v) => v,
            Self::Float32(v) => v,
            Self::Float64(v) => v,
        };
        if let Some(values) = erased.downcast_mut::<Vec<T>>() {
            return Ok(std::mem::take(values));
        }
        // Plain and OrderedFloat vectors have different Rust types. Convert
        // explicitly instead of making a representation/layout assumption.
        match &mut self {
            Self::Float32(v) if TypeId::of::<T>() == TypeId::of::<f32>() => {
                let plain: Vec<f32> = std::mem::take(v).into_iter().map(|x| x.0).collect();
                return Ok(move_identical_vec(plain));
            }
            Self::Float64(v) if TypeId::of::<T>() == TypeId::of::<f64>() => {
                let plain: Vec<f64> = std::mem::take(v).into_iter().map(|x| x.0).collect();
                return Ok(move_identical_vec(plain));
            }
            _ => (),
        }
        Err(self)
    }

    pub(crate) fn into_values(self) -> Box<dyn ExactSizeIterator<Item = MojomValue>> {
        macro_rules! values {
            ($($variant:ident),*) => {
                match self { $(Self::$variant(v) => Box::new(v.into_iter().map(MojomValue::$variant)),)* }
            };
        }
        values!(
            Bool, Int8, Int16, UInt16, Int32, UInt32, Int64, UInt64, Float32, Float64
        )
    }

    /// The caller has validated the complete body and element storage length.
    pub(crate) fn from_wire(leaf: &PackedLeafType, bytes: &[u8], count: usize) -> Option<Self> {
        macro_rules! read {
            ($ty:ty, $variant:ident) => {
                Self::$variant(
                    bytes
                        .chunks_exact(std::mem::size_of::<$ty>())
                        .map(|chunk| <$ty>::from_le_bytes(chunk.try_into().unwrap()))
                        .collect(),
                )
            };
        }
        Some(match leaf {
            PackedLeafType::Bool => Self::Bool(crate::bitmap::unpack(bytes, count)),
            PackedLeafType::Int8 => {
                Self::Int8(bytes.iter().map(|&x| i8::from_le_bytes([x])).collect())
            }
            PackedLeafType::Int16 => read!(i16, Int16),
            PackedLeafType::UInt16 => read!(u16, UInt16),
            PackedLeafType::Int32 => read!(i32, Int32),
            PackedLeafType::UInt32 => read!(u32, UInt32),
            PackedLeafType::Int64 => read!(i64, Int64),
            PackedLeafType::UInt64 => read!(u64, UInt64),
            PackedLeafType::Float32 => Self::Float32(
                bytes
                    .chunks_exact(4)
                    .map(|chunk| OrderedFloat(f32::from_le_bytes(chunk.try_into().unwrap())))
                    .collect(),
            ),
            PackedLeafType::Float64 => Self::Float64(
                bytes
                    .chunks_exact(8)
                    .map(|chunk| OrderedFloat(f64::from_le_bytes(chunk.try_into().unwrap())))
                    .collect(),
            ),
            _ => return None,
        })
    }

    pub(crate) fn append_payload(&self, bytes: &mut Vec<u8>) {
        let start = bytes.len();
        let size =
            payload_size(&self.leaf_type(), self.len()).expect("validated scalar array storage");
        bytes.resize(start + size, 0);
        let output = &mut bytes[start..];
        macro_rules! write {
            ($v:expr, $width:literal) => {
                for (value, chunk) in $v.iter().zip(output.chunks_exact_mut($width)) {
                    chunk.copy_from_slice(&value.to_le_bytes());
                }
            };
        }
        match self {
            Self::Bool(v) => crate::bitmap::pack(v, output),
            Self::Int8(v) => write!(v, 1),
            Self::Int16(v) => write!(v, 2),
            Self::UInt16(v) => write!(v, 2),
            Self::Int32(v) => write!(v, 4),
            Self::UInt32(v) => write!(v, 4),
            Self::Int64(v) => write!(v, 8),
            Self::UInt64(v) => write!(v, 8),
            Self::Float32(v) => {
                for (value, chunk) in v.iter().zip(output.chunks_exact_mut(4)) {
                    chunk.copy_from_slice(&value.0.to_le_bytes());
                }
            }
            Self::Float64(v) => {
                for (value, chunk) in v.iter().zip(output.chunks_exact_mut(8)) {
                    chunk.copy_from_slice(&value.0.to_le_bytes());
                }
            }
        }
    }
}
