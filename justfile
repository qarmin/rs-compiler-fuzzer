set export := true

# Local helpers: fetch the Nightly findings package and minimise findings by hand on a strong machine.
#   just doctor                         # is everything installed? (just setup installs what can be)
#   just download                       # findings.zip -> ./findings, ./old_findings, ./INDEX.md
#   just minimize findings/arith__Oz__4a1a1f9e41
# Ctrl+C is safe: the smallest program found so far is kept in reduced.rs and ISSUE.md / STATUS.md / status.cfg / finding.json
# are rewritten for it; run the same command again to continue from there.

BIN_NAME := "rs-compiler-fuzzer"
url := "https://github.com/qarmin/rs-compiler-fuzzer/releases/download/Nightly/findings.zip"
bin := "./target/release/" + BIN_NAME
# parallel oracle runs / Shrink Ray workers; every one compiles with rustc, so keep it at the number of cores
jobs := `nproc`

default:
    @just --list

## fix
# Fixes most pure-Rust problems on its own (formatting, auto-fixable clippy lints).

# fmt + clippy --fix + fmt
[group('fix')]
fix:
    cargo +nightly fmt
    cargo clippy --fix --allow-dirty --allow-staged --all-features --all-targets
    cargo +nightly fmt
    cargo fmt 2>/dev/null

## run

# debug
[group('run')]
run *args:
    cargo run --bin {{BIN_NAME}} -- {{args}}

# release
[group('run')]
runr *args:
    cargo run --release --bin {{BIN_NAME}} -- {{args}}

# AddressSanitizer on nightly (needs: nightly + rust-src + llvm-tools-preview, see `doctor`)
[group('run')]
runs *args:
    ASAN_SYMBOLIZER_PATH=$(which llvm-symbolizer) ASAN_OPTIONS="symbolize=1:detect_leaks=0" RUST_BACKTRACE=1 RUSTFLAGS="-Zsanitizer=address" cargo +nightly run --target x86_64-unknown-linux-gnu --bin {{BIN_NAME}} -- {{args}}

## build / test

[group('test')]
build:
    cargo build --release

[group('test')]
test:
    #!/usr/bin/env bash
    # crashing compilers and programs under test leave core.* files behind
    ulimit -c 0
    status=0
    cargo test --release --workspace --all-features --all-targets || status=$?
    rm -f core core.*
    exit $status

# reduce any Rust file with the standalone minimizer: just reduce ice.rs ./check.sh (check.sh exits 0 while the problem shows)
[group('findings')]
reduce file test *args:
    cargo run --release -- reduce {{file}} --test {{test}} {{args}}

## install

[group('install')]
install *args:
    cargo install --locked --path . {{args}}

# Bumps version requirements in Cargo.toml, including incompatible ones (needs cargo-edit, see `doctor`)
[group('install')]
upgrade:
    cargo upgrade -i
    cargo update

## findings

# fetch findings.zip from the Nightly release and unpack it here
[group('findings')]
download:
    curl -fL --retry 3 -o findings.zip {{url}}
    unzip -oq findings.zip
    @echo "unpacked: $(ls findings | wc -l) findings, see INDEX.md"

# list the findings with their open checklist steps
[group('findings')]
list:
    @grep -E '^\|' INDEX.md | head -80

# minimise one finding with all cores for hours; extra options: --jobs N, --budget SECS, --deadline SECS, --bisect SECS, --search
[group('findings')]
minimize dir *args: doctor-quick build
    {{bin}} minimize {{dir}} --jobs {{jobs}} {{args}}

# minimise several findings one after another (Ctrl+C stops after writing up the current one)
[group('findings')]
minimize-all *dirs: doctor-quick build
    #!/usr/bin/env bash
    set -u
    for d in {{dirs}}; do
        {{bin}} minimize "$d" --jobs {{jobs}} || break
    done

# run the whole checklist again for one finding (reduce for a short time, flags, opt levels, Miri, regression bisection, upstream search)
[group('findings')]
triage dir *args: build
    {{bin}} triage --dir {{dir}} --bisect 3000 --budget 1800 {{args}}

