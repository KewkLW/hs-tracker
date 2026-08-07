fn main() {
    // vendored Npcap SDK import libs; wpcap.dll is delay-loaded so the app
    // starts even without Npcap installed
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("npcap-sdk/Lib/x64");
    println!("cargo:rustc-link-search=native={}", sdk.display());
    println!("cargo:rustc-link-arg=/DELAYLOAD:wpcap.dll");
    println!("cargo:rustc-link-lib=delayimp");
    tauri_build::build()
}
