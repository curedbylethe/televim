#!/usr/bin/env python3
"""televim measurement harness driver (`make measure`).

Runs the synthetic harness several times, times the real release binary's
first frame and one keypress, weighs the release binary, and records the
environment every number was taken in. Writes one report to
`target/memory-report.json` and prints a Markdown table.

The baseline that later PR stages compare against lives in
`docs/memory-baseline.json`; this script never writes it. Copy the report
there deliberately, so a new baseline is a reviewable act.

Usage:
    scripts/memory/measure.py [--runs N] [--skip-build] [--massif]

Environment:
    TELEVIM_MEASURE_RUNS   same as --runs
    TELEVIM_MEASURE_SKIP_BUILD=1   same as --skip-build
    TELEVIM_MEASURE_LAUNCHES=N   how many times to launch the real binary
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import pty
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]
TARGET = ROOT / "target"
REPORT = TARGET / "memory-report.json"
BINARY = TARGET / "release" / "televim"

# Where the measured binary's log is copied afterwards, so the report can point
# at something durable rather than at a temporary directory that is about to go.
KEPT_LOG = TARGET / "measure" / "binary.log"

# Where the measured binary's log is copied afterwards, so the report can point
# at something durable rather than at a temporary directory that is about to go.
KEPT_LOG = TARGET / "measure" / "binary.log"

# The last N invocations' medians, so the report can state the spread *across*
# invocations rather than only within one. See `cross_invocation`.
HISTORY = TARGET / "measure" / "history.jsonl"
HISTORY_KEEP = 20

# The env var the release binary's probes are gated on.
GATE = "TELEVIM_MEASURE"

# How many times the release binary is launched. One cold launch measures the
# page cache; this many measures the program. See `real_binary`.
BINARY_LAUNCHES = int(os.environ.get("TELEVIM_MEASURE_LAUNCHES", 5))

# Inherited into the measured binary's environment. Everything else is dropped:
# the point is that no credential, session path or account reaches it.
ENV_WHITELIST = ("PATH", "TERM", "LANG", "LC_ALL", "LC_CTYPE", "TMPDIR", "USER")

# Prefixes stripped from the inherited environment even if whitelisted, because
# any of these would hand the run an account.
ENV_DENY_PREFIXES = ("TELEGRAM_", "TELEVIM_")

# Written to the log by the probes in `app/src/runtime.rs`. These are the
# patterns; changing them here without changing them there loses the number.
FIRST_FRAME = re.compile(r"first_frame_ms=(?P<ms>[\d.]+)")
INPUT_LATENCY = re.compile(r"input_latency_ms=(?P<ms>[\d.]+)")
MASSIF_PEAK = re.compile(r"^mem_heap_B=(\d+)", re.M)
MASSIF_SNAPSHOT = re.compile(r"^snapshot=(\d+)", re.M)

# What "idle" means here, in the same words docs/memory.md uses. It travels in
# the report so a recorded baseline carries the definition its figures were taken
# under rather than the word alone -- without a value here `check.py` copies null
# into the baseline and the number is unanchored.
IDLE_DEFINITION = (
    "after the synthetic load is applied and drawn, settle_ms of quiet with no "
    "keypress pending and no frame in flight, then five RSS samples "
    "sample_gap_ms apart, of which the median is the figure reported. The "
    "shipped binary is sampled the same way on its own 250 ms idle tick, in a "
    "sandbox outside the repository so no credential reaches the run."
)

NOT_MEASURED = [
    "allocation counts (no counting global allocator is installed; the report "
    "names bytes and frames rather than a count of alloc calls)",
    "heap fragmentation (RSS does not distinguish a fragmented heap from a large one)",
    "cache behaviour",
    "anything inside grammers, including MTProto decoding",
    "the reader thread's blocking crossterm read and the terminal's own paint: "
    "input latency is measured from the loop taking the key to the draw completing, "
    "so it is not end-to-end keystroke-to-photons",
    "the real binary's startup with a populated chat list: reaching one needs a "
    "Telegram round trip, so first-frame is the empty/sign-in frame",
    "a Windows RSS figure (no portable way to ask a process; reported as null)",
]


def run(cmd, **kwargs):
    """Runs a command and returns its stdout, raising with the output on failure."""
    return subprocess.run(
        cmd, cwd=ROOT, check=True, capture_output=True, text=True, **kwargs
    )


def build(skip: bool) -> None:
    """Builds the release binary and the harness example.

    The example is built separately and explicitly: `cargo build --release`
    does not build examples, which is the whole reason the harness is one.
    """
    if skip:
        print("· skipping the build (--skip-build)")
        return
    print("· building the release binary and the harness (this is slow: lto=fat)")
    run(["cargo", "build", "--release", "--locked"])
    run(["cargo", "build", "--release", "--locked", "--example", "memory_harness"])


def harness_runs(count: int) -> list[dict]:
    """Runs the synthetic harness `count` times and returns each run's JSON.

    Always run directly, on every platform. Under valgrind the harness's own
    `VmRSS`/`proc_pidinfo` reading reports *valgrind's* memory, and every timing
    is valgrind's timing rather than the program's — so a valgrind run is not
    the measurement, and running one where a threshold will compare against it
    compares against a distortion. `massif_diagnostic` produces that separately.
    """
    example = TARGET / "release" / "examples" / "memory_harness"
    if not example.exists():
        sys.exit(f"error: {example} is missing; run without --skip-build")

    runs = []
    for n in range(1, count + 1):
        out = subprocess.run([str(example)], cwd=ROOT, check=True, capture_output=True, text=True)
        runs.append(json.loads(out.stdout))
        print(f"· harness run {n}/{count}")
    return runs


def massif_diagnostic() -> dict:
    """One extra harness run under `valgrind --tool=massif`, for diagnosis only.

    Reports massif's own peak heap, parsed from its output file, as a separate
    field. It is deliberately not one of the compared metrics and is not
    thresholded: massif reports the heap it managed, and its instrumentation
    changes both the numbers and the timing of the process it watches. Its value
    is a profile to read after something looks wrong, not a gate.

    Returns a "skipped" record rather than failing when valgrind is absent,
    which is the normal case on a developer machine.
    """
    example = TARGET / "release" / "examples" / "memory_harness"
    massif = shutil.which("valgrind")
    if massif is None:
        return {"ran": False, "reason": "valgrind is not installed on this host"}
    if not example.exists():
        return {"ran": False, "reason": f"{example} is missing"}

    out_file = TARGET / "measure" / "massif.out"
    out_file.parent.mkdir(parents=True, exist_ok=True)
    proc = subprocess.run(
        [massif, "--tool=massif", f"--massif-out-file={out_file}", str(example)],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    peak = None
    if out_file.exists():
        peaks = [int(m.group(1)) for m in MASSIF_PEAK.finditer(out_file.read_text())]
        if peaks:
            peak = max(peaks)
    return {
        "ran": proc.returncode == 0,
        "tool": subprocess.run([massif, "--version"], capture_output=True, text=True).stdout.split("\n")[0],
        "peak_heap_bytes": peak,
        "out_file": str(out_file.relative_to(ROOT)),
        "compared": False,
        "note": "massif's peak heap, reported separately; under valgrind the harness's "
        "own RSS is valgrind's memory, so this run is never the compared measurement",
    }


def summarize(values: list[float], unit: str, scale: float = 1.0) -> dict:
    """The three figures every metric carries: a value, and the spread around it.

    The spread is the noise floor stage 2 calibrates thresholds against, so it
    is recorded rather than thrown away.
    """
    if not values:
        return {"unit": unit, "measured": False, "reason": "no sample produced"}
    return {
        "unit": unit,
        "measured": True,
        "value": round(statistics.median(values) * scale, 3),
        "min": round(min(values) * scale, 3),
        "max": round(max(values) * scale, 3),
        "spread": round((max(values) - min(values)) * scale, 3),
        "samples": [round(v * scale, 3) for v in values],
    }


def process_rss_bytes(pid: int) -> int | None:
    """This process's resident set size, read the same way the harness reads its own.

    Same sources, one level out: `/proc/<pid>/status` on Linux, `ps` on macOS.
    So the two figures are the same measurement of different processes, rather
    than two tools that disagree about what RSS means.
    """
    try:
        if os.uname().sysname == "Linux":
            status = pathlib.Path(f"/proc/{pid}/status").read_text()
            for line in status.splitlines():
                if line.startswith("VmRSS:"):
                    return int(line.split()[1]) * 1024
            return None
        out = subprocess.run(
            ["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True
        ).stdout.strip()
        # ps reports kibibytes on both Linux and macOS.
        return int(out) * 1024 if out else None
    except (OSError, ValueError):
        return None


def make_sandbox() -> pathlib.Path:
    """Creates the sandbox the measured binary runs inside.

    Outside the repository, and that is the whole point: `dotenvy::dotenv()`
    searches the working directory and then every parent of it, so a sandbox
    under `target/` still finds the repository root's `.env` and still hands the
    run a developer's application credentials. A temporary directory somewhere
    with no `.env` above it is the only placement that reliably prevents it.
    """
    sandbox = pathlib.Path(tempfile.mkdtemp(prefix="televim-measure-"))
    (sandbox / "home").mkdir(parents=True, exist_ok=True)
    (sandbox / "tmp").mkdir(parents=True, exist_ok=True)
    return sandbox


def sandbox_env(sandbox: pathlib.Path) -> dict:
    """A deliberately bare environment for the measured binary.

    The binary reaches Telegram if it is handed credentials, and it reads a
    `.env` if one is anywhere above its working directory. Measured with a
    developer's real environment, the "real binary" run signs in, fetches the
    chat list, and its RSS becomes a function of an account's history rather
    than of the program — a figure that cannot be reproduced on a clean host and
    that this report would have described as "no account".

    So the measured process is given: a temporary `HOME`, a working directory
    outside the repository with no `.env` above it, an empty config file, a
    session path inside the sandbox, and only the environment variables needed to
    start a process. Every `TELEGRAM_*` and `TELEVIM_*` name is dropped, so
    there are no application credentials at all — which is the point: with none,
    the program deterministically takes the unauthorized path on any machine.

    Returns the audit record of what was removed, which goes into the report.
    """
    removed = sorted(name for name in os.environ if name.startswith(ENV_DENY_PREFIXES))
    env = {k: os.environ[k] for k in ENV_WHITELIST if k in os.environ}
    env.update(
        TERM="xterm-256color",
        HOME=str(sandbox / "home"),
        TMPDIR=str(sandbox / "tmp"),
        # The one TELEVIM_ name that must be set: it is what arms the probes.
        TELEVIM_MEASURE="1",
        # Belt and braces. With no credentials the client is never built, so the
        # session store should never be touched; if that ever changed, it would
        # at least be a file inside the sandbox rather than a developer's own.
        TELEVIM_SESSION_PATH=str(sandbox / "session.json"),
    )
    return {
        "env": env,
        "isolation": {
            "sandbox": str(sandbox),
            "outside_repository": True,
            "home": str(sandbox / "home"),
            "cwd": str(sandbox),
            "config": str(sandbox / "televim.toml"),
            "session_path": str(sandbox / "session.json"),
            "inherited": sorted(env),
            "removed_prefixes": list(ENV_DENY_PREFIXES),
            "removed_names": removed,
            "ambient_dotenv_in_repo": (ROOT / ".env").exists(),
            "dotenv_search": "dotenvy::dotenv() searches the cwd and every parent, "
            "so the sandbox is outside the repository: an in-repo sandbox would "
            "still load the root .env",
            "credentials_supplied": False,
            "note": "no TELEGRAM_*/TELEVIM_* credential reaches the run, so the "
            "binary takes the unauthorized path, builds no client, and its chat "
            "list is empty",
        },
    }


def launch_once(config: pathlib.Path, log: pathlib.Path, env: dict, sandbox: pathlib.Path) -> dict:
    """One launch of the release binary on a pty, and what it said about itself.

    The binary enters raw mode and the alternate screen, so a pipe is not
    enough: it needs something that looks like a terminal. The keypress is
    written into the pty, which is how the input-latency figure gets a number
    rather than a method.
    """
    master, slave = pty.openpty()
    log.unlink(missing_ok=True)

    started = time.monotonic()
    proc = subprocess.Popen(
        [str(BINARY), "--config", str(config)],
        cwd=sandbox,
        stdin=slave,
        stdout=slave,
        stderr=slave,
        env=env,
        close_fds=True,
    )
    os.close(slave)

    rss: list[int] = []
    try:
        # Long enough for the first frame, the bring-up failure, and a key.
        time.sleep(6.0)
        # Sampled with nothing in flight and no key pending: the binary sits on
        # its own idle 250 ms tick between events.
        for _ in range(5):
            sample = process_rss_bytes(proc.pid)
            if sample is not None:
                rss.append(sample)
            time.sleep(0.2)
        os.write(master, b"jjk")
        time.sleep(2.0)
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)
        os.close(master)

    wall = time.monotonic() - started
    text = log.read_text() if log.exists() else ""

    first = [float(m.group("ms")) for m in FIRST_FRAME.finditer(text)]
    latency = [float(m.group("ms")) for m in INPUT_LATENCY.finditer(text)]
    return {
        "measured": bool(first),
        "first_frame_ms": first,
        "input_latency_ms": latency,
        "rss_idle_bytes": rss,
        "wall_seconds": round(wall, 2),
    }


def real_binary(launches: int = BINARY_LAUNCHES) -> dict:
    """Launches the release binary `launches` times and reports the warm median.

    Repeated launches, because a single one measures the page cache rather than
    the program: the first launch after a build pages the binary in from disk
    and reads 10-17 ms, while the launches after it read ~0.6 ms. A single cold
    sample is a fact about the machine, and a threshold calibrated on it would
    be a threshold on disk speed.

    The cold figure is kept, as `first_frame_cold_ms`, because it is real and
    worth having — it is what the very first launch of a session costs. The
    median of the launches after it is what the program costs.

    RSS is sampled in the state the binary reaches with no account: the chat
    list is empty, because there is no offline way to put fifty chats in front
    of it. The loaded figure comes from the harness instead, and the two are
    recorded as two things rather than averaged into one.
    """
    if not BINARY.exists():
        return {"measured": False, "reason": f"{BINARY} is missing"}

    # A fresh sandbox every run: anything a previous launch left behind must not
    # be able to influence this one.
    sandbox_dir = make_sandbox()
    config = sandbox_dir / "televim.toml"
    log = config.with_suffix(".log")

    # A config file with nothing in it, so the run takes the defaults.
    config.write_text("")
    sandbox = sandbox_env(sandbox_dir)

    runs = [
        launch_once(config, log, sandbox["env"], sandbox_dir) for _ in range(max(1, launches))
    ]
    first = [f for r in runs for f in r["first_frame_ms"]]
    latency = [v for r in runs for v in r["input_latency_ms"]]
    rss = [v for r in runs for v in r["rss_idle_bytes"]]

    # Keep the log somewhere durable; the sandbox is about to be discarded.
    KEPT_LOG.parent.mkdir(parents=True, exist_ok=True)
    if log.exists():
        KEPT_LOG.write_text(log.read_text())
    shutil.rmtree(sandbox_dir, ignore_errors=True)

    return {
        "measured": bool(first),
        "launches": len(runs),
        "first_frame_ms": first[1:] if len(first) > 1 else first,
        "first_frame_cold_ms": first[0] if first else None,
        "first_frame_ms_all": first,
        "input_latency_ms": latency,
        "rss_idle_bytes": rss,
        "rss_note": "the shipped binary, unauthorised: no credential reaches it, so it "
        "builds no client and its chat list is empty. This is the whole program and "
        "none of its data; see rss_idle_harness_bytes for the loaded figure.",
        "isolation": sandbox["isolation"],
        "log": str(KEPT_LOG.relative_to(ROOT)),
    }


def environment(real: dict) -> dict:
    """Everything a number is only comparable against if it is known about.

    The material confounds are in here on purpose, because each one is a way
    this report could otherwise have described a signed-in session or a
    valgrind-instrumented process as the program: whether credentials or a
    session file were in reach, how many launches the figures came from, what
    the first-frame number excludes, and whether a profiler was involved.
    """
    lock = ROOT / "Cargo.lock"
    profile = ROOT / "Cargo.toml"
    rustc = subprocess.run(["rustc", "-Vv"], capture_output=True, text=True).stdout
    release = profile.read_text().split("[profile.release]")[1].split("\n\n")[0]
    massif = shutil.which("valgrind")
    isolation = real.get("isolation", {})
    return {
        "host": os.uname().sysname + " " + os.uname().machine,
        "uname": subprocess.run(["uname", "-a"], capture_output=True, text=True).stdout.strip(),
        "rustc": rustc.strip().splitlines(),
        "toolchain_pin": (ROOT / "rust-toolchain.toml").read_text().strip(),
        "cargo_profile_release": " ".join(release.split()),
        "cargo_lock_sha256": hashlib.sha256(lock.read_bytes()).hexdigest(),
        "cpu_count": os.cpu_count(),
        "rss_method": "in-process: /proc/self/status (Linux) / proc_pidinfo (macOS)",
        "binary_rss_method": "/proc/<pid>/status (Linux) / ps rss (macOS)",
        "credentials_in_ambient_env": bool(isolation.get("removed_names")),
        "credentials_reaching_the_run": False,
        "ambient_dotenv_present": isolation.get("ambient_dotenv_in_repo"),
        "session_isolated_to_sandbox": True,
        "isolation": isolation,
        "binary_launches": real.get("launches"),
        "first_frame_excludes": "the chat list: this is the empty/sign-in frame drawn "
        "before any network round trip",
        "first_frame_warmth": f"median of launches 2..{real.get('launches')}; launch 1 is "
        "cold (pages the binary in from disk) and is reported separately as "
        "first_frame_cold_ms",
        "input_latency_excludes": "the reader thread's blocking read and the terminal's "
        "paint; measured from the loop taking the key to the draw completing",
        "massif_available": massif is not None,
        "massif_version": (
            subprocess.run([massif, "--version"], capture_output=True, text=True).stdout.split("\n")[0]
            if massif
            else None
        ),
        "profiler_involved_in_comparison": False,
    }


def cross_invocation(report_metrics: dict) -> dict:
    """The spread across whole `make measure` invocations, from the run history.

    A single invocation can only see its own runs, and the within-invocation
    spread understates the noise: what separates two invocations is the state of
    the machine, not the program. So each invocation appends its medians to a
    history file and reports the aggregate over the last `HISTORY_KEEP` of them.

    The per-invocation values are carried in the output, so the aggregate in the
    stored baseline can be recomputed from the tree rather than taken on trust.
    """
    entry = {
        "generated_unix": int(time.time()),
        "metrics": {k: v.get("value") for k, v in report_metrics.items() if v.get("measured")},
    }
    HISTORY.parent.mkdir(parents=True, exist_ok=True)
    history: list[dict] = []
    if HISTORY.exists():
        for line in HISTORY.read_text().splitlines():
            try:
                history.append(json.loads(line))
            except json.JSONDecodeError:
                continue
    history.append(entry)
    history = history[-HISTORY_KEEP:]
    HISTORY.write_text("\n".join(json.dumps(h) for h in history) + "\n")

    aggregate: dict = {
        "_source": f"per-invocation medians from target/measure/history.jsonl "
        f"(last {HISTORY_KEEP} invocations)",
        "invocations": len(history),
    }
    names = sorted({k for h in history for k in h.get("metrics", {})})
    for name in names:
        values = [h["metrics"][name] for h in history if name in h.get("metrics", {})]
        if not values:
            continue
        aggregate[name] = {
            "n": len(values),
            "median": round(statistics.median(values), 4),
            "min": round(min(values), 4),
            "max": round(max(values), 4),
            "values": values,
        }
    # One invocation cannot establish a spread. Saying so beats reporting a
    # range of zero, which reads as a stable measurement.
    if len(history) < 3:
        aggregate["_note"] = (
            f"only {len(history)} invocation(s) recorded; run `make measure` at "
            "least three times for a spread that means anything"
        )
    return aggregate


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=int(os.environ.get("TELEVIM_MEASURE_RUNS", 5)))
    parser.add_argument("--skip-build", action="store_true",
                        default=os.environ.get("TELEVIM_MEASURE_SKIP_BUILD") == "1")
    parser.add_argument(
        "--massif",
        action="store_true",
        help="also run one harness invocation under valgrind --tool=massif, as a "
        "diagnostic only; never part of the compared metrics",
    )
    args = parser.parse_args()

    if not args.runs:
        sys.exit("error: --runs must be at least 1")

    build(args.skip_build)
    runs = harness_runs(args.runs)

    first = runs[0]
    rss_harness = summarize([r["rss_idle_bytes"] for r in runs], "bytes")
    load = summarize([r["startup_populated_load_us"] / 1000 for r in runs], "ms")
    frame = summarize([r["frame_us_median"] / 1000 for r in runs], "ms")
    binary_stats = {
        "unit": "bytes",
        "measured": BINARY.exists(),
        "value": BINARY.stat().st_size if BINARY.exists() else None,
        "command": "ls -lh target/release/televim",
    }
    harness_latency = summarize(
        [statistics.median(r["input_latency_us_samples"]) / 1000 for r in runs], "ms"
    )

    real = real_binary()
    binary_rss_stats = (
        summarize([b / (1024 * 1024) for b in real["rss_idle_bytes"]], "MB")
        if real["rss_idle_bytes"]
        else {"unit": "MB", "measured": False, "reason": real.get("reason", "no sample")}
    )
    binary_rss = binary_rss_stats["value"] if binary_rss_stats["measured"] else None
    harness_rss = (
        rss_harness["value"] / (1024 * 1024) if rss_harness["measured"] else None
    )
    first_frame = (
        summarize(real["first_frame_ms"], "ms")
        if real["measured"]
        else {"unit": "ms", "measured": False, "reason": real.get("reason", "no probe in the log")}
    )
    if real["measured"]:
        # Traceability: how many numbers this is, and what the cold one was, so a
        # reader can tell a warm median from a single sample with no spread.
        first_frame["samples"] = len(real["first_frame_ms"])
        first_frame["cold_ms"] = real.get("first_frame_cold_ms")
        first_frame["of_launches"] = real.get("launches")

    metrics = {
        "rss_idle_harness_bytes": rss_harness,
        "rss_idle_binary_bytes": binary_rss_stats,
        "startup_populated_load_ms": load,
        "startup_first_frame_ms": first_frame,
        "input_latency_ms": summarize(real["input_latency_ms"], "ms") if real["input_latency_ms"] else {"unit": "ms", "measured": False, "reason": "no keypress reached the loop"},
        "frame_ms": frame,
        "harness_input_latency_ms": harness_latency,
        "binary_bytes": binary_stats,
    }
    if real["input_latency_ms"]:
        metrics["input_latency_ms"]["samples"] = len(real["input_latency_ms"])
    if real["rss_idle_bytes"]:
        metrics["rss_idle_binary_bytes"]["samples"] = len(real["rss_idle_bytes"])

    report = {
        "schema": "televim.memory-report/1",
        "generated_unix": int(time.time()),
        "environment": environment(real),
        "load": {
            "chats": first["chats"],
            "messages_supplied": first["messages_supplied"],
            "messages_in_window": first["messages_in_window"],
            "settle_ms": first["settle_ms"],
            "terminal": first["terminal"],
        },
        "idle_definition": IDLE_DEFINITION,
        "metrics": metrics,
        "target_rss_mb_50": dict(
            verdict(50, None, binary=binary_rss, harness=harness_rss),
            attempted=False,
            ceiling_mb=50,
        ),
        "stretch_target_mb_20_30": dict(
            verdict(30, (20, 30), binary=binary_rss, harness=harness_rss),
            attempted=False,
            band_mb=[20, 30],
            note="recorded, not attempted; the 20-30 MB stretch was neither "
            "chased nor abandoned, and the range was not changed",
        ),
        "not_measured": NOT_MEASURED,
        "massif_diagnostic": massif_diagnostic() if args.massif else {"ran": False, "reason": "not requested (--massif)"},
        "cross_invocation": cross_invocation(metrics),
        "runs": runs,
        "real_binary_run": real,
    }

    REPORT.parent.mkdir(parents=True, exist_ok=True)
    REPORT.write_text(json.dumps(report, indent=2) + "\n")
    print_table(report)
    print(f"\nwrote {REPORT.relative_to(ROOT)}")
    return 0


def verdict(ceiling_mb: float, band: tuple[float, float] | None, **figures) -> dict:
    """A ceiling's status, computed from every figure and honest about none being complete.

    No figure is the program under load: the binary is the shipped program with
    an empty chat list (there is no offline way to load fifty chats into it), and
    the harness is 60 chats without the network half. The status is judged on the
    largest of them, and names which that was.

    `band` is a target *range* rather than a ceiling. A figure under the range
    is not "met": the stretch asks for a process between 20 and 30 MB, and 5 MB
    is a different thing from either end of it. It is reported as `below range`.
    """
    measured = {k: v for k, v in figures.items() if v is not None}
    if not measured:
        return {"status": "not measured"}
    largest = max(measured, key=measured.get)
    value = measured[largest]
    if band is None:
        status = "met" if value <= ceiling_mb else "not met"
    elif band[0] <= value <= band[1]:
        status = "met"
    else:
        status = "below range" if value < band[0] else "above range"
    return {
        "status": status,
        "largest_figure": largest,
        "measured_mb": {k: round(v, 2) for k, v in measured.items()},
        "note": "no figure is the program under load; the largest bounds it from one side only",
    }


def print_table(report: dict) -> None:
    env = report["environment"]
    print()
    print("### environment")
    print(f"- host: {env['host']}")
    print(f"- rustc: {env['rustc'][0]}")
    print(f"- release profile: {env['cargo_profile_release']}")
    print(f"- Cargo.lock: {env['cargo_lock_sha256'][:16]}…")
    print(f"- rss method: {env['rss_method']}")
    print(f"- credentials reaching the run: {env['credentials_reaching_the_run']} "
          f"(ambient: {env['credentials_in_ambient_env']}, .env present: {env['ambient_dotenv_present']})")
    print(f"- binary launches: {env['binary_launches']}; first frame = "
          f"{env['first_frame_warmth']}")
    print(f"- profiler in the compared metrics: {env['profiler_involved_in_comparison']}"
          f" (valgrind present: {env['massif_available']})")
    print()
    print("### metrics (median of %d runs)" % len(report["runs"]))
    print("| metric | value | min | max | unit |")
    print("| :--- | ---: | ---: | ---: | :--- |")
    for name, m in report["metrics"].items():
        if not m.get("measured"):
            print(f"| {name} | not measured | — | — | {m['unit']} |")
            continue
        # A single figure (the binary's size) has no spread to report.
        span = f"{m['min']} | {m['max']}" if "min" in m else "— | —"
        print(f"| {name} | {m['value']} | {span} | {m['unit']} |")
    print()
    for key in ("target_rss_mb_50", "stretch_target_mb_20_30"):
        v = report[key]
        print(f"{key}: {v['status']} (measured {v.get('measured_mb')})")
    print("not measured: " + "; ".join(report["not_measured"]))


if __name__ == "__main__":
    sys.exit(main())
