#!/usr/bin/env python3
"""Coverage of the safe, stable std API by the `stdlib` strategy.

Reads the nightly rustdoc JSON of core/alloc/std (`rustup component add rust-docs-json --toolchain nightly`), collects every
public, stable, non-unsafe, non-deprecated function/method (inherent methods by self type, trait methods by trait, free functions
by module) and checks whether the call appears in src/strategies/stdlib*.rs as `.name(`, `::name(` or `name(`.

  tools/stdcov.py                 summary per family + totals
  tools/stdcov.py --missing       also list the uncovered names per family
  tools/stdcov.py --min 0.9       exit 1 when the covered share is below 90 %
"""
import argparse, glob, json, os, re, subprocess, sys

ap = argparse.ArgumentParser()
ap.add_argument("--missing", action="store_true")
ap.add_argument("--min", type=float, default=0.0)
ap.add_argument("--json-dir")
a = ap.parse_args()

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
jdir = a.json_dir
if not jdir:
    sysroot = subprocess.check_output(["rustc", "+nightly", "--print", "sysroot"], text=True).strip()
    jdir = os.path.join(sysroot, "share/doc/rust/json")

INT = {"u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize"}


def family(name):
    if name in INT:
        return "int"
    if name in ("f32", "f64"):
        return "float"
    if re.fullmatch(r"NonZero.*", name):
        return "NonZero"
    if re.fullmatch(r"Atomic.*", name):
        return "Atomic"
    return name


def type_name(t):
    if not isinstance(t, dict):
        return None
    if "primitive" in t:
        return t["primitive"]
    if "resolved_path" in t:
        return t["resolved_path"]["path"].split("::")[-1]
    if "slice" in t:
        return "slice"
    if "array" in t:
        return "array"
    if "borrowed_ref" in t:
        return type_name(t["borrowed_ref"]["type"])
    return None


def version_tuple(v):
    return tuple(int(x) for x in v.split(".")[:2])


STABLE = version_tuple(re.search(r"rustc (\d+\.\d+)", subprocess.check_output(["rustc", "+stable", "-V"], text=True)).group(1))


def stable(it):
    """Stable already in the current stable release (the fuzzer must compile on stable and nightly)."""
    st = it.get("stability")
    if not (bool(st) and isinstance(st.get("level"), dict) and "stable" in st["level"] and not it.get("deprecation")):
        return False
    return version_tuple(st["level"]["stable"]["since"]) <= STABLE


def hidden(it):
    return any("hidden" in str(x) for x in it.get("attrs", [])) or any("doc(hidden)" in str(x) for x in it.get("attrs", []))


found = {}  # (family, name) -> True
for crate in ("core", "alloc", "std"):
    d = json.load(open(os.path.join(jdir, crate + ".json")))
    idx = d["index"]
    paths = d["paths"]
    for it in idx.values():
        inner = it["inner"]
        if not isinstance(inner, dict):
            continue
        if "impl" in inner:
            im = inner["impl"]
            if im["trait"] is not None or im["is_synthetic"]:
                continue  # inherent impls only; trait methods are taken from the trait definitions
            owner = type_name(im["for"])
            if not owner:
                continue
            for mid in im["items"]:
                m = idx.get(str(mid)) or idx.get(mid)
                if not m or not isinstance(m["inner"], dict) or "function" not in m["inner"]:
                    continue
                f = m["inner"]["function"]
                if f["header"].get("is_unsafe") or m["visibility"] != "public" or not stable(m) or hidden(m) or not m["name"]:
                    continue
                found[(family(owner), m["name"])] = True
        elif "trait" in inner:
            if it["visibility"] != "public" or not stable(it) or not it["name"] or hidden(it):
                continue
            tr = inner["trait"]
            if tr.get("is_unsafe"):
                continue
            for mid in tr["items"]:
                m = idx.get(str(mid)) or idx.get(mid)
                if not m or not isinstance(m["inner"], dict) or "function" not in m["inner"]:
                    continue
                if m["inner"]["function"]["header"].get("is_unsafe") or not stable(m) or hidden(m) or not m["name"]:
                    continue
                found[("trait " + it["name"], m["name"])] = True
        elif "module" in inner:
            mp = paths.get(str(it["id"]))
            if not mp:
                continue
            for mid in inner["module"]["items"]:
                m = idx.get(str(mid)) or idx.get(mid)
                if not m or not isinstance(m["inner"], dict) or "function" not in m["inner"]:
                    continue
                if m["visibility"] != "public" or not stable(m) or m["inner"]["function"]["header"].get("is_unsafe") or hidden(m) or not m["name"]:
                    continue
                found[("fn " + "::".join(mp["path"]), m["name"])] = True

