use dockervault_core::portable_records::PortableTables;
use dockervault_service::{
    ownership::Owner,
    recovery_records::{RecordValidationError, check_record_constraints},
};
use serde_json::{Value, json};
fn rows() -> Value {
    serde_json::from_str::<Value>(include_str!(
        "../../dockervault-core/tests/fixtures/portable-rows.TEST_ONLY.json"
    ))
    .unwrap()["tables"]
        .clone()
}
#[tokio::test]
async fn actual_native_memory_stage_accepts_the_provider_records_and_rejects_orphans_and_unique_conflicts()
 {
    let owner = Owner::new(1).unwrap();
    assert!(
        check_record_constraints(
            PortableTables::parse(rows()).unwrap_or_else(|_| panic!("fixture")),
            &owner
        )
        .await
        .is_ok()
    );
    let mut orphan = rows();
    orphan["backups"][0]["target_id"] = json!(99);
    assert_eq!(
        check_record_constraints(
            PortableTables::parse(orphan).unwrap_or_else(|_| panic!("typed orphan")),
            &owner
        )
        .await,
        Err(RecordValidationError::Invalid)
    );
    let mut duplicate = rows();
    let mut second = duplicate["retention_policies"][0].clone();
    second["id"] = json!(2);
    duplicate["retention_policies"]
        .as_array_mut()
        .unwrap()
        .push(second);
    assert_eq!(
        check_record_constraints(
            PortableTables::parse(duplicate).unwrap_or_else(|_| panic!("typed duplicate")),
            &owner
        )
        .await,
        Err(RecordValidationError::Invalid)
    );
    owner.shutdown(None).await;
}
#[tokio::test]
async fn record_staging_refuses_closed_physical_owner() {
    let owner = Owner::new(1).unwrap();
    owner.shutdown(None).await;
    assert_eq!(
        check_record_constraints(
            PortableTables::parse(rows()).unwrap_or_else(|_| panic!("fixture")),
            &owner
        )
        .await,
        Err(RecordValidationError::Owner)
    );
}
