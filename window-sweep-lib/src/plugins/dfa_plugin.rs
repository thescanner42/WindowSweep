use std::{marker::PhantomData, ops::Range};

use regex_automata::{
    Anchored, Input, MatchKind,
    dfa::{
        Automaton, OverlappingState, StartKind,
        dense::{self},
    },
    nfa::thompson::{self, WhichCaptures},
    util::syntax,
};

use crate::plugins::EnginePlugin;

fn builder() -> regex_automata::dfa::regex::Builder {
    let mut builder = regex_automata::dfa::regex::Builder::new();
    builder
        .syntax(
            syntax::Config::new()
                .unicode(false)
                .utf8(false)
                .dot_matches_new_line(true),
        ) // bytes only
        .dense(
            dense::Config::new()
                .starts_for_each_pattern(true) // required for reverse walk logic
                .match_kind(MatchKind::All) // required for overlapping matches
                .start_kind(StartKind::Unanchored),
        ) // search anywhere in input
        .thompson(
            thompson::Config::new()
                .utf8(false) // bytes only
                .which_captures(WhichCaptures::None), // no captures
        );
    builder
}

pub trait DFAPluginStateTrait {
    fn init_state_for_file(pseudo_path: &std::path::Path, first_segment: &[u8]) -> Self;
}

/// handle the matches
///
/// internally the match engine walks forward and finds the end of the match
/// first. this means that matches are sorted by end position. for same end
/// positions it then walks backwards - for same end positions the start
/// positions are in reverse order. it finds all overlapping matches
pub struct MatchHandlerArg<'a, DFAPluginState: DFAPluginStateTrait> {
    /// user defined state reused in the same file
    pub file_state: &'a mut DFAPluginState,
    /// index of which pattern matched
    pub which_pattern: usize,
    /// the matched pattern and the lookahead and lookbehind and
    /// segment overlap
    pub content: &'a [u8],
    /// the specific slice in content of the match - excludes the lookahead and
    /// lookbehind
    pub match_position: Range<usize>,
    /// the offset of the start of the match in the file. this is not
    /// necessarily the offset of content in the file
    pub match_start_offset: usize,
}

pub struct DFAPlugin<
    DFAPluginState: DFAPluginStateTrait,
    MatchHandler: Fn(MatchHandlerArg<DFAPluginState>),
    Data,
> {
    re: regex_automata::dfa::regex::Regex<regex_automata::dfa::dense::DFA<Data>>,

    /// how should the matches be handled
    handler: MatchHandler,

    _marker: PhantomData<DFAPluginState>,
}

/// serialize the compiled engine patterns - used by deserialize patterns
/// ctor
///
/// patterns should not contain anchoring '^' or '$'
///
/// this is helpful to remove compilation overhead on startup - use this in
/// build.rs
pub fn serialize_patterns<P: AsRef<str>>(patterns: &[P]) -> Result<Vec<u8>, String> {
    let re = builder()
        .build_many(patterns)
        .map_err(|e| format!("Failed to build DFA: {}", e))?;
    let mut ret = re.forward().to_bytes_little_endian().0;
    ret.extend(re.reverse().to_bytes_little_endian().0);
    Ok(ret)
}

impl<DFAPluginState: DFAPluginStateTrait, MatchHandler: Fn(MatchHandlerArg<DFAPluginState>)>
    DFAPlugin<DFAPluginState, MatchHandler, Vec<u32>>
{
    /// create from patterns. patterns should not contain anchoring '^' or '$'
    ///
    /// segment_size should be some large value (must be larger than overlap)
    ///
    /// match_overlap should be greater or equal to the length of the realistic longest
    /// pattern
    pub fn new<P: AsRef<str>>(
        match_handler: MatchHandler,
        patterns: &[P],
    ) -> Result<Self, String> {
        let re = builder()
            .build_many(patterns)
            .map_err(|e| format!("Failed to build DFA: {}", e))?;

        Ok(Self {
            re,
            handler: match_handler,
            _marker: PhantomData,
        })
    }
}

impl<'a, DFAPluginState: DFAPluginStateTrait, MatchHandler: Fn(MatchHandlerArg<DFAPluginState>)>
    DFAPlugin<DFAPluginState, MatchHandler, &'a [u32]>
{
    /// create from patterns. patterns should not contain anchoring '^' or '$'
    ///
    /// segment_size should be some large value (must be larger than overlap)
    ///
    /// match_overlap should be greater or equal to the length of the realistic longest
    /// pattern
    pub fn deserialize_patterns(
        match_handler: MatchHandler,
        bytes: &'a [u8],
    ) -> Result<Self, String> {
        let fwd = regex_automata::dfa::dense::DFA::from_bytes(bytes).map_err(|e| e.to_string())?;
        let rev = regex_automata::dfa::dense::DFA::from_bytes(&bytes[fwd.1..])
            .map_err(|e| e.to_string())?;
        let re = builder().build_from_dfas(fwd.0, rev.0);

        Ok(Self {
            re,
            handler: match_handler,
            _marker: PhantomData,
        })
    }
}

