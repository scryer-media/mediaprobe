//! Fixtures taken byte for byte from the repository the parser was extracted
//! from. The parser's own unit and integration tests came with it and assert
//! facts of these exact files (sample timings, inventory offsets, parity
//! profiles), which a regenerated file with the same recipe does not
//! reproduce, because encoder output varies by build. Each is fetched from a
//! pinned commit over HTTPS and checked against the size and BLAKE3 pinned
//! here before it lands; the recipe the same name has elsewhere is replaced,
//! and its expectations still apply.

use super::{Fixture, Generator, Step};

/// The public repository the files come from.
pub(crate) const REPOSITORY: &str = "scryer-media/scryer";
/// The commit they are read at: a published commit whose files are the
/// same bytes as those at the commit the parser source was taken from.
pub(crate) const COMMIT: &str = "be7f03af111b9e51a8d2733e1ecf77abc882af6f";
/// Their directory in that repository.
pub(crate) const DIRECTORY: &str = "crates/scryer-mediainfo/tests/media";

pub(crate) struct Imported {
    pub(crate) name: &'static str,
    pub(crate) size: u64,
    pub(crate) blake3: &'static str,
}

/// Where one imported file is read from.
pub(crate) fn url(name: &str) -> String {
    format!("https://raw.githubusercontent.com/{REPOSITORY}/{COMMIT}/{DIRECTORY}/{name}")
}

/// Replace the recipe of every imported name with the pinned fetch.
pub(crate) fn apply(fixtures: &mut [Fixture]) {
    for imported in IMPORTED {
        let fixture = fixtures
            .iter_mut()
            .find(|f| f.name == imported.name)
            .unwrap_or_else(|| panic!("imported {} has no recipe", imported.name));
        fixture.generator = Generator::Imported;
        fixture.steps = vec![Step::Fetch {
            url: url(imported.name),
            size: imported.size,
            blake3: imported.blake3.to_owned(),
        }];
        fixture.encoders = Vec::new();
        describe_the_imported_bytes(fixture);
    }
}

/// Where an imported file differs from what the replaced recipe would have
/// written, the expectations follow the imported bytes. These files carry
/// parameter sets and configuration records over placeholder frames, so a
/// frame rate is not asserted where they declare none, and ffprobe agrees
/// on the size.
fn describe_the_imported_bytes(fixture: &mut Fixture) {
    let expect = &mut fixture.expect;
    match fixture.name.as_str() {
        "dv_profile5.mkv" | "dv_profile7.mkv" | "dv_profile8.mkv" | "h264_8k_aac.mkv" => {
            expect.fps = 0;
        }
        "hevc_hdr10plus.mkv" => {
            (expect.width, expect.height, expect.fps) = (3840, 2160, 0);
        }
        "hevc_hdr10plus.mp4" => {
            (expect.width, expect.height, expect.fps) = (3840, 2160, 0);
            expect.audio.clear();
        }
        _ => {}
    }
}

