//! `stdlib` family: `char`, `str`, `String`, UTF-8/UTF-16 conversion, formatting.
//! Prelude: `s`, `t`: `&str` (text with multi-byte characters, separators, spaces), `pat`: `&str` pattern,
//! `ch`, `ch2`: `char`, `k: usize` (small index), `ss: String` (= `s.to_string()`), `r: u32` radix, `cp: u32` code point.

use super::stdlib::{bbv, emit, pick_table, wrap};
use crate::rng::Rng;

pub const NAMES: &[&str] = &["char", "str", "string"];

const CHAR: &[&str] = &[
    "h.put(&ch.is_alphabetic()); h.put(&ch.is_alphanumeric()); h.put(&ch.is_numeric()); h.put(&ch.is_whitespace()); h.put(&ch.is_uppercase()); h.put(&ch.is_lowercase()); h.put(&ch.is_control());",
    "h.put(&ch.is_ascii()); h.put(&ch.is_ascii_alphabetic()); h.put(&ch.is_ascii_digit()); h.put(&ch.is_ascii_alphanumeric()); h.put(&ch.is_ascii_uppercase()); h.put(&ch.is_ascii_lowercase()); h.put(&ch.is_ascii_punctuation());",
    "h.put(&ch.is_ascii_graphic()); h.put(&ch.is_ascii_whitespace()); h.put(&ch.is_ascii_hexdigit()); h.put(&ch.is_ascii_control()); h.put(&ch.is_digit(r)); h.put(&ch.to_digit(r)); h.put(&ch2.to_digit(16));",
    "h.put(&ch.to_ascii_uppercase()); h.put(&ch.to_ascii_lowercase()); h.put(&ch.eq_ignore_ascii_case(&ch2)); let mut m = ch; m.make_ascii_uppercase(); h.put(&m);",
    "h.put(&ch.to_uppercase().collect::<String>()); h.put(&ch.to_lowercase().collect::<String>()); h.put(&ch.to_uppercase().count()); h.put(&ch2.to_lowercase().to_string());",
    "h.put(&ch.len_utf8()); h.put(&ch.len_utf16()); let mut buf = [0u8; 4]; h.put(&ch.encode_utf8(&mut buf).len()); h.put(&buf); let mut b16 = [0u16; 2]; h.put(&ch.encode_utf16(&mut b16).len()); h.put(&b16);",
    "h.put(&ch.escape_default().to_string()); h.put(&ch.escape_unicode().to_string()); h.put(&ch.escape_debug().to_string()); h.put(&ch.escape_default().count()); h.put(&ch2.escape_debug().collect::<String>());",
    "h.put(&(ch as u32)); h.put(&u32::from(ch)); h.put(&char::from_u32(cp)); h.put(&char::from_u32(cp ^ 0xd800)); h.put(&char::try_from(cp).ok()); h.put(&char::try_from(cp & 0xff).ok()); h.put(&u8::try_from(ch).ok()); h.put(&u16::try_from(ch2).ok());",
    "h.put(&char::from_digit(k as u32, r.max(2))); h.put(&char::from_digit(k as u32 + 10, 36)); h.put(&char::from(k as u8 + 60)); h.put(&char::from(65u8));",
    "h.put(&char::MAX); h.put(&char::MIN); h.put(&char::REPLACEMENT_CHARACTER); h.put(&char::UNICODE_VERSION.0); h.put(&char::MAX.len_utf8());",
    "h.put(&ch.cmp(&ch2)); h.put(&ch.partial_cmp(&ch2)); h.put(&(ch < ch2)); h.put(&(ch == ch2)); h.put(&ch.max(ch2)); h.put(&ch.min(ch2)); h.put(&ch.clamp('a', 'z'));",
    "h.put(&ch.to_string()); h.put(&format!(\"{}{:?}{:>4}|{:<4}|{:^5}|{:#x}\", ch, ch2, ch, ch2, ch, ch as u32)); h.put(&String::from(ch).len()); h.put(&ch.to_string().chars().next());",
    "h.put(&char::decode_utf16([0xd83d_u16, 0xde00, 0x0041, 0xd800, 0xdc00]).map(|r| r.map_err(|e| e.unpaired_surrogate())).collect::<Vec<_>>());",
    "h.put(&std::char::decode_utf16([cp as u16, (cp >> 4) as u16, 0xdc00]).map(|r| r.is_ok()).collect::<Vec<_>>()); h.put(&char::from_u32(0x1f600).map(|c| c.len_utf16()));",
    "h.put(&(('a'..='e').collect::<String>())); h.put(&(ch..=ch2).count().min(5)); h.put(&('a'..'f').rev().collect::<String>()); h.put(&('a'..='z').step_by(5).collect::<String>()); h.put(&('a'..='c').contains(&ch));",
];

