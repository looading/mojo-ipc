//! Shared deterministic fixtures for the typed matrix and isolated codec probe.
//! No dependency on private codec AST construction or on the native runtime.
use crate::protocol::bench::*;
use anyhow::{Context, Result, ensure};
use ipc::__private::{mojom_value_parser as codec, ordered_float::OrderedFloat};
use ipc::Remote;
use serde_json::{Value, json};
use std::{collections::HashMap, hash::Hash};

pub const CASE_NAMES: [&str; 9] = [
    "utf8_string",
    "record",
    "record_array",
    "nested_array",
    "string_map",
    "numeric_map",
    "union_array",
    "nullable_nested",
    "string_array",
];
pub const SIZES: [(&str, usize); 3] = [("small", 1_024), ("medium", 16_384), ("large", 131_072)];

/// FNV-1a over a typed, length-prefixed, canonical *logical* representation.
/// This is validation, not a wire checksum or a cryptographic integrity claim.
pub struct ContentHash {
    hash: u64,
    pub strings: usize,
    pub utf8_bytes: usize,
    pub arrays: usize,
    pub array_elements: usize,
    pub maps: usize,
    pub map_entries: usize,
    pub records: usize,
    pub unions: usize,
    pub some: usize,
    pub none: usize,
}
impl ContentHash {
    pub fn new() -> Self {
        Self {
            hash: 0xcbf2_9ce4_8422_2325,
            strings: 0,
            utf8_bytes: 0,
            arrays: 0,
            array_elements: 0,
            maps: 0,
            map_entries: 0,
            records: 0,
            unions: 0,
            some: 0,
            none: 0,
        }
    }
    pub fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.hash ^= u64::from(*byte);
            self.hash = self.hash.wrapping_mul(0x100_0000_01b3);
        }
    }
    fn tag(&mut self, tag: u8) {
        self.bytes(&[tag]);
    }
    fn length(&mut self, length: usize) {
        self.bytes(&(length as u64).to_le_bytes());
    }
    pub fn checksum(&self) -> String {
        format!("{:016x}", self.hash)
    }
    pub fn summary(&self) -> Value {
        json!({"strings":self.strings,"utf8_bytes":self.utf8_bytes,"arrays":self.arrays,
            "array_elements":self.array_elements,"maps":self.maps,"map_entries":self.map_entries,
            "records":self.records,"unions":self.unions,"some":self.some,"none":self.none})
    }
}
pub trait CheckedValue: Clone + Send + Sync + 'static {
    fn same_bits(&self, other: &Self) -> bool;
    fn feed(&self, hash: &mut ContentHash);
    fn checksum(&self) -> String {
        let mut hash = ContentHash::new();
        self.feed(&mut hash);
        hash.checksum()
    }
}
macro_rules! integer {
    ($($ty:ty => $tag:expr),* $(,)?) => { $(impl CheckedValue for $ty {
        fn same_bits(&self, other:&Self)->bool { self == other }
        fn feed(&self, hash:&mut ContentHash) { hash.tag($tag); hash.bytes(&self.to_le_bytes()); }
    })* };
}
integer!(u8=>1,u32=>2,u64=>3,i32=>4,i64=>5);
impl CheckedValue for bool {
    fn same_bits(&self, other: &Self) -> bool {
        self == other
    }
    fn feed(&self, hash: &mut ContentHash) {
        hash.tag(6);
        hash.tag(u8::from(*self));
    }
}
impl CheckedValue for OrderedFloat<f64> {
    fn same_bits(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
    fn feed(&self, hash: &mut ContentHash) {
        hash.tag(7);
        hash.bytes(&self.0.to_bits().to_le_bytes());
    }
}
impl CheckedValue for String {
    fn same_bits(&self, other: &Self) -> bool {
        self == other
    }
    fn feed(&self, hash: &mut ContentHash) {
        hash.tag(8);
        hash.length(self.len());
        hash.bytes(self.as_bytes());
        hash.strings += 1;
        hash.utf8_bytes += self.len();
    }
}
impl<T: CheckedValue> CheckedValue for Vec<T> {
    fn same_bits(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().zip(other).all(|(a, b)| a.same_bits(b))
    }
    fn feed(&self, hash: &mut ContentHash) {
        hash.tag(9);
        hash.length(self.len());
        hash.arrays += 1;
        hash.array_elements += self.len();
        for value in self {
            value.feed(hash);
        }
    }
}
impl<T: CheckedValue> CheckedValue for Option<T> {
    fn same_bits(&self, other: &Self) -> bool {
        match (self, other) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same_bits(b),
            _ => false,
        }
    }
    fn feed(&self, hash: &mut ContentHash) {
        hash.tag(10);
        match self {
            None => {
                hash.tag(0);
                hash.none += 1;
            }
            Some(value) => {
                hash.tag(1);
                hash.some += 1;
                value.feed(hash);
            }
        }
    }
}
impl<T: CheckedValue> CheckedValue for Box<T> {
    fn same_bits(&self, other: &Self) -> bool {
        (**self).same_bits(other)
    }
    fn feed(&self, hash: &mut ContentHash) {
        (**self).feed(hash);
    }
}
impl<K: CheckedValue + Ord + Eq + Hash, V: CheckedValue> CheckedValue for HashMap<K, V> {
    fn same_bits(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .all(|(k, v)| other.get(k).is_some_and(|b| v.same_bits(b)))
    }
    fn feed(&self, hash: &mut ContentHash) {
        hash.tag(11);
        hash.length(self.len());
        hash.maps += 1;
        hash.map_entries += self.len();
        let mut keys: Vec<_> = self.keys().collect();
        keys.sort_unstable();
        for key in keys {
            key.feed(hash);
            self[key].feed(hash);
        }
    }
}
impl CheckedValue for ComplexStatus {
    fn same_bits(&self, other: &Self) -> bool {
        self.clone() as i32 == other.clone() as i32
    }
    fn feed(&self, hash: &mut ContentHash) {
        hash.tag(12);
        (self.clone() as i32).feed(hash);
    }
}
impl CheckedValue for ComplexRecord {
    fn same_bits(&self, b: &Self) -> bool {
        self.sequence.same_bits(&b.sequence)
            && self.status.same_bits(&b.status)
            && self.delta.same_bits(&b.delta)
            && self.enabled.same_bits(&b.enabled)
            && self.ratio.same_bits(&b.ratio)
            && self.title.same_bits(&b.title)
            && self.data.same_bits(&b.data)
            && self.note.same_bits(&b.note)
    }
    fn feed(&self, h: &mut ContentHash) {
        h.tag(13);
        h.records += 1;
        self.sequence.feed(h);
        self.status.feed(h);
        self.delta.feed(h);
        self.enabled.feed(h);
        self.ratio.feed(h);
        self.title.feed(h);
        self.data.feed(h);
        self.note.feed(h);
    }
}
impl CheckedValue for ComplexChoice {
    fn same_bits(&self, b: &Self) -> bool {
        match (self, b) {
            (Self::Text(a), Self::Text(b)) => a.same_bits(b),
            (Self::Record(a), Self::Record(b)) => a.same_bits(b),
            (Self::Numbers(a), Self::Numbers(b)) => a.same_bits(b),
            (Self::Metric(a), Self::Metric(b)) => a.same_bits(b),
            _ => false,
        }
    }
    fn feed(&self, h: &mut ContentHash) {
        h.tag(14);
        h.unions += 1;
        match self {
            Self::Text(x) => {
                h.tag(0);
                x.feed(h)
            }
            Self::Record(x) => {
                h.tag(1);
                x.feed(h)
            }
            Self::Numbers(x) => {
                h.tag(2);
                x.feed(h)
            }
            Self::Metric(x) => {
                h.tag(3);
                x.feed(h)
            }
        }
    }
}
impl CheckedValue for ComplexEnvelope {
    fn same_bits(&self, b: &Self) -> bool {
        self.status.same_bits(&b.status)
            && self.primary.same_bits(&b.primary)
            && self.records.same_bits(&b.records)
            && self.matrix.same_bits(&b.matrix)
            && self.choices.same_bits(&b.choices)
            && self.samples.same_bits(&b.samples)
            && self.note.same_bits(&b.note)
    }
    fn feed(&self, h: &mut ContentHash) {
        h.tag(15);
        h.records += 1;
        self.status.feed(h);
        self.primary.feed(h);
        self.records.feed(h);
        self.matrix.feed(h);
        self.choices.feed(h);
        self.samples.feed(h);
        self.note.feed(h);
    }
}
fn bits(index: usize) -> u64 {
    (index as u64)
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .rotate_left((index % 64) as u32)
}
fn status(index: usize) -> ComplexStatus {
    match index % 3 {
        0 => ComplexStatus::Red,
        1 => ComplexStatus::Green,
        _ => ComplexStatus::Blue,
    }
}
pub fn float(index: usize) -> OrderedFloat<f64> {
    const BITS: [u64; 10] = [
        0,
        0x8000_0000_0000_0000,
        0x3ff0_0000_0000_0000,
        0xbff0_0000_0000_0000,
        0x7ff0_0000_0000_0000,
        0xfff0_0000_0000_0000,
        0x7ff8_0000_0000_1234,
        0xfff8_0000_0000_4321,
        1,
        0x7fef_ffff_ffff_ffff,
    ];
    OrderedFloat(f64::from_bits(BITS[index % BITS.len()]))
}
fn record(scale: usize, index: usize) -> ComplexRecord {
    ComplexRecord {
        sequence: bits(index),
        status: status(index),
        delta: bits(index + 1) as i32,
        enabled: index % 3 != 0,
        ratio: float(index),
        title: format!("{index}:{}", "Mojo 通信🦀\0é".repeat(scale)),
        data: (0..scale * 8).map(|i| bits(i + index) as u8).collect(),
        note: if index % 2 == 0 {
            None
        } else {
            Some(if index % 3 == 0 {
                String::new()
            } else {
                "注记\0🦀".into()
            })
        },
    }
}
fn minimal_record(index: usize) -> ComplexRecord {
    let mut x = record(0, index);
    x.title = String::new();
    x.note = None;
    x
}
fn envelope(index: usize) -> Option<Box<ComplexEnvelope>> {
    match index % 4 {
        0 => None,
        1 => Some(Box::new(ComplexEnvelope {
            status: status(index),
            primary: None,
            records: None,
            matrix: None,
            choices: None,
            samples: vec![],
            note: None,
        })),
        2 => Some(Box::new(ComplexEnvelope {
            status: status(index),
            primary: None,
            records: Some(vec![None]),
            matrix: Some(vec![None, Some(vec![])]),
            choices: Some(HashMap::from([("空".into(), None)])),
            samples: vec![None, Some(float(1)), Some(float(6))],
            note: Some("嵌套\0".into()),
        })),
        _ => Some(Box::new(ComplexEnvelope {
            status: status(index),
            primary: Some(Box::new(minimal_record(index))),
            records: Some(vec![Some(Box::new(minimal_record(index + 1)))]),
            matrix: Some(vec![Some(vec![bits(index) as u32, 0])]),
            choices: Some(HashMap::from([(
                "m".into(),
                Some(Box::new(ComplexChoice::Metric(float(index)))),
            )])),
            samples: vec![Some(float(index + 2))],
            note: Some(String::new()),
        })),
    }
}

