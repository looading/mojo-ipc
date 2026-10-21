use mojo_ipc::__private::mojom_value_parser::{MojomParse, deserialize_exact, serialize};
use mojo_ipc::__private::mojom_value_parser_core::{MojomValue, deparse_top_level_value};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, MojomParse)]
#[mojom(runtime = "::mojo_ipc")]
struct Row {
    label: String,
    count: u32,
}

#[derive(Clone, Debug, PartialEq, MojomParse)]
#[mojom(runtime = "::mojo_ipc")]
enum Choice {
    Row(Row),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, MojomParse)]
#[mojom(runtime = "::mojo_ipc")]
struct Envelope {
    rows: Vec<Row>,
    fixed: [Row; 2],
    optional: Option<Box<Row>>,
    map: HashMap<String, Row>,
    choices: Vec<Option<Choice>>,
}

fn row(count: u32) -> Row {
    Row {
        label: format!("类型\0🙂/{count}"),
        count,
    }
}

fn fixture() -> Envelope {
    Envelope {
        rows: vec![row(1), row(2)],
        fixed: [row(3), row(4)],
        optional: Some(Box::new(row(5))),
        map: HashMap::from([("z".into(), row(6)), ("a".into(), row(7))]),
        choices: vec![
            None,
            Some(Choice::Row(row(8))),
            Some(Choice::Text("文本".into())),
        ],
    }
}

fn check_names(value: &MojomValue, named: bool) -> usize {
    match value {
        MojomValue::Struct(names, fields) => {
            if named {
                assert_eq!(names.len(), fields.len());
                assert!(names.iter().all(|name| !name.is_empty()));
            } else {
                assert!(names.is_empty());
            }
            1 + fields.iter().map(|v| check_names(v, named)).sum::<usize>()
        }
        MojomValue::Array(values) => values.iter().map(|v| check_names(v, named)).sum(),
        MojomValue::Map(values) => values
            .iter()
            .map(|(k, v)| check_names(k, named) + check_names(v, named))
            .sum(),
        MojomValue::Nullable(Some(value)) | MojomValue::Union(_, value) => {
            check_names(value, named)
        }
        _ => 0,
    }
}

#[test]
fn serialization_omits_only_diagnostic_names_through_every_container() {
    let value = fixture();
    let public_ast = value.clone().into_mojom_value(&());
    assert_eq!(check_names(&public_ast, true), 9);
    let wire_ast = value.clone().into_mojom_value_for_wire(&());
    assert_eq!(check_names(&wire_ast, false), 9);
    let (expected, handles, offset) =
        deparse_top_level_value(public_ast, <Envelope as MojomParse>::wire_type()).unwrap();
    assert!(handles.is_empty());
    assert_eq!(offset, 0);
    let (actual, handles, offset) = serialize(value.clone(), &());
    assert_eq!(actual, expected);
    assert!(handles.is_empty());
    assert_eq!(offset, 0);
    assert_eq!(
        deserialize_exact::<(), Envelope>(&actual, &mut [], 0, &()).unwrap(),
        value
    );
}

#[test]
fn empty_and_absent_complex_values_keep_the_public_conversion_and_wire() {
    let mut value = fixture();
    value.rows.clear();
    value.optional = None;
    value.map.clear();
    value.choices = vec![None];
    let public_ast = value.clone().into_mojom_value(&());
    assert_eq!(check_names(&public_ast, true), 3);
    let wire_ast = value.clone().into_mojom_value_for_wire(&());
    assert_eq!(check_names(&wire_ast, false), 3);
    let (expected, _, _) =
        deparse_top_level_value(public_ast, <Envelope as MojomParse>::wire_type()).unwrap();
    let (actual, _, _) = serialize(value.clone(), &());
    assert_eq!(actual, expected);
    assert_eq!(
        deserialize_exact::<(), Envelope>(&actual, &mut [], 0, &()).unwrap(),
        value
    );
}
