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

pub fn generate(rng: &mut Rng) -> String {
    let mut body = String::new();
    for _ in 0..rng.range(3, 7) {
        match rng.below(4) {
            0 | 1 => deque(rng, &mut body),
            2 => btree(rng, &mut body),
            _ => strings(rng, &mut body),
        }
    }
    format!("    pub fn run(h: &mut H) {{\n{body}    }}\n")
}