pub trait ComplexCase: 'static {
    type Data: CheckedValue;
    type Params: codec::MojomParse<Self::Params, ()>;
    type Response: codec::MojomParse<Self::Response, ()>;
    const NAME: &'static str;
    const MIN_SCALE: usize = 1;
    fn make(scale: usize) -> Self::Data;
    fn top_level_items(data: &Self::Data) -> usize;
    fn params(id: u64, data: Self::Data) -> Self::Params;
    fn unpack(params: Self::Params) -> (u64, Self::Data);
    fn response(id: u64, data: Self::Data) -> Self::Response;
    fn send(
        remote: &mut Remote<dyn Bench>,
        id: u64,
        data: Self::Data,
        callback: impl FnOnce(u64, Self::Data) + Send + 'static,
    );
}
macro_rules! case {
    ($case:ident,$name:literal,$ty:ty,$minimum:literal,$method:ident,$params:ident,$response:ident,$make:expr,$items:expr) => {
        pub struct $case;
        impl ComplexCase for $case {
            type Data = $ty;
            type Params = $params;
            type Response = $response;
            const NAME: &'static str = $name;
            const MIN_SCALE: usize = $minimum;
            fn make(scale: usize) -> Self::Data {
                ($make)(scale)
            }
            fn top_level_items(data: &Self::Data) -> usize {
                ($items)(data)
            }
            fn params(id: u64, data: Self::Data) -> Self::Params {
                $params { id, data }
            }
            fn unpack(params: Self::Params) -> (u64, Self::Data) {
                (params.id, params.data)
            }
            fn response(echoed_id: u64, echoed_data: Self::Data) -> Self::Response {
                $response {
                    echoed_id,
                    echoed_data,
                }
            }
            fn send(
                remote: &mut Remote<dyn Bench>,
                id: u64,
                data: Self::Data,
                callback: impl FnOnce(u64, Self::Data) + Send + 'static,
            ) {
                remote.$method(id, data, callback);
            }
        }
    };
}
case!(
    Utf8String,
    "utf8_string",
    String,
    1,
    EchoComplexString,
    Bench_EchoComplexString_Params,
    Bench_EchoComplexString_ResponseParams,
    |scale| "Mojo 通信🦀\0é".repeat(scale),
    |x: &String| x.chars().count()
);
case!(
    Record,
    "record",
    ComplexRecord,
    1,
    EchoComplexRecord,
    Bench_EchoComplexRecord_Params,
    Bench_EchoComplexRecord_ResponseParams,
    |scale| record(scale, 7),
    |_: &ComplexRecord| 1
);
case!(
    RecordArray,
    "record_array",
    Vec<ComplexRecord>,
    1,
    EchoComplexRecords,
    Bench_EchoComplexRecords_Params,
    Bench_EchoComplexRecords_ResponseParams,
    |scale| (0..scale).map(|i| record(1, i)).collect(),
    |x: &Vec<ComplexRecord>| x.len()
);
case!(
    NestedArray,
    "nested_array",
    Vec<Vec<u32>>,
    1,
    EchoComplexNested,
    Bench_EchoComplexNested_Params,
    Bench_EchoComplexNested_ResponseParams,
    |scale| (0..scale)
        .map(|i| (0..[0, 1, 7, 8, 9, 31, 64][i % 7])
            .map(|j| bits(i + j) as u32)
            .collect())
        .collect(),
    |x: &Vec<Vec<u32>>| x.len()
);
case!(StringMap,"string_map",HashMap<String,ComplexRecord>,1,EchoComplexStringMap,Bench_EchoComplexStringMap_Params,Bench_EchoComplexStringMap_ResponseParams,
    |scale|(0..scale).map(|i|(format!("键/{i:06}/🦀"),record(1,i))).collect(),|x:&HashMap<String,ComplexRecord>|x.len());
