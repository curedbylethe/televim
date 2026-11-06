#!/usr/bin/env python3
"""Allow release version updates to change only Televim's Cargo.lock entry."""

import re
import subprocess
from pathlib import Path


def normalized(text: str) -> str:
    blocks = re.split(r"(?m)(?=^\[\[package\]\]$)", text)
    matches = 0
    for index, block in enumerate(blocks):
        if re.search(r'(?m)^name = "televim"$', block):
            matches += 1
            blocks[index] = re.sub(
                r'(?m)^version = "[^"]+"$', 'version = "__VERSION__"', block, count=1
            )
    if matches != 1:
        raise SystemExit("Cargo.lock must contain exactly one televim package")
    return "".join(blocks)


before = subprocess.check_output(["git", "show", "HEAD:Cargo.lock"], text=True)
after = Path("Cargo.lock").read_text()
if normalized(before) != normalized(after):
    raise SystemExit("cargo update changed Cargo.lock beyond the televim version")
