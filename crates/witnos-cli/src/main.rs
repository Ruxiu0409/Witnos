//! `witnos` — the headless CLI. One bin serves both hooks (Stop = gate,
//! PostToolUse = delivery), the arm/disarm protocol, and (coming next) the
//! agent-facing contract subcommands.

mod client;
mod cmds;
mod hook_post;
mod hook_session_end;
mod hook_stop;
mod hook_ups;
mod init;
mod paths;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("hook") => match args.get(2).map(String::as_str) {
            Some("stop") => hook_stop::run(),
            Some("post-tool-use") => hook_post::run(),
            Some("user-prompt-submit") => hook_ups::run(),
            Some("session-end") => hook_session_end::run(),
            _ => usage(),
        },
        Some("init") => init::run(),
        Some("goal") => match args.get(2).map(String::as_str) {
            Some("new") => cmds::goal_new(&args[3..]),
            _ => usage(),
        },
        Some("contract") => match args.get(2).map(String::as_str) {
            Some("show") => cmds::contract_show(&args[3..]),
            _ => usage(),
        },
        Some("item") => match args.get(2).map(String::as_str) {
            Some("lay") => cmds::item_lay(&args[3..]),
            Some("interpret") => cmds::item_interpret(&args[3..]),
            _ => usage(),
        },
        Some("evidence") => match args.get(2).map(String::as_str) {
            Some("add") => cmds::evidence_add(&args[3..]),
            _ => usage(),
        },
        Some("oracle") => match args.get(2).map(String::as_str) {
            Some("report") => cmds::oracle_report(&args[3..]),
            _ => usage(),
        },
        Some("reconcile") => cmds::reconcile(&args[2..]),
        Some("arm") => cmd_arm(&args[2..]),
        Some("disarm") => cmd_disarm(),
        Some("status") => cmd_status(),
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "witnos — the verification layer's headless CLI\n\
         \n\
         setup:\n\
         \x20 witnos init                 install the four hooks into ./.claude/settings.json\n\
         \x20 witnos goal new <title…> [--no-watch]   create a goal and watch this project\n\
         \n\
         hooks:\n\
         \x20 witnos hook stop                Stop gate (fails CLOSED while armed)\n\
         \x20 witnos hook post-tool-use       delivery channel (fails OPEN)\n\
         \x20 witnos hook user-prompt-submit  bind session + inject the protocol once\n\
         \x20 witnos hook session-end         account a still-running goal as turn-ended\n\
         \n\
         agent-facing (all accept --goal <id>; required when several goals\n\
         are active — your id is in your [witnos] protocol message):\n\
         \x20 witnos contract show [--since N]      current contract (delta from N)\n\
         \x20 witnos item lay [--blindspot]         lay items; JSON array on stdin\n\
         \x20 witnos item interpret <id> <text…>    record your interpretation\n\
         \x20 witnos evidence add <id>              attach evidence; JSON on stdin\n\
         \x20 witnos oracle report <id> --passed|--failed\n\
         \x20 witnos reconcile --to <version> [--reinterpreted id,…]\n\
         \n\
         human-facing:\n\
         \x20 witnos arm <goal-id> [--version N]    write the armed marker here\n\
         \x20 witnos disarm                         remove the armed marker (escape hatch)\n\
         \x20 witnos status                         show marker + endpoint state"
    );
    // Exit 64 (EX_USAGE). Never 2: exit code 2 is a live signal to the hook
    // runner and must stay reserved for deliberate use.
    ExitCode::from(64)
}

fn cmd_arm(rest: &[String]) -> ExitCode {
    let Some(goal_id) = rest.first() else {
        eprintln!("usage: witnos arm <goal-id> [--version N]");
        return ExitCode::from(64);
    };
    let version = rest
        .iter()
        .position(|a| a == "--version")
        .and_then(|i| rest.get(i + 1))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let marker = paths::ArmedMarker {
        v: 2,
        auto: false,
        default_goal: Some(paths::GoalRef {
            goal_id: goal_id.clone(),
            contract_version: version,
            agent_synced_version: 0,
        }),
        sessions: Default::default(),
    };
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cannot determine current directory: {e}");
            return ExitCode::FAILURE;
        }
    };
    let dir = cwd.join(".witnos");
    if let Err(e) =
        std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join("armed.json"), marker.to_pretty()))
    {
        eprintln!("cannot write armed marker: {e}");
        return ExitCode::FAILURE;
    }
    println!(
        "armed: goal {goal_id} (contract v{version}) — the Stop gate now fails closed in this project"
    );
    ExitCode::SUCCESS
}

fn cmd_disarm() -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    match paths::find_marker(&cwd) {
        Some((root, marker_path)) => {
            let was_auto = paths::read_marker(&marker_path).is_some_and(|m| m.auto);
            match std::fs::remove_file(&marker_path) {
                Ok(()) => {
                    println!("disarmed: {} is no longer watched", root.display());
                    if was_auto {
                        println!(
                            "note: this project is registered for auto-watch — the marker \
                             returns when the Witnos app restarts; remove the project in \
                             the app to stop for good"
                        );
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("cannot remove {}: {e}", marker_path.display());
                    ExitCode::FAILURE
                }
            }
        }
        None => {
            println!("nothing to disarm: no armed marker found from {}", cwd.display());
            ExitCode::SUCCESS
        }
    }
}

fn cmd_status() -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    match paths::find_marker(&cwd) {
        Some((root, marker_path)) => match paths::read_marker(&marker_path) {
            Some(m) => {
                println!(
                    "armed{} at {}",
                    if m.auto { " (auto mode)" } else { "" },
                    root.display()
                );
                if let Some(d) = &m.default_goal {
                    println!("  default goal {} (contract v{})", d.goal_id, d.contract_version);
                }
                for (sid, entry) in &m.sessions {
                    let sid8: String = sid.chars().take(8).collect();
                    println!(
                        "  session {sid8}… → goal {} (contract v{}, synced v{})",
                        entry.goal_id, entry.contract_version, entry.agent_synced_version
                    );
                }
                if m.default_goal.is_none() && m.sessions.is_empty() {
                    println!("  no goals yet — the next agent prompt creates one");
                }
            }
            None => println!(
                "armed (marker unreadable — gate still fails closed) at {}",
                root.display()
            ),
        },
        None => println!("not armed: this project is not being watched"),
    }
    match paths::read_endpoint() {
        Ok(ep) => println!("core endpoint: 127.0.0.1:{}", ep.port),
        Err(e) => println!("core endpoint: unavailable ({e})"),
    }
    ExitCode::SUCCESS
}
