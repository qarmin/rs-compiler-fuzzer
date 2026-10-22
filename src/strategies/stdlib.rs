//! Strategy `stdlib`: calls to (almost) every safe, stable, deterministic function of the standard
//! library on edge-case inputs. `cmp::max` was enough to trigger a miscompilation, so the whole API
//! surface gets exercised: integer and float methods, `char`/`str`/`String`, slices, `Vec`, arrays,
//! iterator adapters and consumers, `Option`/`Result`, `cmp`, `mem`, cells, `Rc`/`Box`/`Cow`,
//! ranges, `Duration`, collections.
//!
//! The snippets are plain Rust statements in tables (`stdlib_*.rs`), one `&str` each, written against
//! variables of a family prelude (`a`, `b`, `c`: values of the chosen type; `v`, `w`: vectors; `s`, `t`:
//! strings ...). A case = a prelude plus a few randomly chosen snippets, each in its own block.
//! `tools/stdcov.py` measures which std functions the tables mention (from the rustdoc JSON of core,
//! alloc and std); a snippet that panics is a generator bug (the self-check test runs every snippet).
//!
//! Left out on purpose: `unsafe` functions, I/O, time, threads, hashing with random state, addresses,
//! `HashMap` iteration order, and libm results (`sin`, `exp`, `powf` ... may legitimately differ
//! between constant folding and the runtime library). NaN payloads/signs are normalised by `Feed`.

use super::{lit, signed, UINTS};
use crate::rng::Rng;

static MIRI_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// In Miri mode snippets marked `/*libm*/` are left out: Miri has its own implementations of the libm functions, so their
/// results legitimately differ from the native ones.
pub fn set_miri_mode(on: bool) {
    MIRI_MODE.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn bbv(rng: &mut Rng, expr: String) -> String {
    if rng.chance(60) { format!("bb({expr})") } else { expr }
}

/// `n` randomly chosen snippets (weighted by table size), each in its own block.
pub fn emit(rng: &mut Rng, prelude: &str, tables: &[&[&str]], n: usize) -> String {
    let total: usize = tables.iter().map(|t| t.len()).sum();
    let mut s = String::from(prelude);
    let mut used: Vec<usize> = Vec::new();
    let miri = MIRI_MODE.load(std::sync::atomic::Ordering::Relaxed);
    let mut picked = 0;
    let mut tries = 0;
    while picked < n.min(total) && tries < 100 {
        tries += 1;
        // distinct snippets within a case (more API per case); a few retries are enough
        let mut k = rng.below(total);
        for _ in 0..8 {
            if !used.contains(&k) {
                break;
            }
            k = rng.below(total);
        }
        used.push(k);
        for t in tables {
            if k < t.len() {
                if miri && t[k].starts_with("/*libm*/") {
                    break;
                }
                picked += 1;
                s.push_str("        { ");
                s.push_str(t[k]);
                s.push_str(" }\n");
                break;
            }
            k -= t.len();
        }
    }
    s
}

pub fn wrap(items: &str, body: &str) -> String {
    format!("{items}    pub fn run(h: &mut H) {{\n{body}    }}\n")
}

/// The unsigned/signed integer type of the same width.
pub fn counterpart(ty: &str) -> String {
    if signed(ty) { ty.replacen('i', "u", 1) } else { ty.replacen('u', "i", 1) }
}

/// Small value `0..=max` as a literal of type `ty`.
pub fn small(rng: &mut Rng, ty: &str, max: usize) -> String {
    format!("{}{ty}", rng.below(max + 1))
}

pub fn is_unsigned(ty: &str) -> bool {
    UINTS.contains(&ty)
}

/// Dev aid / CI self-check: `RSFUZZ_STDLIB_TABLE=<name>` makes every case use that snippet table (see `tools/stdlib_selfcheck.sh`).
fn forced() -> Option<String> {
    std::env::var("RSFUZZ_STDLIB_TABLE").ok().filter(|s| !s.is_empty())
}

/// The snippet table to use: the forced one if it belongs to this family, else a random one.
pub fn pick_table<'a>(rng: &mut Rng, tables: &[(&'static str, &'a [&'static str])]) -> &'a [&'static str] {
    if let Some(want) = forced() {
        if let Some(t) = tables.iter().find(|t| t.0 == want) {
            return t.1;
        }
    }
    tables[rng.below(tables.len())].1
}

/// Every snippet table by name (for the self-check).
pub fn table_names() -> Vec<&'static str> {
    let mut v = vec!["int"];
    for fam in [super::stdlib_float::NAMES, super::stdlib_text::NAMES, super::stdlib_seq::NAMES, super::stdlib_misc::NAMES, super::stdlib_coll::NAMES, super::stdlib_more::NAMES] {
        v.extend_from_slice(fam);
    }
    v
}

/// Pick one of the families, weighted roughly by the size of the API they cover.
pub fn generate(rng: &mut Rng) -> String {
    if let Some(want) = forced() {
        let fams: [(&[&str], fn(&mut Rng) -> String); 7] = [
            (&["int"], super::stdlib_int::generate),
            (super::stdlib_float::NAMES, super::stdlib_float::generate),
            (super::stdlib_text::NAMES, super::stdlib_text::generate),
            (super::stdlib_seq::NAMES, super::stdlib_seq::generate),
            (super::stdlib_misc::NAMES, super::stdlib_misc::generate),
            (super::stdlib_coll::NAMES, super::stdlib_coll::generate),
            (super::stdlib_more::NAMES, super::stdlib_more::generate),
        ];
        if let Some((_, g)) = fams.iter().find(|(names, _)| names.contains(&want.as_str())) {
            return g(rng);
        }
    }
    match rng.below(100) {
        0..=22 => super::stdlib_int::generate(rng),
        23..=28 => super::stdlib_float::generate(rng),
        29..=42 => super::stdlib_text::generate(rng),
        43..=58 => super::stdlib_seq::generate(rng),
        59..=72 => super::stdlib_misc::generate(rng),
        73..=82 => super::stdlib_coll::generate(rng),
        _ => super::stdlib_more::generate(rng),
    }
}

pub fn int_lit(rng: &mut Rng, ty: &str) -> String {
    let l = lit(rng, ty);
    bbv(rng, l)
}
