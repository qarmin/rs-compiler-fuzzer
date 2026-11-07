#!/usr/bin/env python3
"""Step `trunk_fix`: which merged llvm-project pull requests could have fixed a finding that LLVM_ISSUE.md reports as FIXED ON TRUNK.

    llvm_fix_search.py FINDING_DIR

The apt.llvm.org pool keeps only the newest snapshot, so the fix cannot be bisected by building: the candidates are merged PRs that name the
culprit pass between the date of the release snapshot that is wrong and the date of the trunk snapshot that is right. They are candidates
to read before asking for a backport, not a verdict. Writes the list into LLVM_ISSUE.md (section "### Possible fixes on trunk") and prints
`<done|n/a|todo>|<note>`.
"""
import json
import os
import re
import subprocess
import sys

MARK = "### Possible fixes on trunk"


def result(status, note):
    print(f"{status}|{note.replace('|', '/').replace(chr(10), ' ')}")
    sys.exit(0)


def main():
    d = sys.argv[1]
    path = os.path.join(d, "LLVM_ISSUE.md")
    if not os.path.exists(path):
        result("n/a", "no LLVM_ISSUE.md: no verified LLVM culprit")
    md = open(path).read()
    first = md.splitlines()[0] if md else ""
    if "FIXED ON TRUNK" not in first:
        result("n/a", "not fixed on trunk: nothing to look for")
    name = re.search(r"^name=(.*)$", open(os.path.join(d, "culprit.txt")).read(), re.M)
    pass_name = name.group(1).strip() if name else ""
    rows = [l for l in md.splitlines() if l.startswith("| LLVM ")]
    wrong = [re.search(r"snapshot (\d{4}-\d{2}-\d{2})", l) for l in rows if "WRONG CODE" in l]
    right = [re.search(r"snapshot (\d{4}-\d{2}-\d{2})", l) for l in rows if "same output" in l and "(trunk)" in l]
    if not wrong or not wrong[-1] or not right or not right[0]:
        result("todo", "cannot read the snapshot dates from LLVM_ISSUE.md")
    older = re.search(r"snapshot (\d{4}-\d{2}-\d{2})", rows[0]) if rows else None
    # the previous release's last snapshot is about when the release rustc uses was branched
    start = older.group(1) if older else "2026-01-01"
    end = right[0].group(1)
    terms = pass_name.replace("-", " ")
    q = f'repo:llvm/llvm-project is:pr is:merged merged:{start}..{end} "{terms}" in:title,body miscompile OR fix OR wrong'
    p = subprocess.run(["gh", "api", "-X", "GET", "search/issues", "-f", f"q={q}", "-f", "per_page=15", "-f", "sort=updated"], capture_output=True, text=True)
    if p.returncode != 0:
        result("todo", "gh search failed: " + p.stderr.strip()[-120:])
    items = json.loads(p.stdout).get("items", [])
    lines = [f"- llvm/llvm-project#{i['number']} ({i['closed_at'][:10]}): {i['title']} - {i['html_url']}" for i in items]
    section = f"{MARK}\n\nMerged pull requests that mention `{pass_name}` ({start} to {end}); candidates only, read them before asking for a backport.\n\n" + ("\n".join(lines) if lines else "none found") + "\n"
    if MARK in md:
        md = md[: md.index(MARK)].rstrip() + "\n"
    open(path, "w").write(md.rstrip() + "\n\n" + section)
    result("done", f"{len(lines)} candidate fix(es) listed" + (": " + " ".join(f"#{i['number']}" for i in items[:5]) if items else ""))


if __name__ == "__main__":
    main()
