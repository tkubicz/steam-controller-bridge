fn main() {
    if let Err(error) = virtual_gamepad::helper::run_from_environment() {
        eprintln!("level=error app=sc-virtual-hid-helper error={error:?}");
        std::process::exit(1);
    }
}