const STR: &[&str] = &[
    "h.put(&s.len()); h.put(&s.is_empty()); h.put(&s.chars().count()); h.put(&s.bytes().len()); h.put(&s.is_ascii()); h.put(&s.is_char_boundary(k)); h.put(&s.is_char_boundary(0));",
    "h.put(&s.trim()); h.put(&s.trim_start()); h.put(&s.trim_end()); h.put(&s.trim_ascii()); h.put(&s.trim_ascii_start()); h.put(&s.trim_ascii_end());",
    "h.put(&s.trim_matches(ch)); h.put(&s.trim_start_matches(pat)); h.put(&s.trim_end_matches(pat)); h.put(&s.trim_matches(|c: char| c.is_ascii_digit())); h.put(&s.trim_start_matches(char::is_whitespace)); h.put(&s.trim_end_matches(&[ch, ch2][..]));",
    "h.put(&s.strip_prefix(pat)); h.put(&s.strip_suffix(pat)); h.put(&s.strip_prefix(ch)); h.put(&s.strip_suffix(ch2)); h.put(&s.strip_prefix(char::is_alphabetic));",
    "h.put(&s.starts_with(pat)); h.put(&s.ends_with(pat)); h.put(&s.starts_with(ch)); h.put(&s.ends_with(ch2)); h.put(&s.contains(pat)); h.put(&s.contains(ch)); h.put(&s.contains(char::is_uppercase)); h.put(&s.contains(&[ch, ch2][..]));",
    "h.put(&s.find(pat)); h.put(&s.rfind(pat)); h.put(&s.find(ch)); h.put(&s.rfind(ch2)); h.put(&s.find(char::is_whitespace)); h.put(&s.rfind(|c: char| !c.is_ascii()));",
    "h.put(&s.matches(pat).count()); h.put(&s.matches(ch).count()); h.put(&s.rmatches(pat).count()); h.put(&s.match_indices(pat).collect::<Vec<_>>()); h.put(&s.rmatch_indices(ch).collect::<Vec<_>>());",
    "h.put(&s.split(pat).collect::<Vec<_>>()); h.put(&s.split(ch).collect::<Vec<_>>()); h.put(&s.rsplit(pat).collect::<Vec<_>>()); h.put(&s.split(char::is_whitespace).count()); h.put(&s.split(&[ch, ch2][..]).collect::<Vec<_>>());",
    "h.put(&s.splitn(k % 4, pat).collect::<Vec<_>>()); h.put(&s.rsplitn(k % 4, ch).collect::<Vec<_>>()); h.put(&s.split_terminator(pat).collect::<Vec<_>>()); h.put(&s.rsplit_terminator(ch).collect::<Vec<_>>()); h.put(&s.split_inclusive(ch).collect::<Vec<_>>());",
    "h.put(&s.split_once(pat)); h.put(&s.rsplit_once(pat)); h.put(&s.split_once(ch)); h.put(&s.rsplit_once(char::is_whitespace)); h.put(&s.split_whitespace().collect::<Vec<_>>()); h.put(&s.split_ascii_whitespace().rev().collect::<Vec<_>>());",
    "h.put(&s.lines().collect::<Vec<_>>()); h.put(&\"a\\r\\nb\\n\\nc\\r\".lines().collect::<Vec<_>>()); h.put(&s.lines().count()); h.put(&s.split('\\n').last());",
    "h.put(&s.replace(pat, \"<>\")); h.put(&s.replace(ch, \"\")); h.put(&s.replacen(pat, \"!\", k)); h.put(&s.replacen(ch2, \"ab\", 1)); h.put(&s.replace(char::is_whitespace, \"_\"));",
    "h.put(&s.to_lowercase()); h.put(&s.to_uppercase()); h.put(&s.to_ascii_lowercase()); h.put(&s.to_ascii_uppercase()); h.put(&s.eq_ignore_ascii_case(t)); h.put(&s.repeat(k % 4));",
    "let mut m = s.to_string(); m.make_ascii_uppercase(); h.put(&m); m.make_ascii_lowercase(); h.put(&m); let mut b = s.to_owned().into_boxed_str(); b.make_ascii_uppercase(); h.put(&&*b);",
    "h.put(&s.chars().rev().collect::<String>()); h.put(&s.chars().nth(k)); h.put(&s.chars().last()); h.put(&s.char_indices().nth(k)); h.put(&s.char_indices().rev().next()); h.put(&s.chars().filter(|c| c.is_alphanumeric()).count());",
    "h.put(&s.bytes().rev().take(5).collect::<Vec<u8>>()); h.put(&s.bytes().nth(k)); h.put(&s.as_bytes().len()); h.put(&s.as_bytes().first()); h.put(&s.bytes().map(|b| b as u32).sum::<u32>()); h.put(&s.bytes().position(|b| b == b' '));",
    "h.put(&s.get(0..k)); h.put(&s.get(k..)); h.put(&s.get(..=k)); h.put(&s.get(k..k + 2)); h.put(&s.get(k + 1..k)); h.put(&s.split_at_checked(k)); h.put(&s.split_at_checked(0)); h.put(&s.get(s.len()..));",
    "if s.is_char_boundary(k) { let (x, y) = s.split_at(k); h.put(&x); h.put(&y); h.put(&&s[..k]); h.put(&&s[k..]); }",
    "h.put(&s.parse::<i64>().ok()); h.put(&s.trim().parse::<u32>().ok()); h.put(&pat.parse::<f64>().ok()); h.put(&s.parse::<bool>().ok()); h.put(&\"true\".parse::<bool>().ok()); h.put(&ch.to_string().parse::<char>().ok()); h.put(&s.parse::<char>().ok());",
    "h.put(&s.cmp(t)); h.put(&s.partial_cmp(t)); h.put(&(s < t)); h.put(&(s == t)); h.put(&s.eq(t)); h.put(&s.max(t)); h.put(&s.min(t)); h.put(&(s != t));",
    "h.put(&s.escape_debug().to_string()); h.put(&s.escape_default().to_string()); h.put(&s.escape_unicode().count()); h.put(&format!(\"{:?}\", s)); h.put(&format!(\"{:#?}\", t));",
    "h.put(&format!(\"{:>12}|\", s)); h.put(&format!(\"{:<12}|\", t)); h.put(&format!(\"{:^12}|\", s)); h.put(&format!(\"{:*^15.4}|\", s)); h.put(&format!(\"{:-<9.2}|\", t)); h.put(&format!(\"{0}{1}{0}\", s, t)); h.put(&format!(\"{s}{t:?}\"));",
    "h.put(&[s, t].concat()); h.put(&[s, t, pat].join(\"-\")); h.put(&[s, t].join(pat)); h.put(&vec![s.to_string(), t.to_string()].concat()); h.put(&[ss.as_str(), t].join(\"\"));",
    "h.put(&std::str::from_utf8(s.as_bytes()).is_ok()); h.put(&std::str::from_utf8(&s.as_bytes()[..k.min(s.len())]).is_ok()); h.put(&std::str::from_utf8(&[0xffu8, b'a']).map_err(|e| (e.valid_up_to(), e.error_len()))); h.put(&std::str::from_utf8(&[b'a', 0xe2, 0x82]).map_err(|e| (e.valid_up_to(), e.error_len())));",
    "h.put(&String::from_utf8_lossy(&[b'a', 0xff, b'b', 0xe2, 0x82]).into_owned()); h.put(&String::from_utf8(vec![0xc3, 0xa9]).ok()); h.put(&String::from_utf8(vec![0xc3]).map_err(|e| e.utf8_error().valid_up_to())); h.put(&String::from_utf16(&[0xd83d, 0xde00]).ok()); h.put(&String::from_utf16(&[0xd83d]).is_err()); h.put(&String::from_utf16_lossy(&[0x41, 0xd800, 0x42]));",
    "for chunk in [b'a', 0xff, b'b', 0xc3, 0xa9, 0xe2].utf8_chunks() { h.put(&chunk.valid()); h.put(&chunk.invalid()); }",
    "h.put(&s.encode_utf16().count()); h.put(&s.encode_utf16().take(5).collect::<Vec<u16>>()); h.put(&s.chars().map(|c| c.len_utf8()).sum::<usize>()); h.put(&s.char_indices().map(|(i, c)| i + c.len_utf16()).max());",
    "h.put(&s.chars().map(|c| c.to_ascii_uppercase()).collect::<String>()); h.put(&s.chars().flat_map(|c| c.to_uppercase()).collect::<String>()); h.put(&s.chars().filter(|c| !c.is_whitespace()).collect::<String>()); h.put(&s.chars().skip(k).step_by(2).collect::<String>());",
    "h.put(&s.chars().zip(t.chars()).filter(|(x, y)| x == y).count()); h.put(&s.chars().eq(t.chars())); h.put(&s.chars().cmp(t.chars())); h.put(&s.bytes().lt(t.bytes())); h.put(&s.chars().rev().eq(s.chars().rev()));",
    "h.put(&s.to_owned()); h.put(&s.to_string().len()); h.put(&String::from(s)); h.put(&Box::<str>::from(s).len()); h.put(&std::rc::Rc::<str>::from(t).len()); h.put(&std::sync::Arc::<str>::from(s).len());",
    "h.put(&s.chars().all(char::is_alphanumeric)); h.put(&s.chars().any(char::is_whitespace)); h.put(&s.chars().position(|c| c == ch));",
    "{ use std::hash::{Hash, Hasher}; let mut x = std::collections::hash_map::DefaultHasher::new(); s.hash(&mut x); t.hash(&mut x); ch.hash(&mut x); h.put(&x.finish()); }",
    "h.put(&s.floor_char_boundary(k)); h.put(&s.ceil_char_boundary(k)); h.put(&s.floor_char_boundary(s.len() + 5)); h.put(&s.strip_circumfix(ch, ch2)); h.put(&s.substr_range(&s[s.floor_char_boundary(k)..]).map(|r| (r.start, r.end))); h.put(&s.substr_range(t).is_some()); h.put(&s.split_at_checked(k)); h.put(&(s.as_ptr() == s.as_ptr()));",
    "let mut b = s.as_bytes().to_vec(); h.put(&std::str::from_utf8_mut(&mut b).map(|x| { x.make_ascii_uppercase(); x.len() }).ok()); h.put(&b); h.put(&s.to_owned().into_boxed_str().into_boxed_bytes().len()); let mut st = s.to_string(); h.put(&st.as_mut_str().len()); h.put(&String::from_utf16le(&[0x41, 0, 0x42, 0]).ok()); h.put(&String::from_utf16be(&[0, 0x41, 0, 0x42]).ok()); h.put(&String::from_utf16le_lossy(&[0x41, 0, 0x42]).len()); h.put(&String::from_utf16be_lossy(&[0, 0x41, 0xd8]).len());",
    "let mut st = ss.clone(); st.reserve_exact(k); h.put(&(st.capacity() >= st.len() + k)); h.put(&st.try_reserve(k + 1).is_ok()); h.put(&st.try_reserve_exact(k + 1).is_ok()); h.put(&String::from_utf8_lossy_owned(vec![0x61, 0xff, 0x62]));",
    "let mut ci = s.char_indices(); ci.next(); h.put(&ci.offset()); h.put(&ci.as_str().len()); let ci2 = s.char_indices().rev(); h.put(&ci2.count());",
];

