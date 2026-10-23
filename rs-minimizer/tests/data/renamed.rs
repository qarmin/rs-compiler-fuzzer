mod d {
pub struct c(u32);
use std::hint::black_box;
impl c {
pub fn d() -> c {
c(0)
}
fn a(&mut self, b: u32) {
self.0 = b
}
pub fn e<e: a>(&mut self, b: &e) {
b.a(self)
}
}
trait a {
fn a(&self, e: &mut c);
}
impl a for i32 {
fn a(&self, e: &mut c) {
e.a(*self as u32)
}
}
pub fn b<e>(a: e) -> e {
black_box(a)
}
pub fn a(a: i8, d: bool, e: &c) {
println!("{}", e.0)
}
}
use d::*;
mod a {
use super::*;
pub fn a(e: &mut c) {
let d: i8 = b(-1);
let c = b(0);
let c = 1i64.checked_add(c).unwrap_or(d as i64);
let a = ((c as i32).abs_diff(d as i32) as i32).rotate_left(d as u8 as u32).wrapping_add(d as i32);
e.e(&a);
0i8.wrapping_div(c as i8);
}
}
fn main() {
let mut e = c::d();
a::a(&mut e);
a(0, true, &e)
}
