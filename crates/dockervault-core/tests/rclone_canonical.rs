use dockervault_core::{
    lossless_json::PythonJson, portable_records::RemoteStorage, rclone_package,
};
use openssl::sha::sha256;
use serde_json::Value;

#[test]
fn canonical_live_package_matches_actual_source_bytes_for_every_accepted_import_control() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/rclone-packages.TEST_ONLY.json")).unwrap();
    let controls: Value =
        serde_json::from_str(include_str!("fixtures/canonical-packages.TEST_ONLY.json")).unwrap();
    let options = serde_json::from_value(fixture["options"].clone()).unwrap();
    let mut count = 0;
    for control in controls["cases"].as_array().unwrap() {
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == control["name"])
            .unwrap();
        assert_eq!(case["accepted"], true);
        let payload = &case["payload"];
        let storages: Vec<RemoteStorage> = control["indices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                serde_json::from_value(case["storages"][i.as_u64().unwrap() as usize].clone())
                    .unwrap()
            })
            .collect();
        let actual = rclone_package::assemble_canonical(
            &payload["remotes"],
            &payload["files"],
            &storages,
            &payload["roots"],
            &options,
            &|| false,
        )
        .unwrap();
        let canonical = PythonJson::parse(&serde_json::to_vec(&actual).unwrap())
            .unwrap()
            .canonical(&|| false)
            .unwrap();
        let hash: String = sha256(&canonical)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hash,
            control["sha256"].as_str().unwrap(),
            "{}",
            control["name"]
        );
        rclone_package::validate(&actual, &storages, &options, &|| false).unwrap();
        let references: Vec<String> = actual["files"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            rclone_package::collect_materials(&actual["files"], &references, &|| false).unwrap(),
            actual["files"]
        );
        assert!(matches!(
            rclone_package::collect_materials(&actual["files"], &references, &|| true),
            Err(rclone_package::RcloneError::Cancelled)
        ));
        assert!(matches!(
            rclone_package::assemble_canonical(
                &payload["remotes"],
                &payload["files"],
                &storages,
                &payload["roots"],
                &options,
                &|| true
            ),
            Err(rclone_package::RcloneError::Cancelled)
        ));
        count += 1;
    }
    assert_eq!(count, controls["cases"].as_array().unwrap().len());
    assert_eq!(
        controls["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["accepted"] == true)
            .count()
    );
}
