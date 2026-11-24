//! ISO BMFF (MP4, MOV, 3GP) box trees: parse, edit, serialise.
//!
//! The parser here is deliberately forgiving and independent of mediaprobe's:
//! anything it cannot read as a box becomes opaque bytes that round-trip
//! unchanged, so a mutation always starts from the input's real structure and
//! never silently drops data.

use crate::rng::{INTERESTING, Rng, perturb};

/// Boxes whose payload is a list of boxes.
const CONTAINERS: [&[u8; 4]; 22] = [
    b"moov", b"trak", b"mdia", b"minf", b"stbl", b"edts", b"dinf", b"udta", b"mvex", b"moof",
    b"traf", b"mfra", b"tref", b"ilst", b"sinf", b"schi", b"rinf", b"gmhd", b"wave", b"tapt",
    b"meco", b"strk",
];

/// Containers that start with a full-box version and flags.
const FULL_CONTAINERS: [&[u8; 4]; 2] = [b"meta", b"iref"];

/// Sample descriptions: version, flags and an entry count, then entries.
const SAMPLE_DESCRIPTIONS: [&[u8; 4]; 1] = [b"stsd"];

/// Visual sample entries: 78 bytes of fixed fields, then child boxes.
const VISUAL_ENTRIES: [&[u8; 4]; 22] = [
    b"avc1", b"avc3", b"hvc1", b"hev1", b"dvh1", b"dvhe", b"dvav", b"dva1", b"dav1", b"av01",
    b"vp08", b"vp09", b"mp4v", b"encv", b"mjpg", b"jpeg", b"s263", b"apch", b"apcn", b"mp2v",
    b"vvc1", b"vvi1",
];

/// Audio sample entries: 28 bytes of fixed fields, then child boxes.
const AUDIO_ENTRIES: [&[u8; 4]; 16] = [
    b"mp4a", b"ac-3", b"ec-3", b"ac-4", b"Opus", b"fLaC", b"alac", b"enca", b"dtsc", b"dtsh",
    b"dtsl", b"dtse", b"mlpa", b"samr", b"sowt", b"twos",
];

