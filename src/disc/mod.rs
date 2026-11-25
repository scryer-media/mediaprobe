//! Inspection of intact mastered optical-disc images.

mod clip_info;
mod cpi;
#[cfg(test)]
mod fixtures;
mod image;
mod navigation;
mod streams;
#[cfg(test)]
mod udf_fixtures;

use crate::warning;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::{Seek, SeekFrom};
use std::path::Path;
use std::time::Instant;

use crate::{
    ANALYSIS_REVISION, AnalysisDetails, DiscMetadata, DiscSegment, DiscSelection, DiscTitle,
    ProbeReport, ProbeStatus, Provenance,
};

use crate::source::{BoundedSource, Extent, ExtentSource, FileSource, MediaSource};
use crate::{ProbeDepth, ProbeError, ProbeOptions};
use image::{Image, ImageError, ImageFile};
use navigation::warning;

const NAVIGATION_BUDGET: u64 = 32 * 1024 * 1024;

pub(crate) fn analyze(
    path: &Path,
    selection: DiscSelection,
    options: &ProbeOptions,
) -> Result<AnalysisDetails, ProbeError> {
    let started = Instant::now();
    let mut source = BoundedSource::new(
        FileSource::open(path)?,
        options
            .read_budget_bytes()
            .unwrap_or(ProbeOptions::DISC_READ_BUDGET_BYTES),
    )
    .with_io_limit(options.io_limit().unwrap_or(ProbeOptions::DISC_IO_LIMIT));
    let result = inspect(&mut source, selection, options.depth());
    let mut details = match result {
        Ok(details) => details,
        Err(error) => AnalysisDetails {
            revision: ANALYSIS_REVISION,
            container_format: Some("iso".into()),
            report: error_report(&error),
            ..Default::default()
        },
    };
    details.report.bytes_read = source.bytes_read;
    details.report.seeks = source.seeks;
    details.report.elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    details.report.budget_exhausted |= source.exhausted;
    if source.exhausted {
        details.report.status = ProbeStatus::Incomplete;
    }
    if !source.get_ref().unchanged(path)? {
        return Err(ProbeError::SourceChanged);
    }
    Ok(details)
}

pub(crate) fn diagnose_source(
    source: &mut dyn MediaSource,
    selection: DiscSelection,
) -> ProbeReport {
    match inspect(source, selection, ProbeDepth::Full) {
        Ok(details) => details.report,
        Err(error) => error_report(&error),
    }
}

