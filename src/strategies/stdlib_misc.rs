//! `stdlib` family: `Option`, `Result`, `bool`, `Ordering`, `cmp`, `mem`, cells, `Rc`/`Arc`/`Box`, ranges, `ControlFlow`,
//! `Duration`, tuples, `Default`/`Clone`/`From`/`Into`, `Reverse`, `Bound`.
//! Prelude: `T` (integer type), `a b c: T`, `o1 o2: Option<T>`, `r1 r2: Result<T, T>`, `k: usize`, `f: bool`.

use super::lit_plain as lit;
use super::stdlib::{bbv, emit, pick_table, wrap};
use crate::rng::Rng;

pub const NAMES: &[&str] = &["option", "result", "cmp", "mem", "cell", "smart"];

const OPTION: &[&str] = &[
    "h.put(&o1.is_some()); h.put(&o1.is_none()); h.put(&o1.is_some_and(|p| p > a)); h.put(&o1.is_none_or(|p| p > a)); h.put(&o1.unwrap_or(b)); h.put(&o1.unwrap_or_default()); h.put(&o1.unwrap_or_else(|| c)); h.put(&o1.map_or(a, |p| p.wrapping_add(b))); h.put(&o1.map_or_else(|| b, |p| p.wrapping_mul(2)));",
    "h.put(&o1.map(|p| p.wrapping_add(1))); h.put(&o1.and_then(|p| p.checked_add(a))); h.put(&o1.and(o2)); h.put(&o1.or(o2)); h.put(&o1.or_else(|| Some(a))); h.put(&o1.xor(o2)); h.put(&o1.filter(|p| *p > b)); h.put(&o1.zip(o2)); h.put(&o1.zip(o2).map(|(p, q)| p.wrapping_sub(q)));",
    "let mut x = o1; h.put(&x.take()); h.put(&x); h.put(&x.replace(a)); h.put(&x); h.put(&*x.insert(b)); h.put(&*x.get_or_insert(c)); h.put(&*x.get_or_insert_with(|| a)); h.put(&x.take_if(|p| *p == b)); h.put(&x);",
    "let mut x = o1; if let Some(p) = x.as_mut() { *p = p.wrapping_add(a); } h.put(&x); h.put(&x.as_ref().map(|p| *p));",
    "h.put(&o1.ok_or(a)); h.put(&o1.ok_or_else(|| b)); h.put(&o1.iter().count()); h.put(&o1.into_iter().chain(o2).collect::<Vec<_>>()); h.put(&o1.as_slice().len());",
    "h.put(&Some(o1).flatten()); h.put(&Some(Some(a)).flatten()); h.put(&Some(r1).transpose()); h.put(&o1.map(|p| (p, p)).unzip()); h.put(&Some((a, b)).unzip());",
    "h.put(&o1.as_ref().copied()); h.put(&o1.as_ref().cloned()); h.put(&Some(&a).copied()); h.put(&Some(&b).cloned()); h.put(&Some(&mut c.clone()).map(|p| *p).unwrap_or(0)); h.put(&o1.cmp(&o2)); h.put(&o1.partial_cmp(&o2)); h.put(&(o1 == o2)); h.put(&(o1 < o2)); h.put(&o1.max(o2)); h.put(&o1.min(o2));",
    "h.put(&Some(a).expect(\"some\")); h.put(&Some(b).unwrap()); h.put(&Option::<T>::None.is_none()); h.put(&Option::<T>::default()); h.put(&Some(a).iter().next()); h.put(&Option::from(a)); h.put(&Some(a).into_iter().sum::<T>()); h.put(&[o1, o2].iter().flatten().count()); h.put(&[o1, o2].into_iter().flatten().max());",
    "h.put(&[Some(a), o1].into_iter().collect::<Option<Vec<T>>>()); h.put(&[Some(a % 3), Some(b % 3)].into_iter().sum::<Option<T>>()); h.put(&[Some(a % 2), o1.map(|p| p % 2)].into_iter().product::<Option<T>>()); h.put(&o1.is_some_and(|p| p == a).then_some(b)); h.put(&f.then(|| a)); h.put(&f.then_some(b));",
    "{ use std::ops::*; let x = (o1, o2); h.put(&x); h.put(&x.0.zip(x.1)); h.put(&(x.0.is_some() as u8 + x.1.is_some() as u8)); h.put(&Some(a).map(|p| p.wrapping_add(1)).filter(|p| *p != 0).and_then(|p| p.checked_mul(2)).unwrap_or(0)); }",
    "h.put(&o1.inspect(|_| {}).is_some()); let mut cnt = 0; let _ = o1.inspect(|_| cnt += 1); h.put(&cnt); h.put(&o1.is_some_and(|p| { cnt += 1; p > a })); h.put(&cnt);",
    "let mut x = o1; h.put(&*x.get_or_insert_default()); let y: Option<String> = Some(String::from(\"ab\")); h.put(&y.as_deref()); let mut z = y.clone(); h.put(&z.as_deref_mut().map(|s| { s.make_ascii_uppercase(); s.len() })); h.put(&z); h.put(&o1.map_or_default(|p| p.wrapping_add(1))); let mut w = Some(vec![a]); h.put(&w.as_mut_slice().len()); h.put(&Some(a).as_slice().len());",
];

