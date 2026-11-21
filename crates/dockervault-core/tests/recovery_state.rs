use dockervault_core::{
    cron::TimeZone,
    recovery_state::{StateError, parse_state},
    timestamp::Timestamp,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
#[test]
fn observed_semantic_validation_cancellation_never_becomes_invalid_or_success() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/recovery-state.TEST_ONLY.json")).unwrap();
    let options: BTreeMap<String, BTreeSet<String>> =
        serde_json::from_value(fixture["options"].clone()).unwrap();
    let now = Timestamp::parse(fixture["now"].as_str().unwrap()).unwrap();
    let zone = TimeZone::utc();
    for name in ["baseline", "rclone-local"] {
        let payload = &fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap()["payload"];
        assert!(parse_state(payload.clone(), now, &zone, &options, &|| false).is_ok());
        for at in 1..=64 {
            let calls = std::cell::Cell::new(0);
            let raised = std::cell::Cell::new(false);
            let cancel = || {
                calls.set(calls.get() + 1);
                let interrupt = calls.get() == at;
                if interrupt {
                    raised.set(true);
                }
                interrupt
            };
            let result = parse_state(payload.clone(), now, &zone, &options, &cancel);
            if raised.get() {
                assert_eq!(result.err(), Some(StateError::Cancelled), "{name}/{at}");
            } else {
                assert!(result.is_ok());
            }
        }
    }
}
