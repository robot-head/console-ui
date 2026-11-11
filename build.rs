fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var_os("CARGO_FEATURE_SELF_CONTAINED").is_some()
    {
        windows_reactor_setup::as_self_contained();
    }
}
