pub mod dfa_plugin;

use std::num::NonZero;

use crate::{
    io::{segment::segmented_reader, walk::ScanElement},
};

use std::path::Path;

use crate::io::segment::Segment;

pub trait EnginePlugin {
    /// user defined state. a plugin can scan whatever it wants here. a new
    /// state is created with each new file being scanned
    type PluginFileScanState;

    /// pseudo_path: a path-like representation for where the file is. if
    /// contained inside an archive, the path element is represented with a '!'
    /// prefix
    /// 
    /// provides the first segment loaded from the file. one time logic like
    /// checking the header bytes should happen here
    /// 
    /// returns the plugin state which is reused as the file is processed
    fn init_state_for_file(&self, pseudo_path: &Path, first_segment: &[u8]) -> Self::PluginFileScanState;

    /// handle each segment provided from the file
    fn handle_segment(&self, state: &mut Self::PluginFileScanState, segment: Segment<'_>);

    /// indicates end of file
    fn done_file(&self, state: &mut Self::PluginFileScanState);
}


pub struct Engine<Plugin: EnginePlugin> {
    segment_size: NonZero<usize>,
    match_overlap: usize,
    match_lookahead: usize,
    match_lookbehind: usize,
    plugin: Plugin,
}

impl<Plugin: EnginePlugin> Engine<Plugin> {
    pub fn new(
        plugin: Plugin,
        segment_size: NonZero<usize>,
        match_overlap: usize,
        match_lookahead: usize,
        match_lookbehind: usize,
    ) -> Self {
        Self {
            segment_size,
            match_overlap,
            match_lookahead,
            match_lookbehind,
            plugin,
        }
    }

    pub fn scan(&self, content: ScanElement) -> Result<Plugin::PluginFileScanState, String> {
        let mut state: Option<Plugin::PluginFileScanState> = None;

        segmented_reader(
            self.segment_size,
            self.match_overlap,
            self.match_lookahead,
            self.match_lookbehind,
            content.entry,
            |segment| {
                if segment.file_offset == 0 {
                    state = Some(self.plugin.init_state_for_file(
                        content.pseudo_path,
                        segment.data,
                    ));
                }
                self.plugin.handle_segment(state.as_mut().unwrap(), segment);
            },
        )
        .map_err(|e| format!("{}: {}", content.pseudo_path.display(), e))?;

        self.plugin.done_file(state.as_mut().unwrap());
        Ok(state.unwrap())
    }
}