# regression search only (stable / nightly / cargo-bisect-rustc), no minimisation
[group('findings')]
bisect dir secs="3000": build
    {{bin}} triage --dir {{dir}} --no-reduce --no-search --bisect {{secs}}

# import hand-made reproducers: DIR holds NAME.rs + NAME.txt pairs (any depth; the txt is the failing rustc command line); results in ./import-out/imported/<key>/
[group('findings')]
import dir *args: doctor-quick build
    {{bin}} import --todo {{dir}} --out import-out --bisect 1500 --budget 900 --budget-max 7200 {{args}}

# next batch of GitHub issues -> tmp/issues/found/NAME.rs + NAME.txt (what compiles into an ICE / crash / hang; with --run also O0 vs O3 output differences, which EXECUTES the programs); `just import tmp/issues/found` triages them
[group('findings')]
gather *args:
    python3 tools/gather_issues.py fetch --comments {{args}}

# run the gathered problems again (a status that is no longer a problem means fixed, or flaky)
[group('findings')]
gather-verify *args:
    python3 tools/gather_issues.py verify {{args}}

# show the issue draft of a finding
[group('findings')]
issue dir:
    @cat {{dir}}/ISSUE.md

# show the checklist of a finding
[group('findings')]
status dir:
    @cat {{dir}}/STATUS.md

# rebuild INDEX.md after changing findings
[group('findings')]
index: build
    {{bin}} pack --pkg .

# list stored findings that still have open checklist steps (what the daily retriage works on)
[group('findings')]
open:
    @grep -l "=todo\|=partial" findings/*/status.cfg 2>/dev/null | xargs -n1 dirname | sort || true

## local fuzzing (heavier than CI: all configurations, LTO, more cores)
# Examples:
#   just fuzz-lto                              # every strategy once, with the LTO configs CI never runs
#   just fuzz-lto memory,shapes 5              # chosen strategies, 5 rounds
#   just fuzz-full arith,loops --duration 3600 # all configs incl. the slow ones, for an hour
# Findings land in ./fuzz-out; `just fuzz-minimize` turns the stored ones into reduced reproducers.

# the CI fuzz job on this machine: nightly, every strategy, all configurations; duration in minutes (default 60)
[group('fuzz')]
fuzz duration="60" strategies="all" *args: doctor-quick build
    {{bin}} fuzz --rustc "rustc +nightly" --strategies {{strategies}} --profile full --duration $(({{duration}} * 60)) --jobs {{jobs}} --out fuzz-out {{args}}

# all configurations except LTO and codegen-units=1 (those need --lto)
[group('fuzz')]
fuzz-full strategies="all" *args: doctor-quick build
    {{bin}} fuzz --strategies {{strategies}} --profile full --jobs {{jobs}} --out fuzz-out {{args}}

# the LTO configurations (thin, thin+overflow checks, fat), normally off because they are too heavy for CI
[group('fuzz')]
fuzz-lto strategies="all" rounds="1" *args: doctor-quick build
    {{bin}} fuzz --lto --strategies {{strategies}} --rounds {{rounds}} --profile full --jobs {{jobs}} --out fuzz-out {{args}}

# only the LTO-related comparison: O0 against thin / thin+checks / fat (faster than all configs)
[group('fuzz')]
fuzz-lto-only strategies="memory,shapes,loops,arith" rounds="3" *args: doctor-quick build
    {{bin}} fuzz --lto --strategies {{strategies}} --rounds {{rounds}} --configs O0,O3,O3-lto-thin,O3-lto-thin-checks,O3-lto-fat --jobs {{jobs}} --out fuzz-out {{args}}

# known miscompilations (canaries/): proves the detector fires on this toolchain; --lto includes #164297
[group('fuzz')]
canary toolchain="nightly": build
    {{bin}} canary --lto --rustc "rustc +{{toolchain}}" --out fuzz-out

# list strategies and configurations (with --lto the LTO ones too)
[group('fuzz')]
configs *args: build
    {{bin}} list {{args}} --lto

# one strategy on the nightly, with a seed to repeat a run: just fuzz-one shapes 42
[group('fuzz')]
fuzz-one strategy seed="" *args: build
    {{bin}} fuzz --strategies {{strategy}} {{ if seed != "" { "--seed " + seed } else { "" } }} --rounds 1 --jobs {{jobs}} --out fuzz-out --rustc "rustc +nightly" {{args}}

# Miri over the generated programs (needs nightly + miri)
[group('fuzz')]
miri strategies="all" *args: build
    {{bin}} miri --strategies {{strategies}} --rounds 1 --out fuzz-out {{args}}

# compile+run every stdlib table on stable and nightly (generator self-check)
[group('fuzz')]
stdlib-selfcheck:
    bash tools/stdlib_selfcheck.sh

# minimise everything the local fuzzing found (fuzz-out/<finding>/), one after another
[group('fuzz')]
fuzz-minimize *args: doctor-quick build
    #!/usr/bin/env bash
    set -u
    for d in fuzz-out/*/; do
        [ -f "$d/case.rs" ] && [ -f "$d/repro.cfg" ] || continue
        {{bin}} minimize "$d" --jobs {{jobs}} {{args}} || break
    done

