#[inline(never)]
fn f(buf: &[u8], x: u64) -> (u64, usize) {
    let mut at = 1;
    let mut acc = 0u64;
    loop {
        acc ^= x;
        let more = at < buf.len();
        at += more as usize;
        if !more {
            break;
        }
    }
    (acc, buf[at..].len())
}

fn main() {
    let buf = vec![0u8; std::hint::black_box(4)];
    println!("{:?}", f(&buf, 1));
}
