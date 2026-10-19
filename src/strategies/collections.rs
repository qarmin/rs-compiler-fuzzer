//! Strategy `collections`: random operation sequences on std containers and strings
//! (`VecDeque`, `BTreeMap`/`BTreeSet`, `BinaryHeap`, `String`). Heavily generic std code that
//! gets inlined and monomorphised differently per opt level. Ordered containers only - no
//! `HashMap`, whose iteration order is not deterministic. Every index/key is reduced modulo
//! the current length or guarded by `is_empty`.

use super::lit;
use crate::rng::Rng;
use crate::w;

const TY: &[&str] = &["u8", "u16", "u32", "u64", "i32", "i64", "usize"];

fn deque(rng: &mut Rng, body: &mut String) {
    let ty = *rng.pick(TY);
    w!(body, "        {{ let mut d: std::collections::VecDeque<{ty}> = (0..bb({}usize)).map(|i| (i as {ty}).wrapping_mul({})).collect();", rng.range(0, 20), lit(rng, ty));
    for _ in 0..rng.range(4, 10) {
        let op = match rng.below(12) {
            0 => format!("d.push_back({});", lit(rng, ty)),
            1 => format!("d.push_front({});", lit(rng, ty)),
            2 => "h.put(&d.pop_front());".into(),
            3 => "h.put(&d.pop_back());".into(),
            4 => format!("if !d.is_empty() {{ let k = bb({}usize) % d.len(); d.rotate_left(k); }}", rng.below(100)),
            5 => "d.make_contiguous().sort();".into(),
            6 => format!("if !d.is_empty() {{ let i = bb({}usize) % d.len(); h.put(&d.remove(i)); }}", rng.below(100)),
            7 => format!("{{ let i = bb({}usize) % (d.len() + 1); d.insert(i, {}); }}", rng.below(100), lit(rng, ty)),
            8 => format!("h.put(&d.iter().rev().enumerate().fold(0u64, |a, (i, &x)| a.wrapping_mul(31).wrapping_add(x as u64 ^ i as u64)));"),
            9 => format!("{{ let k = bb({}usize) % (d.len() + 1); let t = d.split_off(k); d.extend(t.into_iter().rev()); }}", rng.below(100)),
            10 => "d.truncate(bb(12usize));".into(),
            _ => format!("h.put(&d.get(bb({}usize)).copied());", rng.below(30)),
        };
        w!(body, "        {op}");
    }
    w!(body, "        h.put(&d.iter().copied().collect::<Vec<{ty}>>()); }}");
}

fn btree(rng: &mut Rng, body: &mut String) {
    let ty = *rng.pick(&["u8", "u16", "u32", "i32", "u64"]);
    w!(body, "        {{ let mut m: std::collections::BTreeMap<{ty}, u64> = std::collections::BTreeMap::new();");
    for _ in 0..rng.range(4, 12) {
        let op = match rng.below(10) {
            0 | 1 => format!("m.insert({}, bb({}u64));", lit(rng, ty), rng.next_u64() % 1000),
            2 => format!("*m.entry({}).or_insert(1) += bb({}u64);", lit(rng, ty), rng.below(9)),
            3 => format!("h.put(&m.remove(&{}));", lit(rng, ty)),
            4 => "h.put(&m.first_key_value().map(|(k, v)| (*k as u64, *v)));".into(),
            5 => "h.put(&m.pop_last().map(|(k, v)| (k as u64, v)));".into(),
            6 => format!("h.put(&m.range({}..).map(|(k, v)| (*k as u64).wrapping_add(*v)).take(4).collect::<Vec<u64>>());", lit(rng, ty)),
            7 => format!("m.retain(|k, v| (*k as u64 ^ *v) & {} != 0);", 1 + rng.below(3)),
            8 => format!("{{ let t = m.split_off(&{}); h.put(&t.len()); m.extend(t.into_iter().take(2)); }}", lit(rng, ty)),
            _ => "h.put(&m.values().fold(0u64, |a, &v| a.rotate_left(5) ^ v));".into(),
        };
        w!(body, "        {op}");
    }
    w!(body, "        h.put(&m.len()); h.put(&m.keys().map(|&k| k as u64).collect::<Vec<u64>>()); }}");
    // sets and heaps
    w!(body, "        {{ let mut s = std::collections::BTreeSet::new(); let mut hp = std::collections::BinaryHeap::new();");
    w!(body, "        for i in 0..bb({}u32) {{ let v = i.wrapping_mul({}) ^ {}; s.insert(v % 50); hp.push(v % 97); if i % 3 == 0 {{ hp.pop(); }} }}", rng.range(0, 40), rng.below(50) | 1, rng.below(100));
    w!(body, "        h.put(&s.iter().copied().collect::<Vec<u32>>()); h.put(&hp.into_sorted_vec()); }}");
}

