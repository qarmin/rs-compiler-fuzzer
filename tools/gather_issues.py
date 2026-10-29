#!/usr/bin/env python3
"""Collects a corpus of known compiler problems from GitHub issues, without any LLM: issues with the chosen labels are fetched in
batches, every Rust code block is compiled with the default arguments and with the arguments the reporter wrote down, and what
looks like a compiler problem (ICE, crash by signal, hang, LLVM error; with --run also a program crash and different output at
different optimisation levels) is stored as NAME.rs + NAME.txt, the pair format of `rs-compiler-fuzzer import`.

  tools/gather_issues.py fetch  [--label I-ICE ...] [--batch 50]   # next batch of issues -> OUT/manifest.jsonl, OUT/found/
  tools/gather_issues.py crawl  [--run]                            # ALL issues, open first, then closed; resumable (cursor.json)
  tools/gather_issues.py verify [--toolchain nightly]              # run everything in OUT/found/ again, update the status
  tools/gather_issues.py stats

Already processed issues are skipped, so `fetch` can be repeated until the labels are exhausted. --run EXECUTES the programs from the
issues (limited by timeout and memory): use a container or a VM for that. Needs `gh` (authenticated) and rustc.
"""
import argparse, json, os, re, resource, shlex, signal, subprocess, sys, tempfile, time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

REPO = "rust-lang/rust"
LABELS = ["I-ICE", "I-crash", "I-hang", "I-llvm-error", "I-miscompile", "I-unsound"]
# options that take a value in the next word (`-C opt-level=3`); the joined form (`-Copt-level=3`) needs no entry
WITH_VALUE = {"-C", "-Z", "-A", "-W", "-D", "-F", "--cfg", "--edition", "--crate-type", "--emit", "--cap-lints", "--check-cfg", "--crate-name", "--error-format"}
JOINED = ("-C", "-Z", "-A", "-W", "-D", "-F")
SKIP_OPTIONS = {"-o", "--out-dir", "--target", "--extern", "-L", "--sysroot", "-l", "--print", "--explain", "--version", "-V", "--help", "-h", "-v"}
ITEM = re.compile(r"\b(fn|struct|enum|trait|impl|mod|type|const|static|union|use|macro_rules!)\b")
FENCE = re.compile(r"^[ \t]*(`{3,}|~{3,})[ \t]*([\w,+-]*)[^\n]*\n(.*?)^[ \t]*\1[ \t]*$", re.S | re.M)
RUST_TAGS = {"", "rust", "rs", "no_run", "should_panic", "ignore", "edition2018", "edition2021", "edition2024", "compile_fail"}
# programs touching the file system, processes, the network or raw syscalls are never compiled or run: they come from strangers
DANGER = re.compile(r"\b(std::fs|fs::|File::|OpenOptions|std::process|process::|Command::new|std::net|TcpStream|UdpSocket|TcpListener|std::os::unix|libc::|remove_file|remove_dir|unlink|rmdir|rename|chmod|chown|symlink|system|execv?p?e?|fork|kill|set_var|remove_var|set_current_dir|current_exe|include_str|include_bytes|syscall|mmap|ptrace)\b|asm!|include!|/etc/|/dev/|/proc/|/home|\brm\s+-")
ICE_MARKERS = ("internal compiler error", "the compiler unexpectedly panicked", "LLVM ERROR", "error: could not compile", "rustc-ice")


_calls = 0


def wait_for_quota(minimum=200):
    """Sleeps until the core rate limit has room again (`gh api rate_limit` itself is free)."""
    while True:
        r = subprocess.run(["gh", "api", "rate_limit", "--jq", ".resources.core | [.remaining, .reset] | @tsv"], capture_output=True, text=True)
        try:
            remaining, reset = map(int, r.stdout.split())
        except ValueError:
            return
        if remaining >= minimum:
            return
        pause = max(10, reset - int(time.time()) + 5)
        print(f"rate limit: {remaining} calls left, sleeping {pause}s", file=sys.stderr, flush=True)
        time.sleep(pause)


