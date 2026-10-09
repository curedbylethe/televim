#!/usr/bin/env python3
"""A/B benchmark compare for the criterion micro-benches (`make bench`).

Two legs, one report shape:
  in-binary  a criterion group that declares one `reference` id and one or more
             `candidate` ids (the last path segment of each id), from the same run.
             Groups without that declaration are not paired.
  revision   this run against a named saved baseline (`--baseline NAME`), or
             this run saved as one (`--save-baseline NAME`) for a later compare.

Writes target/bench-compare.json and target/bench-compare.md, and prints the
Markdown. Exits 1 on harness errors only (failed cargo run, missing or stale
criterion output, missing baseline), naming the leg or bench. Slowness is
reported as a delta and never fails the run.
"""

import argparse
import json
import os
import pathlib
import re
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]
TARGET = pathlib.Path(os.environ.get("CARGO_TARGET_DIR") or ROOT / "target")
CRITERION = TARGET / "criterion"
OUT_JSON = TARGET / "bench-compare.json"
OUT_MD = TARGET / "bench-compare.md"

# (package, bench target), as the [[bench]] entries in the crate manifests name them.
BENCHES = [
    ("domain", "history_window"),
    ("domain", "search"),
    ("domain", "vim"),
    ("tui", "wrap_rows"),
]

# Fixed flags, so a rerun asks criterion for the same work. criterion's minimum
# sample size is 10.
MODES = {
    "smoke": ["--sample-size", "10", "--warm-up-time", "0.5", "--measurement-time", "1"],
    "full": ["--sample-size", "100", "--warm-up-time", "3", "--measurement-time", "10"],
}
NOPLOT = ["--noplot"]
NAME_RE = re.compile(r"^[A-Za-z0-9_.-]+$")


class HarnessError(Exception):
    """A harness fault, not a slow result. The message names the leg or bench."""


def load_json(path):
    if not path.is_file():
        raise HarnessError(f"missing criterion file {path}")
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError) as err:
        raise HarnessError(f"unreadable criterion file {path}: {err}") from err


def read_text(path, what):
    try:
        return path.read_text()
    except OSError as err:
        raise HarnessError(f"cannot read {what} at {path}: {err}") from err


def toolchain():
    text = read_text(ROOT / "rust-toolchain.toml", "toolchain pin")
    m = re.search(r'channel\s*=\s*"([^"]+)"', text)
    if not m:
        raise HarnessError("rust-toolchain.toml has no channel")
    return m.group(1)


def runner_version():
    lock = read_text(ROOT / "Cargo.lock", "Cargo.lock")
    m = re.search(r'name = "criterion"\nversion = "([^"]+)"', lock)
    if not m:
        raise HarnessError("criterion is not in Cargo.lock")
    return m.group(1)


def summarize(est_path, sample_path):
    """median (the value), a 95% CI on it, and min/max of per-iteration times, in ns."""
    med = load_json(est_path)["median"]
    ci = med["confidence_interval"]
    sample = load_json(sample_path)
    per_iter = [t / i for t, i in zip(sample["times"], sample["iters"]) if i]
    if not per_iter:
        raise HarnessError(f"no samples in {sample_path}")
    return {
        "value": round(med["point_estimate"], 3),
        "ci_low": round(ci["lower_bound"], 3),
        "ci_high": round(ci["upper_bound"], 3),
        "confidence_level": ci["confidence_level"],
        "min": round(min(per_iter), 3),
        "max": round(max(per_iter), 3),
        "spread": round(max(per_iter) - min(per_iter), 3),
        "samples": len(per_iter),
    }


def delta(new, old):
    """new minus old. Descriptive only: `cis_overlap` says whether the noise bands touch."""
    abs_ns = round(new["value"] - old["value"], 3)
    pct = round(abs_ns / old["value"] * 100, 2) if old["value"] else None
    overlap = not (new["ci_high"] < old["ci_low"] or old["ci_high"] < new["ci_low"])
    return {"abs_ns": abs_ns, "pct": pct, "cis_overlap": overlap}


