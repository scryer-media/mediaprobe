//! Bounded CLPI entry-point maps. Packet ranges include the surrounding access
//! units needed for header inspection; playlist times remain the runtime source.
use super::image::ImageError;

type Result<T> = std::result::Result<T, ImageError>;
const MAX_POINTS: usize = 131_072;

#[derive(Clone)]
pub(super) struct EntryMap {
    pub pid: u16,
    /// Access points in strictly increasing packet order.
    points: Vec<Point>,
    /// Ascending indices `i` whose successor does not advance in time, so a
    /// range can find a timing reset without walking its points.
    resets: Vec<usize>,
}
#[derive(Clone, Copy)]
struct Point {
    packet: u32,
    time: u32,
}

fn bytes(data: &[u8], at: usize, length: usize) -> Result<&[u8]> {
    data.get(
        at..at
            .checked_add(length)
            .ok_or(ImageError::Malformed("CPI offset overflow"))?,
    )
    .ok_or(ImageError::Malformed("truncated CPI table"))
}
fn word(data: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(bytes(data, at, 4)?.try_into().unwrap()))
}

pub(super) fn parse(data: &[u8], source_packets: u32) -> Result<Vec<EntryMap>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    if bytes(data, 0, 2)?[1] & 15 != 1 {
        return Err(ImageError::Unsupported("CLPI CPI map type"));
    }
    let count = bytes(data, 2, 2)?[1] as usize;
    let table_end = 4 + count * 12;
    bytes(data, 4, count * 12)?;
    let mut maps = Vec::<EntryMap>::new();
    let mut occupied = Vec::<std::ops::Range<usize>>::new();
    let mut remaining = MAX_POINTS;
    for entry in data[4..table_end].as_chunks::<12>().0 {
        let pid = u16::from_be_bytes(entry[..2].try_into().unwrap());
        let packed = entry[2..8]
            .iter()
            .fold(0_u64, |value, byte| value << 8 | u64::from(*byte));
        let coarse_count = ((packed >> 18) & 0xffff) as usize;
        let fine_count = (packed & 0x3ffff) as usize;
        if pid >= 8192
            || maps.iter().any(|map| map.pid == pid)
            || coarse_count == 0
            || fine_count == 0
        {
            return Err(ImageError::Malformed("invalid CPI stream or empty map"));
        }
        remaining = remaining
            .checked_sub(coarse_count + fine_count)
            .ok_or(ImageError::Budget)?;
        let at = 2_usize
            .checked_add(word(entry, 8)? as usize)
            .ok_or(ImageError::Malformed("CPI map offset overflow"))?;
        if at < table_end {
            return Err(ImageError::Malformed("CPI map overlaps stream directory"));
        }
        let fine_offset = word(data, at)? as usize;
        if fine_offset < 4 + coarse_count * 8 {
            return Err(ImageError::Malformed(
                "overlapping CPI coarse and fine tables",
            ));
        }
        let fine_at = at
            .checked_add(fine_offset)
            .ok_or(ImageError::Malformed("CPI fine offset overflow"))?;
        let coarse = bytes(data, at + 4, coarse_count * 8)?;
        let fine = bytes(data, fine_at, fine_count * 4)?;
        let end = fine_at + fine.len();
        if occupied
            .iter()
            .any(|range| at < range.end && range.start < end)
        {
            return Err(ImageError::Malformed("overlapping CPI stream maps"));
        }
        occupied.push(at..end);
        let mut points = Vec::<Point>::with_capacity(fine_count);
        for (index, entry) in coarse.as_chunks::<8>().0.iter().enumerate() {
            let timing = word(entry, 0)?;
            let first = (timing >> 14) as usize;
            let next = if index + 1 < coarse_count {
                (word(coarse, (index + 1) * 8)? >> 14) as usize
            } else {
                fine_count
            };
            if (index == 0 && first != 0) || first >= next || next > fine_count {
                return Err(ImageError::Malformed(
                    "CPI coarse entry references invalid fine range",
                ));
            }
            let base_packet = word(entry, 4)? & !0x1ffff;
            let base_time = (timing & 0x3ffe) << 18;
            for fine in fine[first * 4..next * 4].as_chunks::<4>().0 {
                let fine = word(fine, 0)?;
                let packet = base_packet | (fine & 0x1ffff);
                let time = base_time | (((fine >> 17) & 0x7ff) << 8);
                if packet >= source_packets
                    || points
                        .last()
                        .is_some_and(|previous| previous.packet >= packet)
                {
                    return Err(ImageError::Malformed(
                        "CPI entry packet outside clip or out of order",
                    ));
                }
                points.push(Point { packet, time });
            }
        }
        maps.push(EntryMap::new(pid, points));
    }
    Ok(maps)
}

