use std::hint::black_box;
trait Num: Copy + PartialOrd {
    fn wadd(self, o: Self) -> Self;
    fn wsub(self, o: Self) -> Self;
    fn from_u64(v: u64) -> Self;
    fn hi(self) -> u64;
}
impl Num for i128 {
    fn wadd(self, o: Self) -> Self {
        self.wrapping_add(o)
    }
    fn wsub(self, o: Self) -> Self {
        self.wrapping_sub(o)
    }
    fn from_u64(v: u64) -> Self {
        v as i128
    }
    fn hi(self) -> u64 {
        self as u64
    }
}
fn e<T: Num>(a: T, b: T, n: u32) -> T {
    let mut d = T::from_u64(0);
    for i in 0..n {
        let a = a.wadd(T::from_u64(i as u64));
        d = if T::from_u64(i as u64) >= T::from_u64(if T::from_u64(19937u64) >= a { a } else { T::from_u64(6u64) }.hi()) {
            T::from_u64(if T::from_u64(19937u64) >= a { T::from_u64(19937u64).wsub(a) } else { a }.hi())
        } else {
            T::from_u64(if T::from_u64(19937u64) >= a { T::from_u64(19937u64).wsub(a) } else { a }.hi()).wsub(T::from_u64(1))
        }
    }
    match 0 {
        _ => d,
    }
}
fn main() {
    let c: fn(i128, i128, u32) -> i128 = black_box(e);
    println!("{}", c(Num::from_u64(2961832378146261130u64), 0, 2))
}
