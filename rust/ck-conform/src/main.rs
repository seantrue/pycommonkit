//! `ck-conform <command> [<json> | @<file> | -]`: drive the `commonkit` crate and print JSON.
//! The commands are those of `conformance/ck_conform.py`; see its docstring. `batch` takes a
//! list of `{"cmd": ..., ...}` objects and prints a list of results.

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::io::Read;
use std::process::ExitCode;

use ck_conform::{COMMANDS, emit, run};
use serde_json::Value;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let Some(cmd) = argv.get(1) else {
        eprintln!(
            "usage: ck-conform <{}|batch> [<json>|@<file>|-]",
            COMMANDS.join("|")
        );
        return ExitCode::from(2);
    };
    let raw = match argv.get(2).map(String::as_str) {
        None => "{}".to_owned(),
        Some("-") => {
            let mut s = String::new();
            if std::io::stdin().read_to_string(&mut s).is_err() {
                eprintln!("ck-conform: can't read stdin");
                return ExitCode::from(2);
            }
            s
        }
        Some(a) => match a.strip_prefix('@') {
            Some(path) => match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("ck-conform: {path}: {e}");
                    return ExitCode::from(2);
                }
            },
            None => a.to_owned(),
        },
    };
    let args: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("ck-conform: bad JSON: {e}");
            return ExitCode::from(2);
        }
    };
    let result = if cmd == "batch" {
        let ops = args.as_array().cloned().unwrap_or_default();
        ops.iter()
            .map(|op| {
                let c = op.get("cmd").and_then(Value::as_str).unwrap_or("");
                run(c, op)
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array)
    } else {
        run(cmd, &args)
    };
    match result {
        Ok(v) => {
            emit(&v);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("ck-conform: {e}");
            ExitCode::from(1)
        }
    }
}
