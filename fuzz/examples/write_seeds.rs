//! Seeds for the wrapping targets, which have no corpus files of their own:
//! every selector byte with a few payload shapes, and the tag target's
//! header shapes including the tag flood. Parser-free, so it runs
//! anywhere. `fuzz/scripts/seed-corpora.sh` calls it.
//!
//! Usage: cargo run --locked --example write_seeds -- <corpus-dir>

use std::path::PathBuf;

const PAYLOADS: [&[u8]; 4] = [
    &[],
    &[0x00, 0x00, 0x03, 0x01, 0xFF],
    &[
        0x01, 0x10, 0x10, 0x00, 0x00, 0x13, 0x88, 0x02, 0x00, 0x00, 0x03, 0xE8,
    ],
    &[
        0x40, 0x01, 0x0C, 0x01, 0xFF, 0xFF, 0x02, 0x20, 0x00, 0x00, 0x03, 0x00,
    ],
];

fn main() {
    let root: PathBuf = std::env::args_os()
        .nth(1)
        .expect("usage: write_seeds <corpus-dir>")
        .into();
    let mut written = 0;
    for target in ["codec_config", "bitstream", "hdr_sei", "dolby_vision"] {
        let dir = root.join(target);
        std::fs::create_dir_all(&dir).expect("creating the corpus directory");
        for pick in 0..=255u8 {
            for (i, payload) in PAYLOADS.iter().enumerate() {
                let data = [&[pick][..], payload].concat();
                std::fs::write(dir.join(format!("seed-{pick:03}-{i}")), data)
                    .expect("writing a seed");
                written += 1;
            }
        }
    }
    // The tag target reads a header, not a selector: a few shapes and the
    // flood the parser bounds.
    let dir = root.join("mkv_tags");
    std::fs::create_dir_all(&dir).expect("creating the corpus directory");
    let shapes: [&[u8]; 4] = [
        &[0, 1, 0, 4, 0, 0, 0x00, 0x00, 0x03, b'k', b'e', b'y'],
        &[3, 8, 0, 16, 0, 0x81, 0xC8, 0x07, 0x00, 0x00],
        &[0, 0, 1, 0, 1, 0x40],
        &[],
    ];
    for (i, shape) in shapes.iter().enumerate() {
        std::fs::write(dir.join(format!("seed-shape-{i}")), shape).expect("writing a seed");
        written += 1;
    }
    std::fs::write(
        dir.join("seed-flood"),
        mediaprobe_fuzz::inputs::mkv_tags_flood_seed(),
    )
    .expect("writing a seed");
    written += 1;
    println!("wrote {written} seeds under {}", root.display());
}