case!(NumericMap,"numeric_map",HashMap<i32,Vec<u8>>,1,EchoComplexNumericMap,Bench_EchoComplexNumericMap_Params,Bench_EchoComplexNumericMap_ResponseParams,
    |scale|(0..scale).map(|i|(if i%2==0 {i as i32}else{-(i as i32)},(0..i%18).map(|j|bits(i+j)as u8).collect())).collect(),|x:&HashMap<i32,Vec<u8>>|x.len());
case!(
    UnionArray,
    "union_array",
    Vec<ComplexChoice>,
    4,
    EchoComplexUnions,
    Bench_EchoComplexUnions_Params,
    Bench_EchoComplexUnions_ResponseParams,
    |scale| (0..scale)
        .map(|i| match i % 4 {
            0 => ComplexChoice::Text(format!("联合/{i}/🦀\0")),
            1 => ComplexChoice::Record(record(1, i)),
            2 => ComplexChoice::Numbers((0..i % 11).map(|j| bits(i + j) as i64).collect()),
            _ => ComplexChoice::Metric(float(i)),
        })
        .collect(),
    |x: &Vec<ComplexChoice>| x.len()
);
case!(
    NullableNested,
    "nullable_nested",
    Vec<Option<Box<ComplexEnvelope>>>,
    4,
    EchoComplexNullable,
    Bench_EchoComplexNullable_Params,
    Bench_EchoComplexNullable_ResponseParams,
    |scale| (0..scale).map(envelope).collect(),
    |x: &Vec<Option<Box<ComplexEnvelope>>>| x.len()
);
case!(
    StringArray,
    "string_array",
    Vec<String>,
    1,
    EchoComplexStrings,
    Bench_EchoComplexStrings_Params,
    Bench_EchoComplexStrings_ResponseParams,
    |scale| (0..scale)
        .map(|i| if i % 5 == 0 {
            String::new()
        } else {
            format!("{i}:{}", "字符串🦀\0".repeat(i % 7 + 1))
        })
        .collect(),
    |x: &Vec<String>| x.len()
);

