use std::{num::NonZeroUsize, path::PathBuf};

use clap::Parser;

use window_sweep_lib::{
    self,
    plugins::{
        Engine,
        dfa_plugin::{DFAPlugin, DFAPluginStateTrait},
    },
};
use regex_automata::{
    Anchored, Input,
    dfa::{Automaton, StartKind, dense},
    nfa::thompson::{self, WhichCaptures},
    util::syntax,
};
use serde::Serialize;

pub const DEFAULT_SEGMENT_SIZE: NonZeroUsize = unsafe { NonZeroUsize::new_unchecked(100000) };
pub const DEFAULT_MATCH_OVERLAP: usize = 1000;

#[derive(Parser, Debug)]
#[command(name = "window-sweep")]
#[command(about = "Binary Scanner")]
pub struct Args {
    /// path to scan. files and folders recursively
    pub scan_path: PathBuf,
    /// matching buffer total size
    #[arg(default_value_t = DEFAULT_SEGMENT_SIZE)]
    pub segment_size: NonZeroUsize,
    /// match overlap - set this to a value which equals or exceeds the longest
    /// realistic possible match from the expressions
    #[arg(default_value_t = 100)]
    pub match_overlap: usize,
    /// if not at end of file, minimum number of bytes to provide after a
    /// pattern match
    #[arg(default_value_t = DEFAULT_MATCH_OVERLAP)]
    pub match_lookahead: usize,
    /// if not at beginning of file, minimum number of bytes to provide before
    /// pattern match
    #[arg(default_value_t = 0)]
    pub match_lookbehind: usize,
    /// set of regular expressions for paths to exclude
    #[arg(long)]
    pub exclude: Vec<String>,
    /// set of expressions to match for
    #[arg(long)]
    pub expressions: Vec<String>,
}

fn main() -> Result<(), String> {
    let args = Args::parse();

    #[derive(Default)]
    struct DFAPluginState {
        pseudo_file_path: PathBuf,
    }

    impl DFAPluginStateTrait for DFAPluginState {
        fn init_state_for_file(pseudo_path: &std::path::Path, _first_segment: &[u8]) -> Self {
            Self {
                pseudo_file_path: pseudo_path.to_owned(),
            }
        }
    }

    let plugin = DFAPlugin::<DFAPluginState, _, _>::new(
        |arg| {
            // what happens when a pattern matches in this case just print it out
            #[derive(Serialize)]
            struct Out {
                pseudo_file_path: PathBuf,
                match_start_offset: usize,
                content: String,
                which_pattern: usize,
                lookahead: String,
                lookbehind: String,
            }
            let out = Out {
                pseudo_file_path: arg.file_state.pseudo_file_path.clone(),
                match_start_offset: arg.match_start_offset,
                which_pattern: arg.which_pattern,
                content: String::from_utf8_lossy(&arg.content[arg.match_position.clone()])
                    .to_string(),
                lookbehind: String::from_utf8_lossy(&arg.content[..arg.match_position.start])
                    .to_string(),
                lookahead: String::from_utf8_lossy(&arg.content[arg.match_position.end..])
                    .to_string(),
            };
            println!("{}", serde_json::to_string(&out).unwrap());
        },
        &args.expressions,
    )?;

    let engine = Engine::new(
        plugin,
        args.segment_size,
        args.match_overlap,
        args.match_lookahead,
        args.match_lookbehind,
    );

    let mut builder = regex_automata::dfa::dense::Builder::new();

    builder
        .syntax(
            syntax::Config::new()
                .unicode(false)
                .utf8(false)
                .dot_matches_new_line(true),
        )
        .configure(dense::Config::new().start_kind(StartKind::Anchored))
        .thompson(
            thompson::Config::new()
                .utf8(false)
                .which_captures(WhichCaptures::None),
        );

    let exclude = builder
        .build_many(&args.exclude)
        .map_err(|e| e.to_string())?;

    window_sweep_lib::io::walk::walk(
        &args.scan_path,
        |e| {
            let bytes = e.as_os_str().to_string_lossy();
            let bytes = bytes.as_bytes();
            let input = Input::new(bytes).anchored(Anchored::Yes);
            let mut state = exclude.start_state_forward(&input).unwrap();

            for &b in bytes {
                state = exclude.next_state(state, b);
            }
            exclude.is_match_state(state)
        },
        50,
        |h| match engine.scan(h) {
            Ok(_v) => {}
            Err(e) => {
                eprintln!("{e}");
            }
        },
    );

    Ok(())
}
