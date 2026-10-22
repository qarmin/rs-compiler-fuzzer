//! `stdlib` family, part 2: formatting machinery (`Formatter`, `debug_*` builders, the `fmt` traits on user types), in-memory
//! `io` (`Read`/`Write`/`BufRead`/`Seek`, `Cursor`, `BufReader`...), atomics, sync primitives used from one thread,
//! default methods of the `Iterator`/`DoubleEndedIterator` traits on a hand written iterator, `Deref`/`Index`/`FromStr`/`Error`
//! impls, `CString`/`CStr`, `Pin`, `ptr`.

use super::stdlib::{bbv, emit, pick_table, wrap};
use super::{lit, small_ints};
use crate::rng::Rng;

pub const NAMES: &[&str] = &["fmt", "io", "atomic", "sync", "custom", "ffi"];

// ---------------------------------------------------------------------------------------------------- fmt

/// Bodies of `fmt` for a wrapper `struct W(T)`; `f` is the formatter, `self.0` the value.
const FMT_BODIES: &[&str] = &[
    "f.pad(&self.0.to_string())",
    "f.pad_integral(self.0 as i128 >= 0, \"0x\", &format!(\"{}\", (self.0 as i128).unsigned_abs()))",
    "write!(f, \"[{}|{:?}|{}|{}|{}|{}|{}]\", f.width().unwrap_or(0), f.precision(), f.alternate(), f.sign_plus(), f.sign_minus(), f.fill(), f.sign_aware_zero_pad())",
    "f.debug_struct(\"W\").field(\"v\", &self.0).field(\"w\", &(self.0 as u8)).finish()",
    "f.debug_struct(\"W\").field(\"v\", &self.0).finish_non_exhaustive()",
    "f.debug_tuple(\"W\").field(&self.0).field(&\"s\").finish()",
    "f.debug_tuple(\"W\").field(&self.0).finish_non_exhaustive()",
    "f.debug_list().entries([self.0, self.0]).entry(&1u8).finish()",
    "f.debug_list().entries([self.0].iter()).finish_non_exhaustive()",
    "f.debug_set().entries([self.0]).entry(&\"x\").finish()",
    "f.debug_set().entries([1u8, 2]).finish_non_exhaustive()",
    "f.debug_map().entries([(1u8, self.0)]).entry(&2u8, &\"v\").key(&3u8).value(&self.0).finish()",
    "f.debug_map().entries([(1u8, 2u8)]).finish_non_exhaustive()",
    "f.write_str(\"x\")?; f.write_char('y')?; f.write_fmt(format_args!(\"{}\", self.0))",
    "match f.align() { Some(std::fmt::Alignment::Left) => f.write_str(\"L\"), Some(std::fmt::Alignment::Right) => f.write_str(\"R\"), Some(std::fmt::Alignment::Center) => f.write_str(\"C\"), None => f.write_str(\"-\") }",
    "std::fmt::Display::fmt(&self.0, f)",
    "std::fmt::Debug::fmt(&self.0, f)",
    "std::fmt::LowerHex::fmt(&self.0, f)",
    "std::fmt::UpperHex::fmt(&self.0, f)",
    "std::fmt::Binary::fmt(&self.0, f)",
    "std::fmt::Octal::fmt(&self.0, f)",
    "std::fmt::LowerExp::fmt(&self.0, f)",
    "std::fmt::UpperExp::fmt(&self.0, f)",
    "if f.alternate() { write!(f, \"alt{}\", self.0) } else { write!(f, \"{:>w$}\", self.0, w = f.width().unwrap_or(1)) }",
    "let s = std::fmt::format(format_args!(\"{:03}\", self.0)); f.pad(&s)",
    "std::fmt::write(f, format_args!(\"{}{}\", self.0, 1))",
];

const FMT_TRAITS: &[&str] = &["Display", "Debug", "LowerHex", "UpperHex", "Binary", "Octal", "LowerExp", "UpperExp"];