macro_rules! all_cases {
    ($function:ident $(,$argument:expr)* $(,)?) => {
        $function::<$crate::complex_cases::Utf8String>($($argument),*)?;
        $function::<$crate::complex_cases::Record>($($argument),*)?;
        $function::<$crate::complex_cases::RecordArray>($($argument),*)?;
        $function::<$crate::complex_cases::NestedArray>($($argument),*)?;
        $function::<$crate::complex_cases::StringMap>($($argument),*)?;
        $function::<$crate::complex_cases::NumericMap>($($argument),*)?;
        $function::<$crate::complex_cases::UnionArray>($($argument),*)?;
        $function::<$crate::complex_cases::NullableNested>($($argument),*)?;
        $function::<$crate::complex_cases::StringArray>($($argument),*)?;
    };
}
pub(crate) use all_cases;

pub fn encode<C: ComplexCase>(id: u64, data: C::Data) -> Result<Vec<u8>> {
    let (bytes, handles, offset) = codec::serialize::<(), C::Params>(C::params(id, data), &());
    ensure!(
        handles.is_empty() && offset == 0,
        "complex case produced handles/interface IDs"
    );
    Ok(bytes)
}
pub fn decode<C: ComplexCase>(bytes: &[u8]) -> Result<(u64, C::Data)> {
    let value = codec::deserialize_exact::<(), C::Params>(bytes, &mut [], 0, &())
        .with_context(|| format!("decode {}", C::NAME))?;
    Ok(C::unpack(value))
}
pub struct Fixture<C: ComplexCase> {
    pub data: C::Data,
    pub scale: usize,
    pub size_label: &'static str,
    pub budget: usize,
    pub request_bytes: usize,
    pub response_bytes: usize,
    pub checksum: String,
    pub content: Value,
}
impl<C: ComplexCase> Fixture<C> {
    pub fn new(size_label: &'static str, budget: usize) -> Result<Self> {
        let encoded_size = |scale| -> Result<usize> { Ok(encode::<C>(0, C::make(scale))?.len()) };
        ensure!(
            encoded_size(C::MIN_SCALE)? <= budget,
            "{} minimum scale {} exceeds budget {}",
            C::NAME,
            C::MIN_SCALE,
            budget
        );
        let mut low = C::MIN_SCALE;
        let mut high = low.saturating_mul(2).max(low + 1);
        while encoded_size(high)? <= budget {
            low = high;
            high = high
                .checked_mul(2)
                .context("complex fixture scale overflow")?;
        }
        while low + 1 < high {
            let middle = low + (high - low) / 2;
            if encoded_size(middle)? <= budget {
                low = middle;
            } else {
                high = middle;
            }
        }
        let data = C::make(low);
        let bytes = encode::<C>(0, data.clone())?;
        let (id, actual) = decode::<C>(&bytes)?;
        let mut hash = ContentHash::new();
        data.feed(&mut hash);
        let checksum = hash.checksum();
        ensure!(
            id == 0 && data.same_bits(&actual) && actual.checksum() == checksum,
            "complex preflight round-trip mismatch"
        );
        let (response, handles, offset) =
            codec::serialize::<(), C::Response>(C::response(0, data.clone()), &());
        ensure!(
            handles.is_empty() && offset == 0,
            "complex response produced handles/interface IDs"
        );
        Ok(Self {
            data,
            scale: low,
            size_label,
            budget,
            request_bytes: bytes.len(),
            response_bytes: response.len(),
            checksum,
            content: hash.summary(),
        })
    }
    pub fn metadata(&self) -> Value {
        json!({"complex_case":C::NAME,"size_label":self.size_label,"scale":self.scale,
            "top_level_items":C::top_level_items(&self.data),"target_encoded_body_budget_bytes":self.budget,
            "encoded_body_bytes":self.request_bytes,"request_encoded_body_bytes":self.request_bytes,
            "response_encoded_body_bytes":self.response_bytes,"content_checksum_fnv1a64":self.checksum,
            "content_summary":self.content,"content_validated":true,"window":1})
    }
    pub fn validate(&self, id: u64, actual_id: u64, actual: &C::Data) -> bool {
        id == actual_id && self.data.same_bits(actual) && actual.checksum() == self.checksum
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn boundaries<C: ComplexCase>() -> Result<()> {
        for scale in [0, 1, 4, 9] {
            let expected = C::make(scale);
            let bytes = encode::<C>(u64::MAX, expected.clone())?;
            let (id, actual) = decode::<C>(&bytes)?;
            ensure!(
                id == u64::MAX
                    && expected.same_bits(&actual)
                    && expected.checksum() == actual.checksum(),
                "{} scale {scale}",
                C::NAME
            );
        }
        Ok(())
    }
    #[test]
    fn complex_generated_codec_boundaries() -> Result<()> {
        all_cases!(boundaries);
        Ok(())
    }
    #[test]
    fn all_small_fixtures_fit_budget() -> Result<()> {
        fn check<C: ComplexCase>() -> Result<()> {
            let fixture = Fixture::<C>::new("small", 1024)?;
            ensure!(fixture.request_bytes <= 1024 && fixture.scale >= C::MIN_SCALE);
            Ok(())
        }
        all_cases!(check);
        Ok(())
    }
    #[test]
    fn canonical_maps_and_float_bits() {
        let a = HashMap::from([(2_i32, "二".to_owned()), (-1, "一".to_owned())]);
        let b = HashMap::from([(-1_i32, "一".to_owned()), (2, "二".to_owned())]);
        assert_eq!(a.checksum(), b.checksum());
        assert!(a.same_bits(&b));
        assert!(!float(0).same_bits(&float(1)));
        assert_ne!(float(0).checksum(), float(1).checksum());
        assert!(!float(6).same_bits(&float(7)));
        assert_ne!(None::<String>.checksum(), Some(String::new()).checksum());
        assert_ne!(
            ComplexChoice::Text(String::new()).checksum(),
            ComplexChoice::Numbers(vec![]).checksum()
        );
    }
}
