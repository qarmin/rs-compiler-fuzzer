#!/usr/bin/env python3
"""Verify that an LLVM pass is to blame for a wrong-code finding and write LLVM_ISSUE.md.

Called by `rs-compiler-fuzzer triage` (step `llvm_issue`) after the pass bisection found an LLVM pass (culprit.txt).

    llvm_issue.py FINDING_DIR --src reduced.rs --flags "-C opt-level=2" [--work DIR] [--cache DIR]

What it does (nothing here is a custom minimiser or custom LLVM tooling, only rustc and the LLVM binaries of apt.llvm.org):
 1. rustc +nightly <flags> -C codegen-units=1 -C llvm-args=-opt-bisect-limit=N-1 --emit=llvm-bc: the module right before pass N
 2. for LLVM M-1 (older release), M (the release rustc uses, release branch snapshot) and M+1 (trunk snapshot):
    `opt -passes=<pass>` on that module, `llc` of the module with and without the pass, linked into the program by rustc
    (through a linker wrapper that swaps the object file) and run. A different output = the pass of that LLVM version is wrong.
 3. writes LLVM_ISSUE.md (rust-lang style, with the IR of the function) and prints one line: `<done|n/a|todo>|<note>`.

The LLVM binaries are downloaded (small .deb packages, extracted with dpkg -x, no root) into the cache directory.
"""
import argparse
import glob
import gzip
import os
import re
import shutil
import subprocess
import sys
import urllib.request

ARCHIVE = "https://apt.llvm.org"
UBUNTU = "http://archive.ubuntu.com/ubuntu"
RUN_TIMEOUT = 30
IR_INLINE_LINES = 300


def sh(cmd, timeout=600, env=None, cwd=None):
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, env=env, cwd=cwd)
        return r.returncode, r.stdout, r.stderr
    except subprocess.TimeoutExpired:
        return 124, "", "timeout"
    except FileNotFoundError as e:
        return 127, "", str(e)


def result(status, note):
    print(f"{status}|{note.replace('|', '/').replace(chr(10), ' ')}")
    sys.exit(0)


def read_cfg(path):
    out = {}
    if os.path.exists(path):
        for line in open(path, encoding="utf-8", errors="replace"):
            if "=" in line:
                k, v = line.rstrip("\n").split("=", 1)
                out[k] = v
    return out


def codename():
    for line in open("/etc/os-release"):
        if line.startswith("VERSION_CODENAME="):
            return line.split("=", 1)[1].strip()
    return "noble"


def fetch(url, dest):
    with urllib.request.urlopen(url, timeout=120) as r, open(dest, "wb") as f:
        shutil.copyfileobj(r, f)


def packages(url):
    try:
        with urllib.request.urlopen(url, timeout=60) as r:
            text = gzip.decompress(r.read()).decode("utf-8", "replace")
    except Exception:
        return {}
    pk = {}
    for block in text.split("\n\n"):
        f = dict(l.split(": ", 1) for l in block.splitlines() if ": " in l and not l.startswith(" "))
        if "Package" in f:
            pk[f["Package"]] = f
    return pk


def snapshot(version):
    """'1:24~++20261102085133+cfc479d87d4e-1...' -> 'snapshot 2026-11-02, cfc479d87d4e'."""
    m = re.search(r"\+\+(\d{4})(\d{2})(\d{2})\d*\+([0-9a-f]+)", version)
    return f"snapshot {m.group(1)}-{m.group(2)}-{m.group(3)}, {m.group(4)}" if m else ""


