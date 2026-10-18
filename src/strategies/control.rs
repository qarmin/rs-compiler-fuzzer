//! Strategy `control`: random structured control flow over boolean/integer state, in the
//! spirit of rust-lang/rust#163311 (nested `loop` + labeled blocks + a mutable static whose
//! final value depends on exact branch order).
//!
//! Termination: every `loop` carries a fuel counter and breaks when it is exhausted; `for`
//! ranges are constant. Jumps are only emitted to labels that are in scope. All integer
//! arithmetic is wrapping on `u32`. The function's result is a digest of the trace
//! (`OUT`, either a `static mut` or a local) and the final state.

use crate::rng::Rng;
use crate::w;

#[derive(Clone)]
struct Lbl {
    name: String,
    is_loop: bool,
}

struct Gen<'a> {
    rng: &'a mut Rng,
    nb: usize,
    ni: usize,
    extra_ints: Vec<String>,
    labels: Vec<Lbl>,
    fresh: usize,
    budget: i32,
    use_static: bool,
}

impl Gen<'_> {
    fn bvar(&mut self) -> String {
        format!("b{}", self.rng.below(self.nb))
    }

    fn ivar(&mut self) -> String {
        let total = self.ni + self.extra_ints.len();
        let i = self.rng.below(total);
        if i < self.ni { format!("n{i}") } else { self.extra_ints[i - self.ni].clone() }
    }

    fn bexpr(&mut self, depth: u32) -> String {
        match self.rng.below(if depth == 0 { 4 } else { 9 }) {
            0 | 1 => self.bvar(),
            2 => format!("!{}", self.bvar()),
            3 => {
                let v = self.ivar();
                let op = *self.rng.pick(&["<", ">", "==", "!=", "<=", ">="]);
                format!("{v} {op} {}", self.rng.below(5))
            }
            4 => format!("({} && {})", self.bexpr(depth - 1), self.bexpr(depth - 1)),
            5 => format!("({} || {})", self.bexpr(depth - 1), self.bexpr(depth - 1)),
            6 => {
                let v = self.ivar();
                format!("({v} & {}) == 0", 1u32 << self.rng.below(3))
            }
            7 => format!("!({})", self.bexpr(depth - 1)),
            _ => {
                let v = self.ivar();
                format!("{v}.wrapping_mul({}) % 3 == {}", self.rng.below(10) | 1, self.rng.below(3))
            }
        }
    }

    fn trace(&mut self) -> String {
        let k = self.rng.below(1000);
        let upd = match self.rng.below(3) {
            0 => format!("out.wrapping_mul(131).wrapping_add({k})"),
            1 => format!("out.rotate_left(5) ^ {k}"),
            _ => {
                let v = self.ivar();
                format!("out.wrapping_mul(31) ^ ({v} as u64 + {k})")
            }
        };
        if self.use_static { format!("OUT = {}; ", upd.replace("out", "OUT")) } else { format!("out = {upd}; ") }
    }

    fn jump(&mut self) -> Option<String> {
        if self.labels.is_empty() {
            return None;
        }
        let l = self.rng.pick(&self.labels.clone()).clone();
        let kind = if l.is_loop && self.rng.chance(40) { "continue" } else { "break" };
        Some(format!("{kind} '{};", l.name))
    }

    fn block(&mut self, depth: u32, ind: usize, out: &mut String) {
        let n = self.rng.range(1, if depth == 0 { 4 } else { 3 });
        for _ in 0..n {
            self.stmt(depth, ind, out);
        }
    }

    fn stmt(&mut self, depth: u32, ind: usize, out: &mut String) {
        self.budget -= 1;
        let pad = "    ".repeat(ind);
        let kind = if self.budget <= 0 || depth >= 5 { self.rng.below(5) } else { self.rng.below(15) };
        match kind {
            0 => {
                let v = self.bvar();
                let e = self.bexpr(2);
                w!(out, "{pad}{v} = {e};");
            }
            1 => {
                let v = self.ivar();
                let c = self.rng.below(7);
                w!(out, "{pad}{v} = {v}.wrapping_add({c});");
            }
            2 => {
                let t = self.trace();
                w!(out, "{pad}{t}");
            }
            3 => {
                let v = self.ivar();
                let e = self.ivar();
                w!(out, "{pad}{v} = {e}.wrapping_mul({}) ^ {};", self.rng.below(9) | 1, self.rng.below(4));
            }
            4 => {
                if let Some(j) = self.jump() {
                    let c = self.bexpr(1);
                    w!(out, "{pad}if {c} {{ {j} }}");
                } else {
                    let t = self.trace();
                    w!(out, "{pad}{t}");
                }
            }
            5..=7 => {
                let c = self.bexpr(2);
                w!(out, "{pad}if {c} {{");
                self.block(depth + 1, ind + 1, out);
                if self.rng.chance(60) {
                    w!(out, "{pad}}} else {{");
                    self.block(depth + 1, ind + 1, out);
                }
                w!(out, "{pad}}}");
            }
            8 | 9 => {
                // fuel-bounded loop
                self.fresh += 1;
                let id = self.fresh;
                let fuel = self.rng.range(1, 4);
                w!(out, "{pad}let mut fuel{id} = 0u32;");
                w!(out, "{pad}'l{id}: loop {{");
                w!(out, "{pad}    fuel{id} += 1;");
                w!(out, "{pad}    if fuel{id} > {fuel} {{ break 'l{id}; }}");
                self.labels.push(Lbl { name: format!("l{id}"), is_loop: true });
                self.block(depth + 1, ind + 1, out);
                self.labels.pop();
                if self.rng.chance(40) {
                    w!(out, "{pad}    break;");
                }
                w!(out, "{pad}}}");
            }
            10 => {
                // for loop with a mutable, reassignable induction variable
                self.fresh += 1;
                let id = self.fresh;
                let hi = self.rng.range(1, 3);
                w!(out, "{pad}'f{id}: for mut x{id} in 0u32..{hi} {{");
                self.labels.push(Lbl { name: format!("f{id}"), is_loop: true });
                self.extra_ints.push(format!("x{id}"));
                self.block(depth + 1, ind + 1, out);
                self.extra_ints.pop();
                self.labels.pop();
                w!(out, "{pad}}}");
            }
            11 | 12 => {
                // labeled block
                self.fresh += 1;
                let id = self.fresh;
                w!(out, "{pad}'k{id}: {{");
                self.labels.push(Lbl { name: format!("k{id}"), is_loop: false });
                self.block(depth + 1, ind + 1, out);
                self.labels.pop();
                w!(out, "{pad}}}");
            }
            13 => {
                let v = self.ivar();
                w!(out, "{pad}match {v} % 4 {{");
                for arm in 0..self.rng.range(2, 3) {
                    w!(out, "{pad}    {arm} => {{");
                    self.block(depth + 1, ind + 2, out);
                    w!(out, "{pad}    }}");
                }
                w!(out, "{pad}    _ => {{");
                self.block(depth + 1, ind + 2, out);
                w!(out, "{pad}    }}");
                w!(out, "{pad}}}");
            }
            _ => {
                let c = self.bexpr(2);
                let t = self.trace();
                w!(out, "{pad}if {c} {{ {t}return {}; }}", self.digest());
            }
        }
    }

    fn digest(&self) -> String {
        let o = if self.use_static { "OUT" } else { "out" };
        let mut s = format!("{o}");
        for i in 0..self.nb.min(4) {
            s += &format!(" ^ ((b{i} as u64) << {})", i + 40);
        }
        for i in 0..self.ni.min(3) {
            s += &format!(" ^ ((n{i} as u64) << {})", i * 8);
        }
        s
    }
}

