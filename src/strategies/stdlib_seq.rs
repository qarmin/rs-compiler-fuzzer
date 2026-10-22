//! `stdlib` family: slices, arrays, `Vec`, `VecDeque`, iterator adapters and consumers.
//! Prelude: `T` (element type, integers), `v`, `w`: `Vec<T>` (duplicates likely), `a`, `b`: `T`, `k`, `j`: small `usize`
//! (use `% len` before indexing), `sv`: sorted `v`.

use super::stdlib::{bbv, emit, pick_table, wrap};
use super::{lit, small_ints};
use crate::rng::Rng;

pub const NAMES: &[&str] = &["slice", "vec", "iter"];

const SLICE: &[&str] = &[
    "h.put(&v.len()); h.put(&v.is_empty()); h.put(&v.first()); h.put(&v.last()); h.put(&v.get(k)); h.put(&v.get(k..j)); h.put(&v.get(..=k)); h.put(&v.get(j..)); h.put(&v.first_chunk::<2>()); h.put(&v.last_chunk::<3>());",
    "let mut x = v.clone(); x.reverse(); h.put(&x); { let q = x.len() + 1; x.rotate_left(k % q); } h.put(&x); { let q = x.len() + 1; x.rotate_right(j % q); } h.put(&x); if x.len() > 1 { { let q = x.len(); x.swap(0, k % q); } } h.put(&x);",
    "let mut x = v.clone(); x.sort(); h.put(&x); x.dedup(); h.put(&x); h.put(&x.binary_search(&a)); h.put(&x.binary_search_by(|p| p.cmp(&b))); h.put(&x.binary_search_by_key(&a, |p| *p)); h.put(&x.partition_point(|p| *p < b)); h.put(&x.is_sorted());",
    "let mut x = v.clone(); x.sort_unstable(); h.put(&x); x.sort_by(|p, q| q.cmp(p)); h.put(&x); x.sort_by_key(|p| p.wrapping_mul(3)); h.put(&x); x.sort_unstable_by_key(|p| std::cmp::Reverse(*p)); h.put(&x); x.sort_by_cached_key(|p| p.wrapping_neg()); h.put(&x);",
    "h.put(&v.is_sorted()); h.put(&v.is_sorted_by(|p, q| p <= q)); h.put(&v.is_sorted_by_key(|p| p.wrapping_add(1))); h.put(&v.iter().is_sorted()); h.put(&sv.is_sorted()); h.put(&sv.windows(2).all(|p| p[0] <= p[1]));",
    "h.put(&v.contains(&a)); h.put(&v.starts_with(&w[..k.min(w.len())])); h.put(&v.ends_with(&w[..j.min(w.len())])); h.put(&v.starts_with(&[])); h.put(&v.strip_prefix(&v[..k.min(v.len())]).map(|r| r.len())); h.put(&v.strip_suffix(&v[v.len() - j.min(v.len())..]).map(|r| r.len()));",
    "h.put(&v.iter().position(|&p| p == a)); h.put(&v.iter().rposition(|&p| p == b)); h.put(&v.iter().copied().max()); h.put(&v.iter().copied().min()); h.put(&v.iter().max_by_key(|p| p.wrapping_neg())); h.put(&v.iter().min_by(|p, q| q.cmp(p)));",
    "for c in v.chunks(k + 1) { h.put(&c); } for c in v.chunks_exact(k + 1) { h.put(&c.len()); } h.put(&v.chunks_exact(k + 1).remainder()); for c in v.rchunks(j + 1) { h.put(&c); } h.put(&v.rchunks_exact(j + 1).remainder());",
    "for c in v.windows(k + 1) { h.put(&c.len()); } h.put(&v.windows(j + 1).count()); let mut x = v.clone(); for c in x.chunks_mut(k + 1) { c.reverse(); } h.put(&x); for c in x.chunks_exact_mut(j + 1) { c.rotate_left(1); } h.put(&x); for c in x.rchunks_mut(k + 2) { c.fill(a); } h.put(&x);",
    "h.put(&v.chunk_by(|p, q| p <= q).map(|c| c.len()).collect::<Vec<_>>()); h.put(&v.chunk_by(|p, q| p == q).count()); let mut x = v.clone(); for c in x.chunk_by_mut(|p, q| p <= q) { c.reverse(); } h.put(&x);",
    "h.put(&v.split_first()); h.put(&v.split_last()); h.put(&v.split_at_checked(k)); h.put(&v.split_at(k.min(v.len()))); h.put(&v.split_at_checked(v.len())); let (x, y) = v.split_at(j.min(v.len())); h.put(&(x, y));",
    "h.put(&v.split(|p| *p == a).collect::<Vec<_>>()); h.put(&v.rsplit(|p| *p == b).count()); h.put(&v.splitn(k + 1, |p| *p == a).count()); h.put(&v.rsplitn(j + 1, |p| *p == a).collect::<Vec<_>>()); h.put(&v.split_inclusive(|p| *p == a).count()); h.put(&v.split(|p| *p == a).map(|c| c.len()).max());",
    "let mut x = v.clone(); let (l, r) = x.split_at_mut(k.min(v.len())); l.reverse(); r.rotate_left(r.len().min(1)); h.put(&x); if let Some((f, rest)) = x.split_first_mut() { *f = a; rest.reverse(); } h.put(&x); if let Some((l, rest)) = x.split_last_mut() { *l = b; rest.sort(); } h.put(&x);",
    "let mut x = v.clone(); x.fill(a); h.put(&x); x.fill_with(Default::default); h.put(&x); let mut cnt = 0; x.fill_with(|| { cnt += 1; a.wrapping_mul(cnt as T) }); h.put(&x); x[..k.min(v.len())].fill(b); h.put(&x);",
    "let mut x = v.clone(); let n = x.len().min(w.len()); x[..n].copy_from_slice(&w[..n]); h.put(&x); x[..n].clone_from_slice(&v[..n]); h.put(&x); let m = x.len().min(w.len()); let (p, q) = (x[..m / 2].len(), m / 2); if p == q { let mut y = w.clone(); x[..p].swap_with_slice(&mut y[..p]); h.put(&x); h.put(&y); }",
    "let mut x = v.clone(); if !x.is_empty() { let i = k % x.len(); x.copy_within(i.., 0); h.put(&x); { let q = x.len() - i; x.copy_within(..q, i); } h.put(&x); }",
    "let mut x = v.clone(); if x.len() > 2 { let (p, q) = (k % x.len(), j % x.len()); x.swap(p, q); h.put(&x); } x.reverse(); h.put(&x.iter().rev().copied().collect::<Vec<T>>()); h.put(&x.to_vec()); h.put(&x.as_slice().len()); h.put(&x.first_mut().map(|p| { *p = a; *p }));",
    "h.put(&v.repeat(k % 3)); h.put(&[v.clone(), w.clone()].concat()); h.put(&[&v[..], &w[..]].concat()); h.put(&[v.as_slice(), w.as_slice()].join(&a)); h.put(&[v.clone(), w.clone()].join(&[a, b][..]));",
    "h.put(&v.iter().rev().skip(k).step_by(2).copied().collect::<Vec<_>>()); h.put(&v.to_vec().into_boxed_slice().len()); h.put(&v.clone().into_boxed_slice().into_vec()); h.put(&v.iter().map(|p| p.wrapping_mul(7)).rev().collect::<Vec<_>>());",
    "h.put(&(v == w)); h.put(&(v < w)); h.put(&v.cmp(&w)); h.put(&v.partial_cmp(&w)); h.put(&(v[..] == w[..])); h.put(&v.eq(&w)); h.put(&(v.as_slice() != w.as_slice())); h.put(&v.iter().eq(w.iter())); h.put(&v.iter().lt(w.iter()));",
    "let mut x = v.clone(); let mut y = w.clone(); std::mem::swap(&mut x, &mut y); h.put(&x); h.put(&y); let z = std::mem::replace(&mut x, vec![a]); h.put(&z); h.put(&x); let t = std::mem::take(&mut y); h.put(&t); h.put(&y.len());",
    "let mut x = v.clone(); if !x.is_empty() { let i = k % x.len(); let (l, m, r) = x.select_nth_unstable_by(i, |p, q| q.cmp(p)); h.put(&(l.len(), *m, r.len())); } let mut y = v.clone(); if !y.is_empty() { let i = j % y.len(); h.put(&*y.select_nth_unstable_by_key(i, |p| p.wrapping_neg()).1); }",
    "let mut x = v.clone(); let mut c = 0; x.sort_by(|p, q| { c += 1; p.cmp(q) }); h.put(&x); h.put(&(c > 0 || x.len() < 2)); x.reverse(); x.sort_by_key(|p| p.count_ones()); h.put(&x);",
    "let ar = [a, b, a, b]; h.put(&ar.len()); h.put(&ar.map(|p| p.wrapping_add(1))); h.put(&ar.iter().rev().collect::<Vec<_>>()); h.put(&ar.each_ref().map(|p| *p)); h.put(&ar.as_slice().first()); let [f, .., l] = ar; h.put(&(f, l)); h.put(&ar.split_first().map(|(p, r)| (*p, r.len())));",
    "let ar: [T; 4] = std::array::from_fn(|i| a.wrapping_add(i as T)); h.put(&ar); h.put(&<[T; 3]>::try_from(&v[..v.len().min(3)]).is_ok()); h.put(&<[T; 2]>::try_from(v.clone()).ok()); let r2: Result<[T; 2], _> = w.clone().try_into(); h.put(&r2.ok()); h.put(&(ar.iter().sum::<T>() == 0));",
    "let ar = [[a, b], [b, a], [a, a]]; h.put(&ar.concat()); h.put(&ar.as_flattened()); h.put(&ar.iter().flatten().count()); h.put(&ar.as_flattened().len()); h.put(&ar.map(|r| r[0].wrapping_add(r[1])));",
    "let mut ar = [a, b, a, b, a]; ar.reverse(); ar.sort(); h.put(&ar); ar.rotate_left(k % 5); h.put(&ar); ar.swap(0, 4); h.put(&ar); ar.fill(b); h.put(&ar); h.put(&(ar == [b; 5])); h.put(&ar.contains(&a)); h.put(&ar.iter().all(|p| *p == b));",
    "let ar = [a; 4]; let br = [b; 4]; h.put(&(ar == br)); h.put(&ar.cmp(&br));",
    "let mut x = v.clone(); h.put(&x.first_chunk_mut::<2>().map(|c| { c[0] = a; c.len() })); h.put(&x.last_chunk_mut::<2>().map(|c| { c[0] = b; c.len() })); h.put(&x.last_mut().map(|p| { *p = a; *p })); h.put(&x);",
    "h.put(&v.split_first_chunk::<2>().map(|(f, r)| (f.len(), r.len()))); h.put(&v.split_last_chunk::<2>().map(|(r, l)| (r.len(), l.len()))); let mut x = v.clone(); h.put(&x.split_first_chunk_mut::<1>().map(|(f, r)| { f[0] = a; r.len() })); h.put(&x.split_last_chunk_mut::<1>().map(|(r, l)| { l[0] = b; r.len() })); h.put(&x);",
    "let mut x = v.clone(); for c in x.rchunks_exact_mut(k + 1) { c.reverse(); } h.put(&x); for c in x.split_mut(|p| *p == a) { c.reverse(); } h.put(&x); for c in x.rsplit_mut(|p| *p == b) { c.fill(a); } h.put(&x); for c in x.splitn_mut(k + 1, |p| *p == a) { c.rotate_left(c.len().min(1)); } h.put(&x); for c in x.rsplitn_mut(j + 1, |p| *p == b) { c.reverse(); } h.put(&x); for c in x.split_inclusive_mut(|p| *p == a) { c.reverse(); } h.put(&x);",
    "let mut x = v.clone(); h.put(&x.split_at_mut_checked(k).map(|(l, r)| { l.fill(a); r.len() })); h.put(&x); h.put(&x.get_disjoint_mut([0usize, 0usize]).is_err()); if x.len() > 1 { if let Ok([p, q]) = x.get_disjoint_mut([0, 1]) { std::mem::swap(p, q); } } h.put(&x);",
    "let mut x = v.clone(); { let mut r: &[T] = &v; h.put(&r.split_off_first()); h.put(&r.split_off_last()); h.put(&r.split_off(..k.min(r.len()))); h.put(&r.len()); } { let mut r: &mut [T] = &mut x; h.put(&r.split_off_first_mut().map(|p| { *p = a; *p })); h.put(&r.split_off_last_mut().map(|p| { *p = b; *p })); h.put(&r.split_off_mut(..k.min(r.len())).map(|s| s.len())); } h.put(&x);",
    "let r = v.as_ptr_range(); h.put(&((r.end as usize) - (r.start as usize))); let mut x = v.clone(); let rm = x.as_mut_ptr_range(); h.put(&((rm.end as usize) - (rm.start as usize))); h.put(&(v.as_ptr() == v.as_slice().as_ptr())); h.put(&(x.as_mut_ptr() as usize % std::mem::align_of::<T>()));",
    "if !v.is_empty() { let i = k % v.len(); h.put(&v.element_offset(&v[i])); h.put(&v.subslice_range(&v[i..]).map(|r| r.end - r.start)); h.put(&v.subslice_range(&w[..]).is_none()); }",
    "let mut x = v.clone(); let (p, rest) = x.as_chunks_mut::<2>(); for c in p.iter_mut() { c.reverse(); } h.put(&rest.len()); h.put(&x); let (c, r) = v.as_chunks::<2>(); h.put(&c); h.put(&r); let (r2, c2) = v.as_rchunks::<2>(); h.put(&r2); h.put(&c2); let mut y = v.clone(); let (r3, c3) = y.as_rchunks_mut::<2>(); h.put(&(r3.len(), c3.len()));",
    "let mut ar = [[a, b], [b, a]]; ar.as_flattened_mut()[0] = c; h.put(&ar); h.put(&v.as_array::<2>()); let mut x = v.clone(); h.put(&x.as_mut_array::<2>().map(|a| { a[0] = b; a.len() }));",
    "let mut x = v.clone(); if !x.is_empty() { let q = x.len(); h.put(&*x.select_nth_unstable(k % q).1); } let mut y = v.clone(); if !y.is_empty() { let q = y.len(); let (l, m, r) = y.select_nth_unstable_by_key(j % q, |p| *p); h.put(&(l.len(), *m, r.len())); }",
    "let mut x = v.clone(); { let c = x.chunks_exact_mut(k + 1); h.put(&c.into_remainder().len()); } { let c = x.rchunks_exact_mut(j + 1); h.put(&c.into_remainder().len()); } h.put(&v.chunks_exact(k + 1).remainder().len());",
];

