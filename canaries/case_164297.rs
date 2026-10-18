use std::mem::MaybeUninit;

struct Stack {
    buf: [MaybeUninit<i32>; 8],
    len: usize,
}

impl Stack {
    #[inline(always)]
    fn push(&mut self, v: i32) {
        unsafe { self.buf.as_mut_ptr().add(self.len).cast::<i32>().write(v) };
        self.len += 1;
    }

    fn pop(&mut self) -> Option<i32> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(unsafe { self.buf[self.len].assume_init_read() })
    }
}

fn f(flag: u16) -> u32 {
    let mut s = Stack { buf: [MaybeUninit::new(0); 8], len: 0 };
    for i in 0..(flag as usize % 3) + 1 {
        s.push(i as i32);
    }
    let mut sum: u64 = 0;
    while let Some(x) = s.pop() {
        sum = sum.wrapping_add(x as u64);
    }
    (sum ^ s.len as u64) as u32
}

fn main() {
    println!("{}", f(std::hint::black_box(0)));
}
