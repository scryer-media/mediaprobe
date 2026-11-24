//! Matroska tag collection stays bounded when one `Tag` names tens of
//! thousands of track UIDs and carries hundreds of long values. Without the
//! bounds every value would be kept once per matching UID; with them the
//! probe keeps at most 256 KiB of tags, warns once, and its heap stays
//! within a few megabytes of the input it was handed.
//!
//! The file is built here, so the test needs no corpus. A counting global
//! allocator measures the probe's peak heap; nextest runs each test in its
//! own process, so nothing else allocates while it runs.
#![cfg(feature = "probe")]

use mediaprobe::{ProbeDepth, ProbeOptions, StreamKind, probe_source};
use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call forwards to the system allocator unchanged; the
// counters only observe sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's layout contract passes through.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `alloc` above with this layout.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller's realloc contract passes through.
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            if new_size >= layout.size() {
                let grow = new_size - layout.size();
                let live = LIVE.fetch_add(grow, Ordering::Relaxed) + grow;
                PEAK.fetch_max(live, Ordering::Relaxed);
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        new
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Most key and value bytes of tags a probe keeps for one file.
const TAG_BYTES_LIMIT: usize = 256 * 1024;
/// Most heap the probe may hold at once beyond what was live before it;
/// the input is allocated before and not counted.
const HEAP_CAP: usize = 32 * 1024 * 1024;

const TARGETS: usize = 40_000;
const SIMPLE_TAGS: usize = 300;
const VALUE_BYTES: usize = 4096;
const TRACK_UID: u64 = 0x5EED;

fn vint_size(len: usize) -> Vec<u8> {
    // Eight-byte sizes throughout: simple and valid for any length here.
    let mut out = vec![0x01];
    out.extend_from_slice(&(len as u64).to_be_bytes()[1..]);
    out
}

fn id_bytes(id: u32) -> Vec<u8> {
    let bytes = id.to_be_bytes();
    let skip = bytes.iter().take_while(|b| **b == 0).count();
    bytes[skip..].to_vec()
}

fn element(id: u32, body: &[u8]) -> Vec<u8> {
    let mut out = id_bytes(id);
    out.extend(vint_size(body.len()));
    out.extend_from_slice(body);
    out
}

fn uint(id: u32, value: u64) -> Vec<u8> {
    element(id, &value.to_be_bytes())
}

fn master(id: u32, children: &[Vec<u8>]) -> Vec<u8> {
    element(id, &children.concat())
}

/// One PCM audio track whose UID is [`TRACK_UID`], a cluster with one
/// frame, and one `Tag` naming [`TARGETS`] track UIDs (every sixteenth is
/// the real track's, the rest match nothing) over [`SIMPLE_TAGS`] simple
/// tags of [`VALUE_BYTES`] each.
fn tag_flood() -> Vec<u8> {
    let header = master(
        0x1A45_DFA3,
        &[
            uint(0x4286, 1),
            uint(0x42F7, 1),
            uint(0x42F2, 4),
            uint(0x42F3, 8),
            element(0x4282, b"matroska"),
            uint(0x4287, 4),
            uint(0x4285, 2),
        ],
    );
    let info = master(
        0x1549_A966,
        &[
            uint(0x2A_D7B1, 1_000_000),
            element(0x4489, &1000f64.to_be_bytes()),
        ],
    );
    let tracks = master(
        0x1654_AE6B,
        &[master(
            0xAE,
            &[
                uint(0xD7, 1),
                uint(0x73C5, TRACK_UID),
                uint(0x83, 2),
                element(0x86, b"A_PCM/INT/LIT"),
                master(
                    0xE1,
                    &[
                        element(0xB5, &48_000f64.to_be_bytes()),
                        uint(0x9F, 2),
                        uint(0x6264, 16),
                    ],
                ),
            ],
        )],
    );
    let mut block = vec![0x81, 0x00, 0x00, 0x80];
    block.extend_from_slice(&[0u8; 64]);
    let cluster = master(0x1F43_B675, &[uint(0xE7, 0), element(0xA3, &block)]);

    let mut targets = vec![uint(0x68CA, 50)];
    targets.extend((0..TARGETS).map(|i| {
        let uid = if i % 16 == 15 {
            TRACK_UID
        } else {
            0x1_0000 + i as u64
        };
        uint(0x63C5, uid)
    }));
    let mut tag = vec![master(0x63C0, &targets)];
    let value: Vec<u8> = (0..VALUE_BYTES).map(|k| b'a' + (k % 26) as u8).collect();
    tag.extend((0..SIMPLE_TAGS).map(|i| {
        master(
            0x67C8,
            &[
                element(0x45A3, format!("tag-{i}").as_bytes()),
                element(0x4487, &value),
            ],
        )
    }));
    let tags = master(0x1254_C367, &[master(0x7373, &tag)]);

    let segment = master(0x1853_8067, &[info, tracks, cluster, tags]);
    [header, segment].concat()
}

#[test]
fn one_tag_naming_many_tracks_keeps_a_bounded_set() {
    let file = tag_flood();
    let options = ProbeOptions::new().with_depth(ProbeDepth::Full);
    assert!(
        (file.len() as u64) < ProbeOptions::DEFAULT_READ_BUDGET_BYTES,
        "the whole flood is within the read budget, so the cap is the tag bound"
    );

    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let details = probe_source(Cursor::new(&file[..]), "mkv", &options).expect("the file probes");
    let peak = PEAK.load(Ordering::Relaxed) - before;
    eprintln!("peak heap during the probe: {peak} bytes");

    assert!(
        peak <= HEAP_CAP,
        "the probe held {peak} bytes of heap at once, over the {HEAP_CAP}-byte cap"
    );
    let kept: usize = std::iter::once(&details.tags)
        .chain(details.streams.iter().map(|s| &s.metadata.tags))
        .flat_map(|scope| scope.iter())
        .map(|(k, v)| k.len() + v.len())
        .sum();
    assert!(kept > 0, "some tags are kept");
    assert!(kept <= TAG_BYTES_LIMIT, "kept {kept} bytes of tags");
    let truncated = details
        .report
        .warnings
        .iter()
        .filter(|w| w.code == "tags_truncated")
        .count();
    assert_eq!(truncated, 1, "one tags_truncated warning");
    assert_eq!(
        details
            .streams
            .iter()
            .filter(|s| s.kind == StreamKind::Audio)
            .count(),
        1,
        "the audio track is still reported"
    );
}