const VEC: &[&str] = &[
    "let mut x = v.clone(); x.push(a); x.push(b); h.put(&x.pop()); h.put(&x.len()); x.insert(k.min(x.len()), a); x.insert(0, b); h.put(&x); x.truncate(j + 2); h.put(&x); x.clear(); h.put(&x.is_empty());",
    "let mut x = v.clone(); if !x.is_empty() { let i = k % x.len(); h.put(&x.remove(i)); } if !x.is_empty() { let q = x.len(); h.put(&x.swap_remove(j % q)); } h.put(&x);",
    "let mut x = v.clone(); x.retain(|p| *p != a); h.put(&x); x.retain_mut(|p| { *p = p.wrapping_add(1); *p != b }); h.put(&x); x.dedup(); h.put(&x); x.dedup_by_key(|p| *p / 2); h.put(&x); x.dedup_by(|p, q| p == q);  h.put(&x);",
    "let mut x = v.clone(); let d: Vec<T> = x.drain(k.min(v.len())..).collect(); h.put(&d); h.put(&x); let mut y = v.clone(); let e: Vec<T> = y.drain(..j.min(v.len())).rev().collect(); h.put(&e); h.put(&y); y.drain(..); h.put(&y.len());",
    "let mut x = v.clone(); let r: Vec<T> = x.splice(..k.min(v.len()), [a, b, a]).collect(); h.put(&r); h.put(&x); let mut y = v.clone(); y.splice(j.min(v.len()).., std::iter::empty()); h.put(&y);",
    "let mut x = v.clone(); let y = x.split_off(k.min(x.len())); h.put(&x); h.put(&y); x.append(&mut w.clone()); h.put(&x); let mut z = w.clone(); x.append(&mut z); h.put(&z.len()); x.extend_from_slice(&y); x.extend(y.iter()); x.extend(w.iter().copied().rev()); h.put(&x);",
    "let mut x = v.clone(); x.resize(k + 3, a); h.put(&x); x.resize(j, b); h.put(&x); x.resize_with(k + j, || a.wrapping_add(1)); h.put(&x); x.extend_from_within(..x.len().min(2)); h.put(&x);",
    "let mut x = v.clone(); x.dedup(); x.shrink_to_fit(); h.put(&(x.capacity() >= x.len())); x.reserve(k); h.put(&(x.capacity() - x.len() >= k)); x.shrink_to(0); let y = Vec::<T>::with_capacity(j); h.put(&(y.capacity() >= j)); h.put(&y.len());",
    "let x = v.clone(); h.put(&x.iter().map(|p| *p as u128).sum::<u128>()); let y: Vec<T> = x.iter().copied().collect(); h.put(&(x == y)); let z: Box<[T]> = x.clone().into(); h.put(&z.len()); let q: Vec<T> = z.into_vec(); h.put(&q); let r: std::rc::Rc<[T]> = std::rc::Rc::from(q.clone()); h.put(&r.len());",
    "let x = vec![a; k + 1]; h.put(&x.len()); let y = vec![vec![b; 2]; j + 1]; h.put(&y.concat()); h.put(&y.iter().map(|r| r.len()).sum::<usize>()); h.put(&Vec::from([a, b])); h.put(&Vec::from(&[a][..])); h.put(&Vec::<T>::new().is_empty());",
    "let mut x = v.clone(); let mut it = x.drain(..); h.put(&it.next()); h.put(&it.next_back()); h.put(&it.len()); drop(it); h.put(&x); let mut y = v.clone(); y.iter_mut().for_each(|p| *p = p.wrapping_mul(3)); h.put(&y); for p in y.iter_mut().rev().step_by(2) { *p = a; } h.put(&y);",
    "let x = v.clone(); let mut i = x.into_iter(); h.put(&i.next()); h.put(&i.next_back()); h.put(&i.len()); h.put(&i.as_slice().len()); h.put(&i.rev().collect::<Vec<_>>()); let y: Vec<(usize, T)> = v.iter().copied().enumerate().collect(); h.put(&y);",
    "let (e, o): (Vec<T>, Vec<T>) = v.iter().partition(|p| **p % 2 == 0); h.put(&e); h.put(&o); let (l, r): (Vec<usize>, Vec<T>) = v.iter().copied().enumerate().unzip(); h.put(&l); h.put(&r);",
    "let x = v.clone(); h.put(&x.first().copied().unwrap_or(a)); h.put(&x.iter().rev().take(k).copied().collect::<Vec<_>>()); h.put(&x.get(k).is_some()); h.put(&x.iter().skip(k).count()); h.put(&x.as_slice().iter().rev().skip_while(|p| **p != a).count());",
    "let mut x = v.clone(); x.sort(); h.put(&x.binary_search(&a).unwrap_or_else(|i| i)); let p = x.partition_point(|y| *y <= b); x.insert(p, b); h.put(&x); h.put(&x.is_sorted()); x.dedup(); h.put(&x);",
    "let mut x: std::collections::VecDeque<T> = v.iter().copied().collect(); x.push_back(a); x.push_front(b); h.put(&x); h.put(&x.pop_front()); h.put(&x.pop_back()); h.put(&x.front()); h.put(&x.back()); h.put(&x.get(k)); h.put(&x.len()); h.put(&x.contains(&a)); h.put(&x.iter().rev().copied().collect::<Vec<_>>());",
    "let mut x: std::collections::VecDeque<T> = v.iter().copied().collect(); { let q = x.len() + 1; x.rotate_left(k % q); } h.put(&x); { let q = x.len() + 1; x.rotate_right(j % q); } h.put(&x); x.make_contiguous().sort(); h.put(&x); h.put(&(x.as_slices().0.len() + x.as_slices().1.len())); x.truncate(k + 1); h.put(&x); h.put(&x.binary_search(&a));",
    "let mut x: std::collections::VecDeque<T> = v.iter().copied().collect(); if !x.is_empty() { let i = k % x.len(); h.put(&x.remove(i)); x.insert(i.min(x.len()), a); h.put(&x.swap_remove_back(0)); h.put(&x.swap_remove_front(0)); } h.put(&x); if x.len() > 1 { x.swap(0, 1); } x.retain(|p| *p != b); h.put(&x);",
    "let mut x: std::collections::VecDeque<T> = v.iter().copied().collect(); let y = x.split_off(k.min(x.len())); h.put(&x); h.put(&y); x.append(&mut y.clone()); x.extend(w.iter()); x.extend(w.iter().copied()); h.put(&x.drain(..k.min(x.len())).collect::<Vec<_>>()); h.put(&x); x.resize(j + 2, a); h.put(&x); x.clear(); h.put(&x.is_empty());",
    "let x: std::collections::VecDeque<T> = v.iter().copied().collect(); let y: Vec<T> = x.clone().into(); h.put(&y); let z = std::collections::VecDeque::from(y.clone()); h.put(&(x == z)); h.put(&x.cmp(&z)); h.put(&x.range(k.min(x.len())..).count()); h.put(&x.iter().position(|p| *p == a)); h.put(&x.partition_point(|p| *p < b));",
    "let mut x = v.clone(); let r: Vec<T> = x.extract_if(.., |p| *p == a || *p % 2 == 0).collect(); h.put(&r); h.put(&x); let mut y = v.clone(); let r2: Vec<T> = y.extract_if(k.min(y.len())..j.max(k).min(y.len()), |p| *p != b).collect(); h.put(&r2); h.put(&y);",
    "let mut x = v.clone(); h.put(&x.pop_if(|p| *p == a)); h.put(&x.pop_if(|p| *p != a)); h.put(&x); x.reserve_exact(k); h.put(&(x.capacity() >= x.len() + k)); h.put(&x.try_reserve(j).is_ok()); h.put(&x.try_reserve_exact(j).is_ok()); h.put(&x.as_mut_slice().len()); h.put(&(x.spare_capacity_mut().len() >= 0));",
    "let x: Vec<[T; 2]> = vec![[a, b], [b, a]]; h.put(&x.into_flattened()); let y = vec![[c; 3]; 2]; h.put(&y.into_flattened().len()); let z = vec![a, b]; h.put(&(z.as_ptr() as usize % std::mem::align_of::<T>())); let mut w2 = vec![b]; h.put(&(w2.as_mut_ptr() as usize % std::mem::align_of::<T>()));",
    "let mut x: std::collections::VecDeque<T> = v.iter().copied().collect(); h.put(&x.pop_back_if(|p| *p == a)); h.put(&x.pop_front_if(|p| *p != a)); x.reserve_exact(k); h.put(&x.try_reserve(j).is_ok()); h.put(&x.try_reserve_exact(j).is_ok()); h.put(&(x.as_mut_slices().0.len() + x.as_mut_slices().1.len() < 1000));",
    "let mut x = v.clone(); *x.push_mut(a) = b; if x.len() > 1 { *x.insert_mut(k % x.len().max(1), c) = a; } h.put(&x); let mut d: std::collections::VecDeque<T> = v.iter().copied().collect(); *d.push_back_mut(a) = b; *d.push_front_mut(c) = a; *d.insert_mut(0, b) = c; h.put(&d);",
];

