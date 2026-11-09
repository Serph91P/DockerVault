use dockervault_tools::archive_worker::{MAX_REQUEST_BYTES, Request};
use serde_json::json;

fn request(extra: serde_json::Value) -> serde_json::Value {
    let mut value = json!({"version":1,"dest":"/backups/test.tar","sources":[["root","/var/lib/docker/volumes/test/_data"]]});
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    value
}

#[test]
fn versioned_request_has_published_compression_and_filter_defaults() {
    let value = Request::parse(&serde_json::to_vec(&request(json!({}))).unwrap()).unwrap();
    assert!(value.compress);
    assert_eq!(value.sources[0].0, "root");
    assert!(value.filters_for("root").unwrap().includes("any/file"));
}

#[test]
fn oversize_unsupported_missing_version_and_unknown_control_fields_are_rejected() {
    assert!(Request::parse(&vec![b' '; MAX_REQUEST_BYTES + 1]).is_err());
    for extra in [
        json!({"version":2}),
        json!({"version":null}),
        json!({"shell":"/bin/sh"}),
    ] {
        assert!(Request::parse(&serde_json::to_vec(&request(extra)).unwrap()).is_err());
    }
    let mut value = request(json!({}));
    value.as_object_mut().unwrap().remove("version");
    assert!(Request::parse(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn lexical_path_boundary_checks_happen_before_any_filesystem_effect() {
    for dest in [
        "/backups2/file",
        "/backups/../escape",
        "/tmp/file",
        "relative",
        "/backups",
        "/backups/inner/../../escape",
    ] {
        assert!(
            Request::parse(&serde_json::to_vec(&request(json!({"dest":dest}))).unwrap()).is_err(),
            "{dest}"
        );
    }
    for name in ["../escape", "root/../../escape"] {
        assert!(
            Request::parse(
                &serde_json::to_vec(&request(
                    json!({"sources":[[name,"/var/lib/docker/volumes/test/_data"]]})
                ))
                .unwrap()
            )
            .is_err()
        );
    }
}

#[test]
fn full_volume_key_precedes_basename_and_empty_rule_values_fall_back_like_python() {
    let value = Request::parse(&serde_json::to_vec(&request(json!({
        "include_paths":["global"],
        "per_volume_rules":{"stack/data":{"include_paths":["full"]},"data":{"include_paths":["base"]}}
    }))).unwrap()).unwrap();
    assert!(value.filters_for("/stack/data").unwrap().includes("full"));
    assert!(!value.filters_for("stack/data").unwrap().includes("base"));
    assert!(value.filters_for("other/data").unwrap().includes("base"));
    assert!(value.filters_for("other").unwrap().includes("global"));
    for empty in [
        json!({}),
        json!(null),
        json!(false),
        json!(0),
        json!(""),
        json!([]),
    ] {
        let value = Request::parse(&serde_json::to_vec(&request(json!({"include_paths":["global"],"per_volume_rules":{"stack/data":empty,"data":{"include_paths":["base"]}}}))).unwrap()).unwrap();
        assert!(value.filters_for("stack/data").unwrap().includes("base"));
    }
}

#[test]
fn explicit_empty_filter_lists_override_global_rules_in_nonempty_rule_object() {
    let value = Request::parse(&serde_json::to_vec(&request(json!({"include_paths":["global"],"per_volume_rules":{"data":{"include_paths":[],"exclude_paths":[]}}}))).unwrap()).unwrap();
    assert!(value.filters_for("data").unwrap().includes("not-global"));
}
