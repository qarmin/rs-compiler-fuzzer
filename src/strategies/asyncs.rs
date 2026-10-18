//! Strategy `asyncs`: async fns/blocks driven by a tiny single-threaded executor.
//! Stresses the coroutine transform: locals and borrows living across `.await`, drop guards
//! whose destructors run on completion *and* when a half-polled future is dropped,
//! loops containing awaits, early returns, nested async calls, two futures interleaved.
//!
//! Safe code only; the executor uses `Waker::noop()` and busy-polls (every future here becomes
//! ready after a bounded number of `Pending`s). Drop order and results go into a shared log.

use crate::rng::Rng;
use crate::w;

const PRELUDE: &str = r#"    use std::cell::RefCell;
    use std::future::Future;
    use std::pin::Pin;
    use std::rc::Rc;
    use std::task::{Context, Poll, Waker};
    type Log = Rc<RefCell<Vec<u32>>>;
    struct Guard(u32, Log);
    impl Drop for Guard { fn drop(&mut self) { self.1.borrow_mut().push(self.0); } }
    fn yield_now() -> impl Future<Output = ()> {
        let mut done = false;
        std::future::poll_fn(move |cx: &mut Context<'_>| {
            if done { Poll::Ready(()) } else { done = true; cx.waker().wake_by_ref(); Poll::Pending }
        })
    }
    fn block_on<F: Future>(f: F) -> F::Output {
        let mut f = Box::pin(f);
        let mut cx = Context::from_waker(Waker::noop());
        loop { if let Poll::Ready(v) = f.as_mut().poll(&mut cx) { return v; } }
    }
    /// Poll `n` times then drop the future (exercises drop of a suspended coroutine).
    fn poll_n<F: Future>(f: F, n: usize) -> Option<F::Output> {
        let mut f = Box::pin(f);
        let mut cx = Context::from_waker(Waker::noop());
        for _ in 0..n { if let Poll::Ready(v) = f.as_mut().poll(&mut cx) { return Some(v); } }
        None
    }
    fn join2<A: Future, B: Future>(a: A, b: B) -> (A::Output, B::Output) {
        let (mut a, mut b) = (Box::pin(a), Box::pin(b));
        let (mut ra, mut rb) = (None, None);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if ra.is_none() { if let Poll::Ready(v) = a.as_mut().poll(&mut cx) { ra = Some(v); } }
            if rb.is_none() { if let Poll::Ready(v) = b.as_mut().poll(&mut cx) { rb = Some(v); } }
            if ra.is_some() && rb.is_some() { return (ra.unwrap(), rb.unwrap()); }
        }
    }
"#;

fn uexpr(rng: &mut Rng, vars: &[String], depth: u32) -> String {
    if depth == 0 || rng.chance(25) {
        return if rng.chance(70) { rng.pick(vars).clone() } else { format!("{}u32", rng.below(1000)) };
    }
    let (a, b) = (uexpr(rng, vars, depth - 1), uexpr(rng, vars, depth - 1));
    match rng.below(6) {
        0 => format!("{a}.wrapping_add({b})"),
        1 => format!("{a}.wrapping_mul({b} | 1)"),
        2 => format!("({a} ^ {b})"),
        3 => format!("{a}.rotate_left({})", rng.below(32)),
        4 => format!("{a}.wrapping_sub({b})"),
        _ => format!("(if {a} & 1 == 0 {{ {b} }} else {{ {a} }})"),
    }
}

fn async_fn(rng: &mut Rng, k: usize, items: &mut String) {
    let mut vars: Vec<String> = vec!["x".into()];
    let mut b = String::new();
    let mut n = 0;
    let mut guards = 0;
    for _ in 0..rng.range(4, 10) {
        match rng.below(12) {
            0 | 1 => {
                n += 1;
                let e = uexpr(rng, &vars, 2);
                w!(b, "        let v{n}: u32 = {e};");
                vars.push(format!("v{n}"));
            }
            2 | 3 => w!(b, "        yield_now().await;"),
            4 => {
                guards += 1;
                w!(b, "        let _g{guards} = Guard({}, log.clone());", 100 * k as u32 + guards as u32 + 1000);
            }
            5 if k > 0 => {
                n += 1;
                let j = rng.below(k);
                let e = uexpr(rng, &vars, 1);
                w!(b, "        let v{n}: u32 = f{j}({e}, log.clone()).await;");
                vars.push(format!("v{n}"));
            }
            6 => {
                let e = uexpr(rng, &vars, 2);
                w!(b, "        for i in 0..{}u32 {{ yield_now().await; acc = acc.wrapping_mul(31).wrapping_add({e} ^ i); }}", rng.range(1, 4));
            }
            7 => {
                let e = uexpr(rng, &vars, 1);
                w!(b, "        {{ let r = &mut acc; yield_now().await; *r = r.wrapping_add({e}); }}");
            }
            8 => {
                let c = uexpr(rng, &vars, 1);
                w!(b, "        if {c} % {} == {} {{ log.borrow_mut().push(acc); return acc ^ {}; }}", rng.range(2, 6), rng.below(2), rng.below(100));
            }
            9 => {
                let e = uexpr(rng, &vars, 2);
                w!(b, "        log.borrow_mut().push({e});");
            }
            10 => {
                n += 1;
                let e = uexpr(rng, &vars, 1);
                w!(b, "        let v{n}: u32 = {{ let blk = async move {{ yield_now().await; {e} }}; blk.await }};");
                vars.push(format!("v{n}"));
            }
            _ => {
                let e = uexpr(rng, &vars, 2);
                w!(b, "        acc = acc.wrapping_add({e});");
            }
        }
    }
    let last = uexpr(rng, &vars, 2);
    w!(items, "    async fn f{k}(x: u32, log: Log) -> u32 {{\n        let mut acc: u32 = x;\n{b}        acc ^ {last}\n    }}");
}

pub fn generate(rng: &mut Rng) -> String {
    let mut items = String::from(PRELUDE);
    let mut body = String::new();
    let nf = rng.range(2, 5);
    for k in 0..nf {
        async_fn(rng, k, &mut items);
    }
    w!(body, "        let log: Log = Rc::new(RefCell::new(Vec::new()));");
    for _ in 0..rng.range(3, 7) {
        let k = rng.below(nf);
        let arg = rng.next_u64() as u32;
        match rng.below(4) {
            0 => w!(body, "        h.put(&block_on(f{k}(bb({arg}), log.clone())));"),
            1 => w!(body, "        h.put(&poll_n(f{k}(bb({arg}), log.clone()), {}));", rng.range(1, 4)),
            2 => {
                let k2 = rng.below(nf);
                w!(body, "        h.put(&join2(f{k}(bb({arg}), log.clone()), f{k2}(bb({}), log.clone())));", rng.next_u64() as u32);
            }
            _ => w!(body, "        h.put(&block_on(async {{ let a = f{k}(bb({arg}), log.clone()).await; yield_now().await; a.wrapping_add(f{}(a, log.clone()).await) }}));", rng.below(nf)),
        }
        w!(body, "        h.put(&*log.borrow()); log.borrow_mut().clear();");
    }
    format!("{items}    pub fn run(h: &mut H) {{\n{body}    }}\n")
}
