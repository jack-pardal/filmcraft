//! Importing media and placing project items on the timeline.

use super::*;

/// Sequence settings matching a media clip (New Sequence From Clip, New Sequence from an item):
/// its frame size, rate and pixel aspect ratio (Interpret Footage's, else the file's), so a
/// 1440 x 1080 clip with 4:3 pixels makes a 1440 x 1080 sequence that displays 16:9.
pub(crate) fn default_seq_settings_for(media: &MediaClip) -> SequenceSettings {
    let info = &media.info;
    let mut st = SequenceSettings::default();
    if let Some(v) = &info.video {
        st.width = v.width;
        st.height = v.height;
        st.frame_rate = v.frame_rate;
        st.par = media.pixel_aspect();
        st.preset = format!("{}x{} {}", v.width, v.height, v.frame_rate.label());
    }
    if let Some(a) = info.audio() {
        st.sample_rate = a.sample_rate.max(8000);
    }
    st
}

/// Import bytes as a media item (engine-level; the UI reads files through services).
pub fn import_bytes(s: &mut Session, path: &str, bytes: std::sync::Arc<[u8]>, bin: Option<filmcraft_project::BinId>) -> Result<ItemId> {
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
    let identity = crate::relink::identity_of_bytes(&bytes);
    let src = s.media.open_bytes(&name, bytes)?;
    import_source(s, path, name, src, identity, bin)
}

pub(super) fn import_source(
    s: &mut Session,
    path: &str,
    name: String,
    src: filmcraft_media::SharedSource,
    identity: filmcraft_project::MediaIdentity,
    bin: Option<filmcraft_project::BinId>,
) -> Result<ItemId> {
    let mut info = src.info().clone();
    // Settings ▸ Labels ▸ Label Defaults
    let label = s.prefs.labels.for_media(info.kind, info.has_video(), info.has_audio());
    // Settings ▸ Media ▸ Indeterminate Media Timebase: the frame rate stills report
    if info.kind == filmcraft_media::MediaKind::Still
        && let Some(v) = info.video.as_mut()
    {
        v.frame_rate = crate::settings::timebase_rate(&s.prefs.media.indeterminate_timebase);
    }
    let path_s = path.to_string();
    let id = s.edit(&format!("Import {name}"), |p, _| {
        Ok(p.add_item(
            &name,
            label,
            ItemKind::Media(MediaClip {
                media: MediaRef::File { path: path_s },
                info,
                interpret: Default::default(),
                mark_in: None,
                mark_out: None,
                markers: vec![],
                offline: false,
                proxy: None,
                identity: Some(identity),
            }),
            bin,
        ))
    })?;
    s.media.insert_file(id, path, src);
    Ok(id)
}

/// The audio clips to place for a source whose Modify ▸ Audio Channels map has `clips` (one entry
/// per clip): always at least one (on the destination track), then one per further entry on the
/// tracks below. The count is capped at [`crate::sequence_tools::MAX_TRACKS`] (the map comes from
/// project files and scripts: never trusted).
pub(crate) fn audio_placement_specs(clips: &[Vec<u16>], streams: usize) -> Vec<AudioPlacementSpec> {
    let mut specs = vec![AudioPlacementSpec { track_offset: 0, audio_stream: 0, source_channels: None }];
    let extra = clips.iter().skip(1).take(crate::sequence_tools::MAX_TRACKS.saturating_sub(1));
    specs.extend(extra.enumerate().map(|(k, chans)| AudioPlacementSpec {
        track_offset: k.saturating_add(1),
        audio_stream: 0,
        source_channels: Some(chans.clone()),
    }));
    // Interpret Footage's channel map describes stream 0; further container streams keep their
    // own channel layouts and get one clip each, after any channel-map clips.
    for stream in 1..streams.min(filmcraft_media::MAX_AUDIO_STREAMS) {
        if specs.len() >= crate::sequence_tools::MAX_TRACKS {
            break;
        }
        specs.push(AudioPlacementSpec { track_offset: specs.len(), audio_stream: stream, source_channels: None });
    }
    specs
}

