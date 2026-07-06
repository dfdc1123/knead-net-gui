fn main() {
    println!("cargo:rustc-check-cfg=cfg(profile_sa)");
    println!("cargo:rustc-check-cfg=cfg(profile_cost)");
}
