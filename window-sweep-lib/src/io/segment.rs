use std::{io::Read, num::NonZero, ops::Range};

#[derive(Clone)]
pub struct Segment<'a> {
    /// offset of beginning of slice within file
    pub file_offset: usize,

    /// the available data including lookarounds and overlap
    pub data: &'a [u8],

    /// range in data that should be scanned for match patterns. this includes
    /// the match_overlap
    pub scan_range: Range<usize>,

    /// the first n bytes in the scan_range overlap with the previous segment
    pub match_overlap: usize,
}

impl<'a> Segment<'a> {
    pub fn is_duplicate(&self, match_end: usize) -> bool {
        if self.file_offset == 0 {
            false
        } else {
            match_end <= self.match_overlap
        }
    }
}

/// segment_size: the total size of the buffer being used for matching. should
/// be some arbitrarily large number
///
/// match_overlap: since rust regex does not support partial matching, some
/// number of bytes at the end of the match region must be retried as part of
/// the beginning of the next segment. match_overlap should equal or exceed the
/// length of the longest realistic possible match. this logic may duplicate
/// matches - immediately reject matches based on Segment::is_duplicate
///
/// match_lookahead: except for the last segment, each possible match will have
/// at least this many bytes ahead of it for further processing after a complete
/// match
///
/// match_lookbehind: except for the first segment (which will have no
/// lookbehind), each possible non-overlap-duplicated match will have at least
/// this many bytes behind it for further processing after a complete match
pub fn segmented_reader<R: Read, H: FnMut(Segment<'_>)>(
    segment_size: NonZero<usize>,
    match_overlap: usize,
    match_lookahead: usize,
    match_lookbehind: usize,
    mut i: R,
    mut o: H,
) -> Result<(), String> {
    let segment_size = segment_size.get();
    let behind_amount = match_overlap.max(match_lookbehind);
    if segment_size <= behind_amount + match_lookahead {
        return Err(
            "invalid cfg: segment size can't accommodate lookarounds and overlap".to_owned(),
        );
    }

    let mut buffer = vec![0u8; segment_size].into_boxed_slice();
    let mut filled = 0usize;
    let mut file_offset: usize = 0;

    loop {
        while filled < segment_size {
            let n = i.read(&mut buffer[filled..]).map_err(|e| format!("read error: {e}"))?;
            if n == 0 {
                break;
            }
            filled += n;
        }

        let is_last = filled < segment_size;
        let is_first = file_offset == 0;

        let scan_range_start = if is_first {
            0
        } else {
            if match_lookbehind > match_overlap {
                match_lookbehind - match_overlap
            } else {
                // segments are rejected if entirely in the overlap anyway
                0
            }
        };

        let scan_range_end = if is_last {
            filled
        } else {
            filled - match_lookahead
        };

        let scan_range = Range {
            start: scan_range_start,
            end: scan_range_end,
        };

        let segment = Segment {
            file_offset,
            data: &buffer[..filled],
            scan_range,
            match_overlap: if is_first { 0 } else { match_overlap },
        };

        o(segment);

        if is_last {
            break;
        }

        let cp_begin = filled - match_lookahead - behind_amount;
        buffer.copy_within(cp_begin.., 0);
        file_offset += cp_begin;
        filled = match_lookahead + behind_amount;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZero;

    #[derive(Default, Debug, PartialEq, Eq)]
    struct OwnedSegment {
        file_offset: usize,
        data: Vec<u8>,
        scan_range: Range<usize>,
        match_overlap: usize,
    }

    fn collect_segments(
        input: &[u8],
        segment_size: usize,
        match_overlap: usize,
        match_lookahead: usize,
        match_lookbehind: usize,
    ) -> Vec<OwnedSegment> {
        let mut out = Vec::new();

        segmented_reader(
            NonZero::new(segment_size).unwrap(),
            match_overlap,
            match_lookahead,
            match_lookbehind,
            input,
            |seg| {
                out.push(OwnedSegment {
                    file_offset: seg.file_offset,
                    data: seg.data.to_vec(),
                    scan_range: seg.scan_range,
                    match_overlap: seg.match_overlap,
                });
            },
        )
        .unwrap();

        out
    }

    #[test]
    fn zero() {
        let input = b"";
        let segments = collect_segments(input, 1000, 2, 2, 2);
        assert_eq!(segments, vec![Default::default()]);
    }

    #[test]
    fn one() {
        let input = b"1234567812345678";
        let segments = collect_segments(input, 1000, 2, 2, 2);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].file_offset, 0);
        assert_eq!(segments[0].data, input);
        assert_eq!(segments[0].match_overlap, 0);
        assert_eq!(segments[0].scan_range, 0..16);
    }

    #[test]
    fn overlap_only() {
        let input = b"1234567812345678";
        let segments = collect_segments(input, 8, 2, 0, 0);
        assert_eq!(segments.len(), 3);

        assert_eq!(segments[0].file_offset, 0);
        assert_eq!(segments[0].data, b"12345678");
        assert_eq!(segments[0].match_overlap, 0);
        assert_eq!(segments[0].scan_range, 0..8);

        assert_eq!(segments[1].file_offset, 6);
        assert_eq!(segments[1].data, b"78123456");
        assert_eq!(segments[1].match_overlap, 2);
        assert_eq!(segments[1].scan_range, 0..8);

        assert_eq!(segments[2].file_offset, 12);
        assert_eq!(segments[2].data, b"5678");
        assert_eq!(segments[2].scan_range, 0..4);
    }

    #[test]
    fn lookahead_only() {
        let input = b"1234567812345678";
        let segments = collect_segments(input, 8, 0, 2, 0);
        assert_eq!(segments.len(), 3);

        assert_eq!(segments[0].file_offset, 0);
        assert_eq!(segments[0].data, b"12345678");
        assert_eq!(segments[0].scan_range, 0..6);

        assert_eq!(segments[1].file_offset, 6);
        assert_eq!(segments[1].data, b"78123456");
        assert_eq!(segments[1].scan_range, 0..6);

        assert_eq!(segments[2].file_offset, 12);
        assert_eq!(segments[2].data, b"5678");
        assert_eq!(segments[2].scan_range, 0..4);
    }

    #[test]
    fn lookbehind_only() {
        let input = b"1234567812345678";
        let segments = collect_segments(input, 8, 0, 0, 2);
        assert_eq!(segments.len(), 3);

        assert_eq!(segments[0].file_offset, 0);
        assert_eq!(segments[0].data, b"12345678");
        assert_eq!(segments[0].scan_range, 0..8);

        assert_eq!(segments[1].file_offset, 6);
        assert_eq!(segments[1].data, b"78123456");
        assert_eq!(segments[1].scan_range, 2..8);

        assert_eq!(segments[2].file_offset, 12);
        assert_eq!(segments[2].data, b"5678");
        assert_eq!(segments[2].scan_range, 2..4);
    }

    #[test]
    fn overlap_lookbehind() {
        let input = b"1234567812345678";
        let segments = collect_segments(input, 8, 2, 0, 3);
        assert_eq!(segments.len(), 3);

        assert_eq!(segments[0].file_offset, 0);
        assert_eq!(segments[0].data, b"12345678");
        assert_eq!(segments[0].scan_range, 0..8);

        assert_eq!(segments[1].file_offset, 5);
        assert_eq!(segments[1].data, b"67812345");
        assert_eq!(segments[1].scan_range, 1..8);

        assert_eq!(segments[2].file_offset, 10);
        assert_eq!(segments[2].data, b"345678");
        assert_eq!(segments[2].scan_range, 1..6);
    }

    #[test]
    fn overlap_lookahead() {
        let input = b"1234567812345678";
        let segments = collect_segments(input, 8, 2, 2, 0);
        assert_eq!(segments.len(), 4);

        assert_eq!(segments[0].file_offset, 0);
        assert_eq!(segments[0].data, b"12345678");
        assert_eq!(segments[0].scan_range, 0..6);

        assert_eq!(segments[1].file_offset, 4);
        assert_eq!(segments[1].data, b"56781234");
        assert_eq!(segments[1].scan_range, 0..6);

        assert_eq!(segments[2].file_offset, 8);
        assert_eq!(segments[2].data, b"12345678");
        assert_eq!(segments[2].scan_range, 0..6);

        assert_eq!(segments[3].file_offset, 12);
        assert_eq!(segments[3].data, b"5678");
        assert_eq!(segments[3].scan_range, 0..4);
    }

    #[test]
    fn all() {
        let input = b"1234567812345678";
        let segments = collect_segments(input, 8, 2, 2, 4);
        assert_eq!(segments.len(), 6);

        assert_eq!(segments[0].file_offset, 0);
        assert_eq!(segments[0].data, b"12345678");
        assert_eq!(segments[0].scan_range, 0..6);

        assert_eq!(segments[1].file_offset, 2);
        //                             11223333
        assert_eq!(segments[1].data, b"34567812");
        assert_eq!(segments[1].scan_range, 2..6);

        assert_eq!(segments[2].file_offset, 4);
        assert_eq!(segments[2].data, b"56781234");
        assert_eq!(segments[2].scan_range, 2..6);

        assert_eq!(segments[3].file_offset, 6);
        assert_eq!(segments[3].data, b"78123456");
        assert_eq!(segments[3].scan_range, 2..6);

        assert_eq!(segments[4].file_offset, 8);
        assert_eq!(segments[4].data, b"12345678");
        assert_eq!(segments[4].scan_range, 2..6);

        assert_eq!(segments[5].file_offset, 10);
        assert_eq!(segments[5].data, b"345678");
        assert_eq!(segments[5].scan_range, 2..6);
    }
}
