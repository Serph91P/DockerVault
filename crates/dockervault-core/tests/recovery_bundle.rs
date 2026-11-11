use dockervault_core::recovery_bundle::{BundleError, decode_bundle, encode_bundle};
use serde_json::{Value, json};
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/recovery-v1.TEST_ONLY.json")).unwrap()
}

#[test]
fn accepted_python_provider_vectors_authenticate_before_rejecting_invalid_payloads() {
    let _serial = SERIAL.lock().unwrap();
    let data = fixture();
    let password = data["public_test_password"].as_str().unwrap();
    let headers: Value =
        serde_json::from_str(include_str!("fixtures/recovery-headers.TEST_ONLY.json")).unwrap();
    for case in data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .chain(headers["cases"].as_array().unwrap())
    {
        let result = decode_bundle(case["bundle"].as_str().unwrap().as_bytes(), password);
        if case["valid"] == true {
            assert_eq!(result.unwrap(), data["payload"]);
        } else {
            assert_eq!(result.unwrap_err(), BundleError::Invalid);
        }
    }
    assert_eq!(
        decode_bundle(
            data["cases"][0]["bundle"].as_str().unwrap().as_bytes(),
            "wrong public test value"
        )
        .unwrap_err(),
        BundleError::Invalid
    );
}

#[test]
fn native_envelopes_use_the_published_crypto_header_and_fresh_salt_and_nonce() {
    let _serial = SERIAL.lock().unwrap();
    let password = "PUBLIC_TEST_RECOVERY_PASSWORD_NOT_A_CREDENTIAL";
    let payload = json!({"unicode":"Grüße🎉","float":0.1,"integer":9007199254740993_u64,"nested":{"false":false,"null":null,"text":"safe, : data"},"rows":[]});
    let first = encode_bundle(payload.clone(), password).unwrap();
    let second = encode_bundle(payload.clone(), password).unwrap();
    assert_eq!(decode_bundle(&first, password).unwrap(), payload);
    let a: Value = serde_json::from_slice(&first).unwrap();
    let b: Value = serde_json::from_slice(&second).unwrap();
    assert_eq!(a["format"], "dockervault-recovery");
    assert_eq!(a["envelope_version"], 1);
    assert_eq!(a["kdf"]["name"], "argon2id");
    assert_eq!(a["kdf"]["memory_kib"], 65536);
    assert_eq!(a["kdf"]["iterations"], 3);
    assert_eq!(a["kdf"]["parallelism"], 1);
    assert_eq!(a["aead"]["name"], "aes-256-gcm");
    assert_eq!(
        openssl::base64::decode_block(a["kdf"]["salt_b64"].as_str().unwrap())
            .unwrap()
            .len(),
        16
    );
    assert_eq!(
        openssl::base64::decode_block(a["aead"]["nonce_b64"].as_str().unwrap())
            .unwrap()
            .len(),
        12
    );
    assert_eq!(
        openssl::base64::decode_block(a["aead"]["tag_b64"].as_str().unwrap())
            .unwrap()
            .len(),
        16
    );
    assert_ne!(a["kdf"]["salt_b64"], b["kdf"]["salt_b64"]);
    assert_ne!(a["aead"]["nonce_b64"], b["aead"]["nonce_b64"]);
    assert_eq!(
        encode_bundle(json!({}), "").unwrap_err(),
        BundleError::Invalid
    );
    assert_eq!(
        encode_bundle(json!({}), &"é".repeat(513)).unwrap_err(),
        BundleError::Invalid
    );
    assert_eq!(
        encode_bundle(json!([]), password).unwrap_err(),
        BundleError::Invalid
    );
}

