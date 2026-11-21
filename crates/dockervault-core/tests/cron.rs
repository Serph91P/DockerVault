use base64::{Engine, engine::general_purpose::STANDARD};
use dockervault_core::{
    cron::{Schedule, TimeZone},
    timestamp::Timestamp,
};
use serde_json::Value;
use std::collections::BTreeMap;
fn source() -> Value {
    serde_json::from_str(include_str!("fixtures/cron-controls.TEST_ONLY.json")).unwrap()
}
#[test]
fn cron_preview_matches_the_actual_apscheduler_timezone_dst_contract() {
    let fixture = source();
    let zones: BTreeMap<String, TimeZone> = fixture["zones"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, z)| {
            let data = STANDARD.decode(z["data_base64"].as_str().unwrap()).unwrap();
            let sha = openssl::sha::sha256(&data)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            assert_eq!(sha, z["sha256"]);
            (name.clone(), TimeZone::from_tz_data(&data).unwrap())
        })
        .collect();
    for case in fixture["cases"].as_array().unwrap() {
        let schedule = Schedule::parse(case["expression"].as_str().unwrap());
        let base = Timestamp::parse(case["base"].as_str().unwrap()).unwrap();
        let actual =
            schedule.and_then(|s| s.next(base, &zones[case["zone"].as_str().unwrap()], &|| false));
        if case["rejected"] == true {
            assert!(actual.is_err(), "{case}");
        } else {
            let fire = actual.unwrap_or_else(|_| panic!("source accepted {case}"));
            assert_eq!(
                fire.timestamp.api(),
                case["expected"].as_str().unwrap(),
                "{case}"
            );
            assert_eq!(
                u64::from(fire.fold),
                case["fold"].as_u64().unwrap(),
                "{case}"
            );
        }
    }
}
#[test]
fn impossible_calendar_search_observes_cancellation_instead_of_reporting_no_future() {
    use dockervault_core::cron::CronError;
    let schedule = Schedule::parse("0 0 31 feb *").unwrap();
    let zone = TimeZone::utc();
    let base = Timestamp::parse("2024-01-01T00:00:00+00:00").unwrap();
    assert_eq!(
        schedule.next(base, &zone, &|| true),
        Err(CronError::Cancelled)
    );
    let calls = std::cell::Cell::new(0);
    assert_eq!(
        schedule.next(base, &zone, &|| {
            calls.set(calls.get() + 1);
            calls.get() >= 5
        }),
        Err(CronError::Cancelled)
    );
    assert_eq!(
        schedule.next(base, &zone, &|| false),
        Err(CronError::NoFuture)
    );
}
#[test]
fn cron_syntax_matches_published_five_field_rules_instead_of_unix_extensions() {
    for case in source()["syntax"].as_array().unwrap() {
        assert_eq!(
            Schedule::parse(case["expression"].as_str().unwrap()).is_ok(),
            case["accepted"].as_bool().unwrap(),
            "{case}"
        );
    }
}
