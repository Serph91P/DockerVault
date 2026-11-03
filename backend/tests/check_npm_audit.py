"""Fail closed on npm audit errors and unreviewed high/critical advisories."""

from datetime import date
import json
from pathlib import Path
import subprocess
import sys


def signature(vulnerability, lock):
    via = [
        {key: entry[key] for key in ("name", "url", "range", "severity")}
        if isinstance(entry, dict)
        else entry
        for entry in vulnerability["via"]
    ]
    return {
        "severity": vulnerability["severity"],
        "range": vulnerability["range"],
        "via": sorted(via, key=lambda entry: json.dumps(entry, sort_keys=True)),
        "installed": {
            node: lock["packages"][node]["version"] for node in vulnerability["nodes"]
        },
    }


def evaluate(report, exceptions, lock, today=None):
    if report.get("error") or report.get("auditReportVersion") != 2:
        raise ValueError("npm audit failed or returned an unsupported report")
    today = today or date.today()
    failures = []
    for name, vulnerability in report["vulnerabilities"].items():
        if vulnerability["severity"] not in ("high", "critical"):
            continue
        entry = exceptions["exceptions"].get(name)
        if (
            not entry
            or date.fromisoformat(entry["expires"]) < today
            or not entry["reason"]
            or not entry["tracking_issue"]
            or entry["signature"] != signature(vulnerability, lock)
        ):
            failures.append(name)
    return failures


def main():
    root = Path(__file__).resolve().parents[2]
    frontend = root / "frontend"
    result = subprocess.run(
        ["npm", "audit", "--json"],
        cwd=frontend,
        text=True,
        capture_output=True,
        timeout=180,
    )
    # Keep the full unfiltered scan in CI artifacts, including known debt and lower severities.
    (frontend / "npm-audit-report.json").write_text(result.stdout)
    if result.returncode not in (0, 1):
        raise RuntimeError(
            f"npm audit execution failed ({result.returncode}): {result.stderr}"
        )
    report = json.loads(result.stdout)
    exceptions = json.loads((root / "docs/npm-audit-exceptions.json").read_text())
    lock = json.loads((frontend / "package-lock.json").read_text())
    failures = evaluate(report, exceptions, lock)
    print(
        json.dumps(
            {
                "scan_totals": report["metadata"]["vulnerabilities"],
                "unreviewed_high_critical": failures,
            }
        )
    )
    if failures:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
