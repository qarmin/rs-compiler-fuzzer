#!/usr/bin/env python3
"""Reduce the LLVM IR of a verified LLVM finding with `llvm-reduce` and add it to LLVM_ISSUE.md (step `llvm_reduce`).

    llvm_reduce.py FINDING_DIR --src reduced.rs --flags "-C opt-level=2" [--work DIR] [--cache DIR] [--budget SECONDS]

Needs LLVM_ISSUE.md from `llvm_issue.py` with the verdict WRONG on some LLVM (trunk when it is still wrong there, otherwise the release rustc uses).
The reduction keeps the module as long as the build without the pass prints the original (correct) output and the build with it prints something else.
Prints one line `<done|n/a|todo>|<note>`. The result may use undefined behaviour the original did not: check it on https://alive2.llvm.org/ce/ before filing.
"""
import argparse
import json
import os
import re
import shutil
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import llvm_issue as li  # noqa: E402

MARK = "### Reduced IR"
# these turn locals into parameters or operands into poison: the program then only "differs" because it reads garbage
UB_PRONE_PASSES = "arguments,arguments-to-return,instructions-to-return,operands-to-args,operands-poison"


def build_pair(cfg, bc, work):
    """(output of the good build, output of the bad build) for the module `bc`, or None when something did not build."""
    t = li.Llvm("tool", cfg["bin"])
    mode, llc_opt, pass_name = cfg["mode"], cfg["llc_opt"], cfg["pass"]
    llc_cpu = cfg["llc_cpu"]
    pic = ["-relocation-model=pic", "-filetype=obj"]
    good_o, bad_o = os.path.join(work, "good.o"), os.path.join(work, "bad.o")
    if mode == "off":
        c1, _, _ = t.run("llc", llc_opt, cfg["off_flag"], *pic, bc, "-o", good_o, timeout=60)
        c2, _, _ = t.run("llc", llc_opt, *pic, bc, "-o", bad_o, timeout=60)
    elif mode == "alone":
        post = os.path.join(work, "post.bc")
        c0, _, _ = t.run("opt", "-passes=" + pass_name, bc, "-o", post, timeout=60)
        if c0:
            return None
        c1, _, _ = t.run("llc", llc_opt, *pic, bc, "-o", good_o, timeout=60)
        c2, _, _ = t.run("llc", llc_opt, *pic, post, "-o", bad_o, timeout=60)
    else:
        opt = os.path.join(work, "opt.bc")
        c0, _, _ = t.run("opt", f"-passes={cfg['pipeline']}", bc, "-o", opt, timeout=60)
        if c0:
            return None
        c1, _, _ = t.run("llc", "-O0", *llc_cpu, *pic, bc, "-o", good_o, timeout=60)
        c2, _, _ = t.run("llc", llc_opt, *llc_cpu, *pic, opt, "-o", bad_o, timeout=60)
    if c1 or c2:
        return None
    flags = cfg["flags"]
    return (
        li.build_and_run(flags, cfg["src"], work, "good", good_o),
        li.build_and_run(flags, cfg["src"], work, "bad", bad_o),
    )


def check(cfg_path, bc):
    cfg = json.load(open(cfg_path))
    if time.time() > cfg["deadline"]:
        sys.exit(1)
    work = tempfile.mkdtemp(prefix="lr_", dir=cfg["work"])
    try:
        pair = build_pair(cfg, os.path.abspath(bc), work)
    finally:
        shutil.rmtree(work, ignore_errors=True)
    if not pair or pair[0][0] is None or pair[1][0] is None:
        sys.exit(1)
    sys.exit(0 if pair[0] == tuple(cfg["expect"]) and pair[1] != pair[0] else 1)


