fn main() {
    println!("cargo:rerun-if-changed=patches/0001-fix-wifi-Expose-Rx-pkt-timstamp-related-calculations.patch");
    embuild::espidf::sysenv::output();
}
