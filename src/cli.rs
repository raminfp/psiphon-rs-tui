//! Minimal command-line parsing that mirrors the Go `flag` package's
//! single-dash style used by upstream ConsoleClient, so this binary accepts
//! exactly the invocation this project was built around:
//!
//!   psiphon -config psiphon.config -serverList server-list-standard.txt -dataRootDirectory data
//!
//! Both `-flag value` and `-flag=value` are accepted, `--flag` variants are
//! accepted too (Go's flag package treats leading `-`/`--` the same way).
//!
//! All three flags are also optional: if omitted, they default to
//! `./psiphon.config`, `./server-list-standard.txt`, and `./data` in the
//! current directory (the -config and -serverList defaults only apply if
//! that file actually exists there - otherwise the old, explicit
//! requirement/empty-value behaviour applies), so `psiphon` with no
//! arguments works when run from a directory laid out that way.

pub struct Args {
    pub config: String,
    pub server_list: String,
    pub data_root_directory: String,
}

const DEFAULT_CONFIG: &str = "psiphon.config";
const DEFAULT_SERVER_LIST: &str = "server-list-standard.txt";
const DEFAULT_DATA_ROOT_DIRECTORY: &str = "data";

const USAGE: &str = "\
psiphon-tui — a Rust TUI front-end for psiphon-tunnel-core

USAGE:
    psiphon [-config <path>] [-serverList <path>] [-dataRootDirectory <dir>]

FLAGS:
    -config <path>              configuration input file
                                 (default: ./psiphon.config, if present)
    -serverList <path>          embedded server entry list input file
                                 (default: ./server-list-standard.txt, if present)
    -dataRootDirectory <dir>    directory where persistent files will be stored
                                 (default: ./data)
    -h, -help, --help           print this help and exit
";

pub enum ParseResult {
    Args(Args),
    Help,
}

pub fn parse(argv: impl Iterator<Item = String>) -> Result<ParseResult, String> {
    let mut config: Option<String> = None;
    let mut server_list: Option<String> = None;
    let mut data_root_directory: Option<String> = None;

    let args: Vec<String> = argv.collect();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let stripped = arg.trim_start_matches('-');
        if arg == "-h" || arg == "-help" || arg == "--help" {
            return Ok(ParseResult::Help);
        }
        if !arg.starts_with('-') {
            return Err(format!("unexpected positional argument: {arg}"));
        }

        let (flag, inline_value) = match stripped.split_once('=') {
            Some((f, v)) => (f, Some(v.to_string())),
            None => (stripped, None),
        };

        let mut take_value = || -> Result<String, String> {
            if let Some(v) = &inline_value {
                return Ok(v.clone());
            }
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| format!("flag -{flag} requires a value"))
        };

        match flag {
            "config" => config = Some(take_value()?),
            "serverList" => server_list = Some(take_value()?),
            "dataRootDirectory" => data_root_directory = Some(take_value()?),
            other => return Err(format!("unknown flag: -{other}")),
        }
        i += 1;
    }

    let config = config.or_else(|| default_if_exists(DEFAULT_CONFIG)).ok_or_else(|| {
        format!(
            "-config is required (no {DEFAULT_CONFIG} found in the current directory either)"
        )
    })?;
    let server_list = server_list
        .or_else(|| default_if_exists(DEFAULT_SERVER_LIST))
        .unwrap_or_default();
    let data_root_directory =
        data_root_directory.unwrap_or_else(|| DEFAULT_DATA_ROOT_DIRECTORY.to_string());

    Ok(ParseResult::Args(Args {
        config,
        server_list,
        data_root_directory,
    }))
}

fn default_if_exists(path: &str) -> Option<String> {
    std::path::Path::new(path).is_file().then(|| path.to_string())
}

pub fn usage() -> &'static str {
    USAGE
}
