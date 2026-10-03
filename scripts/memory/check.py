#!/usr/bin/env python3
"""Compares a measurement run against the stored baseline and fails on a breach.

Reads the per-run report `make measure` wrote (`target/memory-report.json`), the
stored baseline (`docs/memory-baseline*.json`) and the budget
(`scripts/memory/thresholds.json`), and decides whether any measured figure has
risen past what the budget allows.

    metric | measured | baseline | allowed | threshold | verdict

The budget is a guard on *change*, not on the project's declared targets: those
stay where README puts them. A figure that shrinks is never a failure.

Platform: a baseline is only comparable to a run on the platform it was taken
on. A host with no stored baseline is reported and not enforced, because a
threshold cannot be calibrated on a machine it was never measured on; the run
still writes its report, so the first run on a new host can be reviewed and
committed as that host's baseline.

Usage:
    scripts/memory/check.py [--report PATH] [--baseline PATH] [--budget PATH]
                            [--update-baseline] [--allow-uncalibrated]
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import platform
import re
import statistics
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
DOCS = ROOT / "docs"

# `docs/memory-baseline.json` is the recording host's; the pattern also picks up
# a `docs/memory-baseline.<platform>.json` committed for another host.
BASELINE_GLOB = "memory-baseline*.json"


def host_key() -> str:
    """This machine's key among the stored baselines: `darwin-arm64`, `linux-x86_64`."""
    machine = platform.machine() or os.uname().machine
    return f"{os.uname().sysname.lower()}-{machine}".lower()


def find_baseline(explicit: str | None) -> tuple[dict | None, str, str]:
    """The baseline for this host, and where it came from.

    An explicit path wins. Otherwise the file whose recorded `platform` matches
    this host is used, falling back to the unqualified `memory-baseline.json`
    when it names no platform at all.
    """
    if explicit:
        path = pathlib.Path(explicit)
        if not path.exists():
            return None, str(path), "the named baseline does not exist"
        return json.loads(path.read_text()), str(path), ""

    mine = host_key()
    candidates = sorted(DOCS.glob(BASELINE_GLOB))
    fallback = None
    for path in candidates:
        data = json.loads(path.read_text())
        recorded = data.get("platform")
        if recorded == mine:
            return data, str(path.relative_to(ROOT)), ""
        if not recorded and fallback is None:
            fallback = (data, str(path.relative_to(ROOT)))
    if fallback:
        return fallback[0], fallback[1], ""
    return None, "", f"no baseline stored for {mine}"


def allowed_growth(rule: dict, baseline: float) -> float:
    """How far the figure may rise: the larger of the two terms in the band."""
    band = rule["band"]
    return max(band["relative"] * baseline, band["absolute"])


def compare(report: dict, baseline: dict, budget: dict) -> list[dict]:
    """One row per rule, with the verdict the check acts on."""
    rows = []
    measured = report.get("metrics", {})
    base = baseline.get("metrics", {})
    for rule in budget["rules"]:
        name = rule["metric"]
        row = {
            "metric": name,
            "label": rule["label"],
            "unit": rule["unit"],
            "why": rule["why"],
        }
        got = measured.get(name)
        want = base.get(name)
        if not got or not got.get("measured") or not want or not want.get("measured"):
            reason = "not measured in this run" if not got or not got.get("measured") else "absent from the baseline"
            row.update(
                verdict="skipped",
                reason=reason,
                measured=None,
                baseline=want.get("value") if want else None,
            )
            rows.append(row)
            continue

        value = float(got["value"])
        base_value = float(want["value"])
        allowed = allowed_growth(rule, base_value)
        threshold = base_value + allowed
        delta = value - base_value
        delta_pct = (delta / base_value * 100) if base_value else 0.0
        breach = value > threshold
        row.update(
            verdict="BREACH" if breach else "ok",
            measured=round(value, 4),
            baseline=round(base_value, 4),
            allowed=round(allowed, 4),
            threshold=round(threshold, 4),
            delta=round(delta, 4),
            delta_pct=round(delta_pct, 2),
        )
        rows.append(row)
    return rows


def markdown(rows: list[dict], header: list[str]) -> str:
    """The visible report: every metric, its number, and what happened to it."""
    lines = [
        "| metric | measured | baseline | delta | allowed | threshold | verdict |",
        "| :--- | ---: | ---: | ---: | ---: | ---: | :--- |",
    ]
    for r in rows:
        if r["verdict"] == "skipped":
            lines.append(
                f"| {r['label']} | not measured | {_num(r.get('baseline'))} | — | — | — | skipped |"
            )
            continue
        lines.append(
            f"| {r['label']} | {_num(r['measured'])} | {_num(r['baseline'])} | "
            f"{r['delta']:+.4g} ({r['delta_pct']:+.1f}%) | {_num(r['allowed'])} | "
            f"{_num(r['threshold'])} | {r['verdict']} |"
        )
    return "\n".join(lines)