const RESULT: &[&str] = &[
    "h.put(&r1.is_ok()); h.put(&r1.is_err()); h.put(&r1.is_ok_and(|p| p > a)); h.put(&r1.is_err_and(|p| p > a)); h.put(&r1.ok()); h.put(&r1.err()); h.put(&r1.unwrap_or(b)); h.put(&r1.unwrap_or_default()); h.put(&r1.unwrap_or_else(|e| e.wrapping_add(c))); h.put(&r1.map_or(a, |p| p.wrapping_add(b))); h.put(&r1.map_or_else(|e| e, |p| p.wrapping_neg()));",
    "h.put(&r1.map(|p| p.wrapping_add(1))); h.put(&r1.map_err(|e| e.wrapping_sub(1))); h.put(&r1.and_then(|p| if p == a { Err(p) } else { Ok(p) })); h.put(&r1.or_else(|e| if e == b { Err(e) } else { Ok(e) })); h.put(&r1.and(r2)); h.put(&r1.or(r2)); h.put(&r1.as_ref().map(|p| *p)); h.put(&r1.as_ref().err().copied());",
    "h.put(&r1.iter().count()); h.put(&r1.into_iter().chain(r2).collect::<Vec<T>>()); h.put(&r1.ok().xor(r2.ok())); h.put(&r1.ok().zip(r2.err()));",
    "h.put(&Ok::<Option<T>, T>(o1).transpose()); h.put(&Ok::<T, T>(a).ok()); h.put(&Err::<T, T>(b).err()); h.put(&Ok::<T, T>(a).expect(\"ok\")); h.put(&Err::<T, T>(b).expect_err(\"err\")); h.put(&Ok::<T, T>(a).unwrap()); h.put(&Err::<T, T>(b).unwrap_err()); h.put(&Ok::<Result<T, T>, T>(r1).and_then(|x| x));",
    "h.put(&r1.cmp(&r2)); h.put(&r1.partial_cmp(&r2)); h.put(&(r1 == r2)); h.put(&(r1 < r2)); h.put(&r1.max(r2)); h.put(&r1.min(r2)); h.put(&r1.clone().is_ok()); h.put(&r1.inspect(|_| {}).is_ok()); h.put(&r1.inspect_err(|_| {}).is_ok());",
    "h.put(&[r1, r2].into_iter().collect::<Result<Vec<T>, T>>()); h.put(&[Ok::<T, T>(a % 3), Ok(b % 3)].into_iter().sum::<Result<T, T>>()); h.put(&[r1.map(|p| p % 2), Ok(a % 2)].into_iter().product::<Result<T, T>>()); h.put(&[r1, r2].iter().filter(|r| r.is_ok()).count()); h.put(&[r1, r2].into_iter().flatten().count());",
    "let x = u8::try_from(a).map(|p| p as T); h.put(&x.is_ok()); h.put(&x.map_err(|e| e.to_string())); let y: Result<u32, std::num::ParseIntError> = \"12\".parse(); h.put(&y.map_err(|e| e.to_string())); let z: Result<bool, std::str::ParseBoolError> = \"maybe\".parse(); h.put(&z.map_err(|e| e.to_string()));",
    "let q: Result<T, Box<dyn std::error::Error>> = Err(\"boom\".into()); h.put(&q.map_err(|e| e.to_string())); let q2: Result<T, String> = Err(format!(\"{a}\")); h.put(&q2.clone().unwrap_or_default());",
    "let y: Result<String, T> = Ok(String::from(\"ab\")); h.put(&y.as_deref().map(|s| s.len())); let mut z = y.clone(); h.put(&z.as_deref_mut().map(|s| { s.make_ascii_uppercase(); s.len() }).ok()); h.put(&r1.map_or_default(|p| p.wrapping_add(1))); h.put(&r2.unwrap_or_default());",
];

