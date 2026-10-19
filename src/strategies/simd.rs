//! Strategy `simd`: chains of x86-64 SIMD intrinsics (SSE2, SSSE3/SSE4, AVX2) and bit-manipulation
//! intrinsics (BMI2 pdep/pext, lzcnt, ...) on data loaded from `black_box`ed arrays. Compared across
//! optimisation levels, `target-cpu` levels and ISel modes. Related: rust-lang/rust#164298,
//! #164299 (AVX2 / x86-64-v3 crashes).
//!
//! Each feature level is only used behind `is_x86_feature_detected!`, in functions marked with
//! the matching `#[target_feature]`; everything is x86_64-only (`cfg`), empty elsewhere.
//! Shift/shuffle immediates are literal const generics. Unaligned loads/stores only.

use crate::rng::Rng;
use crate::w;

/// (template, needs second operand). `{a}`/`{b}` are registers, `{n}` a small immediate,
/// `{i}` an 8-bit shuffle immediate.
const SSE2: &[&str] = &[
    "_mm_add_epi8({a}, {b})", "_mm_add_epi16({a}, {b})", "_mm_add_epi32({a}, {b})", "_mm_add_epi64({a}, {b})",
    "_mm_sub_epi8({a}, {b})", "_mm_sub_epi16({a}, {b})", "_mm_sub_epi32({a}, {b})", "_mm_sub_epi64({a}, {b})",
    "_mm_adds_epi8({a}, {b})", "_mm_adds_epi16({a}, {b})", "_mm_adds_epu8({a}, {b})", "_mm_adds_epu16({a}, {b})",
    "_mm_subs_epi8({a}, {b})", "_mm_subs_epi16({a}, {b})", "_mm_subs_epu8({a}, {b})", "_mm_subs_epu16({a}, {b})",
    "_mm_mullo_epi16({a}, {b})", "_mm_mulhi_epi16({a}, {b})", "_mm_mulhi_epu16({a}, {b})", "_mm_mul_epu32({a}, {b})",
    "_mm_madd_epi16({a}, {b})", "_mm_and_si128({a}, {b})", "_mm_or_si128({a}, {b})", "_mm_xor_si128({a}, {b})",
    "_mm_andnot_si128({a}, {b})", "_mm_cmpeq_epi8({a}, {b})", "_mm_cmpeq_epi16({a}, {b})", "_mm_cmpeq_epi32({a}, {b})",
    "_mm_cmpgt_epi8({a}, {b})", "_mm_cmpgt_epi16({a}, {b})", "_mm_cmpgt_epi32({a}, {b})", "_mm_cmplt_epi8({a}, {b})",
    "_mm_min_epu8({a}, {b})", "_mm_max_epu8({a}, {b})", "_mm_min_epi16({a}, {b})", "_mm_max_epi16({a}, {b})",
    "_mm_avg_epu8({a}, {b})", "_mm_avg_epu16({a}, {b})", "_mm_sad_epu8({a}, {b})",
    "_mm_unpacklo_epi8({a}, {b})", "_mm_unpackhi_epi8({a}, {b})", "_mm_unpacklo_epi16({a}, {b})", "_mm_unpackhi_epi16({a}, {b})",
    "_mm_unpacklo_epi32({a}, {b})", "_mm_unpackhi_epi32({a}, {b})", "_mm_unpacklo_epi64({a}, {b})", "_mm_unpackhi_epi64({a}, {b})",
    "_mm_packs_epi16({a}, {b})", "_mm_packs_epi32({a}, {b})", "_mm_packus_epi16({a}, {b})",
    "_mm_slli_epi16::<{n}>({a})", "_mm_srli_epi16::<{n}>({a})", "_mm_srai_epi16::<{n}>({a})",
    "_mm_slli_epi32::<{n}>({a})", "_mm_srli_epi32::<{n}>({a})", "_mm_srai_epi32::<{n}>({a})",
    "_mm_slli_epi64::<{n}>({a})", "_mm_srli_epi64::<{n}>({a})", "_mm_slli_si128::<{m}>({a})", "_mm_srli_si128::<{m}>({a})",
    "_mm_shuffle_epi32::<{i}>({a})", "_mm_shufflelo_epi16::<{i}>({a})", "_mm_shufflehi_epi16::<{i}>({a})",
    "_mm_sll_epi32({a}, {b})", "_mm_srl_epi64({a}, {b})", "_mm_sra_epi16({a}, {b})",
];

