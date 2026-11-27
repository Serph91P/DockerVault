use serde_json::Value;
use std::{fs, path::Path};

#[test]
fn dockerfile_npm_version_matches_frontend_package_manager() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dockerfile = fs::read_to_string(root.join("Dockerfile")).unwrap();
    let package: Value =
        serde_json::from_slice(&fs::read(root.join("frontend/package.json")).unwrap()).unwrap();
    let pin = dockerfile
        .lines()
        .filter_map(|line| line.strip_prefix("RUN npm install --global npm@"))
        .filter_map(|line| line.split_whitespace().next())
        .find(|pin| {
            let parts: Vec<_> = pin.split('.').collect();
            parts.len() == 3
                && parts
                    .iter()
                    .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        })
        .expect("Dockerfile must install a literal npm@version");
    assert_eq!(package["packageManager"], format!("npm@{pin}"));
}