const FMT_USES: &[&str] = &[
    "h.put(&format!(\"{}\", W(a))); h.put(&format!(\"{:>9}\", W(b))); h.put(&format!(\"{:<9}|\", W(c))); h.put(&format!(\"{:^9}|\", W(a))); h.put(&format!(\"{:*^9}\", W(b)));",
    "h.put(&format!(\"{:+}\", W(a))); h.put(&format!(\"{:08}\", W(b))); h.put(&format!(\"{:#}\", W(c))); h.put(&format!(\"{:.3}\", W(a))); h.put(&format!(\"{:10.2}\", W(b))); h.put(&format!(\"{:-}\", W(c)));",
    "h.put(&format!(\"{:?}\", W(a))); h.put(&format!(\"{:#?}\", W(b))); h.put(&format!(\"{:>12?}\", W(c))); h.put(&format!(\"{:08?}\", W(a))); h.put(&format!(\"{:x?}\", W(b))); h.put(&format!(\"{:#X?}\", W(c)));",
    "h.put(&format!(\"{:x}\", W(a))); h.put(&format!(\"{:#x}\", W(b))); h.put(&format!(\"{:X}\", W(c))); h.put(&format!(\"{:#010X}\", W(a))); h.put(&format!(\"{:b}\", W(b))); h.put(&format!(\"{:#b}\", W(c))); h.put(&format!(\"{:o}\", W(a))); h.put(&format!(\"{:#o}\", W(b)));",
    "h.put(&format!(\"{:e}\", W(a))); h.put(&format!(\"{:E}\", W(b))); h.put(&format!(\"{:12e}|\", W(c))); h.put(&format!(\"{:.2e}\", W(a))); h.put(&format!(\"{:#e}\", W(b)));",
    "h.put(&W(a).to_string()); h.put(&format!(\"{0}{0:?}{1}\", W(a), W(b))); h.put(&format!(\"{w}{w:?}\", w = W(c))); h.put(&format!(\"{:1$}\", W(a), 7)); h.put(&format!(\"{:.*}\", 2, W(b))); h.put(&format!(\"{:>w$.p$}\", W(c), w = 8, p = 1));",
    "let mut s = String::new(); { use std::fmt::Write; write!(s, \"{}{:?}\", W(a), W(b)).unwrap(); writeln!(s, \"{:x}\", W(c)).unwrap(); s.write_fmt(format_args!(\"{:>4}\", W(a))).unwrap(); } h.put(&s); h.put(&format!(\"{}\", format_args!(\"{}-{}\", W(a), W(b)))); h.put(&format_args!(\"{}\", W(c)).to_string());",
    "h.put(&format!(\"{:?}\", vec![W(a), W(b)])); h.put(&format!(\"{:#?}\", Some(W(c)))); h.put(&format!(\"{:?}\", (W(a), W(b)))); h.put(&format!(\"{:?}\", [W(c)])); h.put(&format!(\"{:?}\", Box::new(W(a)))); h.put(&format!(\"{:?}\", std::rc::Rc::new(W(b)))); h.put(&format!(\"{:?}\", std::cell::RefCell::new(W(c))));",
    "h.put(&format!(\"{:?}\", Ok::<W, W>(W(a)))); h.put(&format!(\"{:?}\", std::collections::BTreeMap::from([(1u8, W(a)), (2, W(b))]))); h.put(&format!(\"{:#?}\", std::collections::BTreeSet::from([1u8, 2]))); h.put(&format!(\"{:?}\", std::cell::Cell::new(a))); h.put(&format!(\"{:?}\", std::num::Wrapping(a))); h.put(&format!(\"{:?}\", 'c'..='e'));",
    "h.put(&format!(\"{}\", true)); h.put(&format!(\"{:5}|{:<5}|{:^5}|{:>5}\", true, false, 'x', \"ab\")); h.put(&format!(\"{:?} {:?}\", \"q\\\"\\n\", 'a')); h.put(&format!(\"{:#?}\", (1u8, \"x\", [1.5f32]))); h.put(&format!(\"{:5?}|{:<5?}|\", (), ()));",
    "h.put(&format!(\"{:p}\", std::ptr::null::<T>())); h.put(&format!(\"{:?}\", std::ptr::null::<u8>())); h.put(&format!(\"{:?}\", std::ptr::null_mut::<u8>().is_null())); h.put(&format!(\"{:?}\", std::time::Duration::from_micros(1500))); h.put(&format!(\"{:.1?}\", std::time::Duration::from_micros(1550))); h.put(&format!(\"{:?}\", std::cmp::Ordering::Less));",
];

fn fmt_family(rng: &mut Rng) -> String {
    let ty = *rng.pick(&["u8", "i8", "u16", "i16", "u32", "i32", "u64", "i64", "usize", "u128", "i128", "isize"]);
    let mut items = String::new();
    items += &format!("    type T = {ty};\n    #[derive(Clone, Copy)] struct W(T);\n");
    for tr in FMT_TRAITS {
        let body = *rng.pick(FMT_BODIES);
        // the numeric-delegating bodies exist for the matching trait only; everything else is fine anywhere
        let delegating = body.starts_with("std::fmt::") && body.contains("::fmt(&self.0");
        let body = if delegating { format!("std::fmt::{tr}::fmt(&self.0, f)") } else { body.to_string() };
        items += &format!("    impl std::fmt::{tr} for W {{ fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{ use std::fmt::Write; {body} }} }}\n");
    }
    let mut p = String::new();
    for v in ["a", "b", "c"] {
        let l = lit(rng, ty);
        p += &format!("        let {v}: T = {};\n", bbv(rng, l));
    }
    let n = rng.range(4, 9);
    let body = emit(rng, &p, &[FMT_USES], n);
    wrap(&items, &body)
}

// ---------------------------------------------------------------------------------------------------- io