def main():
    if len(sys.argv) >= 4 and sys.argv[1] == "--check":
        check(sys.argv[2], sys.argv[3])
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--src", required=True)
    ap.add_argument("--flags", default="")
    ap.add_argument("--work", default=None)
    ap.add_argument("--cache", default=os.path.expanduser("~/.cache/llvm-issue"))
    ap.add_argument("--budget", type=int, default=900)
    a = ap.parse_args()

    md_path = os.path.join(a.dir, "LLVM_ISSUE.md")
    if not os.path.exists(md_path):
        li.result("n/a", "no LLVM_ISSUE.md: the culprit is not a verified LLVM pass")
    md = open(md_path).read()
    first = md.splitlines()[0] if md else ""
    if "STILL WRONG ON TRUNK" not in first and "FIXED ON TRUNK" not in first:
        li.result("todo", "LLVM_ISSUE.md has no verdict yet")
    use_trunk = "STILL WRONG ON TRUNK" in first
    cul = li.read_cfg(os.path.join(a.dir, "culprit.txt"))
    n, pass_name = int(cul["n"]), cul["name"]
    flags = a.flags.split()

    code, vv, _ = li.sh(["rustc", "+nightly", "-vV"])
    m = re.search(r"LLVM version: (\d+)\.", vv)
    if not m:
        li.result("todo", "cannot read the LLVM version of rustc +nightly")
    major = int(m.group(1))
    b = li.apt_llvm(major + 1 if use_trunk else major, use_trunk, a.cache)
    if not b:
        li.result("todo", "could not get the LLVM binaries from apt.llvm.org")
    tool = li.Llvm("tool", b[0])
    if not os.path.exists(os.path.join(b[0], "llvm-reduce")):
        li.result("todo", "llvm-reduce is not in the LLVM package")

    work = os.path.abspath(a.work or os.path.join(a.dir, ".llvm_reduce_work"))
    shutil.rmtree(work, ignore_errors=True)
    os.makedirs(work)
    src = os.path.join(work, "reduced.rs")
    shutil.copy(a.src, src)
    level = re.search(r"opt-level=(\w)", a.flags)
    lv = level.group(1) if level else "0"
    llc_opt = "-O" + {"0": "0", "1": "1", "2": "2", "3": "3", "s": "2", "z": "2"}.get(lv, "2")
    cpu = re.search(r"target-cpu=(\S+)", a.flags)
    base = ["rustc", "+nightly", src, *flags, "-C", "codegen-units=1", "-A", "warnings"]
    cfg = {
        "bin": b[0], "src": src, "flags": flags, "work": work, "pass": pass_name, "llc_opt": llc_opt,
        "llc_cpu": [f"-mcpu={cpu.group(1)}"] if cpu else [], "pipeline": li.pipeline_name(lv),
        "deadline": time.time() + a.budget, "off_flag": li.CODEGEN_PASS_OFF.get(pass_name, ""),
    }
    pre = os.path.join(work, "pre.bc")
    if "miscompilation of the -" in md:
        cfg["mode"] = "pipeline"
        c, _, err = li.sh(base + ["-C", "no-prepopulate-passes", "--emit=llvm-bc", "-o", pre])
    else:
        cfg["mode"] = "off" if cfg["off_flag"] else "alone"
        c, _, err = li.sh(base + ["-C", f"llvm-args=-opt-bisect-limit={n - 1}", "--emit=llvm-bc", "-o", pre])
    if c != 0 or not os.path.exists(pre):
        li.result("todo", "rustc could not emit the bitcode: " + err.strip()[-150:])

    cfg_path = os.path.join(work, "cfg.json")
    json.dump(cfg, open(cfg_path, "w"))
    expect = check_start(cfg, pre)
    if expect is None:
        li.result("todo", "the unreduced module does not show the difference any more (outdated LLVM_ISSUE.md)")
    cfg["expect"] = list(expect)
    json.dump(cfg, open(cfg_path, "w"))
    test = os.path.join(work, "test.sh")
    open(test, "w").write(f'#!/bin/bash\nexec python3 -I "{os.path.abspath(__file__)}" --check "{cfg_path}" "$1"\n')
    os.chmod(test, 0o755)
    out_bc = os.path.join(work, "reduced.bc")
    c, _, err = tool.run("llvm-reduce", f"--test={test}", "--abort-on-invalid-reduction", f"--skip-delta-passes={UB_PRONE_PASSES}", pre, "-o", out_bc, timeout=a.budget + 600)
    if not os.path.exists(out_bc):
        li.result("todo", "llvm-reduce produced nothing: " + " ".join(err.split())[-150:])
    ll = os.path.join(a.dir, "LLVM_REDUCED.ll")
    c, _, _ = tool.run("llvm-dis", out_bc, "-o", ll)
    if c != 0:
        li.result("todo", "llvm-dis failed on the reduced module")
    ir = re.sub(r"^; ModuleID = .*\n", "", open(ll).read(), flags=re.M)
    before = int(tool.run("llvm-dis", pre, "-o", "-")[1].count("\n"))
    lines = ir.count("\n")
    block = ir if lines <= li.IR_INLINE_LINES else "\n".join(ir.splitlines()[:li.IR_INLINE_LINES]) + "\n; ... (the rest is in LLVM_REDUCED.ll)"
    section = (
        f"{MARK}\n\n`llvm-reduce` ({tool.version if hasattr(tool, 'version') else ''}) shrank the module (the build without the pass must keep printing the original output) from {before} to {lines} lines "
        f"while the two builds still print different output. It may rely on undefined behaviour the original did not: check it on https://alive2.llvm.org/ce/.\n\n"
        f"```llvm\n{block}\n```\n"
    )
    if MARK in md:
        md = md[: md.index(MARK)].rstrip() + "\n\n"
    # the section goes before "### Meta" so the file keeps its shape
    if "### Meta" in md:
        md = md.replace("### Meta", section + "\n### Meta", 1)
    else:
        md = md.rstrip() + "\n\n" + section
    open(md_path, "w").write(md)
    shutil.rmtree(work, ignore_errors=True)
    li.result("done", f"IR reduced from {before} to {lines} lines (LLVM_REDUCED.ll)")


def check_start(cfg, bc):
    work = tempfile.mkdtemp(prefix="lr_", dir=cfg["work"])
    try:
        pair = build_pair(cfg, bc, work)
    finally:
        shutil.rmtree(work, ignore_errors=True)
    if pair and pair[0][0] == 0 and pair[1][0] is not None and pair[0] != pair[1]:
        return pair[0]
    return None


if __name__ == "__main__":
    main()
