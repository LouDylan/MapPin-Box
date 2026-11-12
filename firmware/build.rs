fn main() {
    println!("cargo::rustc-check-cfg=cfg(esp32, esp32s3)");
    embuild::espidf::sysenv::output();
}
