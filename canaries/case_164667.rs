use std::hint::black_box as a;
fn main() {
    let b: i8 = a(0);
    let a = a(1);
    let a = (-2i128).rotate_right(1).checked_mul(a).unwrap_or(0);
    println!("{}", a);
    let a = (b as u128).abs_diff(b as u16 as u128);
    println!("{}", a)
}
