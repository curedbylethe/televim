#!/usr/bin/env python3
"""Insert a generated release section above the first release in CHANGELOG.md.

Usage: splice-changelog.py CHANGELOG.md SECTION.md

Called by .github/workflows/release.yml on the release PR, after the file has
been restored to main's copy, so a re-run replaces nothing and adds one section.
Fails loudly when there is no release heading to anchor on.
"""
import sys

path, section_path = sys.argv[1], sys.argv[2]
with open(path, encoding="utf-8") as f:
    text = f.read()
with open(section_path, encoding="utf-8") as f:
    section = f.read().rstrip() + "\n\n"

at = text.index("\n## [") + 1
with open(path, "w", encoding="utf-8") as f:
    f.write(text[:at] + section + text[at:])
