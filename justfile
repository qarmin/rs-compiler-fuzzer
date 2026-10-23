# Local helpers: fetch the Nightly findings package and minimise findings by hand on a strong machine.
#   just download                       # findings.zip -> ./findings, ./INDEX.md, ./fixed.txt
#   just minimize findings/arith__Oz__4a1a1f9e41
# Ctrl+C is safe: the smallest program found so far is kept in reduced.rs and ISSUE.md / STATUS.md / status.cfg / finding.json
# are rewritten for it; run the same command again to continue from there.

url := "https://github.com/qarmin/rs-compiler-fuzzer/releases/download/Nightly/findings.zip"
bin := "./target/release/rs-compiler-fuzzer"
# parallel oracle runs / Shrink Ray workers; every one compiles with rustc, so keep it at the number of cores
jobs := `nproc`

default:
    @just --list

build:
    cargo build --release

test:
    cargo test --release

# tools used by the reduction (Shrink Ray is the strongest; the rest is optional)
tools:
    pip install --user shrinkray
    cargo install --locked treereduce-rust || true
    rustup toolchain install nightly stable --profile minimal -c rustfmt -c miri || true

# fetch findings.zip from the Nightly release and unpack it here
download:
    curl -fL --retry 3 -o findings.zip {{url}}
    unzip -oq findings.zip
    @echo "unpacked: $(ls findings | wc -l) findings, see INDEX.md"

# list the findings with their open checklist steps
list:
    @grep -E '^\|' INDEX.md | head -80

# minimise one finding with all cores for hours; extra options: --jobs N, --budget SECS, --deadline SECS, --bisect SECS, --search
minimize dir *args: build
    {{bin}} minimize {{dir}} --jobs {{jobs}} {{args}}

# minimise several findings one after another (Ctrl+C stops after writing up the current one)
minimize-all *dirs: build
    #!/usr/bin/env bash
    set -u
    for d in {{dirs}}; do
        {{bin}} minimize "$d" --jobs {{jobs}} || break
    done

# run the whole checklist again for one finding (reduce for a short time, flags, opt levels, Miri, regression bisection, upstream search)
triage dir *args: build
    {{bin}} triage --dir {{dir}} --bisect 1500 --budget 900 {{args}}

# regression search only (stable / nightly / cargo-bisect-rustc), no minimisation
bisect dir secs="3000": build
    {{bin}} triage --dir {{dir}} --no-reduce --no-search --bisect {{secs}}

# show the issue draft / the checklist of a finding
issue dir:
    @cat {{dir}}/ISSUE.md
status dir:
    @cat {{dir}}/STATUS.md

# rebuild INDEX.md after changing findings
index: build
    {{bin}} pack --pkg .

# list stored findings that still have open checklist steps (what the daily retriage works on)
open:
    @grep -l "=todo\|=partial" findings/*/status.cfg 2>/dev/null | xargs -n1 dirname | sort || true