pub fn generate(rng: &mut Rng) -> String {
    let mut items = String::new();
    let mut body = String::new();
    let nfn = rng.range(1, 3);
    for f in 0..nfn {
        let use_static = rng.chance(50);
        let nb = rng.range(3, 8);
        let ni = rng.range(2, 4);
        let budget = rng.range(15, 60) as i32;
        let mut g = Gen { rng, nb, ni, extra_ints: vec![], labels: vec![], fresh: 0, budget, use_static };
        let mut code = String::new();
        g.block(0, 2, &mut code);
        g.block(0, 2, &mut code);
        let digest = g.digest();
        let attr = if g.rng.chance(50) { "#[inline(never)]" } else { "" };
        w!(items, "    {attr}\n    unsafe fn ctl{f}(seed: u32) -> u64 {{");
        if use_static {
            w!(items, "        OUT = 0;");
        } else {
            w!(items, "        let mut out = 0u64;");
        }
        for i in 0..nb {
            w!(items, "        let mut b{i} = (seed >> {}) & 1 == {};", i, i % 2);
        }
        for i in 0..ni {
            w!(items, "        let mut n{i} = (seed >> {}) & 3;", i * 2);
        }
        items.push_str(&code);
        w!(items, "        {digest}\n    }}");
        if use_static && f == 0 {
            // the static itself, shared by every static-using function of this case
            items.push_str("    static mut OUT: u64 = 0;\n");
        }
        w!(body, "        for s in [0u32, 1, 2, 5, 9, 17, 255, 1023] {{ h.put(&unsafe {{ ctl{f}(bb(s)) }}); }}");
    }
    // a static is needed iff some function used it; declare it once regardless.
    if !items.contains("static mut OUT") {
        items.push_str("    static mut OUT: u64 = 0;\n");
    }
    format!("{items}    pub fn run(h: &mut H) {{\n{body}    }}\n")
}