def gh(path, **params):
    global _calls
    _calls += 1
    if _calls % 25 == 1:
        wait_for_quota()
    cmd = ["gh", "api", "-X", "GET", path] + [a for k, v in params.items() for a in ("-f", f"{k}={v}")]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0 and ("rate limit" in r.stderr.lower() or "403" in r.stderr):
        wait_for_quota(minimum=1000)
        r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"gh api {path} failed: {r.stderr.strip()}")
    return json.loads(r.stdout)


def fetch_issues(repo, label, state, page, oldest_first, per_page=100):
    extra = {"labels": label} if label else {}
    items = gh(f"repos/{repo}/issues", **extra, state=state, per_page=per_page, page=page, sort="created", direction="asc" if oldest_first else "desc")
    return [i for i in items if "pull_request" not in i], len(items)


def with_main(code):
    """A snippet becomes a program: items only get an empty `main`, bare statements are wrapped into one."""
    if re.search(r"\bfn\s+main\b", code):
        return code
    if re.search(r"^(\s*(pub\s+)?(unsafe\s+)?(extern\s+\"C\"\s+)?(async\s+)?(fn|struct|enum|trait|impl|mod|type|const|static|union|use|macro_rules!)\b|\s*#!?\[)", code, re.M) and not re.search(r"^\s*let\b", code, re.M):
        return code + "fn main() {}\n"
    inner = [l for l in code.splitlines() if not l.startswith("#![")]
    return "\n".join(l for l in code.splitlines() if l.startswith("#![")) + "\nfn main() {\n" + "\n".join(inner) + "\n}\n"


GRAPHQL = """query($owner: String!, $name: String!, $states: [IssueState!], $after: String) {
  repository(owner: $owner, name: $name) {
    issues(first: 100, after: $after, states: $states, orderBy: {field: CREATED_AT, direction: ASC}) {
      pageInfo { hasNextPage endCursor }
      nodes { number title url state body comments { totalCount } labels(first: 20) { nodes { name } } }
    }
  }
}"""


def fetch_issues_cursor(repo, state, after):
    """One page of 100 issues (REST-shaped dicts) after the GraphQL cursor `after`; the REST `page` parameter stops at 10000 items."""
    global _calls
    _calls += 1
    if _calls % 25 == 1:
        wait_for_quota()
    owner, name = repo.split("/")
    cmd = ["gh", "api", "graphql", "-f", f"query={GRAPHQL}", "-F", f"owner={owner}", "-F", f"name={name}", "-f", f"states={state.upper()}"]
    if after:
        cmd += ["-f", f"after={after}"]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        if "rate limit" in r.stderr.lower() or "RATE_LIMIT" in r.stdout:
            wait_for_quota(minimum=500)
        raise RuntimeError(f"graphql failed: {(r.stderr or r.stdout).strip()[:300]}")
    data = json.loads(r.stdout)["data"]["repository"]["issues"]
    issues = [{"number": n["number"], "title": n["title"], "html_url": n["url"], "state": n["state"].lower(), "body": n["body"], "comments": n["comments"]["totalCount"], "labels": n["labels"]["nodes"]} for n in data["nodes"]]
    return issues, data["pageInfo"]["endCursor"], data["pageInfo"]["hasNextPage"]


def code_blocks(text):
    out = []
    for m in FENCE.finditer((text or "").replace("\r\n", "\n")):
        tags = {t.strip() for t in m.group(2).lower().split(",")}
        code = m.group(3)
        looks_like_output = re.search(r"^\s*(error|warning)(\[|:)|^\$ |^\s*-->|^\s*\d+\s*\|", code, re.M)
        statements = re.search(r"^\s*let\b.*;\s*$", code, re.M)
        if tags <= RUST_TAGS and (ITEM.search(code) or statements) and not looks_like_output:
            prog = with_main(code.rstrip() + "\n")
            if prog not in out:
                out.append(prog)
    return out