/// Make sure the sequence has audio tracks for `count` clips starting at track index `first`,
/// adding tracks at the bottom when it has fewer. A new track is a copy of the destination track
/// (channel format, volume, pan, inserts) without its clips, transitions, lock / mute / solo
/// state or mixer routing. Returns the ids of all the sequence's audio tracks, in order.
pub(crate) fn ensure_audio_tracks(p: &mut Project, seq_id: ItemId, first: usize, count: usize, label: &str) -> Result<Vec<TrackId>> {
    let (have, template) = {
        let q = p.sequence(seq_id).ok_or(EngineError::NoSequence)?;
        (q.audio_tracks.len(), q.audio_tracks.get(first).or(q.audio_tracks.last()).cloned())
    };
    let need = first.saturating_add(count);
    if need > crate::sequence_tools::MAX_TRACKS {
        return Err(bad(label, format!("a sequence has at most {} audio tracks", crate::sequence_tools::MAX_TRACKS)));
    }
    if need > have {
        let ids: Vec<(usize, TrackId)> = (have..need).map(|k| (k, TrackId(p.alloc_id()))).collect();
        let q = p.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        for (k, id) in ids {
            let mut t = template.clone().unwrap_or_else(|| Track::new(id, TrackKind::Audio, String::new()));
            t.id = id;
            t.name = format!("Audio {}", k + 1);
            t.items.clear();
            t.transitions.clear();
            t.locked = false;
            t.muted = false;
            t.solo = false;
            t.mixer = Default::default();
            q.audio_tracks.push(t);
        }
    }
    Ok(p.sequence(seq_id).map(|q| q.audio_tracks.iter().map(|t| t.id).collect()).unwrap_or_default())
}

/// `timeline.place` for one item (`item`).
pub(super) fn place_one(s: &mut Session, p: &Value) -> Result<Value> {
    if crate::sequence_tools::wants_new_track(p) {
        return crate::sequence_tools::place_on_new_tracks(s, p);
    }
    let stream_flag = |key: &str| match p.get(key) {
        None => Ok(true),
        Some(Value::Bool(v)) => Ok(*v),
        _ => Err(bad("timeline.place", format!("`{key}` must be a boolean"))),
    };
    let (video, audio) = (stream_flag("video")?, stream_flag("audio")?);
    if !video && !audio {
        return Err(bad("timeline.place", "enable video, audio, or both"));
    }
    let item = item_p(p, "item").ok_or_else(|| bad("timeline.place", "need `item`"))?;
    let at = sequence_time_p(s, p, "", "timeline.place")?.unwrap_or(s.playhead());
    let tg = s.targeting();
    let pi = s.project.item(item).ok_or_else(|| bad("timeline.place", "no such item"))?;
    let (has_video, has_audio) = s.project.source_streams(item);
    let is_audio = |t: TrackId| s.active_sequence().is_some_and(|q| q.audio_tracks.iter().any(|x| x.id == t));
    // `track` is where the picture goes and `audioTrack` where the sound goes. A caller that
    // names an audio track as `track` asks for the sound alone, on that track.
    let (v, a) = match (track_p(s, p, "track", "timeline.place")?, track_p(s, p, "audioTrack", "timeline.place")?) {
        (Some(t), Some(_)) if is_audio(t) => {
            return Err(bad("timeline.place", "`track` names an audio track and `audioTrack` is given as well: name the sound's track once"));
        }
        (_, Some(t)) if !is_audio(t) => return Err(bad("timeline.place", "`audioTrack` names a video track")),
        (Some(t), None) if is_audio(t) && !has_audio => {
            return Err(bad("timeline.place", "`track` names an audio track and this item has no sound"));
        }
        (Some(t), None) if is_audio(t) => (None, Some(t)),
        (Some(_), None) if !has_video => {
            return Err(bad("timeline.place", "`track` names a video track and this item has no picture: name an audio track"));
        }
        (v, a) => (v.or(tg.video_dest), a.or(tg.audio_dest)),
    };
    let v = v.filter(|_| video);
    let a = a.filter(|_| audio);
    if p.get("video").is_some() && video && has_video && v.is_none() {
        return Err(bad("timeline.place", "no video destination: add or enable a video track"));
    }
    if p.get("audio").is_some() && audio && has_audio && a.is_none() {
        return Err(bad("timeline.place", "no audio destination: add or enable an audio track"));
    }
    let full = full_duration(s, pi);
    let (mi, mo) = match &pi.kind {
        ItemKind::Media(m) => (m.mark_in, m.mark_out.map(|o| o + pi.frame_rate().frame_duration())),
        ItemKind::Subclip { range, .. } => (Some(range.start), Some(range.end())),
        _ => (None, None),
    };
    let sin = p.get("sourceIn").and_then(Value::as_i64).map(Tick).or(mi).unwrap_or_default();
    let dur = p.get("duration").and_then(Value::as_i64).map(Tick).unwrap_or_else(|| mo.unwrap_or(full) - sin);
    // The clip goes on the frame grid: its start and its end (`time + duration`) each snap to the
    // nearest frame, so a sub-frame `time` never pushes the end past the one asked for and the
    // sequence never ends between frames (#1101).
    let rate = s.sequence_rate();
    let end = rate.snap_nearest(Tick(at.0.saturating_add(dur.0)));
    let at = rate.snap_nearest(at);
    let dur = end - at;
    let ids = place_item(s, item, TimeRange::new(sin, dur), at, v, a, bool_p(p, "insert").unwrap_or(false), "Place Clip", None)?;
    Ok(json!({"clips": ids.iter().map(|c| c.0).collect::<Vec<_>>()}))
}

