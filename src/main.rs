use std::process::ExitCode;

fn main() -> ExitCode {
    match raz::run() {
        Ok(code) => code,
        Err(err) => raz::cmd::abort(err),
    }
}