def parse_flags(words):
    """Compiler options of a command line (sources, outputs, targets and other things that change the environment dropped)."""
    flags, i = [], 0
    while i < len(words):
        w = words[i]
        if w in SKIP_OPTIONS or (w.startswith("--") and "=" in w and w.split("=")[0] in SKIP_OPTIONS):
            i += 2 if w in SKIP_OPTIONS and w not in ("-v", "-V", "-h") and "=" not in w else 1
            continue
        if w in WITH_VALUE and i + 1 < len(words):
            flags += [w, words[i + 1]]
            i += 2
            continue
        if w in ("-O", "-g"):
            flags.append(w)
        elif w.startswith(JOINED) and len(w) > 2 or w.startswith(("--edition=", "--cfg=", "--crate-type=", "--emit=", "--cap-lints=", "--check-cfg=")):
            flags.append(w)
        i += 1
    return flags


def user_flags(code, texts):
    """Flags found next to the code: `//@ compile-flags:` lines, `rustc ...` command lines in the issue text."""
    found = []
    for m in re.finditer(r"^\s*//@\s*compile-flags:\s*(.+)$", code, re.M):
        found.append(parse_flags(shlex_split(m.group(1))))
    for m in re.finditer(r"^\s*//@\s*edition:\s*(\d+)", code, re.M):
        found.append(["--edition", m.group(1)])
    for text in texts:
        for m in re.finditer(r"(?:^|[`$>]\s*|\s)(?:RUSTFLAGS=\S+\s+)?(?:cargo\s+\+?\S*\s*)?rustc\s+(\+\S+\s+)?([^\n`]+)", text or "", re.M):
            words = shlex_split(m.group(2))
            f = parse_flags(words)
            if f:
                found.append(f)
        for m in re.finditer(r"(?:compiler flags|flags)\s*:\s*`?([^\n`]+)", text or "", re.I):
            f = parse_flags(shlex_split(m.group(1)))
            if f:
                found.append(f)
    uniq = []
    for f in found:
        if f and f not in uniq:
            uniq.append(f)
    return uniq


def shlex_split(s):
    try:
        return shlex.split(s)
    except ValueError:
        return s.split()


MEM_GB = 3
MIN_AVAILABLE_GB = 4


def mem_available_gb():
    for line in open("/proc/meminfo"):
        if line.startswith("MemAvailable:"):
            return int(line.split()[1]) / (1 << 20)
    return 1e9


def wait_for_memory():
    """Does not start another compiler while the machine is short of RAM (the jobs of one run add up)."""
    while mem_available_gb() < MIN_AVAILABLE_GB:
        time.sleep(2)


def limits(mem_gb):
    def apply():
        resource.setrlimit(resource.RLIMIT_AS, (mem_gb << 30, mem_gb << 30))
        resource.setrlimit(resource.RLIMIT_FSIZE, (256 << 20, 256 << 20))
        resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
        os.setsid()
        try:
            with open("/proc/self/oom_score_adj", "w") as f:
                f.write("1000")
        except OSError:
            pass
    return apply


def run(cmd, timeout, cwd, mem_gb=None):
    """Runs `cmd` in its own process group with an address-space limit; on timeout the whole group is killed (rustc leaves linkers behind)."""
    wait_for_memory()
    start = time.time()
    try:
        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, cwd=cwd, preexec_fn=limits(mem_gb or MEM_GB), errors="replace")
    except OSError as e:
        return None, "", f"cannot start: {e}", time.time() - start, False
    try:
        out, err = proc.communicate(timeout=timeout)
        return proc.returncode, out, err, time.time() - start, False
    except subprocess.TimeoutExpired:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        out, err = proc.communicate()
        return None, "", err or "", time.time() - start, True


