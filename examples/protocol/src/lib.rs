//! Example typed protocol generated from the pinned official Mojom toolchain.
//!
//! The renamed `ipc` runtime dependency also exercises generated-code paths.

include!(concat!(env!("OUT_DIR"), "/mojom/mod.rs"));

#[cfg(test)]
mod tests {
    use super::common::{MessageKind, Record};
    use ipc::__private::mojom_value_parser::{deserialize_exact, serialize};

    fn record() -> Record {
        Record {
            id: 42,
            text: "typed Mojom / 类型化接口".into(),
            data: vec![0, 1, 255],
            kind: MessageKind::Binary,
            note: Some("optional field".into()),
        }
    }

    #[test]
    fn generated_imported_record_round_trips_with_renamed_runtime() {
        let expected = record();
        let (payload, handles, interface_ids_offset) = serialize(expected.clone(), &());
        assert!(handles.is_empty());
        let decoded: Record =
            deserialize_exact(&payload, &mut [], interface_ids_offset, &()).unwrap();
        assert_eq!(decoded, expected);
        assert_eq!(MessageKind::Binary, MessageKind::Bytes);
    }

    #[test]
    fn generated_record_rejects_truncated_wire_payload() {
        let (mut payload, _, interface_ids_offset) = serialize(record(), &());
        payload.truncate(payload.len() / 2);
        assert!(
            deserialize_exact::<_, Record>(&payload, &mut [], interface_ids_offset, &(),).is_err()
        );
    }
}