def apt_llvm(major, trunk, cache):
    """bin directory of an extracted apt.llvm.org package (llvm-<major> + libllvm<major>), or None."""
    cn = codename()
    root = os.path.join(cache, f"llvm-{major}-{'trunk' if trunk else 'branch'}-{cn}")
    bindir = os.path.join(root, "usr", "lib", f"llvm-{major}", "bin")
    stamp = os.path.join(root, "STAMP")
    suffix = "" if trunk else f"-{major}"
    idx = packages(f"{ARCHIVE}/{cn}/dists/llvm-toolchain-{cn}{suffix}/main/binary-amd64/Packages.gz")
    want = [p for p in (f"libllvm{major}", f"llvm-{major}") if p in idx]
    if len(want) < 2:
        return None
    version = idx[f"llvm-{major}"]["Version"]
    if os.path.exists(stamp) and open(stamp).read().strip() == version and os.path.exists(os.path.join(bindir, "opt")):
        return bindir, snapshot(version)
    shutil.rmtree(root, ignore_errors=True)
    os.makedirs(root)
    for p in want:
        deb = os.path.join(root, p + ".deb")
        try:
            fetch(f"{ARCHIVE}/{cn}/{idx[p]['Filename']}", deb)
        except Exception as e:
            print(f"download of {p} failed: {e}", file=sys.stderr)
            return None
        sh(["dpkg", "-x", deb, root])
        os.remove(deb)
    libdir = os.path.join(root, "usr", "lib", "x86_64-linux-gnu")
    env = dict(os.environ, LD_LIBRARY_PATH=libdir)
    # libz3 is the one dependency the runners do not always have
    _, out, _ = sh(["ldd", os.path.join(bindir, "opt")], env=env)
    if "libz3" in out and "not found" in "".join(l for l in out.splitlines() if "libz3" in l):
        for comp in ("main", "universe"):
            u = packages(f"{UBUNTU}/dists/{cn}/{comp}/binary-amd64/Packages.gz").get("libz3-4")
            if u:
                deb = os.path.join(root, "z3.deb")
                fetch(f"{UBUNTU}/{u['Filename']}", deb)
                sh(["dpkg", "-x", deb, root])
                os.remove(deb)
                break
    open(stamp, "w").write(version)
    return bindir, snapshot(version)


class Llvm:
    def __init__(self, label, bindir, snap=""):
        self.label = label
        self.snap = snap
        self.bin = bindir
        libdir = os.path.join(os.path.dirname(os.path.dirname(bindir)), "x86_64-linux-gnu")
        self.env = dict(os.environ, LD_LIBRARY_PATH=libdir + ":" + os.environ.get("LD_LIBRARY_PATH", ""))
        _, out, _ = sh([os.path.join(bindir, "opt"), "--version"], env=self.env)
        m = re.search(r"LLVM version (\S+)", out)
        self.version = (m.group(1) if m else "?") + (f", {snap}" if snap else "")

    def run(self, tool, *args, timeout=300):
        return sh([os.path.join(self.bin, tool), *args], timeout=timeout, env=self.env)


WRAPPER = """#!/bin/bash
args=()
for a in "$@"; do
  case "$a" in
    *-cgu.0.rcgu.o) args+=("$OBJ");;
    *) args+=("$a");;
  esac
done
exec cc "${args[@]}"
"""


