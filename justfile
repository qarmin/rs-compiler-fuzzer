set export := true

# Local helpers: fetch the Nightly findings package and minimise findings by hand on a strong machine.
#   just doctor                         # is everything installed? (just setup installs what can be)
#   just download                       # findings.zip -> ./findings, ./INDEX.md, ./fixed.txt
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
    cargo test --release --workspace --all-features --all-targets

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
    {{bin}} triage --dir {{dir}} --bisect 1500 --budget 900 {{args}}

# regression search only (stable / nightly / cargo-bisect-rustc), no minimisation
[group('findings')]
bisect dir secs="3000": build
    {{bin}} triage --dir {{dir}} --no-reduce --no-search --bisect {{secs}}

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