const SSE41: &[&str] = &[
    "_mm_mullo_epi32({a}, {b})", "_mm_max_epi32({a}, {b})", "_mm_min_epu32({a}, {b})", "_mm_max_epu32({a}, {b})",
    "_mm_min_epi8({a}, {b})", "_mm_blend_epi16::<{i}>({a}, {b})", "_mm_blendv_epi8({a}, {b}, _mm_cmpgt_epi8({a}, {b}))",
    "_mm_cmpeq_epi64({a}, {b})", "_mm_cmpgt_epi64({a}, {b})", "_mm_cvtepu8_epi16({a})", "_mm_cvtepi8_epi32({a})",
    "_mm_cvtepu16_epi64({a})", "_mm_packus_epi32({a}, {b})", "_mm_mul_epi32({a}, {b})",
    "_mm_shuffle_epi8({a}, {b})", "_mm_abs_epi32({a})", "_mm_abs_epi8({a})", "_mm_hadd_epi32({a}, {b})", "_mm_hadd_epi16({a}, {b})",
    "_mm_hsub_epi16({a}, {b})", "_mm_maddubs_epi16({a}, {b})", "_mm_sign_epi8({a}, {b})", "_mm_sign_epi16({a}, {b})",
    "_mm_sign_epi32({a}, {b})", "_mm_alignr_epi8::<{m}>({a}, {b})", "_mm_mulhrs_epi16({a}, {b})",
    "_mm_insert_epi32::<{k}>({a}, {w} as i32)", "_mm_insert_epi8::<{h}>({a}, {w} as i32)", "_mm_minpos_epu16({a})",
    "_mm_mpsadbw_epu8::<{k}>({a}, {b})",
];

const AVX2: &[&str] = &[
    "_mm256_add_epi8({a}, {b})", "_mm256_add_epi16({a}, {b})", "_mm256_add_epi32({a}, {b})", "_mm256_add_epi64({a}, {b})",
    "_mm256_sub_epi8({a}, {b})", "_mm256_sub_epi16({a}, {b})", "_mm256_sub_epi32({a}, {b})", "_mm256_sub_epi64({a}, {b})",
    "_mm256_adds_epi8({a}, {b})", "_mm256_adds_epu16({a}, {b})", "_mm256_subs_epu8({a}, {b})",
    "_mm256_mullo_epi16({a}, {b})", "_mm256_mullo_epi32({a}, {b})", "_mm256_mulhi_epu16({a}, {b})", "_mm256_mul_epi32({a}, {b})",
    "_mm256_mul_epu32({a}, {b})", "_mm256_madd_epi16({a}, {b})", "_mm256_maddubs_epi16({a}, {b})",
    "_mm256_and_si256({a}, {b})", "_mm256_or_si256({a}, {b})", "_mm256_xor_si256({a}, {b})", "_mm256_andnot_si256({a}, {b})",
    "_mm256_cmpeq_epi8({a}, {b})", "_mm256_cmpeq_epi32({a}, {b})", "_mm256_cmpgt_epi8({a}, {b})", "_mm256_cmpgt_epi32({a}, {b})",
    "_mm256_cmpgt_epi64({a}, {b})", "_mm256_min_epu8({a}, {b})", "_mm256_max_epi16({a}, {b})", "_mm256_min_epi32({a}, {b})",
    "_mm256_max_epu32({a}, {b})", "_mm256_avg_epu8({a}, {b})", "_mm256_sad_epu8({a}, {b})",
    "_mm256_unpacklo_epi8({a}, {b})", "_mm256_unpackhi_epi16({a}, {b})", "_mm256_unpacklo_epi32({a}, {b})", "_mm256_unpackhi_epi64({a}, {b})",
    "_mm256_packs_epi16({a}, {b})", "_mm256_packus_epi32({a}, {b})", "_mm256_shuffle_epi8({a}, {b})",
    "_mm256_permutevar8x32_epi32({a}, {b})", "_mm256_permute4x64_epi64::<{i}>({a})", "_mm256_permute2x128_si256::<{i}>({a}, {b})",
    "_mm256_slli_epi16::<{n}>({a})", "_mm256_srli_epi32::<{n}>({a})", "_mm256_srai_epi32::<{n}>({a})", "_mm256_slli_epi64::<{n}>({a})",
    "_mm256_sllv_epi32({a}, {b})", "_mm256_srlv_epi32({a}, {b})", "_mm256_srav_epi32({a}, {b})", "_mm256_sllv_epi64({a}, {b})",
    "_mm256_alignr_epi8::<{m}>({a}, {b})", "_mm256_blend_epi32::<{i}>({a}, {b})", "_mm256_blendv_epi8({a}, {b}, _mm256_cmpgt_epi8({a}, {b}))",
    "_mm256_abs_epi16({a})", "_mm256_hadd_epi32({a}, {b})", "_mm256_sign_epi16({a}, {b})", "_mm256_shuffle_epi32::<{i}>({a})",
    "_mm256_cvtepu8_epi16(_mm256_castsi256_si128({a}))", "_mm256_cvtepi16_epi32(_mm256_extracti128_si256::<1>({a}))",
    "_mm256_inserti128_si256::<1>({a}, _mm256_castsi256_si128({b}))", "_mm256_set1_epi32({w} as i32)",
    "_mm256_srli_si256::<{m}>({a})", "_mm256_slli_si256::<{m}>({a})", "_mm256_mulhrs_epi16({a}, {b})",
];