def role(bench_id):
    """`reference` or `candidate` when the id's last segment says so, else None."""
    last = bench_id.rsplit("/", 1)[-1]
    return last if last in ("reference", "candidate") else None


def find_benches():
    """bench id -> {group, dir} for every criterion bench with a `new` run on disk."""
    found = {}
    for meta in CRITERION.rglob("new/benchmark.json"):
        info = load_json(meta)
        found[info["full_id"]] = {"group": info["group_id"], "dir": meta.parent.parent}
    return found


def run_leg(package, target, mode, extra):
    """One `cargo bench` for one target. Its stdout goes to stderr so stdout stays the report."""
    label = f"{package}/{target}"
    started = time.time()
    cmd = ["cargo", "bench", "-p", package, "--bench", target, "--", *MODES[mode], *NOPLOT, *extra]
    print(f"== {label}: {' '.join(cmd[1:])}", file=sys.stderr)
    rc = subprocess.run(cmd, cwd=ROOT, stdout=sys.stderr).returncode
    if rc != 0:
        raise HarnessError(f"leg {label} failed: cargo bench exit {rc}")
    fresh = [p for p in CRITERION.rglob("new/estimates.json") if p.stat().st_mtime >= started - 1]
    if not fresh:
        raise HarnessError(f"leg {label} wrote no criterion results under {CRITERION}")


def build_report(mode, save, baseline):
    benches = find_benches()
    if not benches:
        raise HarnessError(f"no criterion results under {CRITERION}")
    report_benches, groups = [], {}
    for bid in sorted(benches):
        info = benches[bid]
        cur = summarize(info["dir"] / "new" / "estimates.json", info["dir"] / "new" / "sample.json")
        entry = {"id": bid, "group": info["group"], "current": cur}
        if baseline:
            base_dir = info["dir"] / baseline
            if not (base_dir / "estimates.json").is_file():
                raise HarnessError(f"bench {bid}: no saved baseline '{baseline}' at {base_dir}")
            base = summarize(base_dir / "estimates.json", base_dir / "sample.json")
            entry["baseline"] = base
            entry["delta"] = delta(cur, base)
        report_benches.append(entry)
        groups.setdefault(info["group"], []).append(entry)

    in_binary = []
    for group, entries in sorted(groups.items()):
        refs = [e for e in entries if role(e["id"]) == "reference"]
        cands = [e for e in entries if role(e["id"]) == "candidate"]
        if len(refs) != 1 or not cands:
            continue
        ref = refs[0]
        pairs = [
            {"id": e["id"], "current": e["current"], "delta": delta(e["current"], ref["current"])}
            for e in cands
        ]
        in_binary.append({"group": group, "reference": ref["id"], "reference_current": ref["current"], "candidates": pairs})

    host = os.uname()
    return {
        "schema": "televim.bench-compare/1",
        "generated_unix": int(time.time()),
        "runner": {"name": "criterion", "version": runner_version()},
        "toolchain": {"channel": toolchain(), "source": "rust-toolchain.toml"},
        "host": {"sysname": host.sysname, "release": host.release, "machine": host.machine},
        "mode": mode,
        "criterion_flags": [*MODES[mode], *NOPLOT],
        "unit": "ns",
        "legs": {
            "runs": [f"{p}/{t}" for p, t in BENCHES],
            "saved_baseline": save,
            "revision_baseline": baseline,
        },
        "benches": report_benches,
        "in_binary": in_binary,
    }


def fmt_ns(x):
    return f"{x:,.3f}" if abs(x) < 10 else f"{x:,.1f}"


def fmt_pct(x):
    return "n/a" if x is None else f"{x:+.2f}%"