## GitHub Actions (needs an authenticated `gh`; every recipe starts the workflow on the master branch and prints how to follow it)
# Examples:
#   just gh-fuzz                               # the daily fuzz run: 8 groups, nightly, 230 minutes each
#   just gh-fuzz 30 "arith,select;loops"       # 30 minutes, only these groups
#   just gh-runs fuzz                          # list the latest runs of a workflow
#   just gh-watch                              # follow the newest run

# start fuzz.yml: duration in minutes ("" = default), strategy groups separated by ';' ("" = all), profile full|fast ("" = default)
[group('github')]
gh-fuzz duration="" strategies="" profile="":
    gh workflow run fuzz.yml -f duration_minutes="{{duration}}" -f strategies="{{strategies}}" -f profile="{{profile}}"

# start miri.yml: duration in minutes ("" = default), strategy groups separated by ';' ("" = all)
[group('github')]
gh-miri duration="" strategies="":
    gh workflow run miri.yml -f duration_minutes="{{duration}}" -f strategies="{{strategies}}"

# start retriage.yml (continues findings with open checklist steps)
[group('github')]
gh-retriage *args:
    gh workflow run retriage.yml {{args}}

# start ci.yml
[group('github')]
gh-ci:
    gh workflow run ci.yml

# latest runs of a workflow (fuzz, miri, retriage, ci, import, minimize-long)
[group('github')]
gh-runs workflow="fuzz" limit="8":
    gh run list --workflow {{workflow}}.yml --limit {{limit}}

# follow the newest run of a workflow until it ends
[group('github')]
gh-watch workflow="fuzz":
    gh run watch "$(gh run list --workflow {{workflow}}.yml --limit 1 --json databaseId --jq '.[0].databaseId')"

## profiling
# perf permissions (one-time):
#   echo '-1' | sudo tee /proc/sys/kernel/perf_event_paranoid

# just samply
[group('profiling')]
samply *args:
    cargo build --bin {{BIN_NAME}} --profile rdebug
    samply record target/rdebug/{{BIN_NAME}} {{args}}

# just heaptrack; heaptrack_gui "$(ls -t heaptrack.* 2>/dev/null | head -n1)"
[group('profiling')]
heaptrack *args:
    cargo build --profile rdebug --bin {{BIN_NAME}}
    heaptrack target/rdebug/{{BIN_NAME}} {{args}}

# just hotspot; hotspot perf.data
[group('profiling')]
hotspot *args:
    cargo build --bin {{BIN_NAME}} --profile rdebug
    perf record -o perf.data --call-graph dwarf,8192 --aio -z --sample-cpu target/rdebug/{{BIN_NAME}} {{args}}
    hotspot