const IO: &[&str] = &[
    "use std::io::Read; let mut r: &[u8] = data; let mut buf = [0u8; 5]; h.put(&r.read(&mut buf).ok()); h.put(&buf); h.put(&r.len()); let mut rest = Vec::new(); h.put(&r.read_to_end(&mut rest).ok()); h.put(&rest); h.put(&r.read(&mut buf).ok());",
    "use std::io::Read; let mut r: &[u8] = data; let mut buf = vec![0u8; n]; h.put(&r.read_exact(&mut buf).map_err(|e| e.kind() == std::io::ErrorKind::UnexpectedEof)); h.put(&buf); let mut s = String::new(); h.put(&data.read_to_string(&mut s).is_ok()); h.put(&s);",
    "use std::io::Read; h.put(&data.bytes().map(|b| b.unwrap() as u32).sum::<u32>()); h.put(&data.take(n as u64).bytes().count()); h.put(&(&data[..n.min(data.len())]).chain(&data[..]).bytes().count()); let mut t = data.take(3); h.put(&t.limit()); t.set_limit(n as u64); h.put(&t.limit()); h.put(&t.get_ref().len());",
    "use std::io::{Read, Seek, SeekFrom}; let mut c = std::io::Cursor::new(data.to_vec()); let mut b2 = [0u8; 3]; h.put(&c.read(&mut b2).ok()); h.put(&c.position()); h.put(&c.seek(SeekFrom::Start(n as u64)).ok()); h.put(&c.seek(SeekFrom::End(-1)).ok()); h.put(&c.seek(SeekFrom::Current(-2)).ok()); h.put(&c.seek(SeekFrom::Current(-100)).is_err()); c.rewind().unwrap(); h.put(&c.stream_position().ok()); c.seek_relative(2).unwrap(); h.put(&c.position()); c.set_position(1); h.put(&c.get_ref().len()); h.put(&c.into_inner().len());",
    "use std::io::{Read, Write, Seek}; let mut c = std::io::Cursor::new(Vec::new()); h.put(&c.write(data).ok()); h.put(&c.write_all(&data[..n.min(data.len())]).is_ok()); c.set_position(2); h.put(&c.write(b\"zz\").ok()); h.put(&c.flush().is_ok()); h.put(&c.get_ref().len()); h.put(&c.get_mut().len()); h.put(&c.into_inner());",
    "use std::io::{BufRead, BufReader}; let mut r = BufReader::new(data); let mut line = String::new(); h.put(&r.read_line(&mut line).ok()); h.put(&line); h.put(&r.buffer().len()); h.put(&r.get_ref().len()); line.clear(); h.put(&r.read_line(&mut line).ok()); h.put(&line); let mut v = Vec::new(); h.put(&r.read_until(b' ', &mut v).ok()); h.put(&v); h.put(&r.skip_until(b'o').ok()); h.put(&r.lines().count());",
    "use std::io::{BufRead, BufReader}; let r = BufReader::with_capacity(4, data); h.put(&r.capacity()); h.put(&r.lines().map(|l| l.unwrap().len()).collect::<Vec<_>>()); let r2 = BufReader::new(data); h.put(&r2.split(b'\\n').map(|s| s.unwrap().len()).collect::<Vec<_>>()); let mut r3 = BufReader::new(data); let buf = r3.fill_buf().unwrap().len(); r3.consume(buf.min(2)); h.put(&r3.buffer().len()); h.put(&buf);",
    "use std::io::{BufRead, Cursor, Seek}; let mut r = std::io::BufReader::new(Cursor::new(data.to_vec())); let mut s = String::new(); r.read_line(&mut s).unwrap(); h.put(&r.seek_relative(-1).is_ok()); h.put(&r.stream_position().ok()); let mut r2 = Cursor::new(data); h.put(&r2.fill_buf().unwrap().len()); r2.consume(1); h.put(&r2.position()); h.put(&r2.get_ref().len());",
    "use std::io::Write; let mut w: Vec<u8> = Vec::new(); h.put(&w.write(data).ok()); w.write_all(b\"!\").unwrap(); write!(w, \"{}-{:?}\", n, text).unwrap(); writeln!(w).unwrap(); w.flush().unwrap(); h.put(&w); h.put(&w.write_vectored(&[std::io::IoSlice::new(b\"a\"), std::io::IoSlice::new(b\"bc\")]).ok());",
    "use std::io::Write; let mut buf = [0u8; 6]; { let mut w: &mut [u8] = &mut buf; h.put(&w.write(data).ok()); h.put(&w.write_all(b\"abcdefgh\").is_err()); h.put(&w.len()); } h.put(&buf);",
    "use std::io::{BufWriter, LineWriter, Write}; let mut w = BufWriter::new(Vec::new()); w.write_all(data).unwrap(); h.put(&w.buffer().len()); h.put(&w.get_ref().len()); h.put(&(w.capacity() >= 1)); w.write_all(b\"x\\n\").unwrap(); h.put(&w.into_inner().map(|v| v.len()).ok()); let mut l = LineWriter::new(Vec::new()); l.write_all(b\"ab\\ncd\").unwrap(); h.put(&l.get_ref().len()); h.put(&l.into_inner().map(|v| v.len()).ok());",
    "use std::io::{BufWriter, Write}; let mut w = BufWriter::with_capacity(2, Vec::new()); w.write_all(data).unwrap(); let (inner, buf) = w.into_parts(); h.put(&inner.len()); h.put(&buf.is_ok()); let mut w2 = BufWriter::new(Vec::new()); w2.write_all(b\"z\").unwrap(); w2.flush().unwrap(); h.put(&w2.get_mut().len());",
    "use std::io::{Read, Write}; let mut s = std::io::sink(); h.put(&s.write(data).ok()); let mut e = std::io::empty(); let mut b = [0u8; 2]; h.put(&e.read(&mut b).ok()); let mut r = std::io::repeat(b'x').take(n as u64); let mut o = String::new(); r.read_to_string(&mut o).unwrap(); h.put(&o); let mut out = Vec::new(); h.put(&std::io::copy(&mut &data[..], &mut out).ok()); h.put(&out); h.put(&std::io::read_to_string(&data[..]).ok());",
    "let e = std::io::Error::new(std::io::ErrorKind::InvalidData, \"bad\"); h.put(&(e.kind() == std::io::ErrorKind::InvalidData)); h.put(&e.to_string()); h.put(&e.get_ref().map(|x| x.to_string())); h.put(&e.raw_os_error()); h.put(&format!(\"{:?}\", e.kind())); let o = std::io::Error::other(\"oops\"); h.put(&o.to_string()); h.put(&std::io::Error::from(std::io::ErrorKind::NotFound).to_string()); h.put(&e.into_inner().map(|b| b.to_string()));",
    "use std::io::{BufWriter, Write}; let w = BufWriter::new(std::io::Cursor::new(vec![0u8; 3])); let r = w.into_inner(); h.put(&r.is_ok()); let mut w2 = BufWriter::new(Vec::<u8>::new()); w2.write_all(data).unwrap(); let e: Result<Vec<u8>, std::io::IntoInnerError<BufWriter<Vec<u8>>>> = w2.into_inner(); h.put(&e.map(|v| v.len()).ok());",
    "use std::io::Read; let mut c = std::io::Cursor::new(data.to_vec()); let mut ch = (&mut c).take(2).chain(&b\"..\"[..]); let mut s = Vec::new(); ch.read_to_end(&mut s).unwrap(); h.put(&s); h.put(&ch.get_ref().0.limit()); h.put(&ch.get_ref().1.len()); let (a1, b1) = ch.into_inner(); h.put(&b1.len());",
];

// ---------------------------------------------------------------------------------------------------- atomics

