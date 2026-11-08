#!/usr/bin/env python3
"""Report the shipped artifact size, summing each independently served file."""

import gzip
import shutil
import subprocess
import sys
from pathlib import Path


def brotli_size(data: bytes) -> int | None:
    command = shutil.which("brotli")
    if command is None:
        return None
    result = subprocess.run(
        [command, "-q", "11", "-c"], input=data, capture_output=True, check=True
    )
    return len(result.stdout)


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: size.py DIST_DIR", file=sys.stderr)
        return 2

    root = Path(sys.argv[1])
    if not root.is_dir():
        print(f"dist directory missing: {root}", file=sys.stderr)
        return 1

    files = sorted(path for path in root.rglob("*") if path.is_file())
    if not files:
        print("dist contains no files", file=sys.stderr)
        return 1

    totals = [0, 0, 0]
    print(f"{'file':48} {'raw':>10} {'gzip':>10} {'brotli':>10}")
    for path in files:
        data = path.read_bytes()
        sizes = (
            len(data),
            len(gzip.compress(data, compresslevel=9, mtime=0)),
            brotli_size(data),
        )
        for index, size in enumerate(sizes):
            if size is not None:
                totals[index] += size
        relative = path.relative_to(root).as_posix()
        brotli = sizes[2] if sizes[2] is not None else "n/a"
        print(f"{relative:48} {sizes[0]:10} {sizes[1]:10} {brotli:>10}")

    brotli_total = totals[2] if shutil.which("brotli") else "n/a"
    print(f"{'total (per-file sums)':48} {totals[0]:10} {totals[1]:10} {brotli_total:>10}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
