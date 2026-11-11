use dockervault_service::{
    config::Config,
    hooks::{HookError, Hooks, parse_argv},
};
use serde_json::Value;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
fn config(allowed: &str, timeout: &str) -> Config {
    Config::from_pairs([
        ("ALLOWED_HOOK_COMMANDS", allowed),
        ("HOOK_COMMAND_TIMEOUT", timeout),
    ])
    .unwrap()
}
#[test]
fn argv_matches_every_independent_python_shlex_control_without_posix_reinterpretation() {
    let controls: Value = serde_json::from_str(include_str!(
        "../../../docs/contracts/python-control-18d8c3d/hook-argv.json"
    ))
    .unwrap();
    assert_eq!(
        controls["vectors"].as_array().unwrap().len(),
        controls["count"].as_u64().unwrap() as usize
    );
    for vector in controls["vectors"].as_array().unwrap() {
        let parsed = parse_argv(vector["command"].as_str().unwrap());
        if vector["invalid"].as_bool() == Some(true) {
            assert!(parsed.is_err());
        } else {
            assert_eq!(
                serde_json::to_value(parsed.unwrap()).unwrap(),
                vector["argv"]
            );
        }
    }
}
#[tokio::test]
async fn hooks_preserve_exact_allowlisting_without_implicit_shell_or_argument_logging() {
    let hooks = Hooks::new(
        &config("printf", "5"),
        env!("CARGO_BIN_EXE_dockervault-process-guard"),
    )
    .unwrap();
    assert!(matches!(
        hooks.run(" ", CancellationToken::new()).await,
        Err(HookError::Empty)
    ));
    assert!(matches!(
        hooks
            .run("/usr/bin/printf public", CancellationToken::new())
            .await,
        Err(HookError::Denied)
    ));
    assert!(matches!(
        hooks
            .run("printf 'unterminated", CancellationToken::new())
            .await,
        Err(HookError::Syntax)
    ));
    let marker = std::env::temp_dir().join(format!("dv-hook-no-shell-{}", std::process::id()));
    assert!(!marker.exists());
    hooks
        .run(
            &format!("printf 'PUBLIC; $(touch {})'", marker.display()),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!marker.exists());
    assert_eq!(hooks.active(), 0);
    hooks.shutdown(None).await;
}
#[tokio::test]
async fn ignored_hook_output_is_drained_not_captured_or_mistaken_for_a_failed_hook() {
    let hooks = Hooks::new(
        &config("head", "5"),
        env!("CARGO_BIN_EXE_dockervault-process-guard"),
    )
    .unwrap();
    hooks
        .run("head -c 262144 /dev/zero", CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(hooks.active(), 0);
    hooks.shutdown(None).await;
}
#[tokio::test]
async fn timeout_zero_retains_python_immediate_timeout_and_physically_settles_the_child() {
    let hooks = Hooks::new(
        &config("sleep", "0"),
        env!("CARGO_BIN_EXE_dockervault-process-guard"),
    )
    .unwrap();
    assert!(
        hooks
            .run("sleep 30", CancellationToken::new())
            .await
            .is_err()
    );
    assert_eq!(hooks.active(), 0);
    hooks.shutdown(None).await;
}
#[tokio::test]
async fn cancellation_waits_for_process_settlement_instead_of_dropping_the_wait_future() {
    let hooks = Hooks::new(
        &config("sleep", "30"),
        env!("CARGO_BIN_EXE_dockervault-process-guard"),
    )
    .unwrap();
    let cancel = CancellationToken::new();
    let stopping = cancel.clone();
    let cancelled = async {
        while hooks.active() == 0 {
            tokio::task::yield_now().await;
        }
        stopping.cancel();
    };
    let (result, _) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(hooks.run("sleep 30", cancel), cancelled)
    })
    .await
    .unwrap();
    assert!(result.is_err());
    assert_eq!(hooks.active(), 0);
    hooks.shutdown(None).await;
}