def _num(v) -> str:
    if v is None:
        return "—"
    if isinstance(v, float) and abs(v) >= 1000:
        return f"{v:,.0f}"
    return f"{v:,.4g}"


def failure_message(rows: list[dict]) -> str:
    """The message CI shows. It names the metric and the size of the breach."""
    breaches = [r for r in rows if r["verdict"] == "BREACH"]
    lines = [
        f"memory regression: {len(breaches)} figure(s) past the budget in "
        f"scripts/memory/thresholds.json",
        "",
    ]
    for r in breaches:
        lines.append(
            f"  {r['metric']}: measured {r['measured']:,.4g} {r['unit']}, "
            f"baseline {r['baseline']:,.4g}, "
            f"delta {r['delta']:+,.4g} ({r['delta_pct']:+.1f}%), "
            f"threshold {r['threshold']:,.4g} "
            f"(allowed {r['allowed']:,.4g})"
        )
    lines += [
        "",
        "This is the regression budget, not a declared target: the declared "
        "targets are unchanged. A figure may also be over budget because the "
        "noise floor moved - re-run `make measure` before assuming a change in "
        "the program.",
    ]
    return "\n".join(lines)


def record_baseline(reports: list[dict], path: pathlib.Path) -> None:
    """Writes one or more per-run reports out as this host's stored baseline.

    Takes the median of each figure across the reports given, because the budget
    only fails on an *increase*: a baseline captured during one unusually slow
    run sits high, and a rule anchored above what the tree normally does never
    fires however much worse the tree gets. Three ordinary runs is the smallest
    number that resists that.

    Written by hand rather than by the driver, because a baseline is a
    reviewable act: someone read the reports and decided this is what the tree
    costs.
    """
    metrics: dict = {}
    names = reports[0].get("metrics", {}).keys()
    for name in names:
        per_run = [r.get("metrics", {}).get(name, {}) for r in reports]
        usable = [m for m in per_run if m and m.get("measured")]
        if not usable:
            metrics[name] = per_run[0]
            continue
        def count(m: dict) -> int:
            """How many raw readings this figure is the median of.

            A count, not the list: `summarize` already stores the per-invocation
            samples as a list, and summing across invocations would be meaningless.
            A figure with no recorded samples is a single reading.
            """
            s = m.get("samples", 1)
            if isinstance(s, list):
                return len(s)
            return s if isinstance(s, (int, float)) else 1

        merged = {
            "unit": usable[0]["unit"],
            "measured": True,
            "value": statistics.median([m["value"] for m in usable]),
            "samples": sum(count(m) for m in usable),
        }
        for key in ("min", "max", "spread"):
            if key in usable[0]:
                merged[key] = statistics.median([m[key] for m in usable])
        if "of_launches" in usable[0]:
            merged["of_launches"] = statistics.median([m["of_launches"] for m in usable])
            # The cold launch is excluded from the warm median; record it so the
            # exclusion is visible rather than implied.
            colds = [m.get("cold_ms") for m in usable if m.get("cold_ms") is not None]
            if colds:
                merged["cold_ms"] = round(statistics.median(colds), 4)
                merged["warmth"] = "median of launches 2..N; launch 1 is cold and excluded"
        metrics[name] = merged

    # The cross-invocation aggregate is recomputed here from the reports being
    # recorded, rather than taken from the first one: the driver's own history
    # window holds only the invocation that ran last, and a baseline that carried
    # a single invocation's spread would claim a stability it never observed.
    pooled: dict[str, list[float]] = {}
    for r in reports:
        for name, agg in (r.get("cross_invocation") or {}).items():
            if isinstance(agg, dict) and "values" in agg:
                pooled.setdefault(name, []).extend(agg["values"])
        for name, m in r.get("metrics", {}).items():
            if m.get("measured"):
                pooled.setdefault(name, []).append(m["value"])
    cross: dict = {
        "_source": "per-invocation medians across the runs recorded here, pooled with "
        "any earlier invocations in the driver's history",
        "invocations": len(reports),
    }
    for name, values in sorted(pooled.items()):
        cross[name] = {
            "n": len(values),
            "median": round(statistics.median(values), 4),
            "min": round(min(values), 4),
            "max": round(max(values), 4),
            "values": values,
        }

    head = reports[0]
    baseline = {
        "schema": "televim.memory-baseline/1",
        "platform": host_key(),
        "note": "The stored baseline for this host. Produced by `make measure`, "
                "which writes target/memory-report.json per run, and promoted here "
                "with `scripts/memory/check.py --record-baseline`. Each figure is the "
                "median across the runs recorded, so a single slow run cannot anchor a "
                "rule above what the tree normally costs. Compare by field name; "
                "scripts/memory/thresholds.json reads these keys.",
        "taken_unix": head.get("generated_unix"),
        "recorded_from": len(reports),
        "environment": head.get("environment", {}),
        "load": head.get("load", {}),
        "idle_definition": head.get("idle_definition"),
        "metrics": metrics,
        # Carried so the threshold rationale is checkable from the tree: the
        # per-invocation values the aggregate was computed from are in here.
        "cross_invocation": cross,
        "massif_diagnostic": head.get("massif_diagnostic", {}),
        "targets": {
            "target_rss_mb_50": head.get("target_rss_mb_50"),
            "stretch_target_mb_20_30": head.get("stretch_target_mb_20_30"),
        },
        "not_measured": head.get("not_measured", []),
    }
    path.write_text(json.dumps(baseline, indent=2) + "\n")
    try:
        shown = path.resolve().relative_to(ROOT)
    except ValueError:
        shown = path
    print(f"recorded {shown} for {host_key()} from {len(reports)} run(s)")


