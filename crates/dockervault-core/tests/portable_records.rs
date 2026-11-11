use dockervault_core::portable_records::PortableTables;
use serde_json::{Value, json};
fn rows() -> Value {
    serde_json::from_str::<Value>(include_str!("fixtures/portable-rows.TEST_ONLY.json")).unwrap()["tables"].clone()
}
#[test]
fn all_accepted_portable_column_types_roundtrip_without_losing_false_null_zero_or_json_integers() {
    let source = rows();
    let typed = PortableTables::parse(source.clone())
        .unwrap_or_else(|_| panic!("accepted records rejected"));
    assert_eq!(serde_json::to_value(typed).unwrap(), source);
}
#[test]
fn every_published_column_is_required_even_when_its_value_is_nullable() {
    let schema: Value =
        serde_json::from_str(include_str!("fixtures/portable-schema.TEST_ONLY.json")).unwrap();
    for (table, columns) in schema["tables"].as_object().unwrap() {
        for column in columns.as_array().unwrap() {
            let field = column["name"].as_str().unwrap();
            let mut data = rows();
            data[table][0].as_object_mut().unwrap().remove(field);
            assert!(
                PortableTables::parse(data).is_err(),
                "missing {table}.{field} accepted"
            );
        }
    }
}
#[test]
fn wrong_scalar_types_unknown_fields_and_nonpositive_primary_keys_are_rejected() {
    let schema: Value =
        serde_json::from_str(include_str!("fixtures/portable-schema.TEST_ONLY.json")).unwrap();
    for (table, columns) in schema["tables"].as_object().unwrap() {
        for column in columns.as_array().unwrap() {
            let field = column["name"].as_str().unwrap();
            let kind = column["type"].as_str().unwrap();
            if kind == "JSON" {
                continue;
            }
            let mut data = rows();
            data[table][0][field] = if kind == "Integer" {
                json!(false)
            } else {
                json!(17)
            };
            assert!(
                PortableTables::parse(data).is_err(),
                "wrong type {table}.{field} accepted"
            );
        }
        for id in [0, -1] {
            let mut data = rows();
            data[table][0]["id"] = json!(id);
            assert!(
                PortableTables::parse(data).is_err(),
                "nonpositive {table}.id"
            );
        }
        let mut data = rows();
        data[table][0]["UNPUBLISHED_FIELD"] = json!(true);
        assert!(PortableTables::parse(data).is_err());
    }
}
#[test]
fn nullable_columns_accept_null_but_missing_tables_and_excess_rows_do_not() {
    let schema: Value =
        serde_json::from_str(include_str!("fixtures/portable-schema.TEST_ONLY.json")).unwrap();
    let mut data = rows();
    for (table, columns) in schema["tables"].as_object().unwrap() {
        for column in columns.as_array().unwrap() {
            if column["nullable"] == true {
                data[table][0][column["name"].as_str().unwrap()] = Value::Null;
            }
        }
    }
    assert!(PortableTables::parse(data).is_ok());
    let mut data = rows();
    data.as_object_mut().unwrap().remove("backups");
    assert!(PortableTables::parse(data).is_err());
    let mut data = rows();
    let one = data["app_settings"][0].clone();
    data["app_settings"] = Value::Array(vec![one; 100001]);
    assert!(PortableTables::parse(data).is_err());
}
#[test]
fn source_character_lengths_are_not_utf8_byte_limits_and_plaintext_credentials_are_not_varchar_limited()
 {
    let schema: Value =
        serde_json::from_str(include_str!("fixtures/portable-schema.TEST_ONLY.json")).unwrap();
    for (table, columns) in schema["tables"].as_object().unwrap() {
        for column in columns.as_array().unwrap() {
            let Some(max) = column["length"].as_u64() else {
                continue;
            };
            let field = column["name"].as_str().unwrap();
            if column["type"] != "String"
                || (table == "remote_storages"
                    && ["password", "s3_access_key", "s3_secret_key"].contains(&field))
            {
                continue;
            }
            let mut data = rows();
            data[table][0][field] = json!("é".repeat(max as usize));
            assert!(
                PortableTables::parse(data).is_ok(),
                "character boundary {table}.{field}"
            );
            let mut data = rows();
            data[table][0][field] = json!("é".repeat(max as usize + 1));
            assert!(
                PortableTables::parse(data).is_err(),
                "overlength {table}.{field}"
            );
        }
    }
    let mut data = rows();
    data["remote_storages"][0]["password"] = json!("PUBLIC-".repeat(1000));
    assert!(PortableTables::parse(data).is_ok());
}
