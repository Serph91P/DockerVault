use dockervault_core::filters::Filters;
use serde_json::Value;

fn strings(case: &Value, name: &str) -> Vec<String> {
    case[name]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn file_filters_match_every_frozen_engine_and_worker_case() {
    let oracle: Value = serde_json::from_str(include_str!("contracts/filter-oracle.json")).unwrap();
    for case in oracle["file_cases"].as_array().unwrap() {
        let filters = Filters::new(&strings(case, "include"), &strings(case, "exclude"));
        assert_eq!(
            filters.includes(case["path"].as_str().unwrap()),
            case["expected"].as_bool().unwrap(),
            "{case}"
        );
    }
}

#[test]
fn parent_traversal_matches_every_frozen_case_without_expanding_wildcards() {
    let oracle: Value = serde_json::from_str(include_str!("contracts/filter-oracle.json")).unwrap();
    for case in oracle["parent_cases"].as_array().unwrap() {
        let filters = Filters::new(&strings(case, "include"), &[]);
        assert_eq!(
            filters.is_parent_of_included(case["path"].as_str().unwrap()),
            case["expected"].as_bool().unwrap(),
            "{case}"
        );
    }
}

#[test]
fn character_classes_malformed_patterns_unicode_and_repeated_stars_match_python() {
    let oracle: Value =
        serde_json::from_str(include_str!("contracts/fnmatch-control.json")).unwrap();
    for case in oracle.as_array().unwrap() {
        let filters = Filters::new(&[case["pattern"].as_str().unwrap().to_owned()], &[]);
        assert_eq!(
            filters.includes(case["path"].as_str().unwrap()),
            case["expected"].as_bool().unwrap(),
            "{case}"
        );
    }
}

#[test]
fn range_removal_can_reveal_a_negated_empty_class() {
    let filters = Filters::new(&["[a--!]".into()], &[]);
    assert!(filters.includes("a"));
    assert!(filters.includes("\n"));
    assert!(filters.includes("!"));
    assert!(!filters.includes("aa"));
}

#[test]
fn excludes_win_and_parent_checks_do_not_admit_excluded_files() {
    let filters = Filters::new(&["data/keep.txt".into()], &["data/*".into()]);
    assert!(filters.is_parent_of_included("/data/"));
    assert!(!filters.includes("/data/keep.txt"));
    let filters = Filters::new(&["*.txt".into()], &[]);
    assert!(filters.includes("hidden/nested/.data.txt"));
    assert!(!filters.includes("hidden/nested/.data.TXT"));
}
