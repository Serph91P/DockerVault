use base64::{Engine, engine::general_purpose::STANDARD};
use dockervault_core::{lossless_json::PythonJson, portable_records::PortableTables};
use serde_json::{Value, value::RawValue};
use std::collections::BTreeMap;
#[test]
fn portable_opaque_json_retains_actual_source_sql_text_and_full_table_semantics() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/recovery-raw-state.TEST_ONLY.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let input = STANDARD
            .decode(case["payload_json_base64"].as_str().unwrap())
            .unwrap();
        let root: BTreeMap<String, &RawValue> = serde_json::from_slice(&input).unwrap();
        let records = PortableTables::parse_json(root["tables"].get().as_bytes())
            .unwrap_or_else(|_| panic!("{}", case["name"]));
        let field = match case["column"].as_str().unwrap() {
            "backup_metadata" => &records.backups[0].backup_metadata,
            "dependencies" => &records.backup_targets[0].dependencies,
            "selected_volumes" => &records.backup_targets[0].selected_volumes,
            "include_paths" => &records.backup_targets[0].include_paths,
            "exclude_paths" => &records.backup_targets[0].exclude_paths,
            "per_volume_rules" => &records.backup_targets[0].per_volume_rules,
            _ => panic!("fixture column"),
        };
        assert_eq!(
            field.python(&|| false).unwrap().as_bytes(),
            STANDARD
                .decode(case["sql_json_base64"].as_str().unwrap())
                .unwrap(),
            "{}",
            case["name"]
        );
        let serialized = serde_json::to_vec(&records).unwrap();
        let expected = STANDARD
            .decode(case["canonical_payload_base64"].as_str().unwrap())
            .unwrap();
        let expected_root: BTreeMap<String, &RawValue> = serde_json::from_slice(&expected).unwrap();
        assert_eq!(
            PythonJson::parse(&serialized)
                .unwrap()
                .canonical(&|| false)
                .unwrap(),
            expected_root["tables"].get().as_bytes(),
            "{}",
            case["name"]
        );
    }
}
