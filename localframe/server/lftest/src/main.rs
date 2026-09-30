//! lftest: the LocalFrame test tool. `serve` is the server the Mac's LFTest
//! application talks to (echo tests and a chat hub, on one node); `ping`,
//! `chat` and `monitor` are PC-side clients and a diagnostic.

mod chat;
mod chat_client;
mod echo;
mod hub;
mod monitor;
mod ping;
mod serve;
mod teletekst;
mod teletekst_client;

use std::io::BufRead;
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver};

use appletalk::node::OPTIONS_HELP;
use appletalk::{Config, Role};

fn usage() -> String {
    format!(
        "usage: lftest serve [options]     echo server, chat hub and Teletekst
                                  (NAME:LFEcho, NAME:RChat, NAME:Teletekst);
                                  lines typed here go to the chat
       lftest ping [options]      find an echo server and run the ping and bulk tests
       lftest chat [options]      join a chat hub from this PC
       lftest teletekst [options] browse Teletekst from this PC, as a Mac does
       lftest monitor [--iface IP]
                                  print every LToUDP frame on the network, decoded

In the chat (serve, chat): /who lists who is connected, /quit stops.

options:
  --name NAME     server name / chat nickname (default: host name)
  -v              log every request (serve)
  --teletekst HOST  ssh server for Teletekst (serve; default teletekst.nl)
{OPTIONS_HELP}"
    )
}

pub struct Options {
    pub cfg: Config,
    /// NBP object name of the services, and the chat nickname; Mac Roman.
    pub name: Vec<u8>,
    pub verbose: bool,
    /// Where `serve` gets Teletekst from, over ssh.
    pub teletekst_host: String,
}

fn parse_args(role: Role, args: &[String]) -> Result<Options, String> {
    let mut name = hostname();
    let mut o =
        Options { cfg: Config::new(role), name: Vec::new(), verbose: false, teletekst_host: "teletekst.nl".into() };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--name" | "--nick" => name = it.next().ok_or("--name needs a value")?.clone(),
            "-v" => o.verbose = true,
            "--teletekst" => o.teletekst_host = it.next().ok_or("--teletekst needs a value")?.clone(),
            "-h" | "--help" => return Err(usage()),
            _ if o.cfg.parse_arg(a, &mut it)? => {}
            _ => return Err(format!("unknown option {a}\n\n{}", usage())),
        }
    }
    o.name = chat::to_mac(&name);
    if o.name.is_empty() || o.name.len() > chat::MAX_NICK {
        return Err(format!("--name must be 1 to {} characters", chat::MAX_NICK));
    }
    Ok(o)
}

/// This computer's name: /etc/hostname on Linux, COMPUTERNAME on Windows.
fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .map(|s| s.trim().chars().take(chat::MAX_NICK).collect::<String>())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "LocalFrame".into())
}

/// Lines typed on the terminal, read on their own thread.
pub fn stdin_lines() -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    let result = match args.first().map(String::as_str) {
        Some("serve") => parse_args(Role::Server, rest).and_then(|o| serve::run(o).map_err(|e| e.to_string())),
        Some("ping") => parse_args(Role::Workstation, rest).and_then(|o| ping::run(o).map_err(|e| e.to_string())),
        Some("chat") => {
            parse_args(Role::Workstation, rest).and_then(|o| chat_client::run(o).map_err(|e| e.to_string()))
        }
        Some("teletekst") => parse_args(Role::Workstation, rest)
            .and_then(|o| teletekst_client::run(o).map_err(|e| e.to_string())),
        Some("monitor") => {
            parse_args(Role::Workstation, rest).and_then(|o| monitor::run(o.cfg.iface).map_err(|e| e.to_string()))
        }
        _ => Err(usage()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
