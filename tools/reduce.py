#!/usr/bin/env python3
"""usage: reduce.py in.rs out.rs "flagsA" "flagsB" toolchain
Greedy line-based reduction of a program on which two rustc configurations disagree (stdout)."""
import subprocess, sys, os, tempfile, shutil
inp, outp, fa, fb, tc = sys.argv[1:6]
COMMON = ["--edition", "2021", "--cap-lints", "allow", "-C", "debug-assertions=off", "-C", "overflow-checks=off"]
tmp = tempfile.mkdtemp()
def run(src, flags, tag):
    f = os.path.join(tmp, f"{tag}.rs"); open(f, "w").write(src)
    b = os.path.join(tmp, tag)
    r = subprocess.run(["rustc", f"+{tc}", f, "-o", b, *COMMON, *flags.split()], capture_output=True, text=True)
    if r.returncode != 0: return None
    try: p = subprocess.run([b], capture_output=True, text=True, timeout=10)
    except subprocess.TimeoutExpired: return None
    return (p.returncode, p.stdout)
def differs(src):
    a = run(src, fa, "a")
    if a is None: return False
    b = run(src, fb, "b")
    return b is not None and a != b
lines = open(inp).read().split("\n")
assert differs("\n".join(lines)), "input does not diverge"
import re
# pass 0: ddmin-style chunk removal
n = len(lines) // 2
while n >= 1:
    i = 0
    while i < len(lines):
        cand = lines[:i] + lines[i+n:]
        if cand and differs("\n".join(cand)):
            lines = cand
        else:
            i += n
    n //= 2
print(f"chunks: {len(lines)} lines", flush=True)
# pass 1: replace the right-hand side of `let name: ty = EXPR;` by a trivial literal
for i in range(len(lines)):
    m = re.match(r"^(\s*let (?:mut )?\w+: (\w+) = )(.*);\s*$", lines[i])
    if not m: continue
    ty = m.group(2)
    lit = {"bool": "false"}.get(ty, "0")
    cand = lines[:i] + [m.group(1) + f"{lit};"] + lines[i+1:]
    if differs("\n".join(cand)):
        lines = cand
print("simplified lets", flush=True)
changed = True
rounds = 0
while changed and rounds < 6:
    changed = False; rounds += 1
    i = len(lines) - 1
    while i >= 0:
        cand = lines[:i] + lines[i+1:]
        if differs("\n".join(cand)):
            lines = cand; changed = True
        i -= 1
    print(f"round {rounds}: {len(lines)} lines", flush=True)
open(outp, "w").write("\n".join(lines))
shutil.rmtree(tmp)