const ATOMIC: &[&str] = &[
    "let x = A::new(a); h.put(&x.load(Relaxed)); x.store(b, Release); h.put(&x.load(Acquire)); h.put(&x.swap(c, AcqRel)); h.put(&x.load(SeqCst)); h.put(&x.compare_exchange(c, a, SeqCst, Relaxed)); h.put(&x.compare_exchange(c, b, SeqCst, SeqCst)); h.put(&x.load(Relaxed)); h.put(&x.into_inner());",
    "let x = A::new(a); h.put(&x.fetch_add(b, Relaxed)); h.put(&x.fetch_sub(c, Relaxed)); h.put(&x.fetch_and(b, Relaxed)); h.put(&x.fetch_or(c, Relaxed)); h.put(&x.fetch_xor(a, Relaxed)); h.put(&x.fetch_nand(b, Relaxed)); h.put(&x.fetch_max(c, Relaxed)); h.put(&x.fetch_min(a, Relaxed)); h.put(&x.load(Relaxed));",
    "let mut x = A::new(a); *x.get_mut() = b; h.put(&x.load(Relaxed)); h.put(&x.fetch_update(SeqCst, SeqCst, |p| if p == b { Some(c) } else { None })); h.put(&x.fetch_update(SeqCst, SeqCst, |p| if p == b { Some(a) } else { None })); h.put(&x.try_update(SeqCst, SeqCst, |p| p.checked_add(1))); h.put(&x.update(SeqCst, SeqCst, |p| p.wrapping_mul(2))); h.put(&x.load(Relaxed)); h.put(&A::default().load(Relaxed)); h.put(&format!(\"{:?}\", x)); h.put(&A::from(a).load(Relaxed));",
    "let x = A::new(a); h.put(&(x.compare_exchange_weak(a, b, SeqCst, Relaxed).is_ok() || x.load(Relaxed) == a));",
    "let ab = std::sync::atomic::AtomicBool::new(f); h.put(&ab.load(Relaxed)); h.put(&ab.fetch_and(true, SeqCst)); h.put(&ab.fetch_or(false, SeqCst)); h.put(&ab.fetch_xor(true, SeqCst)); h.put(&ab.fetch_nand(true, SeqCst)); h.put(&ab.fetch_not(SeqCst)); h.put(&ab.swap(f, SeqCst)); h.put(&ab.compare_exchange(f, !f, SeqCst, SeqCst)); h.put(&ab.into_inner()); h.put(&std::sync::atomic::AtomicBool::default().load(Relaxed));",
    "std::sync::atomic::fence(SeqCst); std::sync::atomic::compiler_fence(Acquire); std::hint::spin_loop(); let x = A::new(a); h.put(&x.fetch_add(0, SeqCst)); h.put(&std::sync::atomic::AtomicUsize::new(k).fetch_add(k, Relaxed)); let arr = [A::new(a), A::new(b)]; h.put(&arr.iter().map(|z| z.load(Relaxed)).collect::<Vec<_>>()); h.put(&(std::mem::size_of::<A>() == std::mem::size_of::<T>()));",
];

// ---------------------------------------------------------------------------------------------------- sync

const SYNC: &[&str] = &[
    "use std::sync::Mutex; let m = Mutex::new(a); { let mut g = m.lock().unwrap(); *g = g.wrapping_add(b); h.put(&*g); h.put(&m.try_lock().is_err()); } h.put(&m.try_lock().map(|g| *g).ok()); h.put(&m.is_poisoned()); let mut m2 = Mutex::new(vec![a]); m2.get_mut().unwrap().push(b); h.put(&m2.lock().unwrap().len()); h.put(&m2.into_inner().unwrap()); h.put(&Mutex::new(c).lock().map(|g| *g).unwrap());",
    "use std::sync::RwLock; let l = RwLock::new(a); { let r1 = l.read().unwrap(); let r2 = l.read().unwrap(); h.put(&(*r1, *r2)); h.put(&l.try_write().is_err()); } { let mut w = l.write().unwrap(); *w = b; h.put(&l.try_read().is_err()); } h.put(&*l.read().unwrap()); h.put(&l.is_poisoned()); let mut l2 = RwLock::new(c); *l2.get_mut().unwrap() = a; h.put(&l2.into_inner().unwrap());",
    "use std::sync::OnceLock; let o: OnceLock<T> = OnceLock::new(); h.put(&o.get()); h.put(&o.set(a)); h.put(&o.set(b)); h.put(&o.get()); h.put(&o.get_or_init(|| c)); let mut o2: OnceLock<T> = OnceLock::new(); h.put(&o2.get_or_init(|| b)); h.put(&o2.take()); h.put(&o2.into_inner()); let o3 = OnceLock::from(a); h.put(&o3.get());",
    "use std::sync::{LazyLock, Once}; let l = LazyLock::new(|| a.wrapping_add(b)); h.put(&*l); h.put(&*LazyLock::force(&l)); static ONCE: Once = Once::new(); let mut n = 0; ONCE.call_once(|| n += 1); ONCE.call_once(|| n += 1); h.put(&n); h.put(&ONCE.is_completed());",
    "use std::sync::mpsc; let (tx, rx) = mpsc::channel(); tx.send(a).unwrap(); tx.send(b).unwrap(); let tx2 = tx.clone(); tx2.send(c).unwrap(); h.put(&rx.try_recv().ok()); h.put(&rx.recv().ok()); drop(tx); drop(tx2); h.put(&rx.try_iter().collect::<Vec<T>>()); h.put(&rx.recv().is_err()); h.put(&rx.try_recv().is_err()); h.put(&rx.recv_timeout(std::time::Duration::from_millis(1)).is_err());",
    "use std::sync::mpsc; let (tx, rx) = mpsc::sync_channel(2); h.put(&tx.try_send(a).is_ok()); h.put(&tx.send(b).is_ok()); h.put(&tx.try_send(c).is_err()); drop(tx); h.put(&rx.iter().collect::<Vec<T>>()); h.put(&rx.recv().is_err());",
    "let t = std::thread::spawn(move || a.wrapping_add(b)); h.put(&(t.is_finished() || true)); h.put(&t.join().ok()); let r = std::thread::scope(|s| { let h1 = s.spawn(|| c.wrapping_mul(2)); let h2 = s.spawn(|| a.wrapping_sub(b)); (h1.join().unwrap(), h2.join().unwrap()) }); h.put(&r); let bad = std::thread::spawn(|| { if true { panic!(\"x\"); } 1u8 }).join(); h.put(&bad.is_err());",
    "use std::sync::{Arc, Mutex}; let m = Arc::new(Mutex::new(0 as T)); let hs: Vec<_> = (0..3).map(|i| { let m = Arc::clone(&m); std::thread::spawn(move || { let mut g = m.lock().unwrap(); *g = g.wrapping_add(a.wrapping_add(i)); }) }).collect(); for t in hs { t.join().unwrap(); } h.put(&*m.lock().unwrap()); h.put(&Arc::strong_count(&m));",
    "use std::sync::{Arc, Mutex}; let m = Arc::new(Mutex::new(a)); let m2 = Arc::clone(&m); let _ = std::thread::spawn(move || { let _g = m2.lock().unwrap(); panic!(\"poison\"); }).join(); h.put(&m.is_poisoned()); h.put(&m.lock().is_err()); h.put(&m.lock().map_err(|e| **e.get_ref()).unwrap_err()); h.put(&m.lock().unwrap_or_else(|e| e.into_inner()).wrapping_add(1)); m.clear_poison(); h.put(&m.is_poisoned());",
    "use std::sync::{Condvar, Mutex, Barrier}; let pair = (Mutex::new(false), Condvar::new()); { let mut g = pair.0.lock().unwrap(); *g = true; pair.1.notify_one(); pair.1.notify_all(); } let (g, to) = pair.1.wait_timeout(pair.0.lock().unwrap(), std::time::Duration::from_millis(1)).unwrap(); h.put(&*g); h.put(&to.timed_out()); let b = Barrier::new(1); h.put(&b.wait().is_leader());",
    "thread_local! { static TL: std::cell::Cell<u64> = const { std::cell::Cell::new(5) }; static TR: std::cell::RefCell<Vec<u64>> = std::cell::RefCell::new(vec![1]); } TL.with(|c| c.set(c.get() + 1)); h.put(&TL.get()); TL.set(a as u64); h.put(&TL.replace(b as u64)); h.put(&TL.take()); TR.with_borrow_mut(|v| v.push(c as u64)); h.put(&TR.with_borrow(|v| v.len())); h.put(&TR.take()); TR.set(vec![7]); h.put(&TR.replace(vec![]).len()); h.put(&TL.try_with(|c| c.get()).ok()); h.put(&TR.with(|r| r.borrow().len()));",
];

