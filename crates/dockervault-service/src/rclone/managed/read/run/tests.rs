use super::*;
#[test]
fn explicit_ssh_authority_is_confined_and_retained_across_path_substitution() {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).unwrap();
    let id: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let root = std::env::temp_dir().join(format!("dv-ssh-runtime-{id}"));
    std::fs::create_dir(&root).unwrap();
    let allowed = root.join("allowed");
    std::fs::create_dir(&allowed).unwrap();
    let key = allowed.join("PUBLIC-key");
    std::fs::write(&key, b"PUBLIC explicit SSH bytes").unwrap();
    let outside = root.join("outside");
    std::fs::write(&outside, b"PUBLIC foreign").unwrap();
    assert!(key_file(&key, &[]).is_none());
    assert!(key_file(&outside, std::slice::from_ref(&allowed)).is_none());
    assert!(key_file(&allowed, std::slice::from_ref(&allowed)).is_none());
    let link = allowed.join("link");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    assert!(key_file(&link, std::slice::from_ref(&allowed)).is_none());
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&key, &link).unwrap();
    let held = key_file(&link, std::slice::from_ref(&allowed)).unwrap();
    std::fs::rename(&key, allowed.join("held-original")).unwrap();
    std::os::unix::fs::symlink(&outside, &key).unwrap();
    let path = format!("/proc/{}/fd/{}", std::process::id(), held.as_raw_fd());
    assert_eq!(std::fs::read(path).unwrap(), b"PUBLIC explicit SSH bytes");
    assert!(key_file(&key, std::slice::from_ref(&allowed)).is_none());
    assert_eq!(std::fs::read(&outside).unwrap(), b"PUBLIC foreign");
    drop(held);
    std::fs::remove_dir_all(root).unwrap();
}