const CMP: &[&str] = &[
    "h.put(&std::cmp::Ordering::Less.reverse()); h.put(&a.cmp(&b).then(b.cmp(&c))); h.put(&a.cmp(&b).then_with(|| c.cmp(&a))); h.put(&a.cmp(&b).is_lt()); h.put(&a.cmp(&b).is_le()); h.put(&a.cmp(&b).is_gt()); h.put(&a.cmp(&b).is_ge()); h.put(&a.cmp(&b).is_eq()); h.put(&a.cmp(&b).is_ne());",
    "h.put(&(std::cmp::Ordering::Equal as i8)); h.put(&(std::cmp::Ordering::Greater as i32)); h.put(&(a.cmp(&b) == std::cmp::Ordering::Less)); h.put(&a.cmp(&b).cmp(&b.cmp(&c))); h.put(&a.cmp(&a).max(b.cmp(&c))); h.put(&match a.cmp(&b) { std::cmp::Ordering::Less => 1u8, std::cmp::Ordering::Equal => 2, std::cmp::Ordering::Greater => 3 });",
    "{ use std::cmp::Reverse; h.put(&Reverse(a).cmp(&Reverse(b))); h.put(&(Reverse(a) < Reverse(b))); h.put(&Reverse(a).0); h.put(&[Reverse(a), Reverse(b), Reverse(c)].iter().max().map(|r| r.0)); let mut v = vec![Reverse(a), Reverse(b), Reverse(c)]; v.sort(); h.put(&v.iter().map(|r| r.0).collect::<Vec<T>>()); }",
    "h.put(&std::cmp::max(a, b)); h.put(&std::cmp::min(b, c)); h.put(&std::cmp::max_by_key(a, b, |p| p.count_ones())); h.put(&std::cmp::min_by_key(a, c, |p| p.leading_zeros())); h.put(&std::cmp::max_by(b, c, |p, q| q.cmp(p))); h.put(&std::cmp::min_by(b, c, |p, q| q.cmp(p)));",
    "h.put(&(a, b).cmp(&(b, a))); h.put(&(a, b, c).partial_cmp(&(a, b, c))); h.put(&((a, b) < (a, c))); h.put(&(a, b).max((b, a))); h.put(&(a, b).min((c, c))); h.put(&[a, b].cmp(&[a, c])); h.put(&([a, b] < [b, a])); h.put(&((a, b) == (a, b))); h.put(&((a,), (b,)).0.0);",
    "h.put(&(a, \"x\").cmp(&(a, \"y\"))); h.put(&(f, a).cmp(&(!f, a))); h.put(&('a', b).max(('b', a))); h.put(&(Some(a), None::<T>).cmp(&(None, Some(b)))); h.put(&[Some(a), None].iter().max()); h.put(&[Some(a), None].iter().min());",
    "h.put(&f.cmp(&!f)); h.put(&(f & !f)); h.put(&(f ^ true)); h.put(&(!f)); h.put(&f.then_some(a)); h.put(&(f as u8)); h.put(&(f as i32 + 1)); h.put(&bool::from(f)); h.put(&f.max(false)); h.put(&f.min(true)); h.put(&f.to_string());",
    "h.put(&true.partial_cmp(&false)); h.put(&(true > false)); h.put(&u8::from(f)); h.put(&i64::from(!f)); h.put(&format!(\"{f}{:?}\", !f)); h.put(&\"true\".parse::<bool>().unwrap_or(f)); h.put(&bool::default()); h.put(&f.clone());",
    "{ use std::ops::Bound; let b1 = Bound::Included(a); let b2: Bound<T> = Bound::Excluded(b); let b3 = Bound::<T>::Unbounded; h.put(&matches!(b1, Bound::Included(_))); h.put(&match b2 { Bound::Excluded(p) => p, _ => c }); h.put(&match b3 { Bound::Unbounded => 1u8, _ => 0 }); h.put(&(b1.map(|p| p.wrapping_add(1)) == Bound::Included(a.wrapping_add(1)))); h.put(&(b2.as_ref().cloned() == b2)); }",
    "h.put(&(a..b).contains(&c)); h.put(&(a..=b).contains(&c)); h.put(&(..b).contains(&a)); h.put(&(a..).contains(&b)); h.put(&(..=a).contains(&c)); h.put(&(a..b).is_empty()); h.put(&(a..=b).is_empty()); h.put(&(a..=b).start()); h.put(&(a..=b).end()); h.put(&(a..b).start); h.put(&(a..b).end);",
    "{ use std::ops::RangeBounds; let r = a..=b; h.put(&(r.start_bound() == std::ops::Bound::Included(&a))); h.put(&(r.end_bound() == std::ops::Bound::Included(&b))); h.put(&((..b).start_bound() == std::ops::Bound::Unbounded)); h.put(&((a..).end_bound() == std::ops::Bound::Unbounded)); h.put(&RangeBounds::contains(&(a..b), &c)); h.put(&std::ops::RangeInclusive::new(a, b).into_inner()); }",
    "{ use std::ops::*; let r1 = a..b; let r2 = a..=b; let r3 = ..b; let r4 = a..; let r5 = ..=b; let r6: RangeFull = ..; h.put(&(r1 == (a..b))); h.put(&(r2 == (a..=b))); h.put(&r3.end); h.put(&r4.start); h.put(&r5.end); h.put(&(r6 == ..)); h.put(&format!(\"{:?}{:?}{:?}{:?}{:?}\", r1, r2, r3, r4, r5)); }",
    "{ use std::ops::ControlFlow; let c1: ControlFlow<T, T> = if f { ControlFlow::Break(a) } else { ControlFlow::Continue(b) }; h.put(&c1.is_break()); h.put(&c1.is_continue()); h.put(&c1.break_value()); h.put(&c1.continue_value()); h.put(&c1.map_break(|p| p.wrapping_add(1)).break_value()); h.put(&c1.map_continue(|p| p.wrapping_add(2)).continue_value()); h.put(&[a, b, c].iter().try_fold(0 as T, |s, p| if *p == a { ControlFlow::Break(s) } else { ControlFlow::Continue(s.wrapping_add(*p)) }).is_break()); }",
];