fn expr(rng: &mut Rng, tpl: &str, a: usize, b: usize) -> String {
    let m = rng.below(17);
    tpl.replace("{a}", &format!("r{a}"))
        .replace("{b}", &format!("r{b}"))
        .replace("{n}", &rng.below(20).to_string())
        .replace("{m}", &m.to_string())
        .replace("{i}", &rng.below(256).to_string())
        .replace("{h}", &rng.below(16).to_string())
        .replace("{k}", &rng.below(4).to_string())
        .replace("{w}", &format!("{}u32", rng.next_u64() as u32))
}

/// Generate one kernel fn (returns its name and call code).
fn kernel(rng: &mut Rng, id: usize, level: &str, items: &mut String, body: &mut String) {
    let (vec, width, pool, feats, detect, load, store, mov): (&str, usize, Vec<&str>, &str, &str, &str, &str, &str) = match level {
        "sse2" => ("__m128i", 16, SSE2.to_vec(), "", "true", "_mm_loadu_si128", "_mm_storeu_si128", "_mm_movemask_epi8"),
        "sse41" => ("__m128i", 16, SSE2.iter().chain(SSE41.iter()).copied().collect(), "ssse3,sse4.1,sse4.2", "is_x86_feature_detected!(\"ssse3\") && is_x86_feature_detected!(\"sse4.1\") && is_x86_feature_detected!(\"sse4.2\")", "_mm_loadu_si128", "_mm_storeu_si128", "_mm_movemask_epi8"),
        _ => ("__m256i", 32, AVX2.to_vec(), "avx2", "is_x86_feature_detected!(\"avx2\")", "_mm256_loadu_si256", "_mm256_storeu_si256", "_mm256_movemask_epi8"),
    };
    let nregs = rng.range(3, 5);
    let mut b = String::new();
    for r in 0..nregs {
        w!(b, "        let mut r{r} = {load}(x{r}.as_ptr() as *const {vec});");
    }
    for _ in 0..rng.range(5, 24) {
        let tpl = *rng.pick(&pool);
        let (dst, a, bb) = (rng.below(nregs), rng.below(nregs), rng.below(nregs));
        let e = expr(rng, tpl, a, bb);
        // some templates change the vector type (cvt/extract); all pool entries return `vec`
        w!(b, "        r{dst} = {e};");
        if rng.chance(15) {
            w!(b, "        acc = acc.wrapping_mul(31).wrapping_add({mov}(r{dst}) as u32);");
        }
    }
    let attr = if feats.is_empty() { String::new() } else { format!("#[target_feature(enable = \"{feats}\")]\n    ") };
    let mut stores = String::new();
    for r in 0..nregs {
        w!(stores, "        {store}(out[{r}].as_mut_ptr() as *mut {vec}, r{r});");
    }
    w!(items, "    #[cfg(target_arch = \"x86_64\")]\n    {attr}#[inline(never)]\n    unsafe fn k{id}({}) -> ([[u8; {width}]; {nregs}], u32) {{\n        let mut acc = 0u32;\n{b}        let mut out = [[0u8; {width}]; {nregs}];\n{stores}        (out, acc)\n    }}",
        (0..nregs).map(|r| format!("x{r}: &[u8; {width}]")).collect::<Vec<_>>().join(", "));
    for _ in 0..rng.range(1, 3) {
        let args: Vec<String> = (0..nregs).map(|_| format!("&bb(std::array::from_fn::<u8, {width}, _>(|i| (i as u8).wrapping_mul({}).wrapping_add({})))", rng.below(255) | 1, rng.below(256))).collect();
        w!(body, "        if {detect} {{ let (o, a) = unsafe {{ k{id}({}) }}; h.put(&o); h.put(&a); }}", args.join(", "));
    }
}