/// Most items one `timeline.place` with `items` takes: a larger list is refused, not cut.
pub(super) const MAX_PLACE_ITEMS: usize = 10_000;

/// `timeline.place` for `items`: the first at the given time, each next one where the previous one
/// ends, all as one undo step. `track`, `audioTrack`, `insert`, `video` and `audio` apply to every
/// item; `sourceIn` and `duration` name one item's range, so they need `item`.
pub(super) fn place_items(s: &mut Session, p: &Value, items: &Value) -> Result<Value> {
    let ids: Vec<ItemId> = items
        .as_array()
        .ok_or_else(|| bad("timeline.place", "`items` must be a list of item ids"))?
        .iter()
        .map(|v| v.as_u64().map(ItemId).ok_or_else(|| bad("timeline.place", "`items` must be a list of item ids")))
        .collect::<Result<_>>()?;
    if p.get("item").is_some() || p.get("sourceIn").is_some() || p.get("duration").is_some() {
        return Err(bad("timeline.place", "give `item` or `items`, and no `sourceIn` or `duration` with `items`"));
    }
    if ids.is_empty() || ids.len() > MAX_PLACE_ITEMS {
        return Err(bad("timeline.place", format!("`items` needs 1 to {MAX_PLACE_ITEMS} item ids")));
    }
    let mut at = sequence_time_p(s, p, "", "timeline.place")?.unwrap_or(s.playhead());
    s.edit_group("Place Clips", |s| {
        let mut placed = Vec::new();
        // "new" adds a track for the first item only; the rest follow it on that track
        let mut base = p.clone();
        for item in ids {
            let mut one = base.clone();
            if let Some(o) = one.as_object_mut() {
                o.remove("items");
                for k in ["time", "frame", "seconds"] {
                    o.remove(k);
                }
                o.insert("item".into(), json!(item.0));
                o.insert("time".into(), json!(at.0));
            }
            let r = place_one(s, &one)?;
            let clips: Vec<ClipId> = r["clips"].as_array().map(|a| a.iter().filter_map(|c| c.as_u64().map(ClipId)).collect()).unwrap_or_default();
            if crate::sequence_tools::wants_new_track(&base)
                && let Some(q) = s.active_sequence()
            {
                for (key, audio) in [("track", false), ("audioTrack", true)] {
                    if base.get(key).and_then(Value::as_str).is_some_and(|v| v.eq_ignore_ascii_case("new"))
                        && let Some(t) =
                            clips.iter().find_map(|c| q.find_item(*c).map(|(t, _)| t).filter(|t| q.audio_tracks.iter().any(|x| x.id == *t) == audio))
                    {
                        base[key] = json!(t.0);
                    }
                }
            }
            if let Some(end) = s.active_sequence().and_then(|q| clips.iter().filter_map(|c| q.find_item(*c).map(|(_, i)| i.end())).max()) {
                at = end;
            }
            placed.push(json!(clips.iter().map(|c| c.0).collect::<Vec<_>>()));
        }
        Ok(json!({"clips": placed.iter().flat_map(|c| c.as_array().cloned().unwrap_or_default()).collect::<Vec<_>>(), "placed": placed}))
    })
}

