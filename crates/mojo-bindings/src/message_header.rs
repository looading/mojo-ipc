// Copyright 2026 The Chromium Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module defines the header used by Mojom messages, and provides utility
//! functions for manipulating it. For a reference implementation, see the C++
//! version at //mojo/public/cpp/bindings/lib/message_internal.h

use sequenced_task_runner::ffi as cxx_helpers;

/// The header format used by Mojom.
///
/// A header value appears at the beginning of every Mojom message. It is
/// encoded as a regular mojom struct; however, it uses versioning, which is
/// not yet supported by the mojom value parser. Therefore, this type must be
/// serialized and deserialized using its own methods, rather than relying on
/// the methods in the mojom_value_parser API.
///
/// This type can represent a mojom header of version 1, 2, or 3. The versions
/// only differ in non-`pub` fields, so users of this type can typically ignore
/// this distinction.
///
/// The `pub` fields may be read and written by the user, so it is the user's
/// responsibility to ensure that the header is valid when sent. The main
/// validity conditions are:
/// - The `interface_id` and `name` fields exist for the message pipe it's being
///   sent on.
/// - The payload of the message is of the type indicated by the `name` field.
/// - The flags are correct.
/// - If this is a response, then the request_id matches the one in the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageHeader {
    /// The version of the header.
    version: u32,
    /// The ordinal of the mojom `interface` this message corresponds to. Used
    /// for sending multiple interfaces via the same pipe.
    pub interface_id: u32,
    /// The ordinal of the message within the above `interface`.
    pub name: u32,
    /// See the MessageHeaderFlags type for possible flags.
    pub flags: MessageHeaderFlags,
    /// A randomly-generated, hopefully-unique value for a message. Used in
    /// tracing, forming the lower part of the 64-bit trace id, which is
    /// used to match trace events for sending and receiving a message
    /// (`name` forms the upper part).
    trace_nonce: u32,
    /// Contains an ID that's used to match responses with their corresponding
    /// request. Unused for messages that don't expect a response.
    pub request_id: u64,
    /// A mojom pointer (i.e. a byte offset) to the message's payload.
    payload_ptr: u64,
    /// A mojom pointer to the interface ID array at the end of the message (or
    /// 0 if it's absent). This field should only be set for messages that
    /// were received over the wire, or which have been fully serialized and
    /// are ready to be sent.
    /// Note that this field's value is measured from the pointer's location
    /// inside the _header_. Parsing code generally wants the distance from the
    /// beginning of the payload instead, which can be obtained using the
    /// `interface_ids_offset` method.
    interface_ids_ptr: u64,
    /// Tracks when the header was created.
    creation_timeticks_us: i64,
}

impl MessageHeader {
    /// Create a new message header with the given fields.
    ///
    /// Note that `interface_ids_offset` should be the offset from the start of
    /// the payload to the interface ID array (or 0, if there isn't an interface
    /// ID array). The constructor will compute the appropriate value for the
    /// `interface_ids_ptr` header field from that information.
    pub fn new(
        interface_id: u32,
        name: u32,
        flags: MessageHeaderFlags,
        request_id: u64,
        interface_ids_offset: u64,
    ) -> Self {
        Self::new_with_version(
            3,
            interface_id,
            name,
            flags,
            request_id,
            interface_ids_offset,
        )
    }

    /// Create a new message at a specific version with the given fields. Fields
    /// which are not present at that version will be ignored.
    ///
    /// Note that `interface_ids_offset` should be the offset from the start of
    /// the payload to the interface ID array (or 0, if there isn't an interface
    /// ID array). The constructor will compute the appropriate value for the
    /// `interface_ids_ptr` from that information.
    pub fn new_with_version(
        version: u32,
        interface_id: u32,
        name: u32,
        flags: MessageHeaderFlags,
        request_id: u64,
        interface_ids_offset: u64,
    ) -> Self {
        // The header contains two pointers, which are next to each other. This
        // is the number of bytes from the end of the interface ID pointer to
        // the end of the header.
        let payload_ptr = match version {
            1 => 0, // Not present in this version
            2 => 16,
            3 => 24,
            _ => panic!("Mojom headers must have version 1, 2 or 3"),
        };
        let interface_ids_distance = match version {
            1 => 0, // Not present in this version
            2 => 8 + interface_ids_offset,
            3 => 16 + interface_ids_offset,
            _ => panic!("Mojom headers must have version 1, 2 or 3"),
        };
        let interface_ids_ptr = if interface_ids_offset == 0 {
            0
        } else {
            interface_ids_distance
        };
        MessageHeader {
            interface_id,
            name,
            flags,
            // The C++ equivalent uses static_cast<uint32_t> on the result here
            trace_nonce: cxx_helpers::GetNextGlobalTraceId() as u32,
            request_id,
            payload_ptr,
            interface_ids_ptr,
            creation_timeticks_us: cxx_helpers::CurrentTimeTicksInMicroseconds(),
            version,
        }
    }

