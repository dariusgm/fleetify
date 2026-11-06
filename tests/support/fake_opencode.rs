//! A stand-in for `opencode`, used by tests/judge_fake.rs. It is a Rust program and not a shell
//! script, because Windows cannot start a `.sh` file and the tests also run there.
//!
//! It reads `fake-opencode.conf` from the folder it is in: line 1 is the log file, line 2 is the
//! answer it prints. It writes what it was given (working directory, arguments, environment and
//! the input file) to the log, then prints the answer.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let exe = env::current_exe().expect("the fake knows its own path");
    let conf = fs::read_to_string(exe.with_file_name("fake-opencode.conf"))
        .expect("the test writes the config next to the fake");
    let mut lines = conf.lines();
    let log = PathBuf::from(lines.next().unwrap_or_default());
    let answer = lines.next().unwrap_or_default().to_string();

    let mut record = String::new();
    let cwd = env::current_dir().map(|p| p.display().to_string());
    record.push_str(&format!("cwd={}\n", cwd.unwrap_or_default()));
    for arg in env::args_os().skip(1) {
        record.push_str(&format!("arg={}\n", arg.to_string_lossy()));
    }
    let mut vars: Vec<String> = env::vars_os()
        .map(|(k, v)| format!("{}={}", k.to_string_lossy(), v.to_string_lossy()))
        .collect();
    vars.sort();
    for var in vars {
        record.push_str(&format!("env: {var}\n"));
    }

    match fs::read_to_string("input.md") {
        Ok(input) => {
            record.push_str(&input);
            fs::write(&log, record).expect("the fake writes its log");
            println!("{answer}");
            ExitCode::SUCCESS
        }
        Err(_) => {
            fs::write(&log, record).expect("the fake writes its log");
            eprintln!("no input");
            ExitCode::from(2)
        }
    }
}