/// Place a project item on the timeline (drag from Project, or Insert/Overwrite from source).
pub(crate) fn place_item(
    s: &mut Session,
    item: ItemId,
    range: TimeRange,
    at: Tick,
    vdest: Option<TrackId>,
    adest: Option<TrackId>,
    insert: bool,
    label: &str,
    audio_split: Option<(TimeRange, Tick)>,
) -> Result<Vec<ClipId>> {
    let pi = s.project.item(item).ok_or_else(|| bad(label, "no such item"))?.clone();
    // the nest toggle off: a sequence edits in as its clips (a multi-camera source stays one clip)
    if s.state.sequences_as_clips && pi.as_sequence().is_some_and(|q| q.multicam.is_none()) {
        return crate::sequence_tools::place_sequence_clips(s, item, range, at, vdest, adest, insert, label);
    }
    let (video, audio) = s.project.source_streams(item);
    let has_v = video && vdest.is_some();
    let has_a = audio && adest.is_some();
    if !has_v && !has_a {
        return Err(EngineError::Other("no destination track for this media (check source patching)".into()));
    }
    let src_size = match &pi.kind {
        ItemKind::Media(m) => m.info.video.as_ref().map(|v| (v.width, v.height)),
        ItemKind::Sequence(q) => Some((q.settings.width, q.settings.height)),
        ItemKind::AdjustmentLayer { width, height, .. } => Some((*width, *height)),
        _ => None,
    }
    .unwrap_or((1920, 1080));
    // `None`: drawn in the pixels of the sequence it is placed in
    let src_par = s.project.source_par(item);
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let media = s.media.clone();
    let scaling = s.prefs.media.default_media_scaling.clone();
    let is_media = matches!(pi.kind, ItemKind::Media(_) | ItemKind::Subclip { .. });
    s.edit(label, |p, st| {
        let seq = p.sequence(seq_id).ok_or(EngineError::NoSequence)?;
        let rate = seq.settings.frame_rate;
        let frame = (seq.settings.width, seq.settings.height);
        let seq_par = seq.settings.par;
        let mut placements = Vec::new();
        let link = if has_v && has_a { Some(p.alloc_id()) } else { None };
        if let Some(vdest) = vdest.filter(|_| has_v) {
            let mut v = p.make_track_item(item, TrackKind::Video, at, range, rate).ok_or_else(|| bad(label, "bad item"))?;
            v.link = link;
            for e in &mut v.effects {
                resolve_auto_points(e, frame, src_size);
            }
            // the picture's size at its display aspect, in sequence pixels
            let shown = filmcraft_project::conformed_size(src_size, src_par.unwrap_or(seq_par), seq_par);
            if is_media && shown != (f64::from(frame.0), f64::from(frame.1)) {
                crate::settings::apply_media_scaling(&mut v, &scaling, frame, shown);
            }
            placements.push((vdest, v));
        }
        if let Some(adest) = adest.filter(|_| has_a) {
            let (arange, aat) = audio_split.unwrap_or((range, at));
            let mut a = p.make_track_item(item, TrackKind::Audio, aat, arange, rate).ok_or_else(|| bad(label, "bad item"))?;
            // Modify ▸ Audio Channels with several audio clips: one per clip, on the tracks below
            let clips: Vec<Vec<u16>> =
                p.resolve_media(item).and_then(|(_, m, _)| m.interpret.audio_channels.as_ref()).map(|m| m.clips.clone()).unwrap_or_default();
            let streams = p.resolve_media(item).map_or(1, |(_, m, _)| m.info.audio_streams.len());
            let specs = audio_placement_specs(&clips, streams);
            let first = p.sequence(seq_id).and_then(|q| q.audio_tracks.iter().position(|t| t.id == adest)).unwrap_or(0);
            // too few audio tracks below the destination: grow the sequence from the destination track
            let tracks = ensure_audio_tracks(p, seq_id, first, specs.len(), label)?;
            a.link = if link.is_none() && specs.len() > 1 { Some(p.alloc_id()) } else { link };
            for spec in specs {
                let tid = match spec.track_offset {
                    0 => &adest,
                    off => match first.checked_add(off).and_then(|i| tracks.get(i)) {
                        Some(t) => t,
                        None => continue,
                    },
                };
                let mut b = a.clone();
                b.audio_stream = spec.audio_stream;
                if spec.track_offset > 0 {
                    b.id = ClipId(p.alloc_id());
                }
                if spec.audio_stream > 0 {
                    b.source_channels.clear();
                }
                if let Some(chans) = spec.source_channels {
                    b.source_channels = chans;
                }
                placements.push((*tid, b));
            }
        }
        let snapshot = std::sync::Arc::new(p.clone());
        let snap = snapshot.clone();
        let durations = move |id: ItemId| crate::media_duration(&snapshot, &media, id);
        let starts = move |id: ItemId| crate::media_start(&snap, id);
        let min = rate.frame_duration();
        let mut next = p.next_id;
        let seq = p.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        let mut ctx = edit::EditCtx { next_id: &mut next, media_duration: &durations, media_start: &starts, min_duration: min };
        let span = placements.iter().map(|x| x.1.start).min().zip(placements.iter().map(|x| x.1.end()).max());
        let ids = if insert { edit::insert(seq, placements, &mut ctx)? } else { edit::overwrite(seq, placements, &mut ctx)? };
        if insert
            && st.ripple_sequence_markers
            && let Some((a, b)) = span
        {
            crate::sequence_tools::ripple_markers(&mut seq.markers, a, b - a);
        }
        p.next_id = next;
        st.selection = ids.clone();
        Ok(ids)
    })
}