src = ""
for f in glob.glob(os.path.join(root, "src/strategies/stdlib*.rs")) + glob.glob(os.path.join(root, "src/strategies/stdlib/*.rs")):
    src += open(f).read() + "\n"
used = set(re.findall(r"(?:\.|::|\b)([a-z_][a-z0-9_]*)\s*(?:::<[^>]*>)?\(", src))

# nothing here can be compared between two builds of the same program (I/O, OS, time, threads, SIMD intrinsics have their own strategy)
OUT = re.compile(r"^Unix.*|Poll|Wake|Scope|Sender|SyncSender|Receiver|IoSlice.*|Pipe.*|Owned.*|Borrowed.*|As.*Fd|As.*Handle|As.*Socket|Into.*Fd|Into.*Handle|Into.*Socket|IsTerminal|ToSocketAddrs|Termination|Panic.*|Prefix.*|Component.*|Iter(Mut)?|DirBuilder|File.*|SystemTimeError|BarrierWaitResult|WaitTimeoutResult|OnceState|SyncView|trait Seek|trait Future|trait IntoFuture|trait Pointer|NonNull|MaybeUninit|UnsafeCell|BoxedArrayIntoIter|core_arch|::arch|::os\b|::os::|std::(fs|net|process|env|thread|time::Instant|time::SystemTime)|^(File|Path|PathBuf|OsString|OsStr|TcpStream|TcpListener|UdpSocket|Command|Child|Stdin|Stdout|Stderr|Thread|JoinHandle|Instant|SystemTime|Metadata|OpenOptions|DirEntry|ReadDir|Permissions|Builder|Barrier|Condvar|Mutex|RwLock|Once|OnceLock|LazyLock|Stdio|ExitStatus|Output|Child.*|.*Ext|SocketAddr.*|Ipv.*|IpAddr|Layout|System|Location|TypeId|Backtrace.*|Waker|Context|RawWaker.*|VaList.*|Thin)$|trait .*Ext$|::intrinsics|::simd|::ffi|::panic|::alloc|::backtrace|::hint|::sync::atomic|::sync::mpsc|::task|::future|::pin|::ptr|::mem::(forget|transmute)|::io::(stdin|stdout|stderr|copy)|::sync::Exclusive|::any|::marker|::error|::env|::cell::(LazyCell)")
fams = {}
skipped = 0
for (fam, name) in found:
    if OUT.search(fam):
        skipped += 1
        continue
    fams.setdefault(fam, []).append(name)
tot = cov = 0
rows = []
for fam, names in sorted(fams.items()):
    names = sorted(set(names))
    c = [n for n in names if n in used]
    tot += len(names)
    cov += len(c)
    rows.append((fam, len(c), len(names), [n for n in names if n not in used]))
print(f"{'family':<34} {'covered':>8} {'of':>6}")
for fam, c, n, miss in sorted(rows, key=lambda r: -r[2]):
    print(f"{fam:<34} {c:>8} {n:>6}  {100 * c // max(n, 1):>3}%")
    if a.missing and miss:
        print("    missing: " + " ".join(miss))
print(f"(left out as not comparable between builds: {skipped} names - I/O, OS, time, threads, SIMD intrinsics, raw pointers, ...)")
print(f"TOTAL {cov}/{tot} = {100.0 * cov / max(tot, 1):.1f}%")
sys.exit(1 if tot and cov / tot < a.min else 0)