    /// Return the timestamp when this header value was created.
    pub fn creation_timeticks_us(&self) -> i64 {
        self.creation_timeticks_us
    }

    /// Return the offset from the beginning of the _payload_ to the beginning
    /// of the interface IDs array, or 0 if the array is absent. Note that this
    /// is not the same as the `interface_ids_ptr` header field, which is
    /// measured from a point inside the header rather than relative to the
    /// payload.
    pub fn interface_ids_offset(&self) -> u64 {
        if self.interface_ids_ptr == 0 {
            return 0;
        }
        // Use saturating subtraction to prevent underflow if we got a bad
        // message. If it happens, we'll probably fail shortly when we
        // try to actually parse the message.
        match self.version {
            1 => 0, // Not present in this version
            2 => self.interface_ids_ptr.saturating_sub(8),
            3 => self.interface_ids_ptr.saturating_sub(16),
            _ => panic!("Mojom headers must have version 1, 2 or 3"),
        }
    }

    /// Encode the header into stack storage for the message send path.
    /// Only the returned prefix belongs to this header version.
    pub(crate) fn serialize_stack(self) -> ([u8; 56], usize) {
        let len: usize = match self.version {
            1 => 32,
            2 => 48,
            3 => 56,
            _ => panic!("Header version must be 1, 2, or 3"),
        };
        let mut bytes = [0; 56];
        bytes[0..4].copy_from_slice(&(len as u32).to_le_bytes());
        bytes[4..8].copy_from_slice(&self.version.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.interface_id.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.name.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.flags.bits().to_le_bytes());
        bytes[20..24].copy_from_slice(&self.trace_nonce.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.request_id.to_le_bytes());
        if self.version >= 2 {
            bytes[32..40].copy_from_slice(&self.payload_ptr.to_le_bytes());
            bytes[40..48].copy_from_slice(&self.interface_ids_ptr.to_le_bytes());
        }
        if self.version == 3 {
            bytes[48..56].copy_from_slice(&self.creation_timeticks_us.to_le_bytes());
        }
        (bytes, len)
    }

    /// Serialize the header as a mojom value
    pub fn serialize(self) -> Vec<u8> {
        let v1 = mojom_value_parser::MessageHeaderV1 {
            interface_id: self.interface_id,
            name: self.name,
            flags: self.flags.bits(),
            trace_nonce: self.trace_nonce,
            request_id: self.request_id,
        };
        let v2 = mojom_value_parser::MessageHeaderV2 {
            payload_ptr: self.payload_ptr,
            interface_ids_ptr: self.interface_ids_ptr,
        };
        let v3 = mojom_value_parser::MessageHeaderV3 {
            creation_timeticks_us: self.creation_timeticks_us,
        };

        let header = match self.version {
            1 => mojom_value_parser::MessageHeader::V1(v1),
            2 => mojom_value_parser::MessageHeader::V2(v1, v2),
            3 => mojom_value_parser::MessageHeader::V3(v1, v2, v3),
            _ => panic!("Header version must be 1, 2, or 3"),
        };

        header.serialize()
    }