// ---------------------------------------------------------------------------------------------------- custom iterator, Deref, FromStr ...

const CUSTOM_ITEMS: &str = "    #[derive(Clone)] struct It { lo: T, hi: T }
    impl Iterator for It { type Item = T; fn next(&mut self) -> Option<T> { if self.lo >= self.hi { None } else { let v = self.lo; self.lo = self.lo.wrapping_add(1); Some(v) } } }
    impl DoubleEndedIterator for It { fn next_back(&mut self) -> Option<T> { if self.lo >= self.hi { None } else { self.hi = self.hi.wrapping_sub(1); Some(self.hi) } } }
    struct Wrap(Vec<T>);
    impl std::ops::Deref for Wrap { type Target = [T]; fn deref(&self) -> &[T] { &self.0 } }
    impl std::ops::DerefMut for Wrap { fn deref_mut(&mut self) -> &mut [T] { &mut self.0 } }
    impl std::ops::Index<usize> for Wrap { type Output = T; fn index(&self, i: usize) -> &T { &self.0[i % self.0.len().max(1)] } }
    impl std::ops::IndexMut<usize> for Wrap { fn index_mut(&mut self, i: usize) -> &mut T { let n = self.0.len().max(1); &mut self.0[i % n] } }
    #[derive(Debug, PartialEq)] struct Pt(i16, i16);
    impl std::str::FromStr for Pt { type Err = std::num::ParseIntError; fn from_str(s: &str) -> Result<Pt, Self::Err> { let (a, b) = s.split_once(',').unwrap_or((s, \"0\")); Ok(Pt(a.trim().parse()?, b.trim().parse()?)) } }
    #[derive(Debug)] struct MyErr(std::num::ParseIntError);
    impl std::fmt::Display for MyErr { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, \"my error\") } }
    impl std::error::Error for MyErr { fn source(&self) -> Option<&(dyn std::error::Error + 'static)> { Some(&self.0) } }
    #[derive(Debug, Default, Clone, PartialEq)] struct Big { v: Vec<T>, s: String }
    #[derive(Debug)] struct FailW;
    impl std::io::Write for FailW { fn write(&mut self, _b: &[u8]) -> std::io::Result<usize> { Err(std::io::Error::other(\"write failed\")) } fn flush(&mut self) -> std::io::Result<()> { Err(std::io::Error::other(\"flush failed\")) } }
    #[derive(Default)] struct Cnt(u64);
    impl std::hash::Hasher for Cnt { fn write(&mut self, b: &[u8]) { for x in b { self.0 = self.0.wrapping_mul(31).wrapping_add(*x as u64); } } fn finish(&self) -> u64 { self.0 } }
";

const CUSTOM: &[&str] = &[
    "let it = It { lo: a.min(b), hi: a.max(b).min(a.min(b).saturating_add(20)) }; h.put(&it.clone().collect::<Vec<T>>()); h.put(&it.clone().rev().collect::<Vec<T>>()); h.put(&it.clone().sum::<T>()); h.put(&it.clone().count()); h.put(&it.clone().last()); h.put(&it.clone().nth(k)); h.put(&it.clone().nth_back(k)); h.put(&it.clone().size_hint());",
    "let it = It { lo: 0, hi: (k + 4) as T }; h.put(&it.clone().try_fold(0 as T, |s, p| s.checked_add(p))); h.put(&it.clone().try_rfold(0 as T, |s, p| s.checked_add(p * 2))); h.put(&it.clone().rfold(0 as T, |s, p| s.wrapping_mul(3).wrapping_add(p))); h.put(&it.clone().rfind(|p| p % 2 == 1)); h.put(&it.clone().find(|p| *p > 1)); h.put(&it.clone().rev().position(|p| p == 1));",
    "let it = It { lo: 0, hi: (k + 6) as T }; h.put(&it.clone().step_by(2).collect::<Vec<_>>()); h.put(&it.clone().skip(2).take(3).collect::<Vec<_>>()); h.put(&it.clone().zip(it.clone().skip(1)).map(|(x, y)| x + y).collect::<Vec<_>>()); h.put(&it.clone().chain(it.clone().rev()).collect::<Vec<_>>()); h.put(&it.clone().enumerate().filter(|(i, _)| i % 2 == 0).count()); h.put(&it.clone().peekable().peek().copied()); h.put(&it.clone().cycle().take(10).collect::<Vec<_>>());",
    "let it = It { lo: 0, hi: (k + 6) as T }; h.put(&it.clone().map_while(|p| if p < 4 { Some(p * 2) } else { None }).collect::<Vec<_>>()); h.put(&it.clone().scan(0 as T, |s, p| { *s += p; Some(*s) }).collect::<Vec<_>>()); h.put(&it.clone().flat_map(|p| [p, p]).count()); h.put(&it.clone().inspect(|_| {}).fuse().count()); h.put(&it.clone().partition::<Vec<T>, _>(|p| p % 2 == 0)); h.put(&it.clone().map(|p| (p, p.wrapping_neg())).unzip::<T, T, Vec<T>, Vec<T>>());",
    "let it = It { lo: 0, hi: (k + 6) as T }; h.put(&it.clone().min_by_key(|p| (*p as i32 - 3).abs())); h.put(&it.clone().max_by(|x, y| (x % 3).cmp(&(y % 3)))); h.put(&it.clone().reduce(|x, y| x.wrapping_mul(2).wrapping_add(y))); h.put(&it.clone().all(|p| p < 100)); h.put(&it.clone().any(|p| p == 3)); h.put(&it.clone().cmp(it.clone().skip(1))); h.put(&it.clone().eq(it.clone())); h.put(&it.clone().lt(it.clone().skip(1))); h.put(&it.clone().is_sorted());",
    "let mut it = It { lo: 0, hi: 9 }; h.put(&it.by_ref().take(2).collect::<Vec<_>>()); h.put(&it.next()); h.put(&it.next_back()); let mut pk = it.clone().peekable(); h.put(&pk.next_if(|p| *p == 3)); h.put(&pk.next_if_eq(&99)); h.put(&pk.next_if_map(|p| if p > 3 { Ok(p) } else { Err(p) }));",
    "let mut w = Wrap(vec![a, b, c]); h.put(&w.len()); h.put(&w.iter().copied().max()); w.sort(); h.put(&w.to_vec()); w.reverse(); h.put(&w.first().copied()); h.put(&w[5]); w[7] = a; h.put(&w[1]); w[0] = b; h.put(&w.0); for x in w.iter_mut() { *x = x.wrapping_add(1); } h.put(&w.contains(&a.wrapping_add(1))); h.put(&w.binary_search(&c).is_ok());",
    "h.put(&\"3, 4\".parse::<Pt>().map(|p| (p.0, p.1)).map_err(|e| e.to_string())); h.put(&\"x,1\".parse::<Pt>().map(|p| (p.0, p.1)).map_err(|e| e.to_string())); h.put(&\"7\".parse::<Pt>().map(|p| p == Pt(7, 0)).ok());",
    "use std::error::Error; let e = MyErr(\"x\".parse::<u8>().unwrap_err()); h.put(&e.to_string()); h.put(&e.source().map(|s| s.to_string())); h.put(&e.source().and_then(|s| s.source()).is_none()); let be: Box<dyn Error> = Box::new(e); h.put(&be.to_string()); h.put(&be.downcast_ref::<MyErr>().is_some()); h.put(&be.is::<MyErr>()); let be2: Box<dyn Error + Send + Sync> = From::from(\"plain\"); h.put(&be2.to_string()); h.put(&be2.source().is_none()); h.put(&format!(\"{:?}\", be2));",
    "let b1 = Big { v: vec![a], s: String::from(\"x\") }; let mut b2 = Big::default(); b2.clone_from(&b1); h.put(&(b1 == b2)); let mut sl = vec![0 as T; 2]; [a, b].as_slice().clone_into(&mut sl); h.put(&sl); let mut st = String::from(\"old\"); \"new\".clone_into(&mut st); h.put(&st); let o: Vec<T> = [c][..].to_owned(); h.put(&o); h.put(&format!(\"{:?}\", b1)); h.put(&format!(\"{:#?}\", b2));",
    "use std::hash::{BuildHasher, BuildHasherDefault, Hash, Hasher}; let mut s = Cnt::default(); s.write_u8(a as u8); s.write_u16(b as u16); s.write_u32(c as u32); s.write_u64(a as u64); s.write_u128(b as u128); s.write_usize(k); s.write_i8(a as i8); s.write_i16(b as i16); s.write_i32(c as i32); s.write_i64(a as i64); s.write_i128(b as i128); s.write_isize(k as isize); s.write(b\"xyz\"); h.put(&s.finish()); let bh = BuildHasherDefault::<Cnt>::default(); h.put(&bh.hash_one(a)); h.put(&bh.hash_one((a, b))); h.put(&bh.hash_one(\"s\")); h.put(&bh.hash_one(&[a, b][..]));",
    "use std::hash::{Hash, Hasher}; let mut s = std::collections::hash_map::DefaultHasher::new(); s.write_u8(1); s.write_u32(a as u32); s.write(b\"ab\"); s.write_usize(k); s.write_i64(b as i64); Hash::hash(&(a, b), &mut s); [a, b].hash(&mut s); \"str\".hash(&mut s); Some(c).hash(&mut s); true.hash(&mut s); 'c'.hash(&mut s); ().hash(&mut s); h.put(&s.finish());",
    "let r = a..b; h.put(&(r.start, r.end)); let it = It { lo: a.min(b), hi: a.max(b).min(a.min(b).saturating_add(5)) }; let v: Vec<T> = it.clone().collect(); h.put(&v); let boxed: Box<dyn Iterator<Item = T>> = Box::new(it.clone()); h.put(&boxed.count()); let dynd: &mut dyn DoubleEndedIterator<Item = T> = &mut it.clone(); h.put(&dynd.next_back()); h.put(&it.clone().by_ref().rev().collect::<Vec<_>>());",
    "use std::hash::{BuildHasher, BuildHasherDefault, Hasher}; let bh = BuildHasherDefault::<Cnt>::default(); let mut hs = bh.build_hasher(); hs.write_u8(a as u8); hs.write(b\"k\"); h.put(&hs.finish()); use std::ops::RemAssign; if !(a == T::MIN && (b | 1) == !0) { let mut x = a; RemAssign::rem_assign(&mut x, b | 1); h.put(&x); }",
    "use std::io::Write; let mut w = std::io::BufWriter::new(FailW); w.write_all(b\"abc\").unwrap(); let e = w.into_inner().err().unwrap(); h.put(&e.error().to_string()); h.put(&(e.error().kind() == std::io::ErrorKind::Other)); let mut w2 = e.into_inner(); h.put(&w2.flush().is_err()); let mut w3 = std::io::BufWriter::new(FailW); w3.write_all(b\"x\").unwrap(); let e3 = w3.into_inner().unwrap_err(); h.put(&e3.into_error().to_string());",
];

// ---------------------------------------------------------------------------------------------------- ffi, pin, ptr

const FFI: &[&str] = &[
    "use std::ffi::{CStr, CString}; let c = CString::new(text.replace('\\0', \"\")).unwrap(); h.put(&c.as_bytes().len()); h.put(&c.as_bytes_with_nul().len()); h.put(&c.to_str().ok()); h.put(&c.as_c_str().to_bytes().len()); h.put(&c.clone().into_bytes_with_nul().len()); h.put(&c.clone().into_bytes()); h.put(&c.clone().into_boxed_c_str().to_bytes().len()); h.put(&c.clone().into_string().ok()); h.put(&c.count_bytes()); h.put(&CString::new(\"a\\0b\").map(|c| c.as_bytes().len()).map_err(|e| e.nul_position()));",
    "use std::ffi::{CStr, CString}; h.put(&CStr::from_bytes_with_nul(b\"ab\\0\").map(|c| c.to_bytes().len()).ok()); h.put(&CStr::from_bytes_with_nul(b\"ab\").is_err()); h.put(&CStr::from_bytes_until_nul(b\"ab\\0cd\\0\").map(|c| c.to_bytes().to_vec()).ok()); h.put(&CStr::from_bytes_until_nul(b\"ab\").is_err()); h.put(&CString::from_vec_with_nul(vec![b'x', 0]).ok().map(|c| c.to_bytes().len())); h.put(&c\"lit\".to_bytes_with_nul()); h.put(&c\"é\".to_str().ok()); h.put(&c\"é\".to_string_lossy().len()); h.put(&c\"zz\".count_bytes()); h.put(&format!(\"{:?}\", c\"d\")); h.put(&c\"a\".to_owned().as_bytes().len()); h.put(&(c\"a\" == c\"a\")); h.put(&(c\"a\".as_ptr() == c\"a\".as_ptr() || true));",
    "use std::pin::Pin; let mut x = a; let mut p = Pin::new(&mut x); *p = b; h.put(&*p.as_ref()); h.put(&*p.as_mut().get_mut()); p.set(c); h.put(&*Pin::into_inner(p)); let p2 = Pin::new(&c); h.put(p2.get_ref()); h.put(&Pin::into_inner(p2)); let bp = Box::pin(a); h.put(&*bp); let mut bm = Box::pin(b); h.put(&*bm.as_mut().get_mut());",
    "use std::ptr; let x = a; let y = a; h.put(&ptr::eq(&x, &x)); h.put(&ptr::eq(&x, &y)); h.put(&ptr::eq(&[a, b][..], &[a, b][..])); h.put(&ptr::null::<T>().is_null()); h.put(&ptr::null_mut::<T>().is_null()); h.put(&ptr::addr_eq(&x, &x)); h.put(&(ptr::dangling::<T>().addr() == std::mem::align_of::<T>())); h.put(&ptr::without_provenance::<T>(k).addr()); h.put(&ptr::null::<T>().wrapping_add(k).addr()); h.put(&ptr::null::<T>().wrapping_byte_add(k).addr()); h.put(&(&x as *const T).is_aligned()); h.put(&ptr::from_ref(&x).cast::<u8>().is_null());",
    "use std::ptr::NonNull; let mut x = a; let nn = NonNull::from(&mut x); h.put(&(nn.as_ptr() == nn.as_ptr())); h.put(&(nn.addr().get() % std::mem::align_of::<T>())); h.put(&NonNull::<T>::dangling().addr().get()); h.put(&nn.cast::<u8>().as_ptr().is_null()); h.put(&NonNull::from_ref(&x).is_aligned()); h.put(&NonNull::<T>::without_provenance(std::num::NonZero::new(k + 1).unwrap()).addr().get()); h.put(&nn.align_offset(1)); h.put(&NonNull::slice_from_raw_parts(NonNull::<T>::dangling(), 0).len()); h.put(&(nn.with_addr(nn.addr()) == nn)); h.put(&(nn.map_addr(|p| p) == nn));",
    "std::mem::forget(a); std::mem::forget((a, b)); std::mem::forget(c); h.put(&a);",
    "let ar = std::array::from_ref(&a); h.put(&ar); let sl = std::slice::from_ref(&b); h.put(&sl); let mut m = c; let sm = std::slice::from_mut(&mut m); sm[0] = a; h.put(&m); let mut arr = [a, b]; h.put(&arr.as_mut_slice().len()); for r in arr.each_mut() { *r = c; } h.put(&arr); h.put(&arr.each_ref().len());",
    "use std::ops::ControlFlow; h.put(&ControlFlow::<T, T>::Break(a).break_ok().ok()); h.put(&ControlFlow::<T, T>::Continue(b).continue_ok().ok()); h.put(&ControlFlow::<T, T>::Break(c).continue_ok().is_err()); let mut pk = [a, b, c].into_iter().peekable(); h.put(&pk.next_if_map(|p| if p == a { Ok(p) } else { Err(p) })); h.put(&pk.next_if_map_mut(|p| if *p == b { Some(*p) } else { None }));",
    "let mut l = std::cell::LazyCell::new(|| a); *std::cell::LazyCell::force_mut(&mut l) = b; h.put(&*l); let cc = std::cell::Cell::new([a, b]); h.put(&cc.as_array_of_cells().len()); h.put(&std::iter::once_with(|| c).next()); h.put(&std::path::is_separator('/')); h.put(&std::path::is_separator('x'));",
    "h.put(&String::from_utf8(vec![0x61, 0xff, 0x62]).unwrap_err().into_utf8_lossy()); let e = std::ffi::CString::new(vec![0xffu8, b'a']).unwrap().into_string().unwrap_err(); h.put(&e.utf8_error().valid_up_to()); h.put(&e.into_cstring().as_bytes().len());",
    "let mut slots = [const { std::mem::MaybeUninit::<T>::uninit() }; 3]; h.put(&slots.write_copy_of_slice(&[a, b, c]).to_vec()); let mut s2 = [const { std::mem::MaybeUninit::<T>::uninit() }; 2]; h.put(&s2.write_clone_of_slice(&[b, a]).to_vec()); h.put(&Box::<[T]>::new_zeroed_slice(3).len()); h.put(&Box::<[T]>::new_uninit_slice(2).len()); h.put(&std::mem::size_of_val(&*Box::<T>::new_zeroed())); h.put(&std::rc::Rc::<[T]>::new_zeroed_slice(2).len()); h.put(&std::sync::Arc::<[T]>::new_uninit_slice(2).len()); h.put(&(std::rc::Rc::<T>::new_zeroed().as_ref() as *const _ as usize % 1));",
    "let mut x = a; let p = std::pin::Pin::new(&mut x); h.put(&*std::pin::Pin::into_ref(p)); h.put(&*std::pin::Pin::static_ref(&5u8)); h.put(&std::pin::Pin::static_ref(&7u8).get_ref());",
    "use std::io::Read; let mut r: &[u8] = b\"abcdef\"; let (mut b1, mut b2) = ([0u8; 2], [0u8; 3]); h.put(&r.read_vectored(&mut [std::io::IoSliceMut::new(&mut b1), std::io::IoSliceMut::new(&mut b2)]).ok()); h.put(&(b1, b2)); h.put(&r.len());",
];

fn atomic_family(rng: &mut Rng) -> String {
    let (name, ty) = *rng.pick(&[("AtomicU8", "u8"), ("AtomicI8", "i8"), ("AtomicU16", "u16"), ("AtomicI16", "i16"), ("AtomicU32", "u32"), ("AtomicI32", "i32"), ("AtomicU64", "u64"), ("AtomicI64", "i64"), ("AtomicUsize", "usize"), ("AtomicIsize", "isize")]);
    let mut items = format!("    type T = {ty};\n    use std::sync::atomic::{{{name} as A, Ordering::*}};\n");
    items.push_str("    ");
    items.pop();
    let mut p = String::new();
    for v in ["a", "b", "c"] {
        let l = lit(rng, ty);
        p += &format!("        let {v}: T = {};\n", bbv(rng, l));
    }
    let (k, f) = (rng.below(9), rng.chance(50));
    p += &format!("        let k: usize = {}; let f: bool = {};\n", bbv(rng, format!("{k}usize")), bbv(rng, format!("{f}")));
    let n = rng.range(4, 8);
    let body = emit(rng, &p, &[ATOMIC], n);
    wrap(&items, &body)
}

fn simple(rng: &mut Rng, table: &[&str], with_text: bool) -> String {
    let ty = *rng.pick(&["u8", "i8", "u16", "i16", "u32", "i32", "u64", "i64", "usize", "u128", "i128", "isize"]);
    let mut items = format!("    type T = {ty};\n");
    if std::ptr::eq(table, CUSTOM) {
        items += CUSTOM_ITEMS;
    }
    let mut p = String::new();
    for v in ["a", "b", "c"] {
        let l = if rng.chance(50) { lit(rng, ty) } else { small_ints(rng, ty) };
        p += &format!("        let {v}: T = {};\n", bbv(rng, l));
    }
    let (k, f) = (rng.below(9), rng.chance(50));
    p += &format!("        let k: usize = {}; let f: bool = {};\n", bbv(rng, format!("{k}usize")), bbv(rng, format!("{f}")));
    if with_text {
        let data = *rng.pick(&["hello world\\nsecond line\\n", "a b c", "", "x\\ny\\nz", "hello world, foo\\n\\nbar", "0123456789"]);
        let nv = format!("{}usize", rng.below(8));
        p += &format!("        let mut data: &[u8] = {}; let text: &str = {}; let n: usize = {};\n", bbv(rng, format!("b\"{data}\"")), bbv(rng, format!("\"{data}\"")), bbv(rng, nv));
    }
    let n = rng.range(4, 8);
    let body = emit(rng, &p, &[table], n);
    wrap(&items, &body)
}

pub fn generate(rng: &mut Rng) -> String {
    let which = pick_table(rng, &[("fmt", &[][..]), ("io", IO), ("atomic", ATOMIC), ("sync", SYNC), ("custom", CUSTOM), ("ffi", FFI)]);
    if std::ptr::eq(which, IO) {
        simple(rng, IO, true)
    } else if std::ptr::eq(which, ATOMIC) {
        atomic_family(rng)
    } else if std::ptr::eq(which, SYNC) {
        simple(rng, SYNC, false)
    } else if std::ptr::eq(which, CUSTOM) {
        simple(rng, CUSTOM, false)
    } else if std::ptr::eq(which, FFI) {
        simple(rng, FFI, true)
    } else {
        fmt_family(rng)
    }
}