const MEM: &[&str] = &[
    "let mut x = a; let mut y = b; std::mem::swap(&mut x, &mut y); h.put(&(x, y)); h.put(&std::mem::replace(&mut x, c)); h.put(&x); h.put(&std::mem::take(&mut y)); h.put(&y); let mut z = [a, b, c]; z.swap(0, 2); h.put(&z); let (p, q) = z.split_at_mut(1); std::mem::swap(&mut p[0], &mut q[1]); h.put(&z);",
    "h.put(&std::mem::size_of::<T>()); h.put(&std::mem::align_of::<T>()); h.put(&std::mem::size_of::<Option<T>>()); h.put(&std::mem::size_of::<(T, u8)>()); h.put(&std::mem::size_of::<[T; 3]>()); h.put(&std::mem::size_of_val(&a)); h.put(&std::mem::align_of_val(&b)); h.put(&std::mem::size_of::<Option<Box<T>>>()); h.put(&std::mem::size_of::<&T>());",
    "h.put(&std::mem::needs_drop::<T>()); h.put(&std::mem::needs_drop::<Vec<T>>()); h.put(&std::mem::size_of_val(&[a, b][..])); h.put(&std::mem::size_of_val(\"héllo\")); h.put(&(std::mem::discriminant(&o1) == std::mem::discriminant(&o2))); h.put(&(std::mem::discriminant(&r1) == std::mem::discriminant(&r2))); h.put(&std::mem::size_of::<std::cmp::Ordering>());",
    "let v = vec![a, b]; let w = std::mem::ManuallyDrop::new(v.clone()); h.put(&w.len()); h.put(&(*w == v)); let m = std::mem::ManuallyDrop::into_inner(w); h.put(&m); drop(m); let c2 = a; std::mem::drop(c2);",
    "let mut x = Some(a); let y = x.take(); h.put(&(x, y)); let mut v = vec![a, b]; let t = std::mem::take(&mut v); h.put(&(v.len(), t.len())); let mut s = String::from(\"ab\"); let u = std::mem::replace(&mut s, \"cd\".into()); h.put(&(s, u)); let mut o = (a, b); std::mem::swap(&mut o.0, &mut o.1); h.put(&o);",
    "h.put(&T::from_ne_bytes(a.to_ne_bytes())); h.put(&T::default()); h.put(&(<T as Default>::default())); h.put(&<(T, bool)>::default()); h.put(&<[T; 2]>::default()); h.put(&<Option<T>>::default()); h.put(&String::default()); h.put(&<Vec<T>>::default()); h.put(&<()>::default()); h.put(&<(T, T, T)>::default());",
    "h.put(&std::convert::identity(a)); h.put(&T::from(false)); h.put(&(a as i128).clamp(-5, 5)); h.put(&std::hint::black_box(b)); h.put(&Into::<T>::into(c)); let x: T = From::from(a); h.put(&x); let y: Option<T> = Some(a).into(); h.put(&y); let z: Vec<T> = [a, b].into(); h.put(&z); let bx: Box<T> = Box::from(a); h.put(&*bx);",
    "{ use std::convert::TryFrom; h.put(&u8::try_from(a).is_ok()); h.put(&<[T; 2]>::try_from(vec![a, b].as_slice()).ok()); h.put(&<(T, T)>::from([a, b])); h.put(&<[T; 2]>::from((a, b))); h.put(&<(T, T, T)>::from([a, b, c])); h.put(&u128::from(u64::MAX)); h.put(&i64::from(i32::MIN)); h.put(&f64::from(u32::MAX)); h.put(&char::from(b'a')); h.put(&u32::from('é')); }",
];

