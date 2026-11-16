//! Actual pinned CLI qualification; run explicitly with guard and rclone paths.
use dockervault_service::{ownership::Owner, process::ProcessOwner, rclone::provider_options};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
use tokio_util::sync::CancellationToken;
#[tokio::main]
async fn main() {
    let args = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    assert_eq!(args.len(), 2);
    let processes = ProcessOwner::new(1, &args[0]).unwrap();
    let blocking = Owner::new(1).unwrap();
    let actual = provider_options(&processes, &blocking, &args[1], CancellationToken::new())
        .await
        .unwrap();
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../dockervault-core/tests/fixtures/rclone-packages.TEST_ONLY.json"
    ))
    .unwrap();
    let expected: BTreeMap<String, BTreeSet<String>> =
        serde_json::from_value(fixture["options"].clone()).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(processes.active(), 0);
    assert_eq!(blocking.active(), 0);
    println!(
        "rclone-capabilities: {} accepted-provider catalogs match actual offline CLI: PASS",
        actual.len()
    );
    blocking.shutdown(None).await;
    processes.shutdown(None).await;
}
