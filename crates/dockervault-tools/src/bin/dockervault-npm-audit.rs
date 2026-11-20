use serde_json::{Map, Value, json};
use std::{
    error::Error,
    fs,
    path::Path,
    process::{Command, ExitCode},
};
use time::{Date, UtcDateTime, format_description::well_known::Iso8601};

type Check<T> = Result<T, Box<dyn Error>>;

fn signature(vulnerability: &Value, lock: &Value) -> Check<Value> {
    let mut via = Vec::new();
    for entry in vulnerability["via"]
        .as_array()
        .ok_or("Missing advisory list")?
    {
        if let Some(entry) = entry.as_object() {
            let mut fields = Map::new();
            for key in ["name", "range", "severity", "url"] {
                fields.insert(
                    key.into(),
                    Value::String(
                        entry
                            .get(key)
                            .and_then(Value::as_str)
                            .ok_or("Missing advisory field")?
                            .into(),
                    ),
                );
            }
            via.push(Value::Object(fields));
        } else if entry.is_string() {
            via.push(entry.clone());
        } else {
            return Err("Invalid advisory entry".into());
        }
    }
    via.sort_by_cached_key(dockervault_core::encoding::python_json);
    let mut installed = Map::new();
    for node in vulnerability["nodes"]
        .as_array()
        .ok_or("Missing installed nodes")?
    {
        let node = node.as_str().ok_or("Invalid installed node")?;
        let version = lock["packages"][node]["version"]
            .as_str()
            .ok_or("Missing installed version")?;
        installed.insert(node.into(), Value::String(version.into()));
    }
    let severity = vulnerability["severity"]
        .as_str()
        .ok_or("Invalid severity")?;
    let range = vulnerability["range"]
        .as_str()
        .ok_or("Missing vulnerability range")?;
    Ok(json!({"severity":severity,"range":range,"via":via,"installed":installed}))
}

fn evaluate(report: &Value, exceptions: &Value, lock: &Value, today: &str) -> Check<Vec<String>> {
    if report["auditReportVersion"].as_u64() != Some(2) || !report["error"].is_null() {
        return Err("npm audit failed or returned an unsupported report".into());
    }
    let today = Date::parse(today, &Iso8601::DATE)?;
    let findings = report["vulnerabilities"]
        .as_object()
        .ok_or("Missing vulnerabilities")?;
    let exceptions = exceptions["exceptions"]
        .as_object()
        .ok_or("Missing exception map")?;
    let mut failures = Vec::new();
    for (name, vulnerability) in findings {
        match vulnerability["severity"].as_str() {
            Some("info" | "low" | "moderate") => continue,
            Some("high" | "critical") => (),
            _ => return Err("Invalid vulnerability severity".into()),
        }
        if let Some(entry) = exceptions.get(name) {
            let expiry = Date::parse(
                entry["expires"].as_str().ok_or("Missing expiry")?,
                &Iso8601::DATE,
            )?;
            if expiry >= today
                && ["reason", "tracking_issue"]
                    .iter()
                    .all(|&key| entry[key].as_str().is_some_and(|value| !value.is_empty()))
                && entry["signature"] == signature(vulnerability, lock)?
            {
                continue;
            }
        }
        failures.push(name.clone());
    }
    Ok(failures)
}

fn scan(root: &Path) -> Check<bool> {
    let frontend = root.join("frontend");
    let result = Command::new("timeout")
        .args(["--kill-after=1s", "180s", "npm", "audit", "--json"])
        .current_dir(&frontend)
        .output()?;
    fs::write(frontend.join("npm-audit-report.json"), &result.stdout)?;
    if !matches!(result.status.code(), Some(0 | 1)) {
        return Err(format!(
            "npm audit execution failed ({}): {}",
            result.status,
            String::from_utf8_lossy(&result.stderr)
        )
        .into());
    }
    let report: Value = serde_json::from_slice(&result.stdout)?;
    let exceptions =
        serde_json::from_slice(&fs::read(root.join("docs/npm-audit-exceptions.json"))?)?;
    let lock = serde_json::from_slice(&fs::read(frontend.join("package-lock.json"))?)?;
    let today = UtcDateTime::now().date().format(&Iso8601::DATE)?;
    let failures = evaluate(&report, &exceptions, &lock, &today)?;
    let totals = report["metadata"]["vulnerabilities"]
        .as_object()
        .ok_or("Missing audit totals")?;
    println!(
        "{}",
        json!({"scan_totals":totals,"unreviewed_high_critical":failures})
    );
    Ok(failures.is_empty())
}

fn main() -> ExitCode {
    match scan(Path::new(".")) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> (Value, Value, Value) {
        let via = json!([{"name":"fixture","url":"https://example.test/owned-advisory","range":"<2","severity":"high"}]);
        let report = json!({"auditReportVersion":2,"vulnerabilities":{"fixture":{"severity":"high","range":"<2","nodes":["node_modules/fixture"],"via":via}}});
        let exceptions = json!({"exceptions":{"fixture":{"expires":"2026-12-03","reason":"Owned existing debt fixture","tracking_issue":"fixture","signature":{"severity":"high","range":"<2","via":via,"installed":{"node_modules/fixture":"1.0.0"}}}}});
        let lock = json!({"packages":{"node_modules/fixture":{"version":"1.0.0"}}});
        (report, exceptions, lock)
    }

    #[test]
    fn exact_reviewed_debt_only() {
        let (report, exceptions, lock) = fixture();
        assert!(
            evaluate(&report, &exceptions, &lock, "2026-12-03")
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            evaluate(&report, &exceptions, &lock, "2026-12-04").unwrap(),
            ["fixture"]
        );
        for (key, replacement) in [
            ("url", "https://example.test/changed"),
            ("range", "<3"),
            ("severity", "critical"),
            ("version", "1.1.0"),
        ] {
            let (mut report, exceptions, mut lock) = fixture();
            match key {
                "url" => report["vulnerabilities"]["fixture"]["via"][0]["url"] = json!(replacement),
                "version" => {
                    lock["packages"]["node_modules/fixture"]["version"] = json!(replacement)
                }
                _ => report["vulnerabilities"]["fixture"][key] = json!(replacement),
            }
            assert_eq!(
                evaluate(&report, &exceptions, &lock, "2026-11-03").unwrap(),
                ["fixture"],
                "{key}"
            );
        }
    }

    #[test]
    fn error_and_unknown_findings_do_not_pass() {
        let (mut report, exceptions, lock) = fixture();
        report["vulnerabilities"]["unknown-high"] = json!({"severity":"high"});
        report["vulnerabilities"]["unknown-critical"] = json!({"severity":"critical"});
        assert_eq!(
            evaluate(&report, &exceptions, &lock, "2026-11-03").unwrap(),
            ["unknown-high", "unknown-critical"]
        );
        report["error"] = json!({"code":"ENETUNREACH"});
        assert!(evaluate(&report, &exceptions, &lock, "2026-11-03").is_err());
        assert!(
            evaluate(
                &json!({"auditReportVersion":2}),
                &exceptions,
                &lock,
                "2026-11-03"
            )
            .is_err()
        );
    }
}
