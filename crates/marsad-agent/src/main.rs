//! `marsad-agent`: see the crate docs of the library.

use std::ffi::OsString;
use std::process::ExitCode;

use marsad_agent::{cli::Options, serve, version};

fn usage() -> ExitCode {
    eprintln!(
        "usage: marsad-agent serve [--hub] [options] | bridge [options] | stop [options] | shim <args> | mcp --socket <path> --session <id> | --version"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    // ssh runs `$SSH_ASKPASS "<prompt>"`: this binary, recognised by the environment the hub
    // sets for its links.
    if std::env::var_os(marsad_agent::hub::provision::ASKPASS_HOST_ENV).is_some() {
        let prompt: Vec<String> = std::env::args().skip(1).collect();
        let prompt = match prompt.first().map(String::as_str) {
            Some("askpass") => prompt[1..].join(" "),
            _ => prompt.join(" "),
        };
        return match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt.block_on(serve::askpass(prompt)),
            Err(e) => {
                eprintln!("marsad-agent askpass: {e}");
                ExitCode::from(1)
            }
        };
    }
    let mut args = std::env::args_os().skip(1);
    let Some(cmd) = args.next() else {
        return usage();
    };
    if cmd == "shim" {
        // First: the shim builds its own runtime, and its guard re-exec comes back through
        // here with the same argv.
        let rest: Vec<OsString> = args.collect();
        let code = wasit_shim::run(rest);
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    match cmd.to_str() {
        Some("--version") | Some("-V") | Some("version") => {
            match serde_json::to_string(&version::info()) {
                Ok(l) => {
                    println!("{l}");
                    return ExitCode::SUCCESS;
                }
                Err(e) => {
                    eprintln!("marsad-agent: {e}");
                    return ExitCode::from(1);
                }
            }
        }
        Some("mcp") => {
            // The Wasl MCP server an agent CLI launches (`--socket <daemon socket> --session
            // <id>`, injected into every session by the hub); it builds its own runtime.
            let rest: Result<Vec<String>, _> = args.map(|a| a.into_string()).collect();
            return match rest {
                Ok(rest) => wasl_mcp::run(&rest),
                Err(bad) => {
                    eprintln!("marsad-agent mcp: argument is not UTF-8: {bad:?}");
                    ExitCode::from(2)
                }
            };
        }
        Some("serve" | "bridge" | "stop") => {}
        _ => return usage(),
    }
    let rest: Vec<String> = match args.map(|a| a.into_string()).collect() {
        Ok(v) => v,
        Err(bad) => {
            eprintln!("marsad-agent: argument is not UTF-8: {bad:?}");
            return ExitCode::from(2);
        }
    };
    let opts = match Options::parse(&rest) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("marsad-agent: {e}");
            return usage();
        }
    };
    // Only fails if a subscriber is already set.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("MARSAD_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("marsad-agent: cannot start the runtime: {e}");
            return ExitCode::from(1);
        }
    };
    match cmd.to_str() {
        Some("serve") => rt.block_on(serve::serve(opts)),
        Some("bridge") => rt.block_on(serve::bridge(opts)),
        Some("stop") => rt.block_on(serve::stop(opts)),
        _ => usage(),
    }
}
