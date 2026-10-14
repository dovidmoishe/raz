use std::process::ExitCode;

fn main() -> ExitCode {
    match pocket_code::run() {
        Ok(code) => code,
        Err(err) => pocket_code::cmd::abort(err),
    }
}