/// How long an item runs on the Timeline without In/Out marks: its duration, or for a still (and a
/// generator without a length) Settings ▸ Timeline ▸ Still Image Default Duration.
pub(crate) fn full_duration(s: &Session, pi: &filmcraft_project::ProjectItem) -> Tick {
    match &pi.kind {
        ItemKind::Media(m)
            if matches!(m.info.kind, filmcraft_media::MediaKind::Still)
                || (matches!(m.info.kind, filmcraft_media::MediaKind::Synthetic) && m.info.duration.0 <= 0) =>
        {
            s.prefs.timeline.still_duration(s.sequence_rate())
        }
        _ => pi.duration(),
    }
}

pub(crate) fn source_range(s: &Session) -> Option<(ItemId, TimeRange)> {
    let item = s.state.source_item?;
    let pi = s.project.item(item)?;
    let dur = match &pi.kind {
        ItemKind::Media(m) if matches!(m.info.kind, filmcraft_media::MediaKind::Still) => s.prefs.timeline.still_duration(s.sequence_rate()),
        _ => pi.duration(),
    };
    let (mi, mo) = match &pi.kind {
        ItemKind::Media(m) => (m.mark_in, m.mark_out),
        ItemKind::Sequence(q) => (q.mark_in, q.mark_out),
        _ => (None, None),
    };
    // a subclip shows its range of the parent's media (its clips use the parent's media time)
    if let ItemKind::Subclip { range, .. } = &pi.kind {
        return Some((item, *range));
    }
    let start = mi.unwrap_or(Tick::ZERO);
    let rate = pi.frame_rate();
    let end = mo.map(|o| o + rate.frame_duration()).unwrap_or(dur).min(if dur.0 > 0 { dur } else { Tick::MAX });
    Some((item, TimeRange::from_bounds(start, end.max(start + rate.frame_duration()))))
}