const CELL: &[&str] = &[
    "{ use std::cell::Cell; let x = Cell::new(a); h.put(&x.get()); x.set(b); h.put(&x.replace(c)); h.put(&x.get()); h.put(&x.take()); h.put(&x.get()); let y = Cell::new(a); x.swap(&y); h.put(&(x.get(), y.get())); h.put(&x.into_inner()); let z = Cell::new([a, b]); h.put(&z.get()); z.set([b, a]); h.put(&z.into_inner()); let s = Cell::new(a); h.put(&s.update(|p| p.wrapping_add(1))); }",
    "{ use std::cell::RefCell; let x = RefCell::new(vec![a]); x.borrow_mut().push(b); h.put(&x.borrow().len()); { let r1 = x.borrow(); let r2 = x.borrow(); h.put(&(r1.len(), r2[0])); h.put(&x.try_borrow_mut().is_err()); h.put(&x.try_borrow().is_ok()); } h.put(&x.try_borrow_mut().is_ok()); h.put(&x.replace(vec![c])); h.put(&x.take()); h.put(&x.into_inner()); }",
    "{ use std::cell::RefCell; let x = RefCell::new(a); *x.borrow_mut() = b; h.put(&*x.borrow()); { let mut m = x.borrow_mut(); *m = m.wrapping_add(1); h.put(&x.try_borrow().is_err()); } h.put(&x.replace_with(|p| p.wrapping_mul(2))); h.put(&x.borrow().cmp(&RefCell::new(c).borrow())); let y = RefCell::new(c); x.swap(&y); h.put(&(*x.borrow(), *y.borrow())); }",
    "{ use std::cell::{Ref, RefCell, RefMut}; let x = RefCell::new((a, vec![b])); let r = Ref::map(x.borrow(), |t| &t.0); h.put(&*r); drop(r); { let m = RefMut::map(x.borrow_mut(), |t| &mut t.1); h.put(&m.len()); } let (p, q) = Ref::map_split(x.borrow(), |t| (&t.0, &t.1)); h.put(&(*p, q.len())); }",
    "{ use std::cell::OnceCell; let x: OnceCell<T> = OnceCell::new(); h.put(&x.get()); h.put(&x.set(a)); h.put(&x.set(b)); h.put(&x.get()); h.put(&x.get_or_init(|| c)); let mut y: OnceCell<T> = OnceCell::new(); h.put(&y.get_or_init(|| b)); h.put(&y.take()); h.put(&y.into_inner()); let z = OnceCell::from(a); h.put(&z.get()); }",
    "{ use std::cell::LazyCell; let x = LazyCell::new(|| a.wrapping_add(b)); h.put(&*x); h.put(&*x); let y = LazyCell::new(|| vec![a; 2]); h.put(&y.len()); h.put(&LazyCell::force(&y).len()); }",
    "{ use std::cell::{Cell, UnsafeCell}; let c1 = Cell::new(a); let c2 = Cell::new(b); h.put(&(c1.get() == c2.get())); h.put(&c1.cmp(&c2)); h.put(&c1.partial_cmp(&c2)); let s = Cell::from_mut(&mut [a, b][..]).as_slice_of_cells().len(); h.put(&s); let u = UnsafeCell::new(a); h.put(&u.into_inner()); }",
];

