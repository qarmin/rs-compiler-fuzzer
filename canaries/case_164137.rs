static mut g1: *mut u8 = std::ptr::null_mut::<u8>();
static mut g4: *mut u8 = std::ptr::null_mut::<u8>();
static mut g21: *mut u8 = std::ptr::null_mut::<u8>();

unsafe fn fn_0() {
    for i in 0u32..2u32 {
        if i == 0u32 {
            std::ptr::write(&raw mut g21, (&raw mut g21).cast::<u8>());
            std::ptr::write(&raw mut g1, std::ptr::read((std::ptr::read::<*mut u8>(&raw const g21)).cast::<*mut u8>()));
        } else {
            let g9: *mut u8 = std::ptr::read((std::ptr::read::<*mut u8>(&raw const g1)).cast::<*mut u8>());
            std::ptr::write(g9.cast::<*mut u8>(), std::ptr::null_mut::<u8>());
            std::ptr::write(g9.cast::<*mut u8>(), std::ptr::read::<*mut u8>(&raw const g21));
            std::ptr::write((std::ptr::read::<*mut u8>(&raw const g1)).cast::<*mut u8>(), std::ptr::read::<*mut u8>(&raw const g1));
            std::ptr::write(&raw mut g4, std::ptr::read((std::ptr::read::<*mut u8>(&raw const g21)).cast::<*mut u8>()));
            return;
        }
    }
}

fn main() {
    unsafe { fn_0(); }
    println!("ok");
}
