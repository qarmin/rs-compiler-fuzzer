#![feature(custom_mir, core_intrinsics)]

use std::intrinsics::mir::*;

#[repr(u8)]
enum Foo {
    A(u8),
    B(u8),
}

#[custom_mir(dialect = "runtime")]
fn to_a(x: Foo) -> Foo {
    mir! {
        {
            RET = Foo::A(Field(Variant(x, 0), 0));
            Return()
        }
    }
}

fn main() {
    assert!(matches!(to_a(Foo::B(5)), Foo::A(5)));
}