const ITER: &[&str] = &[
    "h.put(&v.iter().copied().fold(0 as T, |s, p| s.wrapping_add(p))); h.put(&v.iter().copied().map(|p| p.wrapping_mul(a)).fold(0 as T, |s, p| s ^ p)); h.put(&v.iter().copied().reduce(|s, p| s.wrapping_sub(p))); h.put(&v.iter().rfold(0 as T, |s, p| s.wrapping_mul(31).wrapping_add(*p)));",
    "h.put(&v.iter().sum::<T>()); h.put(&v.iter().copied().product::<T>()); h.put(&v.iter().copied().map(|p| p as u128).sum::<u128>()); h.put(&v.iter().map(|p| *p as u64).max());",
    "h.put(&v.iter().count()); h.put(&v.iter().last()); h.put(&v.iter().nth(k)); h.put(&v.iter().nth_back(j)); h.put(&v.iter().skip(k).next()); h.put(&v.iter().rev().nth(k)); h.put(&v.iter().step_by(k + 1).count()); h.put(&v.iter().size_hint());",
    "h.put(&v.iter().any(|p| *p == a)); h.put(&v.iter().all(|p| *p != b)); h.put(&v.iter().find(|p| **p > a)); h.put(&v.iter().rfind(|p| **p < b)); h.put(&v.iter().find_map(|p| p.checked_add(a))); h.put(&v.iter().filter_map(|p| p.checked_sub(b)).count()); h.put(&v.iter().position(|p| *p == a));",
    "h.put(&v.iter().copied().filter(|p| p % 2 == 1 || *p == a).collect::<Vec<_>>()); h.put(&v.iter().take_while(|p| **p != a).count()); h.put(&v.iter().skip_while(|p| **p != b).count()); h.put(&v.iter().map_while(|p| p.checked_add(a)).count()); h.put(&v.iter().take(k).skip(j).count());",
    "h.put(&v.iter().zip(w.iter()).map(|(p, q)| p.wrapping_add(*q)).collect::<Vec<_>>()); h.put(&v.iter().zip(w.iter().rev()).filter(|(p, q)| p < q).count()); h.put(&v.iter().copied().zip(0..).last()); h.put(&v.iter().chain(w.iter()).count()); h.put(&v.iter().chain(w.iter()).rev().take(3).copied().collect::<Vec<_>>());",
    "h.put(&v.iter().enumerate().map(|(i, p)| (i as T).wrapping_add(*p)).collect::<Vec<_>>()); h.put(&v.iter().enumerate().max_by_key(|(_, p)| **p)); h.put(&v.iter().enumerate().min_by_key(|(i, p)| (**p, std::cmp::Reverse(*i)))); h.put(&v.iter().enumerate().rev().find(|(i, _)| i % 2 == 0));",
    "h.put(&v.iter().scan(0 as T, |s, p| { *s = s.wrapping_add(*p); Some(*s) }).collect::<Vec<_>>()); h.put(&v.iter().scan(a, |s, p| { *s = s.wrapping_mul(*p | 1); if *s == b { None } else { Some(*s) } }).count());",
    "h.put(&v.iter().flat_map(|p| [*p, p.wrapping_add(1)]).collect::<Vec<_>>()); h.put(&v.iter().map(|p| vec![*p; 2]).flatten().count()); h.put(&v.iter().map(|p| Some(*p).filter(|q| q % 2 == 0)).flatten().count()); h.put(&vec![v.clone(), w.clone()].into_iter().flatten().count()); h.put(&v.iter().flat_map(|p| std::iter::repeat(*p).take(2)).count());",
    "h.put(&v.iter().peekable().peek().copied()); let mut pk = v.iter().peekable(); let mut o = Vec::new(); while let Some(p) = pk.next() { if pk.peek().map_or(false, |q| *q == p) { o.push(*p); } } h.put(&o); h.put(&pk.next_if(|p| **p == a)); h.put(&pk.next_if_eq(&&b));",
    "let mut cnt = 0; h.put(&v.iter().inspect(|_| cnt += 1).count()); h.put(&cnt); h.put(&v.iter().fuse().count()); h.put(&v.iter().cloned().cycle().take(k + j).collect::<Vec<_>>()); h.put(&v.iter().copied().cycle().skip(k).step_by(3).take(5).collect::<Vec<_>>());",
    "h.put(&v.iter().copied().rev().enumerate().skip(k).collect::<Vec<_>>()); h.put(&v.iter().rev().rev().count()); h.put(&v.iter().rev().zip(w.iter()).count()); h.put(&v.chunks(2).rev().map(|c| c.len()).collect::<Vec<_>>()); h.put(&v.iter().copied().rev().chain(w.iter().copied()).last());",
    "h.put(&v.iter().cmp(w.iter())); h.put(&v.iter().partial_cmp(w.iter())); h.put(&v.iter().eq(w.iter())); h.put(&v.iter().ne(w.iter())); h.put(&v.iter().lt(w.iter())); h.put(&v.iter().le(w.iter())); h.put(&v.iter().gt(w.iter())); h.put(&v.iter().ge(w.iter()));",
    "h.put(&v.iter().min_by(|p, q| p.cmp(q))); h.put(&v.iter().max_by(|p, q| p.cmp(q))); h.put(&v.iter().min_by_key(|p| p.wrapping_sub(a))); h.put(&v.iter().max_by_key(|p| p.wrapping_sub(b))); h.put(&v.iter().copied().max().zip(v.iter().copied().min()));",
    "h.put(&v.iter().try_fold(0 as T, |s, p| s.checked_add(*p))); h.put(&v.iter().try_fold(1 as T, |s, p| s.checked_mul(*p))); h.put(&v.iter().copied().try_for_each(|p| if p == a { Err(p) } else { Ok(()) })); h.put(&v.iter().map(|p| p.checked_add(a)).collect::<Option<Vec<T>>>()); h.put(&v.iter().map(|p| p.checked_div(*p)).collect::<Option<Vec<T>>>());",
    "h.put(&v.iter().map(|p| if *p != a { Ok(*p) } else { Err(*p) }).collect::<Result<Vec<T>, T>>());",
    "h.put(&v.iter().copied().collect::<std::collections::BTreeSet<T>>().len()); h.put(&v.iter().copied().collect::<std::collections::BTreeSet<T>>().iter().copied().collect::<Vec<_>>()); h.put(&v.iter().copied().map(|p| (p, 1u8)).collect::<std::collections::BTreeMap<T, u8>>().len()); h.put(&v.iter().copied().collect::<std::collections::VecDeque<T>>());",
    "h.put(&v.iter().copied().collect::<std::collections::BinaryHeap<T>>().into_sorted_vec()); h.put(&v.iter().copied().collect::<std::collections::LinkedList<T>>().len()); h.put(&v.iter().copied().collect::<Box<[T]>>().len()); h.put(&v.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(\",\")); h.put(&v.iter().map(|p| format!(\"{p:x}\")).collect::<String>());",
    "h.put(&std::iter::repeat(a).take(k).count()); h.put(&std::iter::repeat_n(b, j).count()); h.put(&std::iter::once(a).chain(std::iter::once(b)).collect::<Vec<_>>()); h.put(&std::iter::empty::<T>().count()); h.put(&std::iter::repeat_with(|| a).take(3).collect::<Vec<_>>());",
    "h.put(&std::iter::successors(Some(a), |p| p.checked_mul(2)).take(40).count()); let mut st = a; h.put(&std::iter::from_fn(|| { st = st.wrapping_add(b | 1); if st == 0 { None } else { Some(st) } }).take(6).collect::<Vec<_>>()); h.put(&std::iter::zip(v.iter(), w.iter()).count());",
    "h.put(&(0..k).map(|i| i * 2).sum::<usize>()); h.put(&(0..=k).rev().step_by(2).collect::<Vec<_>>()); h.put(&(k..j + 8).len()); h.put(&(k..j + 8).rev().nth(1)); h.put(&(1..=j + 1).product::<usize>()); h.put(&(0..k).zip(0..j).count()); h.put(&(0..10).filter(|i| i % 3 == 0).last());",
    "h.put(&(a..=b).take(5).count()); h.put(&((a & 15)..=(b & 15)).sum::<T>()); h.put(&((a & 7)..(b & 7)).rev().count()); h.put(&((a & 31)..).take(3).collect::<Vec<_>>()); h.put(&(0..5).map(|i| i as T).rev().collect::<Vec<_>>());",
    "let mut it = v.iter().copied(); let mut o = Vec::new(); while let Some(p) = it.next() { if p == a { break; } o.push(p); } let rest: Vec<T> = it.collect(); h.put(&o); h.put(&rest); let mut it2 = v.iter().copied(); h.put(&it2.by_ref().take(k).count()); h.put(&it2.count());",
    "let mut x = v.clone(); x.iter_mut().for_each(|p| *p = p.wrapping_add(a)); h.put(&x); for (p, q) in x.iter_mut().zip(w.iter()) { *p = p.wrapping_mul(*q); } h.put(&x); x.iter_mut().rev().enumerate().for_each(|(i, p)| *p = p.wrapping_add(i as T)); h.put(&x);",
    "let mut x = v.clone(); x.extend(std::iter::repeat(a).take(k)); x.extend((0..j).map(|i| i as T)); h.put(&x); let y: Vec<T> = Vec::from_iter(x.iter().copied().rev()); h.put(&y); let mut z = Vec::new(); z.extend_from_slice(&y[..y.len().min(k)]); h.put(&z);",
    "h.put(&v.iter().copied().max_by(|p, q| p.cmp(q)).into_iter().chain(v.iter().copied().min_by_key(|p| *p)).collect::<Vec<_>>());",
    "h.put(&v.iter().copied().sum::<T>().wrapping_add(w.iter().copied().product::<T>())); h.put(&v.iter().zip(w.iter()).map(|(p, q)| p.wrapping_mul(*q)).sum::<T>());",
    "let it = v.iter().copied().map(|p| p.wrapping_add(1)); let bx: Box<dyn Iterator<Item = T>> = Box::new(it); h.put(&bx.filter(|p| p % 2 == 0).count()); let mut dy: Box<dyn DoubleEndedIterator<Item = &T> + '_> = Box::new(v.iter()); h.put(&dy.next_back()); h.put(&dy.next());",
    "let mut it = v.iter().copied(); h.put(&it.len()); h.put(&it.nth(k)); h.put(&it.len()); let mut it2 = v.iter().rev(); h.put(&it2.len()); h.put(&it2.next_back()); h.put(&it2.rev().len());",
    "h.put(&v.iter().map(|p| (p, 1)).count()); h.put(&v.iter().rev().copied().min_by_key(|p| p.wrapping_neg())); h.put(&v.iter().copied().is_sorted()); h.put(&v.iter().copied().collect::<Vec<_>>().iter().eq(v.iter()));",
];