impl<
    DFAPluginState: DFAPluginStateTrait,
    MatchHandler: Fn(MatchHandlerArg<DFAPluginState>),
    Data: AsRef<[u32]>,
> EnginePlugin for DFAPlugin<DFAPluginState, MatchHandler, Data>
{
    type PluginFileScanState = DFAPluginState;

    fn init_state_for_file(
        &self,
        pseudo_path: &std::path::Path,
        first_segment: &[u8],
    ) -> Self::PluginFileScanState {
        DFAPluginState::init_state_for_file(pseudo_path, first_segment)
    }

    fn handle_segment(
        &self,
        state: &mut Self::PluginFileScanState,
        segment: crate::io::segment::Segment<'_>,
    ) {
        // requires custom handling as high level API does not expose
        // find_iter_overlapping
        //
        // modified FROM:
        // https://github.com/rust-lang/regex/issues/822#issuecomment-1210677794
        //
        // has worst case QUADRATIC time complexity - avoid patterns
        // with unbounded match length (not even possible with segments)

        // TODO: currently the fwd state is tossed out between segments.
        // dfa forward state could be retained instead of recalculated.
        // this would be pretty hard for me to implement, as it would
        // require implementing the try_search_overlapping_fwd and not
        // messing anything up
        //
        // current: drop state from previous segment. begin matching
        // again at beginning of segment (including the match overlap if
        // this is not the first segment)
        //
        // improvement: retain previous fwd state. on the forward pass,
        // begin matching not at the segment scan range start, but after
        // the match_overlap (which is self.match_overlap if not first
        // segment). the reverse would still traverse into the overlap
        //
        // workaround: use a sufficiently large segment size so this
        // extra computation is negligible. and, duplicate matches are
        // rejected via is_duplicate

        let mut fwd_state = OverlappingState::start();
        let (fwd_dfa, rev_dfa) = (self.re.forward(), self.re.reverse());

        let input = Input::new(&segment.data[segment.scan_range.clone()]).anchored(Anchored::No);
        loop {
            fwd_dfa
                .try_search_overlapping_fwd(&input, &mut fwd_state)
                .map_err(|e| format!("DFA or input misconfigured: {}", e))
                .unwrap();
            let forward_match_end = match fwd_state.get_match() {
                Some(v) => v,
                None => break,
            };

            let reverse_input = input
                .clone()
                .anchored(Anchored::Pattern(forward_match_end.pattern()))
                .range(input.start()..forward_match_end.offset());

            let mut rev_state = OverlappingState::start();
            rev_dfa
                .try_search_overlapping_rev(&reverse_input, &mut rev_state)
                .map_err(|e| format!("DFA or input misconfigured: {}", e))
                .unwrap();

            if let Some(reverse_match_start) = rev_state.get_match() {
                let start = reverse_match_start.offset() + segment.scan_range.start;
                let end = forward_match_end.offset() + segment.scan_range.start;

                // reject duplicate match fully contained in match overlap
                if segment.is_duplicate(end) {
                    continue;
                }

                (self.handler)(MatchHandlerArg {
                    file_state: state,
                    which_pattern: forward_match_end.pattern().as_usize(),
                    match_start_offset: segment.file_offset as usize + start,
                    content: segment.data,
                    match_position: Range { start, end },
                })
            }
        }
    }
    
    fn done_file(&self, _state: &mut Self::PluginFileScanState) {}
}