def render_md(report):
    lines = [
        "# Benchmark compare",
        "",
        f"- runner: {report['runner']['name']} {report['runner']['version']}",
        f"- toolchain: {report['toolchain']['channel']} ({report['toolchain']['source']})",
        f"- host: {report['host']['sysname']} {report['host']['release']} {report['host']['machine']}",
        f"- mode: {report['mode']} (criterion flags: `{' '.join(report['criterion_flags'])}`)",
        f"- revision leg: {'current vs saved baseline `' + report['legs']['revision_baseline'] + '`' if report['legs']['revision_baseline'] else 'none (this run is not compared)'}"
        + (f"; saved as `{report['legs']['saved_baseline']}`" if report["legs"]["saved_baseline"] else ""),
        f"- unit: {report['unit']}. value = criterion median; CI = 95% on the median; min/max over per-iteration samples. Deltas are descriptive, not a pass/fail gate.",
        "",
        "## Benches",
        "",
    ]
    has_base = report["legs"]["revision_baseline"] is not None
    head = "| bench | n | median | 95% CI | min..max |"
    rule = "|---|---:|---:|---:|---:|"
    if has_base:
        head += " baseline median | delta | delta % | CIs overlap |"
        rule += "---:|---:|---:|:---:|"
    lines += [head, rule]
    for b in report["benches"]:
        c = b["current"]
        row = f"| `{b['id']}` | {c['samples']} | {fmt_ns(c['value'])} | {fmt_ns(c['ci_low'])}..{fmt_ns(c['ci_high'])} | {fmt_ns(c['min'])}..{fmt_ns(c['max'])} |"
        if has_base:
            d = b["delta"]
            row += f" {fmt_ns(b['baseline']['value'])} | {d['abs_ns']:+,.1f} | {fmt_pct(d['pct'])} | {'yes' if d['cis_overlap'] else 'no'} |"
        lines.append(row)
    lines += ["", "## In-binary pairs", ""]
    if not report["in_binary"]:
        lines.append("No group in this run declares a `reference` and `candidate` pair.")
    else:
        lines += [
            "Candidate minus reference, same run. Pairs are declared by bench id: a group with one `reference` id and `candidate` ids.",
            "",
            "| group | candidate | reference | candidate median | reference median | delta | delta % | CIs overlap |",
            "|---|---|---|---:|---:|---:|---:|:---:|",
        ]
        for g in report["in_binary"]:
            for p in g["candidates"]:
                d = p["delta"]
                lines.append(
                    f"| {g['group']} | `{p['id']}` | `{g['reference']}` | {fmt_ns(p['current']['value'])} | {fmt_ns(g['reference_current']['value'])} | {d['abs_ns']:+,.1f} | {fmt_pct(d['pct'])} | {'yes' if d['cis_overlap'] else 'no'} |"
                )
    return "\n".join(lines) + "\n"


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--mode", choices=sorted(MODES), default="smoke")
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--save-baseline", metavar="NAME", help="also save this run as a named criterion baseline")
    group.add_argument("--baseline", metavar="NAME", help="compare this run against a saved criterion baseline")
    args = parser.parse_args(argv)
    try:
        for name in (args.save_baseline, args.baseline):
            if name is not None and not NAME_RE.match(name):
                raise HarnessError(f"baseline name '{name}' must match {NAME_RE.pattern}")
        extra = []
        if args.save_baseline:
            extra = ["--save-baseline", args.save_baseline]
        elif args.baseline:
            if not any(CRITERION.rglob(f"{args.baseline}/estimates.json")):
                raise HarnessError(f"no saved baseline named '{args.baseline}' under {CRITERION}; run with --save-baseline {args.baseline} first")
            extra = ["--baseline", args.baseline]
        for package, target in BENCHES:
            run_leg(package, target, args.mode, extra)
        report = build_report(args.mode, args.save_baseline, args.baseline)
    except HarnessError as err:
        print(f"error: {err}", file=sys.stderr)
        return 1
    TARGET.mkdir(exist_ok=True)
    OUT_JSON.write_text(json.dumps(report, indent=2) + "\n")
    md = render_md(report)
    OUT_MD.write_text(md)
    print(md, end="")
    print(f"wrote {OUT_JSON} and {OUT_MD}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
