#!/usr/bin/env python3
"""Random multi-deletion search on many threads: a manual complement to rs-minimizer for programs it cannot shrink.

  tools/handmin.py FILE CHECK [--jobs N] [--seconds S] [--mem-mb M] [--max-k K] [--out FILE]

CHECK is a script that gets the candidate path and exits 0 while the bug shows. Every candidate deletes 1..K random line ranges
(a range starts at a random line and extends to the end of the bracket group opened on it, or is a single line); the first
interesting candidate that is smaller than the current program is adopted and the search continues from it. Every check runs under
a `ulimit -v` of M MB so a candidate cannot eat the machine's RAM.
"""
import argparse, os, random, subprocess, sys, tempfile, threading, time
from concurrent.futures import ThreadPoolExecutor

def group_end(lines, i):
    depth = 0
    for j in range(i, len(lines)):
        depth += sum(lines[j].count(c) for c in "{([") - sum(lines[j].count(c) for c in "})]")
        if depth <= 0 and j >= i:
            return j
        if j - i > 60:
            break
    return i

def candidate(lines, rng, max_k):
    drop = set()
    for _ in range(rng.randint(1, max_k)):
        i = rng.randrange(len(lines))
        j = group_end(lines, i) if rng.random() < 0.6 else i
        drop.update(range(i, j + 1))
    return [l for n, l in enumerate(lines) if n not in drop]

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("file"); ap.add_argument("check")
    ap.add_argument("--jobs", type=int, default=os.cpu_count() // 2)
    ap.add_argument("--seconds", type=int, default=300)
    ap.add_argument("--mem-mb", type=int, default=4096)
    ap.add_argument("--max-k", type=int, default=4)
    ap.add_argument("--out")
    a = ap.parse_args()
    out = a.out or a.file + ".hand.rs"
    cur = open(a.file).read().splitlines(keepends=True)
    lock = threading.Lock()
    state = {"cur": cur, "gen": 0, "tried": 0, "hits": 0}
    end = time.time() + a.seconds
    tmp = tempfile.mkdtemp(prefix="handmin")

    def interesting(text, tag):
        p = os.path.join(tmp, f"{tag}.rs")
        open(p, "w").write(text)
        r = subprocess.run(["bash", "-c", f"ulimit -c 0; ulimit -v {a.mem_mb * 1024}; exec \"$0\" \"$@\"", a.check, p],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        return r.returncode == 0

    def worker(w):
        rng = random.Random(w * 7919 + int(time.time()))
        while time.time() < end:
            with lock:
                base, gen = state["cur"], state["gen"]
            cand = candidate(base, rng, a.max_k)
            if len(cand) >= len(base):
                continue
            text = "".join(cand)
            ok = interesting(text, f"w{w}")
            with lock:
                state["tried"] += 1
                if ok and gen == state["gen"]:
                    state["cur"], state["gen"] = cand, gen + 1
                    state["hits"] += 1
                    open(out, "w").write(text)
                    print(f"[{int(time.time() - end + a.seconds)}s] {len(base)} -> {len(cand)} lines ({state['tried']} tried)", flush=True)

    if not interesting("".join(cur), "start"):
        sys.exit("the file does not show the problem")
    with ThreadPoolExecutor(a.jobs) as ex:
        list(ex.map(worker, range(a.jobs)))
    print(f"done: {len(cur)} -> {len(state['cur'])} lines, {state['tried']} candidates, written to {out}")

main()
