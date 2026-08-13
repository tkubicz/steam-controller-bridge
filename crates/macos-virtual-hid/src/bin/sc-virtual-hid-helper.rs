fn main() {
    if let Err(error) = macos_virtual_hid::helper::run_from_environment() {
        eprintln!("level=error app=sc-virtual-hid-helper error={error:?}");
        std::process::exit(1);
    }
}