const SMART: &[&str] = &[
    "{ use std::rc::Rc; let x = Rc::new(a); let y = Rc::clone(&x); h.put(&Rc::strong_count(&x)); h.put(&Rc::ptr_eq(&x, &y)); h.put(&*x); let w = Rc::downgrade(&x); h.put(&Rc::weak_count(&x)); h.put(&w.upgrade().map(|p| *p)); drop(y); h.put(&Rc::try_unwrap(x).ok()); h.put(&w.upgrade().is_none()); }",
    "{ use std::rc::Rc; let mut x = Rc::new(vec![a]); Rc::make_mut(&mut x).push(b); let y = x.clone(); Rc::make_mut(&mut x).push(c); h.put(&*x); h.put(&*y); h.put(&Rc::get_mut(&mut x).is_some()); h.put(&Rc::into_inner(y)); let z: Rc<[T]> = Rc::from(vec![a, b]); h.put(&z.len()); h.put(&Rc::strong_count(&z)); let s: Rc<str> = Rc::from(\"hé\"); h.put(&&*s); }",
    "{ use std::sync::Arc; let x = Arc::new(a); let y = Arc::clone(&x); h.put(&Arc::strong_count(&x)); h.put(&Arc::ptr_eq(&x, &y)); h.put(&*y); let mut m = Arc::new(vec![a]); Arc::make_mut(&mut m).push(b); h.put(&*m); h.put(&Arc::try_unwrap(m).unwrap().len()); let w = Arc::downgrade(&x); h.put(&w.upgrade().is_some()); h.put(&Arc::weak_count(&x)); h.put(&Arc::into_inner(x).is_none()); }",
    "{ let x = Box::new(a); let y: Box<T> = x.clone(); h.put(&(*x, *y)); let mut z = Box::new([a, b, c]); z[1] = a; h.put(&*z); let s: Box<[T]> = vec![a, b].into_boxed_slice(); h.put(&s.len()); h.put(&s.into_vec()); let t: Box<str> = \"abc\".into(); h.put(&&*t); h.put(&t.len()); let v: Vec<T> = Box::new([a, b]).into_iter().collect(); h.put(&v); }",
    "{ let b1: Box<dyn Fn(T) -> T> = Box::new(move |p| p.wrapping_add(a)); let b2: Box<dyn FnMut(T) -> T> = { let mut s = b; Box::new(move |p| { s = s.wrapping_add(p); s }) }; h.put(&b1(c)); let mut b2 = b2; h.put(&b2(1 as T)); h.put(&b2(2 as T)); let b3: Box<dyn FnOnce() -> T> = Box::new(move || c); h.put(&b3()); }",
    "{ use std::borrow::Cow; let c1: Cow<[T]> = Cow::Borrowed(&[1 as T, 2]); let mut c2 = c1.clone(); c2.to_mut().push(a); h.put(&c1.len()); h.put(&c2.len()); h.put(&c2.into_owned()); h.put(&matches!(c1, Cow::Borrowed(_))); let c3: Cow<str> = Cow::Owned(String::from(\"x\")); h.put(&c3.len()); h.put(&c3.is_empty()); let c4: Cow<str> = \"y\".into(); h.put(&(c3 == c4)); h.put(&c4.as_ref()); }",
    "{ use std::borrow::{Borrow, BorrowMut, ToOwned}; let v = vec![a, b]; let s: &[T] = v.borrow(); h.put(&s.len()); let o: Vec<T> = s.to_owned(); h.put(&o); let mut w = vec![c]; let m: &mut [T] = w.borrow_mut(); m[0] = a; h.put(&w); let q = String::from(\"q\"); let st: &str = q.borrow(); h.put(&st.len()); h.put(&\"abc\".to_owned()); }",
    "{ use std::marker::PhantomData; let p: PhantomData<T> = PhantomData; h.put(&std::mem::size_of_val(&p)); h.put(&(p == PhantomData)); let u = (); h.put(&u); h.put(&std::mem::size_of::<()>()); let n: std::convert::Infallible; h.put(&std::mem::size_of::<std::convert::Infallible>()); h.put(&(std::mem::size_of::<std::num::Wrapping<T>>() == std::mem::size_of::<T>())); }",
    "{ use std::time::Duration; let d1 = Duration::from_secs(a as u8 as u64); let d2 = Duration::from_millis(b as u16 as u64); let d3 = Duration::new(c as u8 as u64, (a as u32) % 2_000_000_000); h.put(&d1.as_secs()); h.put(&d2.as_millis()); h.put(&d3.as_nanos()); h.put(&d3.subsec_nanos()); h.put(&d3.subsec_micros()); h.put(&d3.subsec_millis()); h.put(&d1.checked_add(d2)); h.put(&d1.checked_sub(d2)); h.put(&d1.saturating_sub(d2)); h.put(&(d1 + d2)); h.put(&(d1 * 3)); h.put(&(d3 / 2)); h.put(&d1.cmp(&d2)); h.put(&d1.max(d2)); h.put(&d1.is_zero()); h.put(&Duration::ZERO); h.put(&Duration::MAX.as_secs()); }",
    "{ use std::time::Duration; let d = Duration::from_secs_f64((a as u8) as f64 / 8.0); h.put(&d.as_secs_f64()); h.put(&d.as_secs_f32()); h.put(&d.mul_f64(1.5).as_nanos()); h.put(&d.div_f64(4.0).as_nanos()); h.put(&Duration::from_nanos(b as u32 as u64).as_micros()); h.put(&Duration::from_micros(c as u32 as u64).abs_diff(d).as_micros()); h.put(&d.checked_mul(3)); h.put(&d.checked_div(0)); h.put(&format!(\"{:?}\", Duration::from_millis(1500))); h.put(&Duration::try_from_secs_f64(-1.0).is_err()); h.put(&(Duration::from_secs(61).as_secs() / 60)); }",
    "{ let t = (a, b, c); let (x, y, z) = t; h.put(&(z, y, x)); h.put(&t.0.wrapping_add(t.2)); let mut u = t; u.1 = a; h.put(&u); h.put(&(t == u)); h.put(&t.cmp(&u)); h.put(&t.clone()); let ((p, q), [r, s]) = ((a, b), [c, a]); h.put(&(p, q, r, s)); h.put(&<(T, T)>::default()); h.put(&(a,).0); h.put(&((a, b), c).0.1); }",
    "{ let x = std::num::NonZero::new(3u8).unwrap(); h.put(&x.get()); let y: Option<std::num::NonZero<T>> = std::num::NonZero::new(a); h.put(&y.map(|p| p.get())); let z = std::num::NonZero::<u32>::new(b as u32 | 1).unwrap(); h.put(&z.leading_zeros()); h.put(&z.trailing_zeros()); h.put(&std::num::NonZeroU8::new(0).map(|p| p.get())); h.put(&std::num::NonZeroUsize::MIN.get()); }",
    "{ let w = std::num::Wrapping(a); let s = std::num::Saturating(b); h.put(&(w + w).0); h.put(&(s + s).0); h.put(&(w * std::num::Wrapping(c)).0); h.put(&(s - std::num::Saturating(c)).0); h.put(&w.cmp(&std::num::Wrapping(b))); h.put(&format!(\"{}{:?}{:x}\", w, s, w)); }",
    "{ use std::time::Duration; let d = Duration::from_secs_f32((a as u8) as f32 / 4.0); h.put(&d.as_secs_f32()); h.put(&d.mul_f32(2.5).as_nanos()); h.put(&d.div_f32(2.0).as_nanos()); h.put(&Duration::from_hours(k as u64 + 1).as_secs()); h.put(&Duration::from_mins(k as u64 + 1).as_secs()); h.put(&d.div_duration_f64(Duration::from_secs(3))); h.put(&d.div_duration_f32(Duration::from_secs(5))); h.put(&Duration::from_nanos_u128(b as u32 as u128 + 1).as_nanos()); h.put(&Duration::try_from_secs_f32(1.5).is_ok()); h.put(&Duration::try_from_secs_f32(-1.5).is_err()); }",
    "{ use std::rc::Rc; let x = Rc::new(a); let y = x.clone(); h.put(&(Rc::as_ptr(&x) == Rc::as_ptr(&y))); h.put(&Rc::unwrap_or_clone(x)); let c = Rc::new_cyclic(|w| { h.put(&w.upgrade().is_none()); b }); h.put(&*c); let w = Rc::downgrade(&y); h.put(&(w.as_ptr() == Rc::as_ptr(&y))); }",
    "{ use std::sync::Arc; let x = Arc::new(a); let y = x.clone(); h.put(&(Arc::as_ptr(&x) == Arc::as_ptr(&y))); h.put(&Arc::unwrap_or_clone(x)); let c = Arc::new_cyclic(|w| { h.put(&w.upgrade().is_none()); b }); h.put(&*c); let p = Arc::pin(c); h.put(&**p); let any: Arc<dyn std::any::Any + Send + Sync> = Arc::new(a); h.put(&any.downcast::<T>().is_ok()); }",
    "{ use std::any::Any; let b1: Box<dyn Any> = Box::new(a); h.put(&b1.is::<T>()); h.put(&b1.downcast_ref::<T>()); h.put(&b1.downcast_ref::<u8>().is_none()); let b2: Box<dyn Any> = Box::new(b); h.put(&b2.downcast::<T>().map(|p| *p).ok()); let b3 = Box::pin(c); h.put(&*b3); let b4 = Box::new(a); let p = Box::into_pin(b4); h.put(&*p); let u = Box::write(Box::<T>::new_uninit(), b); h.put(&*u); }",
    "{ use std::any::{Any, TypeId}; h.put(&(TypeId::of::<T>() == a.type_id())); h.put(&(TypeId::of::<T>() == TypeId::of::<u8>())); h.put(&std::any::type_name::<T>()); h.put(&std::any::type_name::<Option<T>>()); h.put(&std::any::type_name_of_val(&a)); let d: &dyn Any = &a; h.put(&d.is::<T>()); h.put(&(d.type_id() == TypeId::of::<T>())); }",
];