#[test]
fn authenticated_ciphertext_over_the_published_payload_limit_is_rejected() {
    let _serial = SERIAL.lock().unwrap();
    let data = fixture();
    let mut header: Value =
        serde_json::from_str(data["cases"][0]["bundle"].as_str().unwrap()).unwrap();
    header.as_object_mut().unwrap().remove("ciphertext_b64");
    header["aead"].as_object_mut().unwrap().remove("tag_b64");
    let mut nonce = [0; 12];
    openssl::rand::rand_bytes(&mut nonce).unwrap();
    header["aead"]["nonce_b64"] = json!(openssl::base64::encode_block(&nonce));
    let salt = openssl::base64::decode_block(header["kdf"]["salt_b64"].as_str().unwrap()).unwrap();
    let params = argon2::Params::new(65536, 3, 1, Some(32)).unwrap();
    let mut key = [0; 32];
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
        .hash_password_into(
            data["public_test_password"].as_str().unwrap().as_bytes(),
            &salt,
            &mut key,
        )
        .unwrap();
    header.sort_all_objects();
    let aad = serde_json::to_vec(&header).unwrap();
    let mut plaintext = b"{}".to_vec();
    plaintext.resize(24 * 1024 * 1024 - 1023, b' ');
    let mut tag = [0; 16];
    let ciphertext = openssl::symm::encrypt_aead(
        openssl::symm::Cipher::aes_256_gcm(),
        &key,
        Some(&nonce),
        &aad,
        &plaintext,
        &mut tag,
    )
    .unwrap();
    header["aead"]["tag_b64"] = json!(openssl::base64::encode_block(&tag));
    header["ciphertext_b64"] = json!(openssl::base64::encode_block(&ciphertext));
    let wire = serde_json::to_vec(&header).unwrap();
    assert!(wire.len() < 32 * 1024 * 1024);
    assert_eq!(
        decode_bundle(&wire, data["public_test_password"].as_str().unwrap()).unwrap_err(),
        BundleError::Invalid
    );
    assert_eq!(
        encode_bundle(
            json!({"large":"A".repeat(24*1024*1024)}),
            "PUBLIC_TEST_RECOVERY_PASSWORD_NOT_A_CREDENTIAL"
        )
        .unwrap_err(),
        BundleError::Invalid
    );
}

#[test]
fn concurrent_crypto_refuses_busy_work_instead_of_running_unbounded_kdfs() {
    let _serial = SERIAL.lock().unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            encode_bundle(
                json!({"public":"test"}),
                "PUBLIC_TEST_RECOVERY_PASSWORD_NOT_A_CREDENTIAL",
            )
        }));
    }
    let results: Vec<_> = workers
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert!(results.iter().any(Result::is_ok));
    assert!(
        results
            .iter()
            .any(|result| matches!(result, Err(BundleError::Busy)))
    );
    assert!(
        results
            .iter()
            .all(|result| matches!(result, Ok(_) | Err(BundleError::Busy)))
    );
}

#[test]
fn hostile_public_headers_are_rejected_without_honoring_attacker_kdf_parameters() {
    let data = fixture();
    let password = data["public_test_password"].as_str().unwrap();
    let original = data["cases"][0]["bundle"].as_str().unwrap();
    let envelope: Value = serde_json::from_str(original).unwrap();
    for (field, value) in [
        ("memory_kib", json!(1)),
        ("memory_kib", json!(4294967295_u64)),
        ("iterations", json!(1)),
        ("parallelism", json!(true)),
        ("iterations", json!(3.0)),
    ] {
        let mut mutated = envelope.clone();
        mutated["kdf"][field] = value;
        assert_eq!(
            decode_bundle(&serde_json::to_vec(&mutated).unwrap(), password).unwrap_err(),
            BundleError::Invalid
        );
    }
    let mut unknown = envelope.clone();
    unknown["kdf"]["extra"] = json!(0);
    assert_eq!(
        decode_bundle(&serde_json::to_vec(&unknown).unwrap(), password).unwrap_err(),
        BundleError::Invalid
    );
    let mut boolean = envelope.clone();
    boolean["envelope_version"] = json!(true);
    assert_eq!(
        decode_bundle(&serde_json::to_vec(&boolean).unwrap(), password).unwrap_err(),
        BundleError::Invalid
    );
    let mut noncanonical = envelope.clone();
    noncanonical["kdf"]["salt_b64"] = json!(
        envelope["kdf"]["salt_b64"]
            .as_str()
            .unwrap()
            .trim_end_matches('=')
    );
    assert_eq!(
        decode_bundle(&serde_json::to_vec(&noncanonical).unwrap(), password).unwrap_err(),
        BundleError::Invalid
    );
    let duplicate = original.replacen("{", "{\"format\":\"dockervault-recovery\",", 1);
    assert_eq!(
        decode_bundle(duplicate.as_bytes(), password).unwrap_err(),
        BundleError::Invalid
    );
    assert_eq!(
        decode_bundle(&vec![b' '; 32 * 1024 * 1024 + 1], password).unwrap_err(),
        BundleError::Invalid
    );
}