const STRING: &[&str] = &[
    "let mut m = ss.clone(); m.push(ch); m.push_str(t); m.push_str(pat); h.put(&m); h.put(&m.len()); h.put(&m.pop()); h.put(&m.pop()); h.put(&m);",
    "let mut m = ss.clone(); m.insert(0, ch); m.insert_str(0, t); if m.is_char_boundary(k) { m.insert(k, ch2); m.insert_str(k, pat); } h.put(&m);",
    "let mut m = ss.clone(); if !m.is_empty() { let i = m.char_indices().nth(k).map(|(i, _)| i).unwrap_or(0); if i < m.len() { h.put(&m.remove(i)); } } h.put(&m);",
    "let mut m = ss.clone(); let cut = m.char_indices().nth(k).map(|(i, _)| i).unwrap_or(m.len()); m.truncate(cut); h.put(&m); m.clear(); h.put(&m.is_empty()); h.put(&m.len());",
    "let mut m = ss.clone(); let cut = m.char_indices().nth(k).map(|(i, _)| i).unwrap_or(m.len()); let d: String = m.drain(..cut).collect(); h.put(&d); h.put(&m);",
    "let mut m = ss.clone(); let cut = m.char_indices().nth(k).map(|(i, _)| i).unwrap_or(m.len()); m.replace_range(..cut, t); h.put(&m); m.replace_range(m.len().., pat); h.put(&m);",
    "let mut m = ss.clone(); m.retain(|c| c != ch && !c.is_ascii_digit()); h.put(&m); m.retain(char::is_alphanumeric); h.put(&m);",
    "let mut m = ss.clone(); let cut = m.char_indices().nth(k).map(|(i, _)| i).unwrap_or(m.len()); let tail = m.split_off(cut); h.put(&m); h.put(&tail); m += &tail; m += t; h.put(&m);",
    "let mut m = ss.clone(); m.extend([ch, ch2, 'z']); m.extend([\"ab\", \"cd\"]); m.extend(vec![String::from(\"ef\")]); m.extend(t.chars().rev()); h.put(&m); h.put(&String::from_iter([ch, ch2])); h.put(&['x', 'y'].iter().collect::<String>());",
    "let m = ss.clone() + t + pat; h.put(&m); h.put(&(ss.clone() + &ss).len()); h.put(&m.as_str()); h.put(&m.as_bytes().len()); h.put(&m.clone().into_bytes()); h.put(&m.clone().into_boxed_str().len()); h.put(&m.chars().rev().collect::<String>());",
    "let mut m = String::with_capacity(k); h.put(&(m.capacity() >= k)); m.reserve(10); h.put(&(m.capacity() >= 10)); m.push_str(s); m.shrink_to_fit(); h.put(&(m.capacity() >= m.len())); m.shrink_to(0); h.put(&m);",
    "{ use std::fmt::Write; let mut m = String::new(); write!(m, \"{}-{:?}-{:>5}\", s, ch, k).unwrap(); writeln!(m, \"{t}\").unwrap(); m.write_str(pat).unwrap(); m.write_char(ch).unwrap(); h.put(&m); }",
    "let mut m = ss.clone(); let cut = m.char_indices().nth(k).map(|(i, _)| i).unwrap_or(m.len()); m.extend_from_within(..cut); h.put(&m);",
    "let m: String = ss.clone(); h.put(&(m == s)); h.put(&(m.as_str() == t)); h.put(&(s == m)); h.put(&m.cmp(&t.to_string())); h.put(&(m < t.to_string())); h.put(&m.eq_ignore_ascii_case(t)); h.put(&m.contains(pat));",
    "let m: std::borrow::Cow<str> = std::borrow::Cow::Borrowed(s); h.put(&m.len()); let mut o = m.clone(); o.to_mut().push_str(t); h.put(&*o); h.put(&o.into_owned()); h.put(&matches!(m, std::borrow::Cow::Borrowed(_))); h.put(&String::from_utf8_lossy(s.as_bytes()).len());",
    "h.put(&(ss.chars().rev().collect::<String>() == ss)); h.put(&ss.chars().rev().collect::<String>().len());",
    "let b: Box<str> = ss.clone().into_boxed_str(); h.put(&b.len()); h.put(&b.into_string()); h.put(&String::from(Box::<str>::from(t)));",
    "h.put(&String::new().is_empty()); h.put(&String::default().len()); h.put(&\"\".to_string().pop()); h.put(&String::from(\"é\").into_bytes()); h.put(&\"ß\".to_uppercase()); h.put(&\"İ\".to_lowercase()); h.put(&\"ǅ\".to_lowercase()); h.put(&\"ΑΣ\".to_lowercase());",
];