def emit_summary(text: str) -> None:
    """Appends to the job summary when CI set one, and always to stdout."""
    print(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as fh:
            fh.write(text + "\n")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--report",
        nargs="+",
        default=[str(ROOT / "target" / "memory-report.json")],
        help="the per-run report(s) to read; several may be given when recording",
    )
    ap.add_argument("--baseline")
    ap.add_argument("--budget", default=str(ROOT / "scripts" / "memory" / "thresholds.json"))
    ap.add_argument(
        "--allow-uncalibrated",
        action="store_true",
        help="exit 0 even when this host has no stored baseline",
    )
    ap.add_argument(
        "--record-baseline",
        metavar="PATH",
        help="promote the last report to a stored baseline instead of comparing",
    )
    args = ap.parse_args()

    report_paths = [pathlib.Path(p) for p in args.report]
    missing = [p for p in report_paths if not p.exists()]
    if missing:
        emit_summary(
            "### memory budget\n\n**not checked**: "
            f"`{missing[0]}` is missing; run `make measure` first."
        )
        return 1
    reports = [json.loads(p.read_text()) for p in report_paths]
    report = reports[0]

    if args.record_baseline:
        record_baseline(reports, pathlib.Path(args.record_baseline))
        return 0

    budget = json.loads(pathlib.Path(args.budget).read_text())
    baseline, baseline_path, why = find_baseline(args.baseline)

    title = f"### memory budget — {host_key()}"
    env_note = (
        f"baseline: `{baseline_path}`"
        if baseline
        else f"baseline: **none for this host** ({why})"
    )
    env = report.get("environment", {})

    if baseline is None:
        rows = compare(report, {"metrics": {}}, budget)
        emit_summary(
            f"{title}\n\n{env_note}\n\n"
            "**not enforced.** A threshold cannot be calibrated on a host it was "
            "never measured on, so this run is reported and not failed. Review it, "
            "then record it as this host's baseline:\n\n"
            f"    scripts/memory/check.py --record-baseline "
            f"docs/memory-baseline.{host_key()}.json\n\n"
            + markdown(rows, [])
        )
        return 0

    # A baseline taken on another host, or against another tree, is not
    # comparable; say so rather than compare anyway.
    recorded_platform = baseline.get("platform")
    if recorded_platform and recorded_platform != host_key():
        emit_summary(
            f"{title}\n\n**not enforced**: baseline `{baseline_path}` was taken on "
            f"`{recorded_platform}`, this host is `{host_key()}`."
        )
        return 0

    rows = compare(report, baseline, budget)
    lock_match = (
        baseline.get("environment", {}).get("cargo_lock_sha256")
        == env.get("cargo_lock_sha256")
    )
    lock_note = (
        f"Cargo.lock matches the baseline."
        if lock_match
        else f"**Cargo.lock differs from the baseline** — the tree has changed since "
        f"it was taken, which moves every figure here. Re-record the baseline if "
        f"that change was intended."
    )

    breaches = [r for r in rows if r["verdict"] == "BREACH"]
    verdict = (
        f"**BREACH** — {len(breaches)} figure(s) past the budget."
        if breaches
        else f"**within budget** — {len(rows)} figures checked."
    )
    emit_summary(f"{title}\n\n{verdict}\n\n{env_note}\n\n{markdown(rows, [])}\n\n{lock_note}")
    emit_summary("\n<details><summary>not measured</summary>\n\n"
                 + "\n".join(f"- {n}" for n in report.get("not_measured", []))
                 + "\n\n</details>")

    if breaches:
        print("\n" + failure_message(rows), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
