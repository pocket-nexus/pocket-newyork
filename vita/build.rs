// vita2d starts GXM for the whole process; the city needs a larger parameter buffer and larger ring buffers than it
// asks for. The linker sends vita2d's calls to `__wrap_sceGxmInitialize` and `__wrap_sceGxmCreateContext`
// (src/main.rs), which raise the sizes and call on.
fn main() {
    println!("cargo:rustc-link-arg=-Wl,--wrap=sceGxmInitialize");
    println!("cargo:rustc-link-arg=-Wl,--wrap=sceGxmCreateContext");
    println!("cargo:rerun-if-changed=build.rs");
}