fn opt(rng: &mut Rng, ty: &str) -> String {
    if rng.chance(30) { format!("None::<{ty}>") } else { format!("Some({})", lit(rng, ty)) }
}

fn res(rng: &mut Rng, ty: &str) -> String {
    if rng.chance(40) { format!("Err::<{ty}, {ty}>({})", lit(rng, ty)) } else { format!("Ok::<{ty}, {ty}>({})", lit(rng, ty)) }
}

pub fn generate(rng: &mut Rng) -> String {
    let ty = *rng.pick(&["u8", "i8", "u16", "i16", "u32", "i32", "u64", "i64", "usize", "u128", "i128", "isize"]);
    let mut p = format!("        type T = {ty};\n");
    for v in ["a", "b", "c"] {
        let l = lit(rng, ty);
        p += &format!("        let {v}: T = {};\n", bbv(rng, l));
    }
    let (o1, o2, r1, r2) = (opt(rng, ty), opt(rng, ty), res(rng, ty), res(rng, ty));
    p += &format!("        let o1: Option<T> = {}; let o2: Option<T> = {};\n", bbv(rng, o1), bbv(rng, o2));
    p += &format!("        let r1: Result<T, T> = {}; let r2: Result<T, T> = {};\n", bbv(rng, r1), bbv(rng, r2));
    let (k, f) = (rng.below(7), rng.chance(50));
    p += &format!("        let k: usize = {}; let f: bool = {};\n", bbv(rng, format!("{k}usize")), bbv(rng, format!("{f}")));
    let n = rng.range(5, 10);
    let table = pick_table(rng, &[("option", OPTION), ("result", RESULT), ("cmp", CMP), ("mem", MEM), ("cell", CELL), ("smart", SMART)]);
    let body = emit(rng, &p, &[table], n);
    wrap("", &body)
}