fn error_report(error: &ImageError) -> ProbeReport {
    let mut report = ProbeReport {
        status: match error {
            ImageError::Malformed(_) => ProbeStatus::Malformed,
            ImageError::Unsupported(_) => ProbeStatus::Unsupported,
            ImageError::Budget | ImageError::Io(_) => ProbeStatus::Incomplete,
        },
        budget_exhausted: matches!(error, ImageError::Budget),
        ..Default::default()
    };
    warning(&mut report, warning::DISC_INSPECTION, &error.to_string());
    report
}
fn read_navigation(
    source: &mut dyn MediaSource,
    file: &ImageFile,
    remaining: &mut u64,
) -> Result<Vec<u8>, ImageError> {
    if file.length > 4 * 1024 * 1024 || file.length > *remaining {
        return Err(ImageError::Budget);
    }
    *remaining -= file.length;
    image::read_file(source, file)
}
fn inspect(
    source: &mut dyn MediaSource,
    selection: DiscSelection,
    profile: ProbeDepth,
) -> Result<AnalysisDetails, ImageError> {
    let image = image::open(source)?;
    let bluray = image
        .files
        .keys()
        .any(|path| path.starts_with("BDMV/PLAYLIST/") && path.ends_with(".MPLS"));
    let dvd = image.files.contains_key("VIDEO_TS/VIDEO_TS.IFO");
    let mut disc = DiscMetadata {
        disc_type: if bluray {
            "bluray"
        } else if dvd {
            "dvd"
        } else {
            "unknown"
        }
        .into(),
        filesystem: image.filesystem.clone(),
        volume_label: image.volume_label.clone(),
        selection,
        ..Default::default()
    };
    let mut navigation_budget = NAVIGATION_BUDGET;
    let mut title_files: BTreeMap<String, Vec<(String, ImageFile)>> = BTreeMap::new();
    if bluray {
        let mut clip_information =
            BTreeMap::<String, std::result::Result<clip_info::ClipInfo, ProbeReport>>::new();
        let mut segment_budget = 65_536_usize;
        let mut stream_inventory_budget = 32_768_usize;
        let mut clip_inventory_budget = 32_768_usize;
        let mut clip_point_budget = 262_144_usize;
        if let Some(index) = image.files.get("BDMV/INDEX.BDMV") {
            let index = read_navigation(source, index, &mut navigation_budget)?;
            if index.get(..8) == Some(b"INDX0300") {
                disc.disc_type = "uhd_bluray".into();
            }
        }
        for (path, file) in &image.files {
            if !path.starts_with("BDMV/PLAYLIST/") || !path.ends_with(".MPLS") {
                continue;
            }
            if disc.titles.len() >= 2048 {
                return Err(ImageError::Budget);
            }
            let id = path
                .trim_start_matches("BDMV/PLAYLIST/")
                .trim_end_matches(".MPLS")
                .to_string();
            if id.len() != 5 || !id.bytes().all(|byte| byte.is_ascii_digit()) {
                continue;
            }
            let title = read_navigation(source, file, &mut navigation_budget)
                .and_then(|bytes| navigation::playlist(&bytes, id.clone()));
            match title {
                Ok(mut title) => {
                    segment_budget = segment_budget
                        .checked_sub(title.segments.len())
                        .ok_or(ImageError::Budget)?;
                    let mut files = Vec::new();
                    for segment in &title.segments {
                        if let Some(file) = image.files.get(&segment.path) {
                            let mut referenced = file.clone();
                            let mut cache_key = segment.path.clone();
                            let info_path = segment
                                .path
                                .replace("BDMV/STREAM/", "BDMV/CLIPINF/")
                                .replace(".M2TS", ".CLPI");
                            if let Some(info_file) = image.files.get(&info_path) {
                                if !clip_information.contains_key(&info_path) {
                                    let info =
                                        read_navigation(source, info_file, &mut navigation_budget)
                                            .and_then(|bytes| clip_info::parse(&bytes))
                                            .map_err(|error| error_report(&error));
                                    if let Ok(info) = &info {
                                        clip_inventory_budget = clip_inventory_budget
                                            .checked_sub(info.stream_count())
                                            .ok_or(ImageError::Budget)?;
                                        clip_point_budget = clip_point_budget
                                            .checked_sub(info.point_count())
                                            .ok_or(ImageError::Budget)?;
                                    }
                                    clip_information.insert(info_path.clone(), info);
                                }
                                let declared = match &clip_information[&info_path] {
                                    Ok(info)
                                        if u64::from(info.source_packets) * 192 <= file.length =>
                                    {
                                        (|| {
                                            let range = info.packet_range(segment)?;
                                            let streams = info.streams_in(&range)?;
                                            let length = u64::from(range.end - range.start) * 192;
                                            referenced = ImageFile {
                                                length,
                                                extents: image::range(
                                                    &file.extents,
                                                    u64::from(range.start) * 192,
                                                    length,
                                                )?,
                                            };
                                            cache_key = format!(
                                                "{}#{}-{}",
                                                segment.path, range.start, range.end
                                            );
                                            Ok(streams)
                                        })()
                                        .map_err(|error| error_report(&error))
                                    }
                                    Ok(_) => Err(error_report(&ImageError::Malformed(
                                        "CLPI source packet count exceeds referenced media extent",
                                    ))),
                                    Err(report) => Err(report.clone()),
                                };
                                match declared {
                                    Ok(streams) if title.streams.is_empty() => {
                                        title.streams = streams
                                    }
                                    Ok(streams) if title.streams != streams => {
                                        title.report.status = ProbeStatus::Unsupported;
                                        warning(
                                            &mut title.report,
                                            warning::CLIP_DECLARATION_CHANGE,
                                            "CLPI declarations differ across referenced playback segments",
                                        );
                                    }
                                    Ok(_) => {}
                                    Err(report) => {
                                        if title.report.status != ProbeStatus::Malformed {
                                            title.report.status = report.status;
                                        }
                                        title.report.budget_exhausted |= report.budget_exhausted;
                                        title.report.warnings.extend(report.warnings);
                                    }
                                }
                            } else {
                                if !matches!(
                                    title.report.status,
                                    ProbeStatus::Malformed | ProbeStatus::Unsupported
                                ) {
                                    title.report.status = ProbeStatus::Incomplete;
                                }
                                warning(
                                    &mut title.report,
                                    warning::MISSING_CLIP_INFORMATION,
                                    "CLPI is missing; the selected clock sequence and trim coverage could not be verified",
                                );
                            }
                            files.push((cache_key, referenced));
                        } else {
                            title.report.status = ProbeStatus::Malformed;
                            warning(
                                &mut title.report,
                                warning::MISSING_CLIP,
                                &format!("Referenced clip {} is missing", segment.path),
                            );
                        }
                    }
                    stream_inventory_budget = stream_inventory_budget
                        .checked_sub(title.streams.len())
                        .ok_or(ImageError::Budget)?;
                    title_files.insert(id, files);
                    disc.titles.push(title);
                }
                Err(error) => disc.titles.push(DiscTitle {
                    id,
                    report: error_report(&error),
                    ..Default::default()
                }),
            }
        }
    } else if dvd {
        let vmg = read_navigation(
            source,
            &image.files["VIDEO_TS/VIDEO_TS.IFO"],
            &mut navigation_budget,
        )?;
        let mut vts_cache = BTreeMap::new();
        for reference in navigation::dvd_titles(&vmg)? {
            let title_result = (|| {
                let path = format!("VIDEO_TS/VTS_{:02}_0.IFO", reference.set);
                if let std::collections::btree_map::Entry::Vacant(e) =
                    vts_cache.entry(reference.set)
                {
                    let file = image
                        .files
                        .get(&path)
                        .ok_or(ImageError::Malformed("missing DVD title set IFO"))?;
                    e.insert(read_navigation(source, file, &mut navigation_budget)?);
                }
                let (title, cells) = navigation::dvd_title(&vts_cache[&reference.set], &reference)?;
                let extents = dvd_vob_extents(&image, reference.set)?;
                let mut files = Vec::new();
                for (cell, segment) in cells.iter().zip(&title.segments) {
                    let offset = u64::from(cell.first_sector) * 2048;
                    let length =
                        (u64::from(cell.last_sector) - u64::from(cell.first_sector) + 1) * 2048;
                    files.push((
                        segment.path.clone(),
                        ImageFile {
                            length,
                            extents: image::range(&extents, offset, length)?,
                        },
                    ));
                }
                title_files.insert(reference.id.clone(), files);
                Ok::<_, ImageError>(title)
            })();
            disc.titles
                .push(title_result.unwrap_or_else(|error| DiscTitle {
                    id: reference.id,
                    report: error_report(&error),
                    ..Default::default()
                }));
        }
    } else {
        return Err(ImageError::Unsupported(
            "image contains no recognized BDMV or DVD title structure",
        ));
    }

    deduplicate_titles(&mut disc.titles);
    select_title(&mut disc);
    let mut report = ProbeReport {
        status: ProbeStatus::Incomplete,
        ..Default::default()
    };
    if disc.selected_title_id.is_none() {
        warning(
            &mut report,
            warning::DISC_SELECTION_REVIEW,
            "No valid title is selected; a missing saved selection requires review",
        );
    }
    let mut cache: BTreeMap<String, AnalysisDetails> = BTreeMap::new();
    let mut failed_clips: BTreeMap<String, (ProbeStatus, String)> = BTreeMap::new();
    let mut selected_analysis = None;
    let mut resolved_title_id = None;
    let mapped = mapped_titles(&disc);
    // Probe the chosen title first so other titles cannot consume its budget.
    let mut order: Vec<usize> = (0..disc.titles.len()).collect();
    order.sort_by_key(|index| {
        (
            disc.selected_title_id.as_ref() != Some(&disc.titles[*index].id),
            !mapped[*index],
        )
    });
    for index in order {
        let title = &mut disc.titles[index];
        if matches!(
            title.report.status,
            ProbeStatus::Malformed | ProbeStatus::Unsupported | ProbeStatus::Encrypted
        ) {
            if disc.selected_title_id.as_ref() == Some(&title.id) {
                report = title.report.clone();
            }
            continue;
        }
        let mut first: Option<AnalysisDetails> = None;
        let mapped = mapped[index];
        let could_replace_selection = disc.automatic_selection
            && selected_analysis
                .as_ref()
                .is_none_or(|selected: &AnalysisDetails| {
                    title.duration_seconds.unwrap_or(0.0) > selected.duration_seconds.unwrap_or(0.0)
                        || (title.duration_seconds == selected.duration_seconds
                            && numeric_id(&title.id)
                                < resolved_title_id
                                    .as_deref()
                                    .map(numeric_id)
                                    .unwrap_or(u32::MAX))
                });
        if !mapped && disc.selected_title_id.as_ref() != Some(&title.id) && !could_replace_selection
        {
            title.report.status = ProbeStatus::Incomplete;
            warning(
                &mut title.report,
                warning::DISC_TITLE_NOT_SAMPLED,
                "Navigation inventoried; media inspection is deferred until this title is selected or mapped",
            );
            continue;
        }
        for (key, file) in title_files.get(&title.id).into_iter().flatten() {
            if let Some((status, message)) = failed_clips.get(key) {
                title.report.status = *status;
                warning(&mut title.report, warning::CLIP_PROBE_FAILED, message);
                continue;
            }
            if !cache.contains_key(key) {
                if cache.len() + failed_clips.len() >= 64 {
                    title.report.budget_exhausted = true;
                    warning(
                        &mut title.report,
                        warning::CLIP_BUDGET,
                        "Clip inspection budget exhausted",
                    );
                    break;
                }
                let probe = (|| {
                    let mut clip = ExtentSource::new(source, file.extents.clone())?;
                    if encrypted(&mut clip, bluray)? {
                        return Ok(ClipProbe::Encrypted);
                    }
                    clip.seek(SeekFrom::Start(0))?;
                    // Clips keep the file budgets, whatever the disc's are.
                    crate::probe_bounded(
                        &mut clip,
                        if bluray { "m2ts" } else { "vob" },
                        &ProbeOptions::new().with_depth(profile),
                    )
                    .map(|details| ClipProbe::Analyzed(Box::new(details)))
                })();
                let failure = match probe {
                    Ok(ClipProbe::Analyzed(details)) => {
                        cache.insert(key.clone(), *details);
                        None
                    }
                    Ok(ClipProbe::Encrypted) => Some((
                        ProbeStatus::Encrypted,
                        ProbeError::Unsupported("encrypted disc payload".into()),
                    )),
                    Err(error) => Some((ProbeStatus::Incomplete, error)),
                };
                if let Some((status, error)) = failure {
                    title.report.status = status;
                    failed_clips.insert(key.clone(), (status, error.to_string()));
                    warning(
                        &mut title.report,
                        warning::CLIP_PROBE_FAILED,
                        &error.to_string(),
                    );
                    continue;
                }
            }
            let analysis = &cache[key];
            if analysis.report.budget_exhausted {
                title.report.budget_exhausted = true;
                warning(
                    &mut title.report,
                    warning::CLIP_ENRICHMENT_BUDGET,
                    &format!("Referenced clip {key} exhausted its analysis budget"),
                );
            }
            if matches!(
                analysis.report.status,
                ProbeStatus::Unsupported | ProbeStatus::Encrypted | ProbeStatus::Malformed
            ) {
                title.report.status = analysis.report.status;
                warning(
                    &mut title.report,
                    warning::CLIP_ANALYSIS_INCOMPLETE,
                    &format!(
                        "Referenced clip {key} reported {:?}",
                        analysis.report.status
                    ),
                );
            }
            if let Some(first) = &first {
                if !same_stream_formats(first, analysis) {
                    title.report.status = ProbeStatus::Unsupported;
                    warning(
                        &mut title.report,
                        warning::SEGMENT_FORMAT_CHANGE,
                        "Referenced segments have materially different stream formats",
                    );
                }
            } else {
                first = Some(analysis.clone());
            }
        }
        if let Some(mut analysis) = first {
            streams::apply_navigation(&mut analysis, &title.streams);
            if title.report.warnings.is_empty()
                && analysis
                    .selected_video()
                    .is_some_and(|video| video.codec.is_some())
            {
                title.report.status = ProbeStatus::Complete;
            }
            for warning in &analysis.report.warnings {
                if !title.report.warnings.contains(warning) {
                    title.report.warnings.push(warning.clone());
                }
            }
            let better_automatic_choice = disc.automatic_selection
                && selected_analysis
                    .as_ref()
                    .is_none_or(|previous: &AnalysisDetails| {
                        title.duration_seconds.unwrap_or(0.0)
                            > previous.duration_seconds.unwrap_or(0.0)
                            || (title.duration_seconds == previous.duration_seconds
                                && numeric_id(&title.id)
                                    < resolved_title_id
                                        .as_deref()
                                        .map(numeric_id)
                                        .unwrap_or(u32::MAX))
                    });
            if (better_automatic_choice
                || (!disc.automatic_selection
                    && disc.selected_title_id.as_ref() == Some(&title.id)))
                && title.report.status == ProbeStatus::Complete
            {
                analysis.duration_seconds = title.duration_seconds;
                analysis.duration_provenance = Provenance::Container;
                analysis.chapters = title.chapters.clone();
                analysis.chapter_count = u32::try_from(title.chapters.len()).ok();
                // Whole-image storage rate is unrelated to this title's video bitrate.
                analysis.overall_bitrate_bps = None;
                analysis.report = title.report.clone();
                // Only the chosen title needs its streams twice.
                title.streams = analysis.streams.clone();
                selected_analysis = Some(analysis);
                resolved_title_id = Some(title.id.clone());
            } else {
                title.streams = analysis.streams;
            }
        }
        if disc.selected_title_id.as_ref() == Some(&title.id) {
            report = title.report.clone();
        }
    }
    if disc.automatic_selection {
        disc.selected_title_id = resolved_title_id;
    }
    if let Some(selected) = &selected_analysis {
        report = selected.report.clone();
    }
    let titles_by_id = titles_by_id(&disc.titles);
    for mapping in &disc.selection.episode_mappings {
        if titles_by_id
            .get(mapping.disc_title_id.as_str())
            .is_none_or(|index| disc.titles[*index].report.status != ProbeStatus::Complete)
        {
            if report.status == ProbeStatus::Complete {
                report.status = ProbeStatus::Incomplete;
            }
            warning(
                &mut report,
                warning::DISC_EPISODE_MAPPING_REVIEW,
                &format!(
                    "Mapped playback title {} is missing or could not be inspected; review the saved episode mapping",
                    mapping.disc_title_id
                ),
            );
        }
    }
    let mut analysis = selected_analysis.unwrap_or_default();
    analysis.container_format = Some("iso".into());
    analysis.revision = ANALYSIS_REVISION;
    analysis.disc = Some(disc);
    analysis.report = report;
    Ok(analysis)
}