fn strings(rng: &mut Rng, body: &mut String) {
    w!(body, "        {{ let mut s = String::new();");
    w!(body, "        for i in 0..bb({}u32) {{ s.push(char::from(b'a' + ((i.wrapping_mul({}) + {}) % 26) as u8)); if i % {} == 0 {{ s.push(','); }} }}", rng.range(0, 40), rng.below(30) | 1, rng.below(26), rng.range(2, 6));
    for _ in 0..rng.range(2, 5) {
        let op = match rng.below(8) {
            0 => "h.put(&s.chars().rev().collect::<String>().len());".to_string(),
            1 => "h.put(&s.split(',').map(|p| p.len() as u32).collect::<Vec<u32>>());".to_string(),
            2 => "h.put(&s.to_uppercase().bytes().fold(0u32, |a, b| a.wrapping_mul(7).wrapping_add(b as u32)));".to_string(),
            3 => format!("h.put(&s.char_indices().filter(|(i, c)| (*i + *c as usize) % {} == 0).count());", rng.range(2, 5)),
            4 => format!("h.put(&format!(\"{{:05}}|{{:x}}|{{:>8}}\", bb({}u32), bb({}u64), &s[..s.len().min(5)]).len());", rng.below(100000), rng.next_u64()),
            5 => "h.put(&s.split(',').filter_map(|p| p.bytes().next()).map(|b| b as u32).sum::<u32>());".to_string(),
            6 => format!("h.put(&\"{}\".parse::<u32>().ok());", rng.pick(&["123", "x9", "4294967295", "4294967296", "-1", "007"])),
            _ => "h.put(&s.find(',').map(|p| p as u64));".to_string(),
        };
        w!(body, "        {op}");
    }
    w!(body, "        h.put(&s.len()); }}");
}

/// Sorting/searching on arrays of many lengths and shapes (the std sort implementations switch
/// algorithms at size thresholds and are heavy generic/unsafe code), plus `DefaultHasher`.
fn sorting(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let ty = *rng.pick(&["u8", "u16", "u32", "u64", "i32", "i64", "u128", "usize"]);
    w!(items, "    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)] struct Rec{id} {{ k: {ty}, tag: u8 }}");
    w!(items, "    fn gen{id}(n: usize, shape: u32, seed: u64) -> Vec<{ty}> {{ let mut s = seed | 1; (0..n).map(|i| {{ s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); match shape % 6 {{ 0 => (s >> 33) as {ty}, 1 => i as {ty}, 2 => (n - i) as {ty}, 3 => 7 as {ty}, 4 => ((s >> 40) % 4) as {ty}, _ => (i % 17) as {ty} }} }}).collect() }}");
    w!(body, "        for (n, shape) in [(0usize, 0u32), (1, 1), (2, 2), (5, 0), (10, 4), (20, 1), (21, 2), (32, 5), (33, 0), (64, 3), (100, 4), (257, 0), (500, 5)] {{");
    w!(body, "            let base = gen{id}(bb(n), bb(shape), bb({}u64));", rng.next_u64());
    for _ in 0..rng.range(2, 5) {
        let op = match rng.below(12) {
            0 => "let mut v = base.clone(); v.sort(); h.put(&v);".to_string(),
            1 => "let mut v = base.clone(); v.sort_unstable(); h.put(&v);".to_string(),
            2 => "let mut v = base.clone(); v.sort_by(|a, b| b.cmp(a)); h.put(&v);".to_string(),
            3 => format!("let mut v = base.clone(); v.sort_by_key(|x| x.wrapping_mul({}) >> 1); h.put(&v);", rng.below(100) | 1),
            4 => format!("let mut v = base.clone(); v.sort_unstable_by_key(|x| std::cmp::Reverse(x.wrapping_add({}))); h.put(&v);", rng.below(100)),
            5 => "let mut v = base.clone(); if !v.is_empty() { let k = bb(3usize) % v.len(); let (_, m, _) = v.select_nth_unstable(k); h.put(m); }".to_string(),
            6 => format!("let mut v = base.clone(); v.sort(); h.put(&v.binary_search(&bb({} as {ty})).unwrap_or_else(|e| e)); h.put(&v.partition_point(|x| *x < {} as {ty}));", rng.below(300), rng.below(300)),
            7 => "let mut v = base.clone(); v.sort(); v.dedup(); h.put(&v.len()); h.put(&v.is_sorted());".to_string(),
            8 => format!("let mut r: Vec<Rec{id}> = base.iter().enumerate().map(|(i, &k)| Rec{id} {{ k, tag: (i % 5) as u8 }}).collect(); r.sort(); h.put(&r.iter().map(|x| x.tag as u64).fold(0u64, |a, b| a.rotate_left(3) ^ b)); r.sort_by(|a, b| a.tag.cmp(&b.tag).then(b.k.cmp(&a.k))); h.put(&r.first().map(|x| x.tag));"),
            9 => "let mut v = base.clone(); v.sort(); let mut w = v.clone(); w.reverse(); w.sort_unstable(); h.put(&(v == w)); h.put(&v.iter().rev().step_by(3).count());".to_string(),
            10 => "{ use std::hash::{Hash, Hasher}; let mut s = std::collections::hash_map::DefaultHasher::new(); base.hash(&mut s); (n, shape).hash(&mut s); \"k\".hash(&mut s); h.put(&s.finish()); }".to_string(),
            _ => "let mut v: Vec<String> = base.iter().take(40).map(|x| format!(\"{:x}\", x)).collect(); v.sort(); h.put(&v.join(\",\").len()); h.put(&v.first().map(|s| s.len()));".to_string(),
        };
        w!(body, "            {{ {op} }}");
    }
    w!(body, "        }}");
}

pub fn generate(rng: &mut Rng) -> String {
    let mut items = String::new();
    let mut body = String::new();
    let mut sid = 0;
    for _ in 0..rng.range(3, 7) {
        match rng.below(6) {
            0 => deque(rng, &mut body),
            1 => btree(rng, &mut body),
            2 => strings(rng, &mut body),
            _ => {
                sorting(rng, &mut items, &mut body, sid);
                sid += 1;
            }
        }
    }
    format!("{items}    pub fn run(h: &mut H) {{\n{body}    }}\n")
}
