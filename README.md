# WindowSweep

WindowSweep is a light-weight framework for scanning files and archives.

## Motivation

Suppose a large input is being processed. Fundamentally there are two steps:

- detection: the part that is relevant is identified
- postprocessing. usually a contiguous span of bytes must be provided, which may
  or may not include some number of bytes before or after that matched span

To fullfill those functional requirements, other scanning tools like YARA load
the whole content into memory before scanning. WindowSweep does this differently;
  it works in configurable constant memory and scans with a sliding window. The
  sliding window has the following configurations:

- buffer size: how many bytes does it hold onto at a time
- lookbehind and lookahead: some contiguous number of bytes around the match
must be available
- match overlap: to prevent matches being missed when straddling a segment
    boundary, some bytes are rescanned

## Plugins

A plugin has a [generic interface](./window-sweep-lib/src/plugins/mod.rs)
which describes a state which persists as a file is being scanned and how input
is accepted. It requires that the underlying scanning technology chosen must
have a mode of operation which accepts chunks of data.

This crate provides a [dfa plugin](./window-sweep-lib/src/plugins/dfa_plugin.rs)
which uses the [rust regex dense
DFA](https://docs.rs/regex-automata/latest/regex_automata/dfa/dense/struct.DFA.html).
This allows for scanning with simultaneous regular expressions. The top level
binary of this repo is a CLI wrapper of this plugin.

Other plugins can be created by you. If sub-linear time complexity is desired
then consider using [daachorse](https://docs.rs/daachorse/latest/daachorse/). If
linear time constant space SAST scanning is desired, consider
[LexerSearch](https://github.com/thescanner42/LexerSearch).
