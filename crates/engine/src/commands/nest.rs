//! Nesting clips into a new sequence.

use super::*;

/// The name Nest… offers: the first unused "Nested Sequence NN".
pub fn next_nested_name(s: &Session) -> String {
    (1..=s.project.items.len().saturating_add(1))
        .map(|k| format!("Nested Sequence {k:02}"))
        .find(|n| !s.project.items.values().any(|i| &i.name == n))
        .unwrap_or_else(|| "Nested Sequence".to_string())
}

pub(super) fn nest(s: &mut Session, p: &Value) -> Result<Value> {
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let sel = with_links(s, &s.state.selection.clone());
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?.clone();
    let name = str_p(p, "name").filter(|n| !n.trim().is_empty()).map(str::to_string).unwrap_or_else(|| next_nested_name(s));
    let mut items: Vec<(TrackKind, usize, filmcraft_project::TrackItem)> = Vec::new();
    // transitions go into the nest with their clips: those whose clips are all nested
    let mut transitions: Vec<(TrackKind, usize, filmcraft_project::Transition)> = Vec::new();
    for (k, ts) in [(TrackKind::Video, &q.video_tracks), (TrackKind::Audio, &q.audio_tracks)] {
        // clips on a locked track stay where they are (they cannot be removed from it)
        for (ti, t) in ts.iter().enumerate().filter(|(_, t)| !t.locked) {
            for i in t.items.iter().filter(|i| sel.contains(&i.id)) {
                items.push((k, ti, i.clone()));
            }
            let nested = |c: Option<ClipId>| c.is_none_or(|c| sel.contains(&c));
            for trn in t.transitions.iter().filter(|x| (x.from.is_some() || x.to.is_some()) && nested(x.from) && nested(x.to)) {
                transitions.push((k, ti, trn.clone()));
            }
        }
    }
    if items.is_empty() {
        return Err(EngineError::Other("nothing selected".into()));
    }
    let start = items.iter().map(|i| i.2.start).min().unwrap_or_default();
    let end = items.iter().map(|i| i.2.end()).max().unwrap_or_default();
    let span = TimeRange::new(start, end - start);
    let tracks_of = |kind: TrackKind| -> Vec<usize> {
        let mut v: Vec<usize> = items.iter().filter(|i| i.0 == kind).map(|i| i.1).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let (video_tracks, audio_tracks) = (tracks_of(TrackKind::Video), tracks_of(TrackKind::Audio));
    // Premiere (26.5.2): the nest is a linked picture + sound pair only when the selected sound is
    // on one track and that track is free for the nest's whole length. With sound on several
    // tracks, or another clip in the way on its track, the nest is picture only: the selected sound
    // stays where it is in this sequence, and a copy of it goes into the nest.
    let sound_blocked = |ai: usize| q.audio_tracks.get(ai).is_none_or(|t| t.items.iter().any(|i| !sel.contains(&i.id) && i.range().overlaps(&span)));
    let video_only = !video_tracks.is_empty() && (audio_tracks.len() > 1 || audio_tracks.first().is_some_and(|ai| sound_blocked(*ai)));
    let (lowest_v, lowest_a) = (video_tracks.first().copied(), audio_tracks.first().copied().filter(|_| !video_only));
    // In the nest, picture tracks keep their positions (V1 up to the highest one used). Sound
    // tracks do not: A1 is always there, and the other tracks used follow it in order
    // (A1 + A3 become A1, A2; A3 alone becomes an empty A1 and A2).
    let nv = video_tracks.last().map_or(1, |i| i + 1);
    let audio_at: std::collections::HashMap<usize, usize> =
        audio_tracks.iter().filter(|i| **i != 0).enumerate().map(|(n, i)| (*i, n + 1)).chain(std::iter::once((0, 0))).collect();
    let na = audio_at.values().max().map_or(1, |i| i + 1);
    let nested_track = |k: TrackKind, ti: usize| if k == TrackKind::Audio { audio_at.get(&ti).copied().unwrap_or(ti) } else { ti };
    // what leaves this sequence: everything nested, less the sound that stays behind
    let removed: Vec<ClipId> = items.iter().filter(|i| !(video_only && i.0 == TrackKind::Audio)).map(|i| i.2.id).collect();
    let stays = |k: TrackKind| video_only && k == TrackKind::Audio;
    let r = s.edit("Nest", |pr, st| {
        let nid = pr.new_sequence(&name, q.settings.clone(), nv, na, None);
        // copies of clips and transitions that also stay behind need ids of their own
        let fresh_clips: std::collections::HashMap<ClipId, ClipId> = items.iter().filter(|i| stays(i.0)).map(|i| (i.2.id, ClipId(pr.alloc_id()))).collect();
        let fresh_transitions: Vec<Option<u64>> = transitions.iter().map(|t| stays(t.0).then(|| pr.alloc_id())).collect();
        let copy = |c: ClipId| fresh_clips.get(&c).copied().unwrap_or(c);
        {
            let nq = pr.sequence_mut(nid).ok_or(EngineError::NoSequence)?;
            // tracks take the name and channel layout of the track their clips came from
            for (tr, src) in nq.video_tracks.iter_mut().zip(&q.video_tracks) {
                tr.name = src.name.clone();
            }
            for (from, to) in &audio_at {
                if let (Some(tr), Some(src)) = (nq.audio_tracks.get_mut(*to), q.audio_tracks.get(*from)) {
                    tr.channels = src.channels;
                    if from == to {
                        tr.name = src.name.clone();
                    }
                }
            }
            for (k, ti, it) in &items {
                let mut it = it.clone();
                it.id = copy(it.id);
                it.start -= start;
                nq.tracks_mut(*k).get_mut(nested_track(*k, *ti)).ok_or(EngineError::NoSequence)?.items.push(it);
            }
            for ((k, ti, trn), fresh) in transitions.iter().zip(&fresh_transitions) {
                let mut trn = trn.clone();
                if let Some(id) = fresh {
                    trn.id = filmcraft_project::TransitionId(*id);
                }
                (trn.from, trn.to) = (trn.from.map(copy), trn.to.map(copy));
                trn.start -= start;
                nq.tracks_mut(*k).get_mut(nested_track(*k, *ti)).ok_or(EngineError::NoSequence)?.transitions.push(trn);
            }
            for t in nq.all_tracks_mut() {
                t.sort();
                edit::remove_orphan_transitions(t);
            }
            nq.check().map_err(EngineError::Other)?;
        }
        let rate = q.settings.frame_rate;
        // the nest shows its sequence from the beginning, for the length of what was nested
        let whole = TimeRange::new(Tick::ZERO, span.duration);
        let mut v =
            pr.make_track_item(nid, TrackKind::Video, start, whole, rate).ok_or_else(|| EngineError::Other("cannot place the nested sequence".into()))?;
        for e in &mut v.effects {
            resolve_auto_points(e, (q.settings.width, q.settings.height), (q.settings.width, q.settings.height));
        }
        let a = pr.make_track_item(nid, TrackKind::Audio, start, whole, rate).ok_or_else(|| EngineError::Other("cannot place the nested sequence".into()))?;
        let (link, new_video, new_audio) = (pr.alloc_id(), pr.alloc_id(), pr.alloc_id());
        let seq = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        edit::delete_items(seq, &removed);
        // sound left behind has lost its picture: it is no longer linked
        if video_only {
            let links: Vec<u64> = items.iter().filter(|i| i.0 == TrackKind::Video).filter_map(|i| i.2.link).collect();
            for it in seq.audio_tracks.iter_mut().flat_map(|t| t.items.iter_mut()) {
                if it.link.is_some_and(|l| links.contains(&l)) {
                    it.link = None;
                }
            }
        }
        // Where the nest goes, as Premiere Pro 26.5.2 places it (about 35 nests observed). With L
        // the lowest video track of the selection and `a` the number of the highest audio track
        // holding selected sound, the picture takes the lowest track T from L up such that
        //   1. T is free of other clips for the nest's whole length, and
        //   2. so is every video track numbered from T up to a - 1 that the selection had clips on.
        // If no track will do, a new one is added on top. So the nest never lands on another clip,
        // and it can be pushed above an unselected clip that shares a track with the selection
        // when the sound sits on a higher-numbered track. The sound of a linked nest is on the one
        // track its clips came from, which is free (a selection of sound alone moves up to the
        // first free track).
        let highest_sound = audio_tracks.last().map_or(0, |i| i + 1);
        let mut ids = Vec::new();
        for (kind, lowest, mut clip, new_track) in [(TrackKind::Video, lowest_v, v, new_video), (TrackKind::Audio, lowest_a, a, new_audio)] {
            let Some(lowest) = lowest else { continue };
            let tracks = seq.tracks_mut(kind);
            let clear = |i: usize| tracks.get(i).is_some_and(|t| !t.locked && edit::track_range_empty(t, span));
            let selection_clear = |t: usize| kind != TrackKind::Video || video_tracks.iter().filter(|i| **i >= t && **i + 1 < highest_sound).all(|i| clear(*i));
            let free = (lowest..tracks.len()).find(|t| clear(*t) && selection_clear(*t));
            let at = match free {
                Some(i) => i,
                None => {
                    let (word, n) = (if kind == TrackKind::Video { "Video" } else { "Audio" }, tracks.len() + 1);
                    tracks.push(filmcraft_project::Track::new(TrackId(new_track), kind, format!("{word} {n}")));
                    tracks.len() - 1
                }
            };
            clip.link = (lowest_v.is_some() && lowest_a.is_some()).then_some(link);
            ids.push(clip.id);
            let t = tracks.get_mut(at).ok_or(EngineError::NoSequence)?;
            t.items.push(clip);
            t.sort();
        }
        seq.check().map_err(EngineError::Other)?;
        // like Premiere: nothing stays selected in the timeline; the new sequence is selected in the Project panel
        st.selection.clear();
        st.project_selection = vec![nid];
        Ok((nid, ids))
    });
    r.map(|(n, ids)| json!({"sequence": n.0, "clips": ids.iter().map(|c| c.0).collect::<Vec<_>>()}))
}