/// One clip's probe outcome; an encrypted payload is detected before parsing.
enum ClipProbe {
    Analyzed(Box<AnalysisDetails>),
    Encrypted,
}
fn dvd_vob_extents(image: &Image, set: u8) -> Result<Vec<Extent>, ImageError> {
    let mut extents = Vec::new();
    let mut missing = false;
    for index in 1..=9 {
        if let Some(file) = image
            .files
            .get(&format!("VIDEO_TS/VTS_{set:02}_{index}.VOB"))
        {
            if missing || file.length % 2048 != 0 {
                return Err(ImageError::Malformed("DVD VOB file sequence or size"));
            }
            extents.extend(file.extents.iter().copied());
        } else {
            missing = true;
        }
    }
    Ok(extents)
}
fn encrypted(source: &mut dyn MediaSource, bluray: bool) -> Result<bool, ProbeError> {
    let length = source.len().min(64 * 1024) as usize;
    let mut bytes = vec![0; length];
    source.read_exact(&mut bytes)?;
    if bluray {
        for packet in bytes.as_chunks::<192>().0 {
            if packet[4] == 0x47 && packet[7] & 0xc0 != 0 {
                return Ok(true);
            }
        }
    } else {
        for at in 0..bytes.len().saturating_sub(7) {
            if bytes[at..at + 3] == [0, 0, 1]
                && matches!(bytes[at + 3], 0xbd | 0xc0..=0xef)
                && bytes[at + 6] & 0xb0 == 0xb0
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
fn same_stream_formats(a: &AnalysisDetails, b: &AnalysisDetails) -> bool {
    a.streams.len() == b.streams.len()
        && a.streams.iter().zip(&b.streams).all(|(a, b)| {
            a.kind == b.kind
                && a.codec == b.codec
                && a.width == b.width
                && a.height == b.height
                && a.channels == b.channels
                && a.language == b.language
                && a.metadata.profile == b.metadata.profile
                && a.metadata.level == b.metadata.level
                && a.metadata.bit_depth == b.metadata.bit_depth
                && a.metadata.pixel_format == b.metadata.pixel_format
                && a.metadata.field_order == b.metadata.field_order
                && a.metadata.display_aspect_ratio == b.metadata.display_aspect_ratio
                && a.metadata.sample_aspect_ratio == b.metadata.sample_aspect_ratio
                && a.metadata.rotation_degrees == b.metadata.rotation_degrees
                && a.metadata.declared_frame_rate == b.metadata.declared_frame_rate
                && a.metadata.sample_rate == b.metadata.sample_rate
                && a.metadata.sample_format == b.metadata.sample_format
                && a.metadata.sample_bit_depth == b.metadata.sample_bit_depth
                && a.metadata.channel_layout == b.metadata.channel_layout
                && a.metadata.hdr == b.metadata.hdr
                && a.metadata.color.primaries == b.metadata.color.primaries
                && a.metadata.color.transfer == b.metadata.color.transfer
                && a.metadata.color.matrix == b.metadata.color.matrix
                && a.metadata.color.full_range == b.metadata.color.full_range
        })
}
fn numeric_id(id: &str) -> u32 {
    id.parse().unwrap_or(u32::MAX)
}
/// Each id and alias, mapped to the first title that answers to it.
fn titles_by_id(titles: &[DiscTitle]) -> HashMap<&str, usize> {
    let mut by_id = HashMap::new();
    for (index, title) in titles.iter().enumerate() {
        for id in std::iter::once(&title.id).chain(&title.aliases) {
            by_id.entry(id.as_str()).or_insert(index);
        }
    }
    by_id
}
/// Whether a saved episode mapping names each title or one of its aliases.
fn mapped_titles(disc: &DiscMetadata) -> Vec<bool> {
    let mapped: HashSet<&str> = disc
        .selection
        .episode_mappings
        .iter()
        .map(|mapping| mapping.disc_title_id.as_str())
        .collect();
    disc.titles
        .iter()
        .map(|title| {
            std::iter::once(&title.id)
                .chain(&title.aliases)
                .any(|id| mapped.contains(id.as_str()))
        })
        .collect()
}
/// A playback timeline that hashes the way `DiscSegment` compares: `-0.0`
/// equals `0.0`. Timelines holding NaN equal nothing, so they have no key.
struct Timeline<'a>(&'a [DiscSegment]);
impl<'a> Timeline<'a> {
    fn new(segments: &'a [DiscSegment]) -> Option<Self> {
        (!segments.is_empty()
            && segments
                .iter()
                .all(|segment| !segment.in_seconds.is_nan() && !segment.out_seconds.is_nan()))
        .then_some(Self(segments))
    }
}
impl PartialEq for Timeline<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for Timeline<'_> {}
impl Hash for Timeline<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.len().hash(state);
        for segment in self.0 {
            segment.path.hash(state);
            // Adding zero turns -0.0 into 0.0 and leaves every other value.
            (segment.in_seconds + 0.0).to_bits().hash(state);
            (segment.out_seconds + 0.0).to_bits().hash(state);
            segment.angle.hash(state);
            segment.sequence_id.hash(state);
        }
    }
}
fn deduplicate_titles(titles: &mut Vec<DiscTitle>) {
    titles.sort_by_key(|title| numeric_id(&title.id));
    // The first title with each timeline keeps it; later ones become its aliases.
    let mut first_with: HashMap<Timeline<'_>, usize> = HashMap::with_capacity(titles.len());
    let keeper: Vec<usize> = titles
        .iter()
        .enumerate()
        .map(|(index, title)| {
            Timeline::new(&title.segments).map_or(index, |timeline| {
                *first_with.entry(timeline).or_insert(index)
            })
        })
        .collect();
    drop(first_with);
    let mut position = vec![0; titles.len()];
    let mut distinct: Vec<DiscTitle> = Vec::new();
    for (index, title) in titles.drain(..).enumerate() {
        if keeper[index] == index {
            position[index] = distinct.len();
            distinct.push(title);
        } else {
            distinct[position[keeper[index]]].aliases.push(title.id);
        }
    }
    *titles = distinct;
}
fn select_title(disc: &mut DiscMetadata) {
    disc.automatic_selection = disc.selection.title_id.is_none();
    disc.selected_title_id = if let Some(id) = &disc.selection.title_id {
        disc.titles
            .iter()
            .find(|title| &title.id == id || title.aliases.contains(id))
            .map(|title| title.id.clone())
    } else {
        disc.titles
            .iter()
            .filter(|title| {
                !matches!(
                    title.report.status,
                    ProbeStatus::Malformed | ProbeStatus::Unsupported | ProbeStatus::Encrypted
                ) && title
                    .duration_seconds
                    .is_some_and(|duration| duration.is_finite() && duration > 0.0)
            })
            .max_by(|a, b| {
                a.duration_seconds
                    .unwrap()
                    .total_cmp(&b.duration_seconds.unwrap())
                    .then_with(|| numeric_id(&b.id).cmp(&numeric_id(&a.id)))
            })
            .map(|title| title.id.clone())
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DiscSegment;
    fn title(id: &str, seconds: f64, clip: &str) -> DiscTitle {
        DiscTitle {
            id: id.into(),
            duration_seconds: Some(seconds),
            segments: vec![DiscSegment {
                path: clip.into(),
                out_seconds: seconds,
                ..Default::default()
            }],
            ..Default::default()
        }
    }
    #[test]
    fn segment_compatibility_preserves_layout_hdr_and_encoded_precision() {
        let mut first = AnalysisDetails::default();
        first.streams.push(crate::StreamDetail {
            kind: crate::StreamKind::Audio,
            codec: Some("pcm_bluray".into()),
            channels: Some(6),
            metadata: crate::StreamMetadata {
                channel_layout: Some("5.1".into()),
                sample_bit_depth: Some(24),
                ..Default::default()
            },
            ..Default::default()
        });
        let mut other = first.clone();
        other.streams[0].metadata.channel_layout = Some("6.0".into());
        assert!(!same_stream_formats(&first, &other));
        other = first.clone();
        other.streams[0].metadata.sample_bit_depth = Some(16);
        assert!(!same_stream_formats(&first, &other));
        first.streams.push(crate::StreamDetail {
            codec: Some("hevc".into()),
            ..Default::default()
        });
        other = first.clone();
        other.streams[1].metadata.hdr.dolby_vision = Some(true);
        assert!(!same_stream_formats(&first, &other));
        other = first.clone();
        other.streams[1].metadata.color.transfer = Some(16);
        assert!(!same_stream_formats(&first, &other));
        other = first.clone();
        other.streams[1].metadata.bitrate_bps = Some(42_000_000);
        other.streams[1].metadata.duration_seconds = Some(10.0);
        assert!(
            same_stream_formats(&first, &other),
            "clip duration and variable bitrate do not change stream formats"
        );
    }
    /// The pairwise form deduplication used before timelines were hashed.
    fn deduplicate_pairwise(titles: &mut Vec<DiscTitle>) {
        titles.sort_by_key(|title| numeric_id(&title.id));
        let mut distinct: Vec<DiscTitle> = Vec::new();
        for title in titles.drain(..) {
            if !title.segments.is_empty()
                && let Some(previous) = distinct
                    .iter_mut()
                    .find(|previous| previous.segments == title.segments)
            {
                previous.aliases.push(title.id);
            } else {
                distinct.push(title);
            }
        }
        *titles = distinct;
    }
    fn generated_titles(seed: u32, count: usize) -> Vec<DiscTitle> {
        let mut state = seed;
        let mut next = |bound: u32| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state % bound
        };
        let seconds = [0.0, -0.0, 1.5, 2.0, f64::NAN, 600.25];
        (0..count)
            .map(|index| {
                let segments = (0..next(4))
                    .map(|_| DiscSegment {
                        path: format!("BDMV/STREAM/{:05}.M2TS", next(3)),
                        in_seconds: seconds[next(seconds.len() as u32) as usize],
                        out_seconds: seconds[next(seconds.len() as u32) as usize],
                        angle: next(2),
                        sequence_id: [None, Some(0), Some(1)][next(3) as usize],
                    })
                    .collect();
                DiscTitle {
                    // Repeated and non-numeric ids exercise the stable sort.
                    id: match next(10) {
                        0 => format!("x{index}"),
                        1 => format!("{:05}", next(8)),
                        _ => format!("{:05}", index),
                    },
                    segments,
                    ..Default::default()
                }
            })
            .collect()
    }
    fn identities(titles: &[DiscTitle]) -> Vec<(String, Vec<String>)> {
        titles
            .iter()
            .map(|title| (title.id.clone(), title.aliases.clone()))
            .collect()
    }
    #[test]
    fn hashed_deduplication_matches_pairwise_comparison() {
        for seed in 1..=300_u32 {
            let titles = generated_titles(seed.wrapping_mul(0x9e37_79b9), 1 + seed as usize % 60);
            let mut hashed = titles.clone();
            let mut pairwise = titles;
            deduplicate_titles(&mut hashed);
            deduplicate_pairwise(&mut pairwise);
            assert_eq!(identities(&hashed), identities(&pairwise), "seed {seed}");
        }
    }
    #[test]
    fn mapping_lookups_match_a_scan_of_ids_and_aliases() {
        let mut titles = generated_titles(0x51ed_270b, 200);
        deduplicate_titles(&mut titles);
        let mut disc = DiscMetadata {
            titles,
            ..Default::default()
        };
        let mut ids: Vec<String> = (0..20).map(|index| format!("{index:05}")).collect();
        ids.extend(["x3".into(), "missing".into(), "00001".into()]);
        disc.selection.episode_mappings = ids
            .iter()
            .map(|id| crate::DiscEpisodeMapping {
                disc_title_id: id.clone(),
                ..Default::default()
            })
            .collect();
        let answers =
            |title: &DiscTitle, id: &String| &title.id == id || title.aliases.contains(id);
        let mapped = mapped_titles(&disc);
        for (index, title) in disc.titles.iter().enumerate() {
            let scanned = disc
                .selection
                .episode_mappings
                .iter()
                .any(|mapping| answers(title, &mapping.disc_title_id));
            assert_eq!(mapped[index], scanned);
        }
        let by_id = titles_by_id(&disc.titles);
        for id in &ids {
            assert_eq!(
                by_id.get(id.as_str()).copied(),
                disc.titles.iter().position(|title| answers(title, id)),
                "{id}"
            );
        }
    }
    #[test]
    fn selection_deduplicates_timelines_and_uses_numeric_ties() {
        let mut disc = DiscMetadata {
            titles: vec![
                title("10", 100.0, "a"),
                title("2", 100.0, "b"),
                title("3", 100.0, "a"),
            ],
            ..Default::default()
        };
        deduplicate_titles(&mut disc.titles);
        assert_eq!(disc.titles.len(), 2);
        assert_eq!(disc.titles[1].aliases, ["10"]);
        select_title(&mut disc);
        assert_eq!(disc.selected_title_id.as_deref(), Some("2"));
        disc.selection.title_id = Some("10".into());
        select_title(&mut disc);
        assert_eq!(disc.selected_title_id.as_deref(), Some("3"));
        disc.selection.title_id = Some("99".into());
        select_title(&mut disc);
        assert!(disc.selected_title_id.is_none());
        assert!(!disc.automatic_selection);
    }
}