def build_and_run(rustc_flags, src, work, tag, obj):
    """rustc builds the program, but its object file is replaced by `obj`; returns (exit code, stdout) of the program."""
    wrapper = os.path.join(work, "linkwrap.sh")
    if not os.path.exists(wrapper):
        open(wrapper, "w").write(WRAPPER)
        os.chmod(wrapper, 0o755)
    exe = os.path.join(work, f"exe_{tag}")
    env = dict(os.environ, OBJ=obj)
    code, _, err = sh(["rustc", "+nightly", src, *rustc_flags, "-C", "codegen-units=1", "-C", f"linker={wrapper}", "-A", "warnings", "-o", exe], env=env)
    if code != 0:
        return None, "link failed: " + " ".join(err.split())[:300]
    code, out, _ = sh([exe], timeout=RUN_TIMEOUT)
    return code, out.strip()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--src", required=True)
    ap.add_argument("--flags", default="")
    ap.add_argument("--work", default=None)
    ap.add_argument("--cache", default=os.path.expanduser("~/.cache/llvm-issue"))
    a = ap.parse_args()

    out_md = os.path.join(a.dir, "LLVM_ISSUE.md")
    cul = read_cfg(os.path.join(a.dir, "culprit.txt"))
    if os.path.exists(out_md):
        os.remove(out_md)
    if cul.get("kind") != "llvm" or int(cul.get("n", "0") or 0) <= 0:
        result("n/a", "the culprit is not a single LLVM pass")
    if os.path.exists(os.path.join(a.dir, "signature.txt")) and open(os.path.join(a.dir, "signature.txt")).read().strip():
        result("n/a", "a compiler crash, not a wrong-code finding (only wrong code is verified)")
    n, pass_name = int(cul["n"]), cul["name"]
    flags = a.flags.split()
    if any("target" in f for f in flags if f.startswith("--target")) or "--target" in flags:
        result("n/a", "cross-compiled finding: the program cannot be linked and run here")

    work = os.path.abspath(a.work or os.path.join(a.dir, ".llvm_issue_work"))
    shutil.rmtree(work, ignore_errors=True)
    os.makedirs(work)
    src = os.path.join(work, "reduced.rs")
    shutil.copy(a.src, src)
    os.makedirs(a.cache, exist_ok=True)

    code, vv, _ = sh(["rustc", "+nightly", "-vV"])
    m = re.search(r"LLVM version: (\d+)\.", vv)
    if not m:
        result("todo", "cannot read the LLVM version of rustc +nightly")
    major = int(m.group(1))
    rustc_version = vv

    tools = []
    for label, mj, trunk in ((f"LLVM {major - 1} (release branch)", major - 1, False), (f"LLVM {major} (release branch, what rustc uses)", major, False), (f"LLVM {major + 1} (trunk)", major + 1, True)):
        b = apt_llvm(mj, trunk, a.cache)
        if b:
            tools.append(Llvm(label, *b))
    ref = next((t for t in tools if t.label.startswith(f"LLVM {major} ")), None)
    if ref is None:
        result("todo", f"could not get LLVM {major} from apt.llvm.org")

    # the module right before the pass, from rustc itself
    stem = os.path.join(work, "pre")
    base = ["rustc", "+nightly", src, *flags, "-C", "codegen-units=1", "-A", "warnings"]
    code, _, err = sh(base + ["-C", f"llvm-args=-opt-bisect-limit={n - 1}", "--emit=llvm-bc", "-o", stem + ".bc"])
    if code != 0 or not os.path.exists(stem + ".bc"):
        result("todo", "rustc could not emit the bitcode before the pass: " + err.strip()[-150:])
    code, _, err = sh(base + ["-C", f"llvm-args=-opt-bisect-limit={n}", "-o", os.path.join(work, "listing")])
    m = re.search(rf"BISECT: running pass \({n}\) (\S+) on (.*)", err)
    raw_target = m.group(2).strip() if m else ""
    if m and m.group(1) != pass_name:
        result("todo", f"pass {n} is `{m.group(1)}` now, not `{pass_name}`: the bisection is outdated")
    is_function = bool(raw_target) and not raw_target.startswith("[")

    level = re.search(r"opt-level=(\w)", a.flags)
    llc_opt = "-O" + {"0": "0", "1": "1", "2": "2", "3": "3", "s": "2", "z": "2"}.get(level.group(1) if level else "0", "2")
    rows = []
    for t in tools:
        row = {"llvm": t, "pre": None, "post": None, "verdict": "", "note": ""}
        rows.append(row)
        pre_obj, post_obj, post_bc = (os.path.join(work, f"{t.label.split()[1]}_{x}") for x in ("pre.o", "post.o", "post.bc"))
        c, _, e = t.run("opt", "-passes=" + pass_name, stem + ".bc", "-o", post_bc)
        if c != 0:
            row["verdict"], row["note"] = "n/a", "opt: " + (e.strip().splitlines() or ["failed"])[-1][:160]
            continue
        c1, _, e1 = t.run("llc", llc_opt, "-relocation-model=pic", "-filetype=obj", stem + ".bc", "-o", pre_obj)
        c2, _, e2 = t.run("llc", llc_opt, "-relocation-model=pic", "-filetype=obj", post_bc, "-o", post_obj)
        if c1 or c2:
            row["verdict"], row["note"] = "n/a", "llc: " + ((e1 or e2).strip().splitlines() or ["failed"])[-1][:160]
            continue
        tag = re.sub(r"\W", "", t.label.split()[1])
        row["pre"] = build_and_run(flags, src, work, tag + "pre", pre_obj)
        row["post"] = build_and_run(flags, src, work, tag + "post", post_obj)
        if row["pre"][0] is None or row["post"][0] is None:
            row["verdict"], row["note"] = "n/a", (row["pre"][1] if row["pre"][0] is None else row["post"][1])
        else:
            row["verdict"] = "WRONG CODE" if row["pre"] != row["post"] else "same output"

    row_ref = next(r for r in rows if r["llvm"] is ref)
    row_trunk = rows[-1] if tools[-1].label.endswith("(trunk)") else None
    if row_ref["verdict"] != "WRONG CODE":
        result("done", f"not verified: with {ref.label} ({ref.version}) the pass `{pass_name}` gives: {row_ref['verdict']} {row_ref['note']}".strip())

    # the IR of the function (or of the module when the pass works on more than a function)
    ir_note = "the whole module"
    ll = os.path.join(work, "pre.ll")
    code, _, _ = ref.run("llvm-dis", stem + ".bc", "-o", ll)
    ir_path = ll
    if is_function and code == 0:
        c, _, _ = ref.run("llvm-extract", "-S", f"--func={raw_target}", stem + ".bc", "-o", os.path.join(work, "fn.ll"))
        if c == 0 and os.path.exists(os.path.join(work, "fn.ll")):
            ir_path, ir_note = os.path.join(work, "fn.ll"), "the function `" + cul.get("target", raw_target) + "`"
    ir = re.sub(r"^; ModuleID = .*\n", "", open(ir_path).read(), flags=re.M) if code == 0 else ""
    ir_lines = ir.count("\n")
    ir_block = ir if ir_lines <= IR_INLINE_LINES else f"; {ir_lines} lines, too long to paste: attach the file produced with the command above\n" + "\n".join(ir.splitlines()[:40]) + "\n; ..."

    fixed_on_trunk = bool(row_trunk and row_trunk["verdict"] == "same output")
    rel = ref.label.split(" (")[0]
    if fixed_on_trunk:
        status_line = f"FIXED ON TRUNK ({row_trunk['llvm'].version}): do not file, maybe ask for a backport to the release branch"
        intro = "It works on trunk, so this is only interesting as a backport request."
    elif row_trunk and row_trunk["verdict"] == "WRONG CODE":
        status_line = f"STILL WRONG ON TRUNK ({row_trunk['llvm'].version}): worth filing"
        intro = f"Present on trunk ({row_trunk['llvm'].version}) too."
    else:
        status_line = "trunk was not checked"
        intro = "Trunk was not tested."
    title = f"[{pass_name}] miscompilation: wrong output after `{pass_name}` ({rel})"

    table = ["| LLVM | version | program with the IR before the pass | with `opt -passes=" + pass_name + "` | verdict |", "|---|---|---|---|---|"]
    for r in rows:
        t = r["llvm"]
        show = lambda x: "-" if not x or x[0] is None else f"`{x[1][:60]}`" + ("" if x[0] == 0 else f" (exit {x[0]})")
        table.append(f"| {t.label} | {t.version} | {show(r['pre'])} | {show(r['post'])} | {r['verdict']}{(' - ' + r['note']) if r['note'] else ''} |")

    rust_src = open(a.src).read().rstrip()
    bad_cmd = "rustc +nightly " + " ".join(flags) + " reduced.rs"
    md = f"""<!-- {status_line} -->
<!-- title: {title} -->
# {title}

`opt -passes={pass_name}` ({ref.label.split(' (')[0]}) changes the behaviour of the program below. The program is compiled with `llc` in both cases, only the pass differs. {intro}

Found as a miscompilation of rustc (it uses {ref.label.split(' (')[0]}); bisected with `-C llvm-args=-opt-bisect-limit`: pass {n} of {cul.get('total', '?')}, `{pass_name}` on `{cul.get('target', raw_target)}`.

### Reproducer

The IR is {ir_note}, as it is right before the pass (`rustc` with `-C llvm-args=-opt-bisect-limit={n - 1} --emit=llvm-bc`, then `llvm-dis`):

```llvm
{ir_block}
```

```
opt -passes={pass_name} pre.bc -S -o post.ll
```

The whole program (it prints a different value after the pass):

```rust
{rust_src}
```

```
{bad_cmd} -C llvm-args=-opt-bisect-limit={n - 1}; ./reduced   # {row_ref['pre'][1][:80] if row_ref['pre'] else ''}
{bad_cmd} -C llvm-args=-opt-bisect-limit={n}; ./reduced       # {row_ref['post'][1][:80] if row_ref['post'] else ''}
```

### Results

{chr(10).join(table)}

{status_line}.

### Meta

`rustc +nightly --version --verbose`:
```
{rustc_version.strip()}
```
Binaries: LLVM snapshots from https://apt.llvm.org (the version column is `opt --version`).

Problem was found by rust fuzzer(created by llm) - https://github.com/qarmin/rs-compiler-fuzzer. The IR was not reduced further: run `llvm-reduce` on it before filing if it is long. Check it by hand on https://alive2.llvm.org/ce/ (paste the IR before and after the pass).
"""
    open(out_md, "w").write(md)
    shutil.rmtree(work, ignore_errors=True)
    result("done", f"{pass_name}: " + status_line)


if __name__ == "__main__":
    main()