def classify_compile(code, flags, toolchain, timeout, workdir):
    """(status, detail, binary-output) of one compilation: ice | signal | hang | llvm | error | ok | run-crash | opt-diff."""
    src = Path(workdir) / "main.rs"
    src.write_text(code)
    cmd = ["rustc", f"+{toolchain}", str(src), "-o", str(Path(workdir) / "bin"), "--cap-lints", "allow", *flags]
    rc, out, err, secs, hung = run(cmd, timeout, workdir)
    if hung:
        return "hang", f"compiler still running after {timeout}s", None
    if rc is not None and rc < 0:
        return "signal", f"rustc killed by signal {-rc}", None
    if "LLVM ERROR" in err:
        return "llvm", first_line(err, ("LLVM ERROR",)), None
    if "(signal:" in err:
        return "signal", first_line(err, ("signal",)), None
    if "internal compiler error" in err or "unexpectedly panicked" in err or "error: the compiler" in err:
        return "ice", first_line(err, ("internal compiler error", "panicked", "error: the compiler")), None
    if rc != 0:
        return "error", first_line(err, ("error",)), None
    return "ok", "", None


def first_line(text, needles):
    for l in text.splitlines():
        if any(n in l for n in needles):
            return l.strip()[:200]
    return text.strip().splitlines()[0][:200] if text.strip() else ""


def run_binary(workdir, timeout):
    rc, out, err, secs, hung = run([str(Path(workdir) / "bin")], timeout, workdir, mem_gb=2)
    return rc, out, hung


def opt_diff(code, flags, toolchain, timeout, run_timeout):
    """Output at -C opt-level=0 against 3 (the issue's own flags kept): ('opt-diff' | 'run-crash' | None, detail)."""
    base = []
    for i, f in enumerate(flags):
        after_c = i > 0 and flags[i - 1] == "-C"
        if f != "-O" and not f.startswith("-Copt-level") and not (after_c and f.startswith("opt-level")) and not (f == "-C" and flags[i + 1:i + 2] and flags[i + 1].startswith("opt-level")):
            base.append(f)
    res = {}
    for name, extra in (("O0", ["-C", "opt-level=0"]), ("O3", ["-C", "opt-level=3"])):
        with tempfile.TemporaryDirectory(prefix="gather_") as d:
            st, detail, _ = classify_compile(code, base + extra, toolchain, timeout, d)
            if st != "ok":
                return (st, f"{name}: {detail}") if st in ("ice", "signal", "hang", "llvm") else (None, "")
            rc, out, hung = run_binary(d, run_timeout)
            res[name] = (rc, out, hung)
    (rc0, out0, h0), (rc3, out3, h3) = res["O0"], res["O3"]
    if h0 or h3 or rc0 is None or rc3 is None:
        return None, ""
    for name, (rc, _, _) in res.items():
        if rc is not None and rc < 0 and rc not in (-6,):
            return "run-crash", f"{name}: program killed by signal {-rc}"
    if (rc0, out0) != (rc3, out3):
        return "opt-diff", f"O0: exit {rc0}, {len(out0)} bytes of output; O3: exit {rc3}, {len(out3)} bytes"
    return None, ""


def evaluate(code, variants, args):
    """Every variant (name, flags) compiled; list of dicts."""
    results = []
    for name, flags in variants:
        with tempfile.TemporaryDirectory(prefix="gather_") as d:
            status, detail, _ = classify_compile(code, flags, args.toolchain, args.timeout, d)
        if status == "ok" and args.run:
            status, detail = opt_diff(code, flags, args.toolchain, args.timeout, args.run_timeout)
            status = status or "ok"
        results.append({"variant": name, "flags": flags, "status": status, "detail": detail})
    return results


INTERESTING = {"ice", "signal", "hang", "llvm", "run-crash", "opt-diff"}