## setup
# doctor = read-only check that what this justfile needs is installed; exits non-zero if something required is missing.
# setup = installs what doctor flags as missing (toolchain, cargo and pip level only; system packages are printed, never installed).

# read-only health check
[group('setup')]
doctor:
    #!/usr/bin/env bash
    set +e
    fail=0
    check() {
        local name="$1"; shift
        local out
        if out="$("$@" 2>&1)"; then
            printf "  [OK]   %-34s %s\n" "$name" "$(head -n1 <<<"$out")"
        else
            printf "  [BRAK] %s\n" "$name"
            fail=$((fail + 1))
        fi
    }
    info() {
        local name="$1"; shift
        local out
        if out="$("$@" 2>&1)"; then
            printf "  [OK]   %-34s %s\n" "$name" "$(head -n1 <<<"$out")"
        else
            printf "  [-]    %s (opcjonalne)\n" "$name"
        fi
    }
    has() { command -v "$1" >/dev/null 2>&1; }

    echo "Toolchain Rust:"
    check "rustc"                        rustc --version
    check "cargo"                        cargo --version
    check "rustup"                       rustup --version
    check "toolchain stable"             rustup run stable rustc --version
    check "toolchain nightly"            rustup run nightly rustc --version
    check "rustfmt (nightly)"            rustup run nightly cargo fmt --version
    check "clippy"                       cargo clippy --version
    check "miri (nightly)"               rustup run nightly cargo miri --version
    check "rust-src (nightly, dla miri)" bash -c "rustup component list --toolchain nightly --installed | grep -q rust-src"

    echo ""
    echo "Narzedzia minimalizacji i triage (jobs = $(nproc)):"
    check "python3"                      python3 --version
    check "curl (just download)"         curl --version
    check "unzip (just download)"        unzip -v
    check "timeout"                      timeout --version
    check "nproc"                        nproc
    info "cargo-bisect-rustc (bisect)"   cargo-bisect-rustc --version
    info "gh (upstream search)"          gh --version
    info "cargo-edit (just upgrade)"     cargo upgrade --version

    echo ""
    echo "Profilowanie i ASAN (opcjonalne):"
    info "samply"                        samply --version
    info "heaptrack"                     heaptrack --version
    info "perf"                          perf --version
    info "llvm-symbolizer (just runs)"   bash -c "command -v llvm-symbolizer"
    info "nightly: llvm-tools-preview"   bash -c "rustup component list --toolchain nightly --installed | grep -q llvm-tools"

    echo ""
    if [ "$fail" -eq 0 ]; then
        echo "Wszystko OK."
    else
        echo "$fail wymaganych narzedzi brakuje - uruchom 'just setup', zeby je zainstalowac."
        exit 1
    fi

# the part of doctor that decides whether a minimisation can work at all (silent when fine)
[private]
doctor-quick:
    #!/usr/bin/env bash
    missing=""
    for t in rustc cargo rustup timeout python3; do command -v $t >/dev/null 2>&1 || missing="$missing $t"; done
    rustup run nightly rustc --version >/dev/null 2>&1 || missing="$missing nightly-toolchain"
    if [ -n "$missing" ]; then echo "missing:$missing - run 'just doctor' and 'just setup'"; exit 1; fi

# installs whatever `doctor` flagged as missing
[group('setup')]
setup:
    #!/usr/bin/env bash
    set -e
    rustup toolchain install stable --profile minimal -c rustfmt -c clippy
    rustup toolchain install nightly --profile minimal -c rustfmt -c clippy -c miri -c rust-src -c llvm-tools-preview
    command -v cargo-upgrade >/dev/null 2>&1 || cargo install cargo-edit --locked
    command -v cargo-bisect-rustc >/dev/null 2>&1 || cargo install cargo-bisect-rustc --locked || true
    for t in curl unzip; do
        command -v $t >/dev/null 2>&1 || echo "$t not found - install it with: sudo apt install $t"
    done
    command -v gh >/dev/null 2>&1 || echo "gh not found (optional, upstream issue search) - see https://cli.github.com"
    echo "Done - run 'just doctor' to verify."
