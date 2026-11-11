use dockervault_service::config::Config;
use std::time::Duration;

#[test]
fn integer_environment_coercion_matches_executed_python_controls() {
    for (value, expected) in [
        ("1.0", 1),
        ("1.00", 1),
        ("1_000", 1000),
        (" +3 ", 3),
        ("-0.0", 0),
        ("01", 1),
    ] {
        assert_eq!(
            Config::from_pairs([("DEFAULT_KEEP_LAST", value)])
                .unwrap()
                .default_keep_last,
            expected
        );
    }
    for value in [
        "1e2", "1.5", ".0", "1.", "1_0.0_0", "1__0", "_1", "1_", "1_.0", "1._0", "0x10", "NaN",
        "inf",
    ] {
        assert!(
            Config::from_pairs([("DEFAULT_KEEP_LAST", value)]).is_err(),
            "{value}"
        );
    }
}

#[test]
fn dotenv_does_not_override_explicit_environment_or_write_process_environment() {
    let path = std::env::temp_dir().join(format!("dv-config-{}.env", std::process::id()));
    std::fs::write(&path, "COOKIE_SECURE=false\nSHUTDOWN_TIMEOUT=23\n").unwrap();
    let before = std::env::var_os("SHUTDOWN_TIMEOUT");
    let config = Config::from_sources(&path, [("COOKIE_SECURE", "true")]).unwrap();
    assert_eq!(std::env::var_os("SHUTDOWN_TIMEOUT"), before);
    assert!(config.cookie_secure);
    assert_eq!(config.shutdown_limit(), Some(Duration::from_secs(23)));
    std::fs::write(&path, "UNKNOWN_PUBLIC_TEST_FLAG=1\n").unwrap();
    assert!(Config::from_sources(&path, std::iter::empty::<(&str, &str)>()).is_err());
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn zero_timeouts_do_not_turn_into_immediate_shutdown_or_worker_expiry() {
    let config = Config::from_pairs([
        ("SHUTDOWN_TIMEOUT", "0"),
        ("TAR_WORKER_TIMEOUT", "0"),
        ("TAR_WORKER_IDLE_TIMEOUT", "0"),
    ])
    .unwrap();
    assert_eq!(config.shutdown_limit(), None);
    assert_eq!(config.tar_worker_limit(), None);
    assert_eq!(config.tar_worker_idle_limit(), None);
    let config = Config::from_pairs([
        ("SHUTDOWN_TIMEOUT", "7"),
        ("TAR_WORKER_TIMEOUT", "9"),
        ("TAR_WORKER_IDLE_TIMEOUT", "11"),
    ])
    .unwrap();
    assert_eq!(config.shutdown_limit(), Some(Duration::from_secs(7)));
    assert_eq!(config.tar_worker_limit(), Some(Duration::from_secs(9)));
    assert_eq!(
        config.tar_worker_idle_limit(),
        Some(Duration::from_secs(11))
    );
}

#[test]
fn explicit_empty_false_and_zero_are_not_replaced_by_truthy_defaults() {
    let config = Config::from_pairs([
        ("COOKIE_SECURE", "false"),
        ("STORAGE_VERIFY_AFTER_UPLOAD", "0"),
        ("DEFAULT_KEEP_LAST", "0"),
        ("KOMODO_API_URL", ""),
        ("CORS_ORIGINS", ""),
        ("ALLOWED_HOOK_COMMANDS", ""),
    ])
    .unwrap();
    assert!(!config.cookie_secure);
    assert!(!config.storage_verify_after_upload);
    assert_eq!(config.default_keep_last, 0);
    assert_eq!(config.komodo_api_url, "");
    assert_eq!(config.cors_origins, "");
    assert_eq!(config.allowed_hook_commands, "");
}

#[test]
fn defaults_keep_the_published_database_mount_and_security_contract() {
    let config = Config::from_pairs(std::iter::empty::<(&str, &str)>()).unwrap();
    assert_eq!(config.database_url, "sqlite+aiosqlite:///./data/backups.db");
    assert_eq!(config.backup_base_path, "/backups");
    assert_eq!(config.docker_socket, "/var/run/docker.sock");
    assert!(config.cookie_secure);
    assert!(config.storage_verify_after_upload);
    assert_eq!(config.storage_upload_chunk_size, 1_048_576);
    assert_eq!(config.s3_multipart_threshold_bytes, 67_108_864);
    assert_eq!(config.s3_multipart_chunk_size, 16_777_216);
    assert_eq!(
        config.tar_worker_idle_limit(),
        Some(Duration::from_secs(1800))
    );
    assert_eq!(config.tz, "Europe/Berlin");
}

#[test]
fn invalid_typed_values_fail_closed_without_echoing_the_value() {
    let secret = "private-invalid-bool-marker";
    let error = Config::from_pairs([("COOKIE_SECURE", secret)]).unwrap_err();
    assert!(error.to_string().contains("COOKIE_SECURE"));
    assert!(!error.to_string().contains(secret));
    assert!(Config::from_pairs([("SHUTDOWN_TIMEOUT", "not-an-integer")]).is_err());
    let signed = Config::from_pairs([
        ("SHUTDOWN_TIMEOUT", "-1"),
        ("DEFAULT_KEEP_LAST", "-1"),
        ("STORAGE_UPLOAD_CHUNK_SIZE", "0"),
    ])
    .unwrap();
    assert_eq!(signed.shutdown_limit(), Some(Duration::ZERO));
    assert_eq!(signed.default_keep_last, -1);
    assert_eq!(signed.storage_upload_chunk_size, 0);
}

#[test]
fn secret_configuration_never_appears_in_debug_output() {
    let config = Config::from_pairs([
        ("CREDENTIAL_ENCRYPTION_KEY", "private-test-fernet-material"),
        ("KOMODO_API_KEY", "private-test-api-key"),
        ("KOMODO_API_SECRET", "private-test-api-secret"),
        (
            "KOMODO_API_URL",
            "https://private-test-user:private-test-pass@example.invalid",
        ),
    ])
    .unwrap();
    let debug = format!("{config:?}");
    for value in [
        "private-test-fernet-material",
        "private-test-api-key",
        "private-test-api-secret",
        "private-test-pass",
    ] {
        assert!(!debug.contains(value));
    }
    assert_eq!(config.komodo_api_key.expose(), "private-test-api-key");
}

#[test]
fn published_boolean_spellings_and_case_sensitive_names_are_preserved() {
    for value in ["true", "TRUE", "1", "on", "yes"] {
        assert!(
            Config::from_pairs([("COOKIE_SECURE", value)])
                .unwrap()
                .cookie_secure
        );
    }
    for value in ["false", "FALSE", "0", "off", "no"] {
        assert!(
            !Config::from_pairs([("COOKIE_SECURE", value)])
                .unwrap()
                .cookie_secure
        );
    }
    assert!(
        Config::from_pairs([("cookie_secure", "false")])
            .unwrap()
            .cookie_secure
    );
}