    /// Deserialize the header from a mojom value.
    pub fn deserialize(data: &[u8]) -> mojom_value_parser::ParsingResult<(&[u8], Self)> {
        let (remaining, header) = mojom_value_parser::MessageHeader::deserialize(data)?;

        let result = match header {
            mojom_value_parser::MessageHeader::V1(v1) => MessageHeader {
                interface_id: v1.interface_id,
                name: v1.name,
                flags: MessageHeaderFlags::from_bits_truncate(v1.flags),
                trace_nonce: v1.trace_nonce,
                request_id: v1.request_id,
                payload_ptr: 0,
                interface_ids_ptr: 0,
                creation_timeticks_us: 0,
                version: 1,
            },
            mojom_value_parser::MessageHeader::V2(v1, v2) => MessageHeader {
                interface_id: v1.interface_id,
                name: v1.name,
                flags: MessageHeaderFlags::from_bits_truncate(v1.flags),
                trace_nonce: v1.trace_nonce,
                request_id: v1.request_id,
                payload_ptr: v2.payload_ptr,
                interface_ids_ptr: v2.interface_ids_ptr,
                creation_timeticks_us: 0,
                version: 2,
            },
            mojom_value_parser::MessageHeader::V3(v1, v2, v3) => MessageHeader {
                interface_id: v1.interface_id,
                name: v1.name,
                flags: MessageHeaderFlags::from_bits_truncate(v1.flags),
                trace_nonce: v1.trace_nonce,
                request_id: v1.request_id,
                payload_ptr: v2.payload_ptr,
                interface_ids_ptr: v2.interface_ids_ptr,
                creation_timeticks_us: v3.creation_timeticks_us,
                version: 3,
            },
        };

        Ok((remaining, result))
    }
}

bitflags::bitflags! {
    /// Flags that can be set for a Mojom message.
    /// Only the first two flags are supported in Rust at the moment.
    #[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
    #[repr(transparent)]
    pub struct MessageHeaderFlags: u32 {
        const EXPECTS_RESPONSE = 1 << 0;
        const IS_RESPONSE = 1 << 1;
        const IS_SYNC = 1 << 2;
        const NO_INTERRUPT = 1 << 3;
        const IS_URGENT = 1 << 4;
    }
}

impl MessageHeaderFlags {
    /// Validate the response flags on a request: it should not itself be a
    /// response, and should only request one if we expect it to.
    pub fn is_valid_request(&self, expects_response: bool) -> bool {
        !self.contains(Self::IS_RESPONSE)
            && self.contains(Self::EXPECTS_RESPONSE) == expects_response
    }

    /// Validate the response flags on a response: it should be a response
    /// (obviously), and should not request one.
    pub fn is_valid_response(&self) -> bool {
        self.contains(Self::IS_RESPONSE) && !self.contains(Self::EXPECTS_RESPONSE)
    }
}

// This section is an implementation detail: we have to manually implement
// (de)serialization for the flag type, since the output of `bitflags!` has
// special semantics: it's a struct, but we want to treat it like an int. Also,
// not all flag combinations are valid.
const _: () = {
    use mojom_value_parser_core::{MojomType, MojomValue};

    impl MessageHeaderFlags {
        /// Check if a given combination of flags is valid.
        /// Validity conditions:
        /// - Responses must not expect a response.
        fn is_valid(&self) -> bool {
            !(self.contains(Self::EXPECTS_RESPONSE) && self.contains(Self::IS_RESPONSE))
        }
    }

    impl<Context> mojom_value_parser::MojomParse<Self, Context> for MessageHeaderFlags {
        fn mojom_type() -> MojomType {
            MojomType::UInt32
        }

        fn into_mojom_value(self, _context: &Context) -> MojomValue {
            MojomValue::UInt32(self.bits())
        }

        fn try_from_mojom_value(value: MojomValue, _context: &Context) -> anyhow::Result<Self> {
            let bits: u32 = if let MojomValue::UInt32(bits) = value {
                bits
            } else {
                anyhow::bail!(
                    "Cannot construct a value of type {} from this MojomValue: {:?}",
                    std::any::type_name::<Self>(),
                    value
                );
            };
            match Self::from_bits(bits) {
                None => anyhow::bail!("Mojom header bitflags set unused bits: {:?}", bits),
                Some(flags) if !flags.is_valid() => {
                    anyhow::bail!("Mojom header bitflags are invalid: {:?}", bits)
                }
                Some(flags) => Ok(flags),
            }
        }
    }
};

#[cfg(test)]
mod stack_serialization_tests {
    use super::*;

    // Construct headers directly: serializing a header does not require a
    // loaded native runtime, unlike the trace/time helpers used by `new`.
    fn asymmetric_header(version: u32) -> MessageHeader {
        MessageHeader {
            version,
            interface_id: 0x4433_2211,
            name: 0x8877_6655,
            flags: MessageHeaderFlags::from_bits_retain(0xa5a5_1213),
            trace_nonce: 0xccbb_aa99,
            request_id: 0x0807_0605_0403_0201,
            payload_ptr: 0x1817_1615_1413_1211,
            interface_ids_ptr: 0x2827_2625_2423_2221,
            creation_timeticks_us: 0xc8c7_c6c5_c4c3_c2c1u64 as i64,
        }
    }