def process(issue, args, found_dir):
    texts = [(issue.get("body") or "").replace("\r\n", "\n")]
    if args.comments and issue.get("comments", 0) > 0:
        texts += [c.get("body") or "" for c in gh(issue["comments_url"].replace("https://api.github.com/", ""), per_page=100)]
    blocks = []
    for t in texts:
        for c in code_blocks(t):
            if c not in blocks:
                blocks.append(c)
    entry = {"issue": issue["number"], "title": issue["title"], "url": issue["html_url"], "state": issue["state"], "labels": [l["name"] for l in issue["labels"]], "blocks": []}
    for k, code in enumerate(blocks[: args.max_blocks]):
        if DANGER.search(code):
            entry["blocks"].append({"index": k, "lines": code.count("\n"), "results": [{"variant": "all", "flags": [], "status": "skipped", "detail": "touches fs / process / net / syscalls"}]})
            continue
        flags_found = user_flags(code, texts)
        variants = [("default", args.default_flags.split())]
        variants += [(f"user{n}", f) for n, f in enumerate(flags_found[:4])]
        results = evaluate(code, variants, args)
        block = {"index": k, "lines": code.count("\n"), "results": results}
        for r in results:
            if r["status"] in INTERESTING:
                name = f"issue_{issue['number']}_{k}_{r['variant']}"
                (found_dir / f"{name}.rs").write_text(code)
                (found_dir / f"{name}.txt").write_text(f"rustc +{args.toolchain} {name}.rs {shlex.join(r['flags'])}\n# {issue['html_url']}: {r['status']}: {r['detail']}\n")
                r["stored"] = name
        entry["blocks"].append(block)
    return entry


def load_manifest(out):
    path = out / "manifest.jsonl"
    return [json.loads(l) for l in path.read_text().splitlines() if l.strip()] if path.exists() else []


def cmd_fetch(args):
    out = Path(args.out)
    found = out / "found"
    found.mkdir(parents=True, exist_ok=True)
    done = {e["issue"] for e in load_manifest(out)}
    todo, seen = [], set(done)
    for label in args.label or LABELS:
        page = 1
        while len(todo) < args.batch:
            items, raw = fetch_issues(args.repo, label, args.state, page, args.oldest_first)
            for i in items:
                if len(todo) < args.batch and i["number"] not in seen:
                    todo.append(i)
                    seen.add(i["number"])
            if raw < 100:
                break
            page += 1
    print(f"{len(todo)} new issue(s), compiling with {args.jobs} job(s) on {args.toolchain}", file=sys.stderr)
    with ThreadPoolExecutor(args.jobs) as pool, open(out / "manifest.jsonl", "a") as mf:
        def safe(i):
            try:
                return process(i, args, found)
            except Exception as e:
                print(f"#{i['number']}: failed: {e!r}", file=sys.stderr)
                return {"issue": i["number"], "title": i["title"], "url": i["html_url"], "state": i["state"], "labels": [], "blocks": [], "error": repr(e)}
        for entry in pool.map(safe, todo):
            mf.write(json.dumps(entry) + "\n")
            mf.flush()
            hits = [r for b in entry["blocks"] for r in b["results"] if r["status"] in INTERESTING]
            print(f"#{entry['issue']}: {len(entry['blocks'])} block(s), {len(hits)} problem(s) {[r['status'] for r in hits]}  {entry['title'][:70]}")
    return 0


def cmd_crawl(args):
    """Every issue of the repository, a page of 100 at a time (oldest first, so that page numbers stay valid while new issues arrive):
    open issues, then closed ones. The position is kept in cursor.json: stop and start again whenever you like."""
    out = Path(args.out)
    (out / "found").mkdir(parents=True, exist_ok=True)
    cursor_path = out / "cursor.json"
    cursor = json.loads(cursor_path.read_text()) if cursor_path.exists() else {}
    if "after" not in cursor:
        cursor = {"states": ["open", "closed"], "after": None, "page": 1}
    done = {e["issue"] for e in load_manifest(out)}
    args.oldest_first = True
    retries = 0
    while cursor["states"]:
        state = cursor["states"][0]
        try:
            items, end, more = fetch_issues_cursor(args.repo, state, cursor["after"])
        except RuntimeError as e:
            retries += 1
            print(f"{e} (retry {retries})", file=sys.stderr, flush=True)
            if retries > 5:
                raise
            time.sleep(30 * retries)
            continue
        retries = 0
        fresh = [i for i in items if i["number"] not in done]
        t0 = time.time()
        found = 0
        with ThreadPoolExecutor(args.jobs) as pool, open(out / "manifest.jsonl", "a") as mf:
            def safe(i):
                try:
                    return process(i, args, out / "found")
                except Exception as e:
                    return {"issue": i["number"], "title": i["title"], "url": i["html_url"], "state": i["state"], "labels": [], "blocks": [], "error": repr(e)}
            for entry in pool.map(safe, fresh):
                mf.write(json.dumps(entry) + "\n")
                done.add(entry["issue"])
                found += sum(r["status"] in INTERESTING for b in entry["blocks"] for r in b["results"])
        print(f"[{time.strftime('%H:%M:%S')}] {state} page {cursor['page']}: {len(fresh)} new issue(s), {found} problem(s), {time.time() - t0:.0f}s", flush=True)
        if more:
            cursor["after"], cursor["page"] = end, cursor["page"] + 1
        else:
            cursor["states"].pop(0)
            cursor["after"], cursor["page"] = None, 1
        cursor_path.write_text(json.dumps(cursor))
    print("every issue has been processed")
    return 0


