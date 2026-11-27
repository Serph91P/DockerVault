use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).unwrap();
        let name: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let path = std::env::temp_dir().join(format!("dv-cleanup-plan-{name}"));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn changed_stage_is_rejected_before_any_key_unlink() {
    for originally_linked in [false, true] {
        let root = Fixture::new();
        let key = root.0.join("key");
        let generation = "00000000000000000000000000000000";
        let stage = root.0.join(format!(".dv-recovery-key-{generation}"));
        let bytes = b"PUBLIC synthetic owned bytes";
        fs::write(&key, bytes).unwrap();
        if originally_linked {
            fs::hard_link(&key, &stage).unwrap();
        }
        let info = fs::metadata(&key).unwrap();
        let digest: String = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let record =
            serde_json::json!({"key_inode":info.ino(),"key_device":info.dev(),"key_digest":digest});
        let plan = plan_owned_key(&key, generation, &record).unwrap();
        if originally_linked {
            fs::remove_file(&stage).unwrap();
        }
        fs::write(&stage, b"PUBLIC foreign substituted stage").unwrap();
        assert_eq!(plan.remove(), Err(PreparationError::UnsafeKeyPreparation));
        assert_eq!(fs::read(&key).unwrap(), bytes);
        assert_eq!(fs::metadata(&key).unwrap().ino(), info.ino());
        assert_eq!(
            fs::read(&stage).unwrap(),
            b"PUBLIC foreign substituted stage"
        );
        fs::remove_file(&stage).unwrap();
        plan_owned_key(&key, generation, &record)
            .unwrap()
            .remove()
            .unwrap();
        assert!(!key.exists());
    }
}

#[test]
fn same_inode_byte_mutation_is_rejected_before_owned_cleanup() {
    let root = Fixture::new();
    let key = root.0.join("key");
    fs::write(&key, b"PUBLIC owned").unwrap();
    let info = fs::metadata(&key).unwrap();
    let digest: String = Sha256::digest(b"PUBLIC owned")
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let record =
        serde_json::json!({"key_inode":info.ino(),"key_device":info.dev(),"key_digest":digest});
    let plan = plan_owned_key(&key, "00000000000000000000000000000000", &record).unwrap();
    fs::write(&key, b"PUBLIC foreign overwritten content").unwrap();
    assert_eq!(fs::metadata(&key).unwrap().ino(), info.ino());
    assert_eq!(plan.remove(), Err(PreparationError::UnsafeKeyPreparation));
    assert_eq!(
        fs::read(&key).unwrap(),
        b"PUBLIC foreign overwritten content"
    );
}
