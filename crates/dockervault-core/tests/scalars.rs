use dockervault_core::{
    encoding::python_json,
    model::{BackupStatus, BackupType, LogLevel},
    timestamp::Timestamp,
};
use serde_json::Value;

const GOLD: &str =
    include_str!("../../../docs/contracts/python-control-18d8c3d/scalar-values.json");

#[test]
fn backup_status_uses_lowercase_wire_values_but_exact_persisted_enum_names() {
    for (value, db, wire) in [
        (BackupStatus::Pending, "PENDING", "pending"),
        (BackupStatus::Running, "RUNNING", "running"),
        (BackupStatus::Completed, "COMPLETED", "completed"),
        (BackupStatus::Failed, "FAILED", "failed"),
        (BackupStatus::Cancelled, "CANCELLED", "cancelled"),
    ] {
        assert_eq!(value.db_name(), db);
        assert_eq!(BackupStatus::from_db(db).unwrap(), value);
        assert_eq!(serde_json::to_value(value).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<BackupStatus>(Value::String(wire.into())).unwrap(),
            value
        );
        assert!(BackupStatus::from_db(wire).is_err());
        assert!(serde_json::from_value::<BackupStatus>(Value::String(db.into())).is_err());
    }
    assert!(BackupStatus::from_db("UNKNOWN").is_err());
}
#[test]
fn backup_type_preserves_full_and_incremental_without_inventing_another_kind() {
    for (value, db, wire) in [
        (BackupType::Full, "FULL", "full"),
        (BackupType::Incremental, "INCREMENTAL", "incremental"),
    ] {
        assert_eq!(value.db_name(), db);
        assert_eq!(BackupType::from_db(db).unwrap(), value);
        assert_eq!(serde_json::to_value(value).unwrap(), wire);
        assert!(BackupType::from_db(wire).is_err());
    }
    assert!(BackupType::from_db("DIFFERENTIAL").is_err());
}
#[test]
fn log_levels_preserve_published_names_and_reject_silent_coercion() {
    for (value, db, wire) in [
        (LogLevel::Debug, "DEBUG", "debug"),
        (LogLevel::Info, "INFO", "info"),
        (LogLevel::Warning, "WARNING", "warning"),
        (LogLevel::Error, "ERROR", "error"),
    ] {
        assert_eq!(value.db_name(), db);
        assert_eq!(LogLevel::from_db(db).unwrap(), value);
        assert_eq!(serde_json::to_value(value).unwrap(), wire);
        assert!(LogLevel::from_db(wire).is_err());
    }
    assert!(LogLevel::from_db("WARN").is_err());
}
#[test]
fn json_bytes_match_independent_python_order_escaping_numbers_and_separators() {
    let controls: Value = serde_json::from_str(GOLD).unwrap();
    for case in controls["json"].as_array().unwrap() {
        assert_eq!(
            python_json(&case["input"]),
            case["output"].as_str().unwrap(),
            "{}",
            case["name"]
        );
    }
}
#[test]
fn floating_json_input_preserves_the_python_ieee754_bits() {
    let controls: Value = serde_json::from_str(GOLD).unwrap();
    for case in controls["float_parsing"].as_array().unwrap() {
        let value: f64 = serde_json::from_str(case["literal"].as_str().unwrap()).unwrap();
        let expected = u64::from_str_radix(case["bits"].as_str().unwrap(), 16).unwrap();
        assert_eq!(value.to_bits(), expected, "{}", case["literal"]);
    }
}
#[test]
fn legacy_float_atoms_match_the_current_python_quarantine_encoding() {
    let controls: Value = serde_json::from_str(GOLD).unwrap();
    for case in controls["float_atoms"].as_array().unwrap() {
        let value: f64 = case["input"].as_str().unwrap().parse().unwrap();
        assert_eq!(
            dockervault_core::encoding::python_float(value),
            case["output"].as_str().unwrap()
        );
    }
}
#[test]
fn sorted_identity_bytes_match_the_independent_frozen_storage_identity() {
    let gold: Value = serde_json::from_str(include_str!(
        "../../../docs/contracts/python-control-18d8c3d/storage-identity.json"
    ))
    .unwrap();
    let mut fields = gold["fields_as_python_strings"].clone();
    fields.as_object_mut().unwrap().sort_keys();
    let bytes = python_json(&fields);
    let encoded: String = bytes.bytes().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(encoded, gold["encoded_utf8_hex"].as_str().unwrap());
}
#[test]
fn sqlite_and_api_datetime_bytes_match_the_actual_sqlalchemy_control() {
    let controls: Value = serde_json::from_str(GOLD).unwrap();
    for case in controls["datetime"].as_array().unwrap() {
        let value = Timestamp::parse(case["input"].as_str().unwrap()).unwrap();
        assert_eq!(value.sql(), case["sqlite"].as_str().unwrap());
        assert_eq!(value.api(), case["api"].as_str().unwrap());
    }
}
#[test]
fn invalid_datetime_never_becomes_a_default_or_leaks_its_input() {
    for input in [
        "",
        "SECRET-invalid-date",
        "0000-01-01T00:00:00",
        "10000-01-01T00:00:00",
        "2024-02-30T00:00:00",
        "2024-01-01T23:59:60",
    ] {
        let error = Timestamp::parse(input).unwrap_err();
        assert!(!format!("{error:?} {error}").contains("SECRET"));
    }
}
