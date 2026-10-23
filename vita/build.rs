// vita2d starts GXM for the whole process; the city needs a larger parameter buffer than it asks for. The
// linker sends vita2d's call to `__wrap_sceGxmInitialize` (src/main.rs), which raises the size and calls on.
fn main() {
    println!("cargo:rustc-link-arg=-Wl,--wrap=sceGxmInitialize");
    println!("cargo:rerun-if-changed=build.rs");
}