    fn core_reference(header: MessageHeader) -> Vec<u8> {
        let v1 = mojom_value_parser::MessageHeaderV1 {
            interface_id: header.interface_id,
            name: header.name,
            flags: header.flags.bits(),
            trace_nonce: header.trace_nonce,
            request_id: header.request_id,
        };
        let v2 = mojom_value_parser::MessageHeaderV2 {
            payload_ptr: header.payload_ptr,
            interface_ids_ptr: header.interface_ids_ptr,
        };
        let v3 = mojom_value_parser::MessageHeaderV3 {
            creation_timeticks_us: header.creation_timeticks_us,
        };
        match header.version {
            1 => mojom_value_parser::MessageHeader::V1(v1),
            2 => mojom_value_parser::MessageHeader::V2(v1, v2),
            3 => mojom_value_parser::MessageHeader::V3(v1, v2, v3),
            _ => unreachable!(),
        }
        .serialize()
    }

    #[test]
    fn stack_header_matches_literal_wire_layout_for_each_version() {
        // Independent, asymmetric bytes catch swapped fields and host-endian
        // writes; flags deliberately retain unknown and conflicting bits.
        let golden_v3 = [
            0x38, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
            0x77, 0x88, 0x13, 0x12, 0xa5, 0xa5, 0x99, 0xaa, 0xbb, 0xcc, 0x01, 0x02, 0x03, 0x04,
            0x05, 0x06, 0x07, 0x08, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x21, 0x22,
            0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8,
        ];
        for (version, expected_len) in [(1, 32), (2, 48), (3, 56)] {
            let mut expected = golden_v3;
            expected[0] = expected_len as u8;
            expected[4] = version as u8;
            let (bytes, len) = asymmetric_header(version).serialize_stack();
            assert_eq!(len, expected_len);
            assert_eq!(&bytes[..len], &expected[..len]);
            assert!(bytes[len..].iter().all(|byte| *byte == 0));
        }
    }

    #[test]
    fn stack_header_matches_core_for_extremes_and_unknown_flags() {
        for version in [1, 2, 3] {
            let base = asymmetric_header(version);
            let cases = [
                base,
                MessageHeader {
                    interface_id: 0,
                    name: 0,
                    flags: MessageHeaderFlags::empty(),
                    trace_nonce: 0,
                    request_id: 0,
                    payload_ptr: 0,
                    interface_ids_ptr: 0,
                    creation_timeticks_us: 0,
                    ..base
                },
                MessageHeader {
                    interface_id: u32::MAX,
                    name: u32::MAX,
                    flags: MessageHeaderFlags::from_bits_retain(u32::MAX),
                    trace_nonce: u32::MAX,
                    request_id: u64::MAX,
                    payload_ptr: u64::MAX,
                    interface_ids_ptr: u64::MAX,
                    creation_timeticks_us: i64::MAX,
                    ..base
                },
                MessageHeader {
                    creation_timeticks_us: i64::MIN,
                    ..base
                },
            ];
            for header in cases {
                let (bytes, len) = header.serialize_stack();
                let reference = core_reference(header);
                assert_eq!(&bytes[..len], reference);
                assert_eq!(header.serialize(), reference);
            }
        }
    }

    #[test]
    fn earlier_versions_omit_later_fields_without_normalizing_pointers() {
        let v1 = asymmetric_header(1);
        let changed_v1 = MessageHeader {
            payload_ptr: 0,
            interface_ids_ptr: 0,
            creation_timeticks_us: 0,
            ..v1
        };
        assert_eq!(v1.serialize_stack(), changed_v1.serialize_stack());

        let v2 = asymmetric_header(2);
        let changed_v2 = MessageHeader {
            creation_timeticks_us: 0,
            ..v2
        };
        assert_eq!(v2.serialize_stack(), changed_v2.serialize_stack());
        let (bytes, len) = v2.serialize_stack();
        assert_eq!(len, 48);
        assert_eq!(&bytes[32..40], &v2.payload_ptr.to_le_bytes());
        assert_eq!(&bytes[40..48], &v2.interface_ids_ptr.to_le_bytes());
    }
}
