//! Where files live on the console and on the development computer.

/// The USB share's folder for this app, when the wired debug host runs.
pub const HOST: &str = "host0:newyork";
pub const GXP_HOST: &str = "host0:newyork/gxp";
/// This app's folder on the memory card.
pub const DATA: &str = "ux0:data/pocket-newyork";
/// Programs compiled on this console.
pub const GXP_CACHE: &str = "ux0:data/pocket-newyork/gxp";
/// Programs shipped in the package.
pub const GXP_PACKAGED: &str = "app0:gxp";
/// The pack on the computer, and what says which one it is.
pub const PACK_HOST: &str = "host0:newyork/city.pack";
pub const PACK_HOST_ID: &str = "host0:newyork/city.json";
/// The copy of it the console reads from, and which one that is.
pub const PACK_CARD: &str = "ux0:data/pocket-newyork/city.pack";
pub const PACK_CARD_ID: &str = "ux0:data/pocket-newyork/city.json";
/// The pack of a packaged build.
pub const PACK_APP: &str = "app0:city.pack";
/// What the interface asked to have stored, in the data folder.
pub const INTERFACE_FILE: &str = "interface.json";

/// Paths a shipped file of the interface (`newyork.js`, `newyork.pak`) is looked for at, in order: the USB share
/// (development builds), the package, the data folder.
pub fn candidates(name: &str) -> Vec<String> {
    let mut v = Vec::new();
    if cfg!(feature = "usb-debug") {
        v.push(format!("{HOST}/{name}"));
    }
    v.push(format!("app0:{name}"));
    v.push(format!("{DATA}/{name}"));
    v
}

/// Writes a file in the data folder through a temporary file, so a power-off mid-write leaves the previous version.
pub fn write_text(name: &str, text: &str) {
    let _ = std::fs::create_dir_all(DATA);
    let (tmp, path) = (format!("{DATA}/{name}.tmp"), format!("{DATA}/{name}"));
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::rename(&tmp, &path);
    }
}