fn pick_vec(rng: &mut Rng, ty: &str) -> String {
    let pool: Vec<String> = (0..rng.range(2, 5)).map(|_| lit(rng, ty)).collect();
    let small: Vec<String> = (0..4).map(|_| small_ints(rng, ty)).collect();
    let n = rng.range(0, 9);
    let items: Vec<String> = (0..n).map(|_| if rng.chance(60) { rng.pick(&pool).clone() } else { rng.pick(&small).clone() }).collect();
    format!("vec![{}]", items.join(", "))
}

pub fn generate(rng: &mut Rng) -> String {
    let ty = *rng.pick(&["u8", "i8", "u16", "i16", "u32", "i32", "u64", "i64", "usize", "u128", "i128", "isize"]);
    let mut p = format!("        type T = {ty};\n");
    let (v, w) = (pick_vec(rng, ty), pick_vec(rng, ty));
    p += &format!("        let v: Vec<T> = {}; let w: Vec<T> = {}; let mut sv = v.clone(); sv.sort();\n", bbv(rng, v), bbv(rng, w));
    let (a, b, c) = (lit(rng, ty), lit(rng, ty), lit(rng, ty));
    p += &format!("        let a: T = {}; let b: T = {}; let c: T = {};\n", bbv(rng, a), bbv(rng, b), bbv(rng, c));
    let (k, j) = (rng.below(7), rng.below(7));
    p += &format!("        let k: usize = {}; let j: usize = {};\n", bbv(rng, format!("{k}usize")), bbv(rng, format!("{j}usize")));
    let n = rng.range(5, 12);
    let table = pick_table(rng, &[("slice", SLICE), ("vec", VEC), ("iter", ITER)]);
    let body = emit(rng, &p, &[table], n);
    wrap("", &body)
}
