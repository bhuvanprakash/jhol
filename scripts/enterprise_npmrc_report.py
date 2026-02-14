#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from datetime import datetime, timezone
from pathlib import Path


def parse_npmrc(contents: str) -> dict:
    out = {
        "registry": None,
        "scopedRegistries": {},
        "hasAuthToken": False,
        "proxy": None,
        "httpsProxy": None,
        "strictSsl": None,
        "cafile": None,
    }
    for raw in contents.splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or line.startswith(";") or "=" not in line:
            continue
        key, value = [x.strip() for x in line.split("=", 1)]
        if key == "registry":
            out["registry"] = value.rstrip("/")
        elif key.endswith(":registry") and key.startswith("@"):
            out["scopedRegistries"][key[:-9]] = value.rstrip("/")
        elif key.endswith(":_authToken"):
            out["hasAuthToken"] = True
        elif key == "proxy":
            out["proxy"] = value
        elif key in ("https-proxy", "https_proxy"):
            out["httpsProxy"] = value
        elif key == "strict-ssl":
            out["strictSsl"] = value.lower() == "true"
        elif key == "cafile":
            out["cafile"] = value
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description="Enterprise .npmrc matrix report")
    parser.add_argument("--config", default="benchmarks/enterprise_guardrails.json")
    parser.add_argument("--out", default="enterprise-npmrc-report.json")
    args = parser.parse_args()

    guardrails = {"minPassRate": 1.0, "minScenarioCount": 4}
    cfg = Path(args.config)
    if cfg.exists():
        guardrails.update(json.loads(cfg.read_text()))

    scenarios = [
        {
            "name": "scoped_registry_and_token",
            "npmrc": "registry=https://registry.npmjs.org/\n@acme:registry=https://npm.acme.local/\n//npm.acme.local/:_authToken=abc\n",
            "expect": {"hasAuthToken": True, "scopedCount": 1},
        },
        {
            "name": "proxy_and_https_proxy",
            "npmrc": "proxy=http://proxy.local:8080\nhttps-proxy=http://proxy.local:8443\n",
            "expect": {"proxy": "http://proxy.local:8080", "httpsProxy": "http://proxy.local:8443"},
        },
        {
            "name": "strict_ssl_false",
            "npmrc": "strict-ssl=false\n",
            "expect": {"strictSsl": False},
        },
        {
            "name": "cafile_present",
            "npmrc": "cafile=/etc/ssl/certs/acme.pem\n",
            "expect": {"cafile": "/etc/ssl/certs/acme.pem"},
        },
    ]

    rows = []
    failures: list[str] = []
    for s in scenarios:
        parsed = parse_npmrc(s["npmrc"])
        expect = s["expect"]
        ok = True
        if "hasAuthToken" in expect and parsed["hasAuthToken"] != expect["hasAuthToken"]:
            ok = False
        if "scopedCount" in expect and len(parsed["scopedRegistries"]) != expect["scopedCount"]:
            ok = False
        for key in ("proxy", "httpsProxy", "strictSsl", "cafile"):
            if key in expect and parsed.get(key) != expect[key]:
                ok = False
        if not ok:
            failures.append(f"scenario {s['name']} failed")
        rows.append({"name": s["name"], "parsed": parsed, "expect": expect, "pass": ok})

    total = len(rows)
    passed = len([r for r in rows if r["pass"]])
    pass_rate = (passed / total) if total else 0.0
    if total < int(guardrails.get("minScenarioCount", 4)):
        failures.append(
            f"scenario count {total} below minScenarioCount {guardrails.get('minScenarioCount')}"
        )
    if pass_rate < float(guardrails.get("minPassRate", 1.0)):
        failures.append(
            f"pass rate {pass_rate:.2%} below threshold {float(guardrails.get('minPassRate', 1.0)):.2%}"
        )

    report = {
        "schemaVersion": "1",
        "generatedAtUtc": datetime.now(timezone.utc).isoformat(),
        "guardrails": guardrails,
        "totals": {
            "scenarioCount": total,
            "passed": passed,
            "failed": total - passed,
            "passRate": pass_rate,
        },
        "rows": rows,
        "failures": failures,
        "status": "pass" if not failures else "fail",
    }
    Path(args.out).write_text(json.dumps(report, indent=2) + "\n")
    print(f"scenarios={total} passed={passed} pass_rate={pass_rate:.2%}")
    print(f"report={args.out}")
    if failures:
        print("Failures:")
        for f in failures:
            print(f"- {f}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