impl EntryMap {
    fn new(pid: u16, points: Vec<Point>) -> Self {
        let resets = points
            .windows(2)
            .enumerate()
            .filter(|(_, pair)| pair[0].time >= pair[1].time)
            .map(|(index, _)| index)
            .collect();
        Self {
            pid,
            points,
            resets,
        }
    }
    pub(super) fn range(
        &self,
        sequence: std::ops::Range<u32>,
        input: u32,
        output: u32,
    ) -> Result<std::ops::Range<u32>> {
        let first = self
            .points
            .partition_point(|point| point.packet < sequence.start);
        let end = self
            .points
            .partition_point(|point| point.packet < sequence.end);
        let points = &self.points[first..end];
        if points.is_empty() {
            return Err(ImageError::Unsupported(
                "CPI has no access point in selected clock sequence",
            ));
        }
        // A reset between points `i` and `i + 1` lies inside the slice when
        // `first <= i` and `i + 1 < end`.
        let reset = self.resets.partition_point(|index| *index < first);
        if self.resets.get(reset).is_some_and(|index| index + 1 < end) {
            return Err(ImageError::Unsupported(
                "CPI timing resets within selected clock sequence",
            ));
        }
        // Times strictly increase across the slice, so both boundaries are
        // binary searches. CPI times omit their low eight bits. Include the
        // predecessor and the successor access unit boundary instead of
        // guessing an exact frame cut.
        let start = match points.partition_point(|point| point.time <= input) {
            0 => sequence.start,
            after => points[after - 1].packet,
        };
        let end = points
            .get(points.partition_point(|point| point.time < output))
            .map_or(sequence.end, |point| point.packet);
        if end <= start {
            return Err(ImageError::Malformed(
                "CPI produces an empty playback range",
            ));
        }
        Ok(start..end)
    }
    pub(super) fn point_count(&self) -> usize {
        self.points.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The linear form this module used before resets were indexed.
    fn linear_range(
        points: &[Point],
        sequence: std::ops::Range<u32>,
        input: u32,
        output: u32,
    ) -> Result<std::ops::Range<u32>> {
        let first = points.partition_point(|point| point.packet < sequence.start);
        let end = points.partition_point(|point| point.packet < sequence.end);
        let points = &points[first..end];
        if points.is_empty() {
            return Err(ImageError::Unsupported(
                "CPI has no access point in selected clock sequence",
            ));
        }
        if points.windows(2).any(|pair| pair[0].time >= pair[1].time) {
            return Err(ImageError::Unsupported(
                "CPI timing resets within selected clock sequence",
            ));
        }
        let start = points
            .iter()
            .rev()
            .find(|point| point.time <= input)
            .map_or(sequence.start, |point| point.packet);
        let end = points
            .iter()
            .find(|point| point.time >= output)
            .map_or(sequence.end, |point| point.packet);
        if end <= start {
            return Err(ImageError::Malformed(
                "CPI produces an empty playback range",
            ));
        }
        Ok(start..end)
    }

    fn outcome(result: Result<std::ops::Range<u32>>) -> String {
        match result {
            Ok(range) => format!("{range:?}"),
            Err(error) => format!("{error:?}"),
        }
    }

    #[test]
    fn indexed_ranges_match_the_linear_scan() {
        let mut state = 0x2545_f491_u32;
        let mut next = |bound: u32| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state % bound
        };
        let mut checked = 0;
        for _ in 0..200 {
            let mut points = Vec::new();
            let mut packet = next(4);
            let mut time = next(50);
            for _ in 0..1 + next(40) {
                points.push(Point { packet, time });
                packet += 1 + next(5);
                // Mostly advancing time, with occasional equal or earlier
                // times so some sequences contain a reset.
                time = match next(12) {
                    0 => time.saturating_sub(next(30)),
                    1 => time,
                    _ => time + 1 + next(20),
                };
            }
            let map = EntryMap::new(0x1011, points.clone());
            let last = packet + 3;
            for _ in 0..60 {
                let a = next(last + 1);
                let b = next(last + 1);
                let sequence = a.min(b)..a.max(b) + 1;
                let horizon = time + 40;
                let input = next(horizon);
                let output = next(horizon);
                assert_eq!(
                    outcome(map.range(sequence.clone(), input, output)),
                    outcome(linear_range(&points, sequence, input, output)),
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 12_000);
    }

    #[test]
    fn resets_are_indexed_once_at_parse_time() {
        let points = [(0, 10), (1, 20), (2, 20), (3, 30), (4, 5), (5, 6)]
            .map(|(packet, time)| Point { packet, time });
        let map = EntryMap::new(0x1011, points.to_vec());
        assert_eq!(map.resets, [1, 3]);
        // Slices that avoid both resets still resolve.
        assert_eq!(outcome(map.range(0..2, 10, 20)), "0..1");
        assert_eq!(outcome(map.range(2..4, 20, 30)), "2..3");
        assert_eq!(outcome(map.range(4..6, 5, 6)), "4..5");
        // A slice spanning either reset refuses.
        assert!(map.range(0..3, 10, 20).is_err());
        assert!(map.range(3..5, 20, 30).is_err());
    }
}