/// Types the box-type mutation substitutes, chosen from the ones the parser
/// dispatches on.
pub const TYPES: [&[u8; 4]; 40] = [
    b"ftyp", b"moov", b"mvhd", b"trak", b"tkhd", b"mdia", b"mdhd", b"hdlr", b"minf", b"stbl",
    b"stsd", b"stts", b"stsc", b"stsz", b"stco", b"co64", b"ctts", b"stss", b"elst", b"edts",
    b"mdat", b"moof", b"traf", b"tfhd", b"trun", b"mvex", b"trex", b"avcC", b"hvcC", b"av1C",
    b"dvcC", b"dvvC", b"dvwC", b"vpcC", b"mdcv", b"clli", b"colr", b"pasp", b"esds", b"chpl",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// Opaque payload.
    Leaf(Vec<u8>),
    /// `prefix` (full-box fields, entry counts, sample-entry fields) then
    /// child boxes, then `trailer` bytes too short to be a box.
    Children {
        prefix: Vec<u8>,
        children: Vec<Mp4Box>,
        trailer: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mp4Box {
    pub kind: [u8; 4],
    /// Written in place of the computed 32-bit size, to model lying sizes.
    pub size_override: Option<u32>,
    /// The box used a 64-bit largesize header.
    pub large: bool,
    pub body: Body,
}

/// A parsed file: boxes, then bytes that do not form a box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mp4File {
    pub boxes: Vec<Mp4Box>,
    pub trailer: Vec<u8>,
}

fn prefix_len(kind: &[u8; 4]) -> Option<usize> {
    if CONTAINERS.contains(&kind) {
        Some(0)
    } else if FULL_CONTAINERS.contains(&kind) {
        Some(4)
    } else if SAMPLE_DESCRIPTIONS.contains(&kind) {
        Some(8)
    } else if VISUAL_ENTRIES.contains(&kind) {
        Some(78)
    } else if AUDIO_ENTRIES.contains(&kind) {
        Some(28)
    } else {
        None
    }
}

fn parse_list(mut data: &[u8], depth: usize) -> (Vec<Mp4Box>, Vec<u8>) {
    let mut boxes = Vec::new();
    while data.len() >= 8 {
        let size32 = u32::from_be_bytes(data[..4].try_into().unwrap()) as u64;
        let kind: [u8; 4] = data[4..8].try_into().unwrap();
        let (header, size, large) = match size32 {
            0 => (8, data.len() as u64, false),
            1 => {
                if data.len() < 16 {
                    break;
                }
                (
                    16,
                    u64::from_be_bytes(data[8..16].try_into().unwrap()),
                    true,
                )
            }
            n => (8, n, false),
        };
        // A size that does not fit, or a size-0 box that is not last, ends
        // the structured part: the rest is kept opaque.
        if size < header as u64 || size > data.len() as u64 || (size32 == 0 && depth > 0) {
            break;
        }
        let payload = &data[header..size as usize];
        let body = match prefix_len(&kind) {
            Some(prefix) if depth < 12 && payload.len() >= prefix => {
                let (children, trailer) = parse_list(&payload[prefix..], depth + 1);
                Body::Children {
                    prefix: payload[..prefix].to_vec(),
                    children,
                    trailer,
                }
            }
            _ => Body::Leaf(payload.to_vec()),
        };
        boxes.push(Mp4Box {
            kind,
            size_override: (size32 == 0).then_some(0),
            large,
            body,
        });
        data = &data[size as usize..];
    }
    (boxes, data.to_vec())
}

impl Mp4File {
    pub fn parse(data: &[u8]) -> Self {
        let (boxes, trailer) = parse_list(data, 0);
        Self { boxes, trailer }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for b in &self.boxes {
            b.write(&mut out);
        }
        out.extend_from_slice(&self.trailer);
        out
    }

    /// Every box in the tree, as an index path from the root.
    pub fn paths(&self) -> Vec<Vec<usize>> {
        fn walk(list: &[Mp4Box], at: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
            for (i, b) in list.iter().enumerate() {
                at.push(i);
                out.push(at.clone());
                if let Body::Children { children, .. } = &b.body {
                    walk(children, at, out);
                }
                at.pop();
            }
        }
        let mut out = Vec::new();
        walk(&self.boxes, &mut Vec::new(), &mut out);
        out
    }

    fn siblings_mut(&mut self, parent: &[usize]) -> &mut Vec<Mp4Box> {
        let mut list = &mut self.boxes;
        for &i in parent {
            list = match &mut list[i].body {
                Body::Children { children, .. } => children,
                Body::Leaf(_) => unreachable!("paths only descend into containers"),
            };
        }
        list
    }

    /// Apply one structure-aware mutation. Returns false when the file has
    /// no box to work on.
    pub fn mutate(&mut self, rng: &mut Rng) -> bool {
        let paths = self.paths();
        if paths.is_empty() {
            return false;
        }
        let path = rng.pick(&paths).clone();
        let (parent, index) = path.split_at(path.len() - 1);
        let index = index[0];
        let siblings = self.siblings_mut(parent);
        match rng.below(7) {
            // A size that lies: interesting values, off by one, or zero.
            0 => {
                let real = siblings[index].len() as u64;
                let lie = match rng.below(3) {
                    0 => *rng.pick(&INTERESTING),
                    1 => real.wrapping_add(1),
                    _ => real.wrapping_sub(1),
                };
                siblings[index].size_override = Some(lie as u32);
            }
            1 => siblings[index].kind = **rng.pick(&TYPES),
            2 => {
                let copy = siblings[index].clone();
                siblings.insert(index + 1, copy);
            }
            3 => {
                siblings.remove(index);
            }
            4 => {
                let other = rng.below(siblings.len());
                siblings.swap(index, other);
            }
            5 => siblings[index].large = !siblings[index].large,
            _ => match &mut siblings[index].body {
                Body::Leaf(bytes) => perturb(bytes, rng),
                Body::Children { prefix, .. } if !prefix.is_empty() => perturb(prefix, rng),
                Body::Children { trailer, .. } => trailer.push(rng.next_u64() as u8),
            },
        }
        true
    }
}

impl Mp4Box {
    pub fn leaf(kind: &[u8; 4], payload: Vec<u8>) -> Self {
        Self {
            kind: *kind,
            size_override: None,
            large: false,
            body: Body::Leaf(payload),
        }
    }

    pub fn container(kind: &[u8; 4], prefix: Vec<u8>, children: Vec<Mp4Box>) -> Self {
        Self {
            kind: *kind,
            size_override: None,
            large: false,
            body: Body::Children {
                prefix,
                children,
                trailer: Vec::new(),
            },
        }
    }

    fn payload_len(&self) -> usize {
        match &self.body {
            Body::Leaf(bytes) => bytes.len(),
            Body::Children {
                prefix,
                children,
                trailer,
            } => prefix.len() + children.iter().map(Mp4Box::len).sum::<usize>() + trailer.len(),
        }
    }

    /// Serialised length, header included.
    pub fn len(&self) -> usize {
        self.payload_len() + if self.large { 16 } else { 8 }
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        let len = self.len() as u64;
        if self.large {
            out.extend_from_slice(&self.size_override.unwrap_or(1).to_be_bytes());
            out.extend_from_slice(&self.kind);
            out.extend_from_slice(&len.to_be_bytes());
        } else {
            let size = self
                .size_override
                .unwrap_or(len.min(u32::MAX as u64) as u32);
            out.extend_from_slice(&size.to_be_bytes());
            out.extend_from_slice(&self.kind);
        }
        match &self.body {
            Body::Leaf(bytes) => out.extend_from_slice(bytes),
            Body::Children {
                prefix,
                children,
                trailer,
            } => {
                out.extend_from_slice(prefix);
                for child in children {
                    child.write(out);
                }
                out.extend_from_slice(trailer);
            }
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len());
        self.write(&mut out);
        out
    }
}

/// libFuzzer-facing entry: mutate `data` structurally, or return `None` when
/// it does not start with a box so the caller falls back to byte mutation.
pub fn mutate(data: &[u8], seed: u64, max_size: usize) -> Option<Vec<u8>> {
    let mut file = Mp4File::parse(data);
    let mut rng = Rng::new(seed);
    let mut changed = false;
    for _ in 0..=rng.below(3) {
        changed |= file.mutate(&mut rng);
    }
    if !changed {
        return None;
    }
    let mut out = file.to_bytes();
    out.truncate(max_size);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        crate::build::mp4_with_config(b"avcC", &crate::params::H264_AVCC, 64, 36)
    }

    #[test]
    fn a_built_file_round_trips_and_is_fully_structured() {
        let bytes = sample();
        let file = Mp4File::parse(&bytes);
        assert!(file.trailer.is_empty());
        assert_eq!(file.to_bytes(), bytes);
        // ftyp, moov, mdat at the top; the avcC box deep inside.
        assert_eq!(file.boxes.len(), 3);
        assert!(file.paths().len() > 15);
    }

    #[test]
    fn opaque_bytes_round_trip() {
        for data in [
            &b""[..],
            b"\x00\x00\x00\x05abc",
            b"\xff\xff\xff\xffmoovxx",
            b"junk",
        ] {
            assert_eq!(Mp4File::parse(data).to_bytes(), data);
        }
    }

    #[test]
    fn mutation_is_deterministic_and_changes_the_input() {
        let bytes = sample();
        let mut changed = 0;
        for seed in 0..200 {
            let a = mutate(&bytes, seed, 1 << 20).unwrap();
            assert_eq!(a, mutate(&bytes, seed, 1 << 20).unwrap());
            changed += usize::from(a != bytes);
        }
        assert!(
            changed > 150,
            "only {changed} of 200 seeds changed the input"
        );
    }
}
