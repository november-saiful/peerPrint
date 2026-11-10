fn main() {
    println!("cargo:rerun-if-changed=assets/peerprint.ico");
    println!("cargo:rerun-if-changed=assets/peerprint.manifest");
    println!("cargo:rerun-if-changed=assets/peerprint.rc");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let _ = embed_resource::compile("assets/peerprint.rc", embed_resource::NONE);
    }
}