pub(crate) const IMPORTED: &[Imported] = &[
    Imported {
        name: "av1_sequence_pq.mkv",
        size: 1368,
        blake3: "40ae783193084bad89abc3746471ba3ca93d3169d4689da4866e42a425a46555",
    },
    Imported {
        name: "av1_sequence_pq.mp4",
        size: 1470,
        blake3: "93b4126e78d9dc1ea0ccf0a56b93d263c26bcb56dbece5578271d9e15bfec707",
    },
    Imported {
        name: "avi_idx1_dense_mpeg4.avi",
        size: 74182,
        blake3: "ac3da8c0ee5ca5bc00179f0467bbca3d6a993b310b308e0acedacae9fc213e3d",
    },
    Imported {
        name: "chapters_nero.mp4",
        size: 7319,
        blake3: "b55db9dec1e166e2164d47bdc753cee4b6be6a44af12e32cdb8a4c9d66c42540",
    },
    Imported {
        name: "chapters_quicktime.mp4",
        size: 7271,
        blake3: "8451f8c9b4f072c5b3d777a0c1efe2499f35f6c5ba928b2a556f154c8148059e",
    },
    Imported {
        name: "dv_profile5.mkv",
        size: 315,
        blake3: "adf12018394958bb32171320ce28043357efdd37b2aed832fc17405720db00d2",
    },
    Imported {
        name: "dv_profile7.mkv",
        size: 315,
        blake3: "6638e826fc94fb8eeb01da7f3606533160affce3f1053147faa886d12a16405d",
    },
    Imported {
        name: "dv_profile7.mp4",
        size: 21432,
        blake3: "e159f0c4ecc2a27c6e587f8261327af5bf3a388d27dafb21322a68e32df00086",
    },
    Imported {
        name: "dv_profile8.mkv",
        size: 315,
        blake3: "6d47b2c2cf1fef8e53ffd6fdd979fd8a5a9e341f3c901b28777ae1ca2ea4b28d",
    },
    Imported {
        name: "dv_profile8.mp4",
        size: 21432,
        blake3: "7145f4f026b3a575c673b637df0358a45638a01580e8caa80a22961a1cb9a13d",
    },
    Imported {
        name: "flv_flv1_aac_mono.flv",
        size: 5668,
        blake3: "d85dd7c768dcf1d372fa46576bfd08e3d38657786237130b2bb938e34873c540",
    },
    Imported {
        name: "flv_flv1_adpcm_swf.flv",
        size: 41734,
        blake3: "6c64d21e677503bdd86eb95477a67b203c26e3f0f45cdff4f2e40beba1eee55d",
    },
    Imported {
        name: "flv_flv1_mp3.flv",
        size: 17132,
        blake3: "a2281e7edc307b2222513043a2b76578a437ca6827964b264cbc7c9174a8ca33",
    },
    Imported {
        name: "flv_flv1_nellymoser.flv",
        size: 39987,
        blake3: "63c6ebdc325d3227d0401d274405c248050f5226b05673f535a871d78989c71b",
    },
    Imported {
        name: "flv_flv1_pcm_mulaw.flv",
        size: 70099,
        blake3: "8dba9ef1c004c977fa09d0b4501f7fd215f1c349dc9b4ade42c2a3f1b55c4f0c",
    },
    Imported {
        name: "flv_flv1_pcm_s16le.flv",
        size: 280987,
        blake3: "0eb34b406df7e888a262c2f2a7f1e8d2af69247312c34d045863e751ffe3869c",
    },
    Imported {
        name: "flv_flv1_pcm_u8.flv",
        size: 67105,
        blake3: "7bd7c40ba61ecedc93ed80e3b00e7f91e34da4a07dd50184d948607952250b09",
    },
    Imported {
        name: "flv_flv1_speex.flv",
        size: 28639,
        blake3: "ffa73cc5c1de08afdfe5dd459ec8b59aab49c87cb9fa7b4544b2a9114ffe436c",
    },
    Imported {
        name: "flv_flv1_video_only.flv",
        size: 4021,
        blake3: "6f64e7319864799bb0a09a0550e15d63986f4fad477104bccb85e603d718d3a3",
    },
    Imported {
        name: "flv_h264_aac.flv",
        size: 21626,
        blake3: "b06127413354f8650dbf7d19381d6b3f0f0d15f86cbc1245c50d8e9e9f8f9ae8",
    },
    Imported {
        name: "flv_h264_mp3.flv",
        size: 18467,
        blake3: "76bc712c51d55ce205fc9a42d63598cd39df02a0a4d37a9287fe637c72914384",
    },
    Imported {
        name: "flv_h264_pcm_alaw.flv",
        size: 71505,
        blake3: "aa810f47c8a6a6b1d15b4a06726ace347581946d24a2ec9d87a6140eaeb1b432",
    },
    Imported {
        name: "h264_8k_aac.mkv",
        size: 101669,
        blake3: "67a46a23e97b311b64d30cad456bacac3dad09c557678f201530fe392ba8c5ab",
    },
    Imported {
        name: "h264_aac.mkv",
        size: 18688,
        blake3: "66241ebfecba6a5e43f8d9e13a05e6d77eff6cff802bdaf31e256ea62f887c75",
    },
    Imported {
        name: "h264_aac.ts",
        size: 52076,
        blake3: "c805bbbf2cefc1c0c6dd4f0ceaa57e4d4aad412d7c5d2182cf016a63d0242bee",
    },
    Imported {
        name: "hevc_hdr10plus.mkv",
        size: 283,
        blake3: "2821f399a74048c058bdd5c845016c2b526616bc0bcf4ea5a86a4cd983f91cba",
    },
    Imported {
        name: "hevc_hdr10plus.mp4",
        size: 691,
        blake3: "3f2403c49d4c9dd29f6ca507fc9e0ea7f1359f5a8fe1afe7df8390b6e87cd488",
    },
    Imported {
        name: "hevc_sequence_pq.mkv",
        size: 6354,
        blake3: "8822abb6cd45d45c7d81c00acb90b44b350f6356e6f20b3367acda34b084c380",
    },
    Imported {
        name: "hevc_sequence_pq.mp4",
        size: 6644,
        blake3: "d37f32516918fbc2b0bff326021727d62890d97d37e267f532c2b7ad4b98330c",
    },
    Imported {
        name: "matrix_avi_002.avi",
        size: 830830,
        blake3: "bde9b3f1f8a904f9b0d09ba85882f66fe2d1cdf3a64e5033468821b23b576747",
    },
    Imported {
        name: "matrix_avi_005.avi",
        size: 44308,
        blake3: "20575e195cffe3534b073147bfa825d8b7511cd7fa247921a0b891991662a894",
    },
    Imported {
        name: "matrix_avi_013.avi",
        size: 38522,
        blake3: "86e913372dbb6f4fdff07425b48050b188ea6fd00e97dfbde5993adb2e4ba599",
    },
    Imported {
        name: "matrix_avi_038.avi",
        size: 868404,
        blake3: "fc62da3819a7ed914eec8083735c55e3d51bee4ff88e61fb7fbcf052ec742728",
    },
    Imported {
        name: "matrix_mkv_001.mkv",
        size: 15986,
        blake3: "be594240a7138a6707490b86f4242ed4c5785434802fefb77daefa56e55726f5",
    },
    Imported {
        name: "matrix_mkv_002.mkv",
        size: 31079,
        blake3: "69e57343012eb946e7cfe0ebd445adca5a7af79791a5ca67f2aa25351f7da42a",
    },
    Imported {
        name: "matrix_mkv_003.mkv",
        size: 42224,
        blake3: "d58ef672fdda3269ae59af6d447519f1559a7dbd1f64ed68694b7124098bbd45",
    },
    Imported {
        name: "matrix_mkv_004.mkv",
        size: 23016,
        blake3: "973f9dc09b17bda0e8844983f42386849a6c7688967a6b56f6f02fc607b6c6f8",
    },
    Imported {
        name: "matrix_mkv_005.mkv",
        size: 24562,
        blake3: "057bbe8637484d18fbe30ac96e334d4099ef1bcad644b1622715a6cfe2e59579",
    },
    Imported {
        name: "matrix_mkv_007.mkv",
        size: 48123,
        blake3: "cef16ef7b0b17e061bdaa0ec992e881f6e554e184670ceb29c96010fea459819",
    },
    Imported {
        name: "matrix_mkv_015.mkv",
        size: 50331,
        blake3: "e7660e6a599eb183cb6349b343b902d0fac32256578a960def68000fd3a915cd",
    },
    Imported {
        name: "matrix_mkv_020.mkv",
        size: 19384,
        blake3: "6e5610ab8fcf1bcb41b5597339f6df1fd85db0ee081af1ea30e1cf18b18bd681",
    },
    Imported {
        name: "matrix_mp4_002.m4v",
        size: 36933,
        blake3: "27dd746f03740fb5774db15b076c23b07a80adaf2e31bc219e88bf73f1696275",
    },
    Imported {
        name: "matrix_mp4_008.m4v",
        size: 8153,
        blake3: "b0b4384584b4b9a802a3659f1ed7f85a09cd1e062e59d92258f38659884fcf5e",
    },
    Imported {
        name: "matrix_mp4_011.m4v",
        size: 27465,
        blake3: "fed1b51b1f6fa7d4abcb940067398aec0cdcdd97ddae76e38129de022ecb4211",
    },
    Imported {
        name: "matrix_ts_001.ts",
        size: 28200,
        blake3: "c29cf27d47481ea33a69c1d84d2c62ec967b25efddeb2e6027bb94d4b99e035d",
    },
    Imported {
        name: "matrix_ts_002.m2ts",
        size: 49152,
        blake3: "cd8a031ee9b049846a32bce84a5287140d1f56d9432b3fc96ab4ffc6ca5ffd54",
    },
    Imported {
        name: "matrix_ts_003.ts",
        size: 68808,
        blake3: "245b6883886943fc15826acd43bad74c833274112f14e1ff60bdea4a25c3dd17",
    },
    Imported {
        name: "matrix_ts_004.m2ts",
        size: 36864,
        blake3: "959544e499ab139a0b8116947003d7632a4e22bad39b8c094d666345542a2fc2",
    },
    Imported {
        name: "matrix_ts_005.ts",
        size: 287264,
        blake3: "ed9a3c0acf49a8421bc970aa9e2e46268d93443e9eea72dc228971cb543c27c9",
    },
    Imported {
        name: "matrix_ts_016.m2ts",
        size: 18432,
        blake3: "007b4b43d49daa8a6579f47d354eaa6faf545bb9ab540d65f18f52afe4d6d450",
    },
    Imported {
        name: "matrix_ts_023.ts",
        size: 68808,
        blake3: "f03f3cd00aaaa3c31d40fed4cf17675d308c4c64e0d51fde34239d5c7048e9f2",
    },
    Imported {
        name: "ogm_audio_only.ogm",
        size: 198,
        blake3: "6029669254d7cbaddc0968a8fc303338e205e9ce39529ce3d178c041ecf4df95",
    },
    Imported {
        name: "ogm_video_audio_text.ogm",
        size: 3838,
        blake3: "5eeb51be6bc0f7a4fc43d5aff4574c7db79a7781fee04691fdd46d68ec3e7452",
    },
    Imported {
        name: "ogv_theora_opus_surround.ogv",
        size: 9687,
        blake3: "38465b0ceacded48ba3d2df1f72278d11076ec71afd3753a34a471f58e505e95",
    },
    Imported {
        name: "ogv_theora_vorbis.ogv",
        size: 12453,
        blake3: "08200d3c615e73a8cf8a9b214d1d26fe04822f1a9b576eeae6b6adca08313235",
    },
    Imported {
        name: "ps_mpeg1_mp2.mpg",
        size: 133120,
        blake3: "d630f7e10d2a9e8745318a4603eef877da66f61fcb3732bd4fb85a9b831ab903",
    },
    Imported {
        name: "ps_mpeg2_mp2.vob",
        size: 135168,
        blake3: "64ba0fe2d020a26e184c2b4f8953c6f96efebc4e1e19226d51aa46b36a18e227",
    },
    Imported {
        name: "simd_dense_h264_aac.mp4",
        size: 32036,
        blake3: "b89809cf28b187df527d27eb7cd99ea8b7cfe4a52c7b1f3335dc7d1a2e4a19d7",
    },
    Imported {
        name: "simd_dense_h264_aac.ts",
        size: 225788,
        blake3: "79feda86a6f923a157c3233e8b554aa67f980ed62de99a8855d900763b076fcb",
    },
    Imported {
        name: "simd_dense_h264_aac_192.m2ts",
        size: 230592,
        blake3: "fb138b161de6a167d0e0728b328d1cc22246a841ecbaab020a618222f20e36a2",
    },
    Imported {
        name: "simd_dense_h264_aac_204.ts",
        size: 245004,
        blake3: "e9e36f595bd90aef1ad1e296375f7bce30b99d88189823ae986765ef8f96da86",
    },
    Imported {
        name: "simd_dense_h264_ac3.ts",
        size: 318096,
        blake3: "fc910c1a741879efb9c80f78159c6b6f99cd6bd53afc647eb20b424aa57401c1",
    },
    Imported {
        name: "simd_dense_h264_dts.ts",
        size: 1768328,
        blake3: "4f07b264db64ac9a73ccfa20f014211715bd51d732f37cbcd3e347e662b2820e",
    },
    Imported {
        name: "simd_dense_mpeg2_mp2.ts",
        size: 344228,
        blake3: "335466eb06fc969394068ec09fe9b0281463563794fb5a5d8d9a16abafec133c",
    },
    Imported {
        name: "simd_dense_vp9_opus.webm",
        size: 34939,
        blake3: "30b83b21c50811c00570191b466248f9f3bdb2de8ffda81d519a7da60264cb97",
    },
    Imported {
        name: "wmv_wmv1_aac_surround.wmv",
        size: 10598,
        blake3: "b2c495cd9909802bc090a5c0a55a0f82b580ecd8bc93a815b32bb5f0b68c93bc",
    },
    Imported {
        name: "wmv_wmv1_mp3_mono.wmv",
        size: 20182,
        blake3: "e65af4f4f4aa3802d1b963f8a4d21abcbba32f9a3873981a7adfb00fe5d8d809",
    },
    Imported {
        name: "wmv_wmv1_wmav1.wmv",
        size: 20178,
        blake3: "3affe169c7cca159ef6a4b1c3c65f06c91c8f42afc7df7e622865f115d65cb6f",
    },
    Imported {
        name: "wmv_wmv2_video_only.wmv",
        size: 7131,
        blake3: "c52d21bd6f2c044a5c302a937bfe477234ce956e9a2c27bae06cc949185cfac2",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_import_is_pinned_and_names_a_recipe() {
        let mut fixtures = super::super::all();
        apply(&mut fixtures);
        let mut seen = std::collections::BTreeSet::new();
        for imported in IMPORTED {
            assert!(seen.insert(imported.name), "{} listed twice", imported.name);
            assert!(
                crate::corpus::is_blake3_hex(imported.blake3),
                "{}",
                imported.name
            );
            assert!(imported.size > 0, "{}", imported.name);
        }
        assert_eq!(
            fixtures
                .iter()
                .filter(|f| f.generator == Generator::Imported)
                .count(),
            IMPORTED.len()
        );
        assert!(url("a.mkv").starts_with("https://"));
    }
}
