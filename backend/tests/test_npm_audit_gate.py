"""Policy controls, including an injected unknown high/critical advisory."""

from copy import deepcopy
from datetime import date

import pytest

from tests.check_npm_audit import evaluate, signature


@pytest.fixture
def audit_policy():
    vulnerability = {
        "severity": "high",
        "range": "<2.0.0",
        "nodes": ["node_modules/owned-fixture"],
        "via": [
            {
                "name": "owned-fixture",
                "url": "https://example.test/owned-advisory",
                "range": "<2.0.0",
                "severity": "high",
            }
        ],
    }
    lock = {"packages": {"node_modules/owned-fixture": {"version": "1.0.0"}}}
    report = {
        "auditReportVersion": 2,
        "vulnerabilities": {"owned-fixture": vulnerability},
    }
    exceptions = {
        "exceptions": {
            "owned-fixture": {
                "expires": "2026-12-03",
                "reason": "Owned existing-debt policy fixture, not a real advisory",
                "tracking_issue": "owned-fixture",
                "signature": signature(vulnerability, lock),
            }
        }
    }
    return report, exceptions, lock


def test_known_exact_debt_is_allowed(audit_policy):
    assert evaluate(*audit_policy, today=date(2026, 11, 3)) == []


@pytest.mark.parametrize("severity", ["high", "critical"])
def test_injected_unknown_high_or_critical_fails_closed(audit_policy, severity):
    report, exceptions, lock = audit_policy
    report["vulnerabilities"]["unknown-fixture"] = {"severity": severity}
    assert evaluate(report, exceptions, lock, date(2026, 11, 3)) == ["unknown-fixture"]


@pytest.mark.parametrize(
    "change", ["advisory", "range", "severity", "version", "expiry"]
)
def test_known_package_is_not_a_blanket_bypass(audit_policy, change):
    report, exceptions, lock = deepcopy(audit_policy)
    vulnerability = report["vulnerabilities"]["owned-fixture"]
    if change == "advisory":
        vulnerability["via"][0]["url"] = "https://example.test/new-unreviewed-advisory"
    elif change == "range":
        vulnerability["range"] = "<3.0.0"
    elif change == "severity":
        vulnerability["severity"] = "critical"
    elif change == "version":
        lock["packages"]["node_modules/owned-fixture"]["version"] = "1.0.1"
    else:
        exceptions["exceptions"]["owned-fixture"]["expires"] = "2026-11-01"
    assert evaluate(report, exceptions, lock, date(2026, 11, 3)) == ["owned-fixture"]


def test_audit_errors_are_not_security_success(audit_policy):
    report, exceptions, lock = audit_policy
    report["error"] = {
        "code": "ENETUNREACH",
        "summary": "owned negative network fixture",
    }
    with pytest.raises(ValueError, match="npm audit failed"):
        evaluate(report, exceptions, lock)
