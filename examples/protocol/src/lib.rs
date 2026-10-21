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
    #[test]
    fn derived_wire_cache_is_per_type_and_concurrent_first_use_is_stable() {
        use ipc::__private::mojom_value_parser_core::{MojomParse, pack_mojom_type};
        use std::sync::{Arc, Barrier};
        #[derive(ipc::__private::mojom_value_parser::MojomParse)]
        #[mojom(runtime = "::ipc")]
        struct CacheFirst {
            number: u32,
        }
        #[derive(ipc::__private::mojom_value_parser::MojomParse)]
        #[mojom(runtime = "::ipc")]
        struct CacheSecond {
            text: String,
            bytes: Vec<u8>,
        }

        let barrier = Arc::new(Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let first = <CacheFirst as MojomParse>::wire_type();
                    let second = <CacheSecond as MojomParse>::wire_type();
                    assert_eq!(
                        first,
                        &pack_mojom_type(&<CacheFirst as MojomParse>::mojom_type())
                    );
                    assert_eq!(
                        second,
                        &pack_mojom_type(&<CacheSecond as MojomParse>::mojom_type())
                    );
                    assert_ne!(first, second);
                    (first as *const _ as usize, second as *const _ as usize)
                })
            })
            .collect();
        let addresses: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert!(addresses.iter().all(|a| *a == addresses[0]));
    }
}