fn bits(rng: &mut Rng, id: usize, items: &mut String, body: &mut String) {
    w!(items, "    #[cfg(target_arch = \"x86_64\")]\n    #[target_feature(enable = \"bmi1,bmi2,lzcnt,popcnt\")]\n    #[inline(never)]\n    unsafe fn bit{id}(a: u64, b: u64, c: u32) -> u64 {{");
    w!(items, "        let mut x = a;");
    for _ in 0..rng.range(3, 10) {
        let s = match rng.below(12) {
            0 => "x = _pdep_u64(x, b);",
            1 => "x = _pext_u64(x, b);",
            2 => "x = x.rotate_left(_lzcnt_u64(x) as u32);",
            3 => "x ^= _tzcnt_u64(x | 1 << 40);",
            4 => "x = x.wrapping_add(_popcnt64((x ^ b) as i64) as u64);",
            5 => "{ let (lo, hi) = (_mulx_u64(x, b, &mut 0u64), 0u64); x ^= lo ^ hi; }",
            6 => "x = _blsi_u64(x) ^ _blsr_u64(b | 1) ^ _blsmsk_u64(x);",
            7 => "x = _bextr_u64(x, (c & 63) as u32, ((c >> 6) & 63) as u32) ^ b;",
            8 => "x = _bzhi_u64(x, c & 127);",
            9 => "x = x.wrapping_add(_pdep_u32(c, (b >> 8) as u32) as u64);",
            10 => "x = x.wrapping_mul(_pext_u32((x >> 3) as u32, b as u32 | 1) as u64 | 1);",
            _ => "x = x.rotate_right(_lzcnt_u32((x >> 7) as u32) + 1);",
        };
        w!(items, "        {s}");
    }
    w!(items, "        x\n    }}");
    w!(body, "        if is_x86_feature_detected!(\"bmi1\") && is_x86_feature_detected!(\"bmi2\") && is_x86_feature_detected!(\"lzcnt\") && is_x86_feature_detected!(\"popcnt\") {{ for (a, b, c) in [({}u64, {}u64, {}u32), (bb(u64::MAX), bb(0x0f0f_0f0f_f0f0_f0f0u64), bb(77u32)), (bb(1u64), bb(0u64), bb(0u32))] {{ h.put(&unsafe {{ bit{id}(a, b, c) }}); }} }}", rng.next_u64(), rng.next_u64(), rng.next_u64() as u32);
}

pub fn generate(rng: &mut Rng) -> String {
    let mut items = String::from("    #[cfg(target_arch = \"x86_64\")]\n    use std::arch::x86_64::*;\n");
    let mut body = String::new();
    w!(body, "        #[cfg(target_arch = \"x86_64\")]\n        {{");
    for id in 0..rng.range(2, 5) {
        match rng.below(7) {
            0 | 1 => kernel(rng, id, "sse2", &mut items, &mut body),
            2 | 3 => kernel(rng, id, "sse41", &mut items, &mut body),
            4 | 5 => kernel(rng, id, "avx2", &mut items, &mut body),
            _ => bits(rng, id, &mut items, &mut body),
        }
    }
    w!(body, "        }}");
    format!("{items}    pub fn run(h: &mut H) {{\n{body}    }}\n")
}