/*
#[cfg(test)]
mod tests {
    use std::{io::Cursor, path::PathBuf};

    use super::*;

    #[derive(Default)]
    struct TestState {
        matches: Vec<(usize, usize, Vec<u8>)>,
    }

    struct TestConfig;

    impl EngineScanFileState<TestConfig> for TestState {
        fn create(_engine: &TestConfig) -> Self {
            Default::default()
        }
    }

    impl EngineConfiguration<TestState> for TestConfig {
        fn match_handler(
            state: &mut TestState,
            pattern: usize,
            offset: usize,
            content: &[u8],
            m: Range<usize>,
        ) {
            state.matches.push((pattern, offset, content[m].to_vec()));
        }

        fn start_file_state(_state: &mut TestState, _pseudo_file_path: &Path, _data: &[u8]) {}
    }

    #[test]
    fn test_overlapping() {
        let config = TestConfig {};
        let engine =
            Engine::new(config, &["aaa", "aaab"], 100.try_into().unwrap(), 0, 0, 0).unwrap();
        let file_content = b"aaabaaa";
        let state = engine
            .scan(ScanElement {
                pseudo_path: &PathBuf::from(""),
                entry: &mut Cursor::new(file_content),
            })
            .unwrap();
        let expected = vec![
            (0, 0, b"aaa".to_vec()),
            (1, 0, b"aaab".to_vec()),
            (0, 4, b"aaa".to_vec()),
        ];
        assert_eq!(state.matches, expected);
    }

    #[test]
    fn test_header() {
        struct HeaderTestConfig;
        impl EngineScanFileState<HeaderTestConfig> for TestState {
            fn create(_engine: &HeaderTestConfig) -> Self {
                Default::default()
            }
        }

        impl EngineConfiguration<TestState> for HeaderTestConfig {
            fn match_handler(
                state: &mut TestState,
                pattern: usize,
                offset: usize,
                content: &[u8],
                m: Range<usize>,
            ) {
                state.matches.push((pattern, offset, content[m].to_vec()));
            }

            fn start_file_state(_state: &mut TestState, _pseudo_file_path: &Path, data: &[u8]) {
                assert!(data.starts_with(b"HEADER"));
            }
        }

        let config = TestConfig {};
        let engine = Engine::new::<&str>(config, &[], 100.try_into().unwrap(), 0, 0, 0).unwrap();
        let file_content = b"HEADER abc 123";
        let state = engine
            .scan(ScanElement {
                pseudo_path: &PathBuf::from(""),
                entry: &mut Cursor::new(file_content),
            })
            .unwrap();
        assert_eq!(state.matches, vec![]);
    }

    // not possible with dfa and not desired

    // #[test]
    // fn test_longest() {
    //     let engine =
    //         Engine::<TestState, TestConfig, _>::new(&["a*"], 100.try_into().unwrap(), 0).unwrap();
    //     let file_content = b"aaaa";
    //     let state = engine
    //         .scan(ScanElement {
    //             pseudo_path: &PathBuf::from(""),
    //             entry: &mut Cursor::new(file_content),
    //         })
    //         .unwrap();
    //     let expected = vec![(0, 0, b"aaaa".to_vec())];
    //     assert_eq!(state.matches, expected);
    // }

    #[test]
    fn sanity_check_reverse_direction() {
        let config = TestConfig {};
        let engine = Engine::new(config, &["ab"], 100.try_into().unwrap(), 0, 0, 0).unwrap();
        let file_content = b"abab";
        let state = engine
            .scan(ScanElement {
                pseudo_path: &PathBuf::from(""),
                entry: &mut Cursor::new(file_content),
            })
            .unwrap();
        let expected = vec![
            (0, 0, b"ab".to_vec()),
            (0, 2, b"ab".to_vec()),
            // erroneous logic would add another match going backwards. it
            // correctly stops since reverse direction is anchored
        ];
        assert_eq!(state.matches, expected);
    }

    #[test]
    fn test_incorrect_reverse_anchoring_cross_pattern_suffix() {
        let config = TestConfig {};
        let engine =
            Engine::new(config, &["foobar", "bar"], 100.try_into().unwrap(), 0, 0, 0).unwrap();

        let input = b"foobar";
        let state = engine
            .scan(ScanElement {
                pseudo_path: &PathBuf::from(""),
                entry: &mut Cursor::new(input),
            })
            .unwrap();
        // if reverse anchoring were not specific to pattern, then 4 matches
        // would incorrectly appear here
        let expected = vec![(0, 0, b"foobar".to_vec()), (1, 3, b"bar".to_vec())];
        assert_eq!(state.matches, expected);
    }

    #[test]
    fn test_serialize() {
        let abc = serialize_patterns(&["foobar", "bar"]).unwrap();
        let config = TestConfig {};
        let engine =
            Engine::deserialize_patterns(config, &abc, 100.try_into().unwrap(), 0, 0, 0).unwrap();
        let input = b"foobar";
        let state = engine
            .scan(ScanElement {
                pseudo_path: &PathBuf::from(""),
                entry: &mut Cursor::new(input),
            })
            .unwrap();
        // if reverse anchoring were not specific to pattern, then 4 matches
        // would incorrectly appear here
        let expected = vec![(0, 0, b"foobar".to_vec()), (1, 3, b"bar".to_vec())];
        assert_eq!(state.matches, expected);
    }

    #[test]
    fn test_overlap_no_dup() {
        let config = TestConfig {};
        let engine = Engine::new(config, &["is"], 4.try_into().unwrap(), 2, 0, 0).unwrap();

        let input = b"this test";
        let state = engine
            .scan(ScanElement {
                pseudo_path: &PathBuf::from(""),
                entry: &mut Cursor::new(input),
            })
            .unwrap();
        let expected = vec![(0, 2, b"is".to_vec())];
        assert_eq!(state.matches, expected);
    }

    #[test]
    fn test_scan_range_offset() {
        let config = TestConfig {};
        let engine = Engine::new(config, &["is"], 4.try_into().unwrap(), 1, 0, 3).unwrap();
        let input = b"1234is test";
        let state = engine
            .scan(ScanElement {
                pseudo_path: &PathBuf::from(""),
                entry: &mut Cursor::new(input),
            })
            .unwrap();
        let expected = vec![(0, 4, b"is".to_vec())];
        assert_eq!(state.matches, expected);
    }
}
*/