def cmd_verify(args):
    out = Path(args.out)
    manifest = load_manifest(out)
    names = {r["stored"]: (e, r) for e in manifest for b in e["blocks"] for r in b["results"] if "stored" in r}
    def one(name):
        code = (out / "found" / f"{name}.rs").read_text()
        e, r = names[name]
        return name, evaluate(code, [(r["variant"], r["flags"])], args)[0]
    with ThreadPoolExecutor(args.jobs) as pool:
        for name, res in pool.map(one, sorted(names)):
            old = names[name][1]["status"]
            print(f"{name}: {old} -> {res['status']}{'  (FIXED?)' if res['status'] not in INTERESTING else ''} {res['detail'][:80]}")
            names[name][1].update({"status_now": res["status"], "toolchain_now": args.toolchain})
    (out / "manifest.jsonl").write_text("".join(json.dumps(e) + "\n" for e in manifest))
    return 0


def cmd_stats(args):
    m = load_manifest(Path(args.out))
    counts = {}
    for e in m:
        for b in e["blocks"]:
            for r in b["results"]:
                counts[r["status"]] = counts.get(r["status"], 0) + 1
    print(f"{len(m)} issues, {sum(len(e['blocks']) for e in m)} code blocks; results: {counts}")
    return 0


def main():
    global MEM_GB, MIN_AVAILABLE_GB
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("mode", choices=["fetch", "crawl", "verify", "stats"])
    ap.add_argument("--out", default="tmp/issues")
    ap.add_argument("--repo", default=REPO)
    ap.add_argument("--label", action="append", help=f"label to collect (repeatable; default: {' '.join(LABELS)})")
    ap.add_argument("--state", default="open", choices=["open", "closed", "all"])
    ap.add_argument("--batch", type=int, default=50, help="new issues per run")
    ap.add_argument("--oldest-first", action="store_true", help="default: the newest issues first (old ones are mostly fixed)")
    ap.add_argument("--toolchain", default="nightly")
    ap.add_argument("--default-flags", default="--edition 2021", help="the 'default arguments' variant")
    ap.add_argument("--comments", action="store_true", help="also read the comments (flags and code are often there)")
    ap.add_argument("--run", action="store_true", help="run the compiled programs (O0 against O3, crashes): EXECUTES untrusted code")
    ap.add_argument("--timeout", type=int, default=60, help="seconds a compilation may take before it counts as a hang")
    ap.add_argument("--run-timeout", type=int, default=10)
    ap.add_argument("--max-blocks", type=int, default=6, help="code blocks per issue")
    ap.add_argument("--jobs", type=int, default=4, help="parallel compilations (each may use up to --mem-gb of RAM)")
    ap.add_argument("--mem-gb", type=int, default=MEM_GB, help="address-space limit of one compiler / program")
    ap.add_argument("--min-available-gb", type=int, default=MIN_AVAILABLE_GB, help="no new process is started while less RAM than this is available")
    args = ap.parse_args()
    MEM_GB, MIN_AVAILABLE_GB = args.mem_gb, args.min_available_gb
    sys.exit({"fetch": cmd_fetch, "crawl": cmd_crawl, "verify": cmd_verify, "stats": cmd_stats}[args.mode](args))


if __name__ == "__main__":
    main()
