use std::io::Write as _;
use std::path::Path;
use std::process::Command;

const MODE_ENV: &str = "SC_BRIDGE_TEST_CHILD_MODE";
const PAYLOAD_ENV: &str = "SC_BRIDGE_TEST_CHILD_PAYLOAD";
const PATH_ENV: &str = "SC_BRIDGE_TEST_CHILD_PATH";

pub fn request_and_wait(payload: &str) -> Command {
    with_payload("request-and-wait", payload)
}

pub fn request(payload: &str) -> Command {
    with_payload("request", payload)
}

pub fn wait_for_line() -> Command {
    command("wait-for-line")
}

pub fn mark_on_close(path: &Path) -> Command {
    let mut command = command("mark-on-close");
    command.env(PATH_ENV, path);
    command
}

pub fn read_to_end() -> Command {
    command("read-to-end")
}

pub fn diagnostic(line: &str) -> Command {
    with_payload("diagnostic", line)
}

fn with_payload(mode: &str, payload: &str) -> Command {
    let mut command = command(mode);
    command.env(PAYLOAD_ENV, payload);
    command
}

fn command(mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("locate test executable"));
    command
        .args([
            "--exact",
            "test_child::run",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(MODE_ENV, mode);
    command
}

#[test]
#[ignore = "invoked only as a subprocess fixture"]
fn run() {
    match std::env::var(MODE_ENV).as_deref() {
        Ok("request-and-wait") => {
            write_payload(false);
            read_line();
        }
        Ok("request") => write_payload(false),
        Ok("wait-for-line") => {
            read_line();
        }
        Ok("mark-on-close") => {
            let line = read_line();
            if line.contains("\"close\"") {
                std::fs::File::create(std::env::var_os(PATH_ENV).expect("test child path"))
                    .expect("create test marker");
            }
        }
        Ok("read-to-end") => {
            std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink())
                .expect("read test input");
        }
        Ok("diagnostic") => write_payload(true),
        mode => panic!("unsupported test child mode: {mode:?}"),
    }
}

fn write_payload(newline: bool) {
    let payload = std::env::var(PAYLOAD_ENV).expect("test child payload");
    let mut stderr = std::io::stderr().lock();
    stderr
        .write_all(payload.as_bytes())
        .expect("write test child payload");
    if newline {
        stderr.write_all(b"\n").expect("write test child newline");
    }
    stderr.flush().expect("flush test child payload");
}

fn read_line() -> String {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .expect("read test child line");
    line
}