fn pick_text(rng: &mut Rng) -> String {
    let parts: &[&str] = &[
        "hello", " ", "wörld", ",", "ß", "", "42", "-7", "\\n", "中文", "😀", "A,b;C", "  x  ", "ǅ", "ab", "AB", "é", "a b c", "\\t", "0x1F", "true", "1.5e3", "ΑΣ", "\\u{0}", "İ", "a\\r\\nb",
    ];
    let mut t = String::new();
    for _ in 0..rng.range(1, 5) {
        t += rng.pick(parts);
    }
    format!("\"{t}\"")
}

fn pick_pat(rng: &mut Rng) -> String {
    format!("\"{}\"", rng.pick(&["", ",", "ab", " ", "ö", "l", "é", "AB", "x", "42", "😀", "\\n", "ß", "o w"]))
}

fn pick_char(rng: &mut Rng) -> String {
    format!("'{}'", rng.pick(&["a", "Z", "0", "é", "ß", "ǅ", "中", "😀", "\\u{0}", " ", "\\n", "\\u{10FFFF}", "z", "9", "_", "İ", "Σ", "\\u{301}", "\\'", "7"]))
}

pub fn generate(rng: &mut Rng) -> String {
    let (s, t, pat, ch, ch2) = (pick_text(rng), pick_text(rng), pick_pat(rng), pick_char(rng), pick_char(rng));
    let mut p = String::new();
    p += &format!("        let s: &str = {}; let t: &str = {}; let pat: &str = {};\n", bbv(rng, s), bbv(rng, t), bbv(rng, pat));
    p += &format!("        let ch: char = {}; let ch2: char = {}; let ss: String = s.to_string();\n", bbv(rng, ch), bbv(rng, ch2));
    let (k, r, cp) = (rng.below(9), rng.range(2, 36), if rng.chance(50) { rng.below(0x11_0000) } else { rng.below(0x300) });
    p += &format!("        let k: usize = {}; let r: u32 = {r}; let cp: u32 = {};\n", bbv(rng, format!("{k}usize")), bbv(rng, format!("{cp}u32")));
    let n = rng.range(5, 12);
    let table = pick_table(rng, &[("char", CHAR), ("str", STR), ("str", STR), ("string", STRING)]);
    let tables = [table];
    let body = emit(rng, &p, &tables, n);
    wrap("", &body)
}
