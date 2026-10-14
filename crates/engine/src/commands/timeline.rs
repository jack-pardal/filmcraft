//! Timeline gestures and trim mode.

use super::*;

pub(super) fn commands() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "timeline.place",
            "Place Clip",
            [],
            None,
            r#"{"item":id|"items":[id] (back to back, one undo step),"track":"V1"|id|"A1" (sound only)|"new" (a video track after the last)?,"audioTrack":"A1"|id|"new"?,"time":ticks|"frame":i64|"seconds":f64,"insert":bool,"sourceIn":ticks?,"duration":ticks?,"video":bool=true,"audio":bool=true}"#,
            has_seq,
            |s, p| match p.get("items") {
                Some(items) => place_items(s, p, items),
                None => place_one(s, p),
            }
        ),
        cmd!("timeline.select", "Select Clips", [], None, r#"{"clips":[id],"transitions":[id]?,"add":bool,"toggle":bool}"#, has_seq, |s, p| {
            if let Some(ids) = p.get("transitions") {
                return select_transitions(s, ids, bool_p(p, "add").unwrap_or(false), bool_p(p, "toggle").unwrap_or(false));
            }
            let clips: Vec<ClipId> =
                p.get("clips").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_u64().map(ClipId)).collect()).unwrap_or_default();
            let clips = with_links(s, &clips);
            // selecting clips leaves trim mode (Premiere: clip and edit point selections are exclusive)
            s.state.edit_points.clear();
            s.state.trim_shift = Default::default();
            // and a clip selection replaces a transition or gap one
            s.state.transition_selection.clear();
            s.state.gap_selection = None;
            if bool_p(p, "toggle").unwrap_or(false) {
                for c in clips {
                    if let Some(i) = s.state.selection.iter().position(|x| *x == c) {
                        s.state.selection.remove(i);
                    } else {
                        s.state.selection.push(c);
                    }
                }
            } else if bool_p(p, "add").unwrap_or(false) {
                for c in clips {
                    if !s.state.selection.contains(&c) {
                        s.state.selection.push(c);
                    }
                }
            } else {
                s.state.selection = clips;
                s.state.caption_selection.clear();
            }
            Ok(json!({"selection": s.state.selection.iter().map(|c| c.0).collect::<Vec<_>>()}))
        }),
        cmd!("timeline.selectGap", "Select Gap", [], None, r#"{"track":"V1"|id,"time":ticks|"frame":n|"seconds":f}"#, has_seq, select_gap),
        cmd!(
            "timeline.move",
            "Move Clips",
            [],
            None,
            r#"{"moves":[{"clip":id,"track":id|"V2"|"new" (after the last of its kind),"time":ticks}],"insert":bool,"linked":bool?,"copy":bool?}"#,
            has_seq,
            move_clips
        ),
        cmd!(
            "timeline.trim",
            "Trim Edit",
            [],
            None,
            r#"{"clip":id,"edge":"in|out","mode":"regular|ripple","delta":ticks|"deltaFrames":i64}"#,
            has_seq,
            |s, p| {
                let c = clip_p(p, "clip").ok_or_else(|| bad("timeline.trim", "need `clip`"))?;
                let edge = if str_p(p, "edge") == Some("in") { Edge::In } else { Edge::Out };
                let mode = if str_p(p, "mode") == Some("ripple") { TrimMode::Ripple } else { TrimMode::Regular };
                let d = p
                    .get("delta")
                    .and_then(Value::as_i64)
                    .map(Tick)
                    .or_else(|| p.get("deltaFrames").and_then(Value::as_i64).map(|f| s.sequence_rate().tick_of(f)))
                    .unwrap_or_default();
                let clips = with_links(s, &[c]);
                let applied = s.edit_sequence(if mode == TrimMode::Ripple { "Ripple Trim" } else { "Trim" }, |q, ctx, st| {
                    if q.find_item(c).is_some_and(|(t, _)| q.track(t).is_some_and(|tr| tr.locked)) {
                        return Err(edit::EditError::Locked.into());
                    }
                    let before = q.find_item(c).map(|(_, i)| i.range());
                    // clamp across all linked partners, then apply the common delta
                    let mut dd = clamp_trim_linked(q, &clips, edge, mode, d, ctx)?;
                    if mode == TrimMode::Ripple {
                        // the clip and its linked partners ripple as one edit
                        let mut group = vec![c];
                        group.extend(clips.iter().copied().filter(|o| *o != c));
                        group.retain(|id| q.find_item(*id).is_some_and(|(t, _)| q.track(t).is_some_and(|tr| !tr.locked)));
                        if group.is_empty() {
                            return Err(edit::EditError::Locked.into());
                        }
                        dd = edit::ripple_trim_group(q, &group, edge, dd, ctx)?;
                        if st.ripple_sequence_markers
                            && let Some(r) = before
                        {
                            let (from, shift) = match edge {
                                Edge::In if dd > Tick::ZERO => (r.start + dd, -dd),
                                Edge::In => (r.start, -dd),
                                Edge::Out => (r.end(), dd),
                            };
                            crate::sequence_tools::ripple_markers(&mut q.markers, from, shift);
                        }
                    } else {
                        for c in &clips {
                            match edit::trim(q, *c, edge, mode, dd, ctx) {
                                Err(edit::EditError::Locked) => continue,
                                Err(e) => return Err(e.into()),
                                Ok(_) => {}
                            }
                        }
                    }
                    Ok(dd)
                })?;
                Ok(json!({"delta": applied.0}))
            }
        ),
        // ================= Trim mode =================
        cmd!("trim.selectEditPoint", "Select Edit Point", [], None, r#"{"clip":id,"edge":"in|out","kind":"trim|ripple|roll","add":bool?}"#, has_seq, |s, p| {
            crate::trim::select(s, p)
        }),
        cmd!("trim.selectNearest", "Select Nearest Edit Point", [], None, r#"{"kind":"rippleIn|rippleOut|roll|trimIn|trimOut"}"#, has_seq, |s, p| {
            crate::trim::select_nearest(s, p)
        }),
        cmd!("trim.edit", "Trim Edit", ["Sequence"], Some("Shift+T"), "{}", has_seq, |s, _| {
            if s.state.edit_points.is_empty() {
                crate::trim::select_nearest(s, &json!({"kind": "roll"}))
            } else {
                Ok(json!({"editPoints": s.state.edit_points}))
            }
        }),
        cmd!("trim.clear", "Clear Edit Point Selection", [], None, "{}", has_edit_points, |s, _| {
            s.state.edit_points.clear();
            s.state.trim_shift = Default::default();
            s.trim_play.around = None;
            Ok(Value::Null)
        }),
        cmd!("trim.toggleType", "Toggle Trim Type", [], Some("Ctrl+T"), "{}", has_edit_points, |s, _| crate::trim::toggle_type(s)),
        cmd!("trim.backward", "Trim Backward", [], Some("Alt+Left"), "{}", has_edit_points, |s, _| crate::trim::nudge(s, &json!({"frames": -1}))),
        cmd!("trim.forward", "Trim Forward", [], Some("Alt+Right"), "{}", has_edit_points, |s, _| crate::trim::nudge(s, &json!({"frames": 1}))),
        cmd!("trim.backwardMany", "Trim Backward Many", [], Some("Alt+Shift+Left"), "{}", has_edit_points, |s, _| {
            let n = -(s.prefs.trim.large_trim_offset as i64);
            crate::trim::nudge(s, &json!({"frames": n}))
        }),
        cmd!("trim.forwardMany", "Trim Forward Many", [], Some("Alt+Shift+Right"), "{}", has_edit_points, |s, _| {
            let n = s.prefs.trim.large_trim_offset as i64;
            crate::trim::nudge(s, &json!({"frames": n}))
        }),
        cmd!("trim.applyDefaultTransition", "Apply Default Transitions to Selection", ["Sequence"], None, "{}", has_edit_points, |s, _| {
            crate::trim::apply_default_transitions(s)
        }),
        cmd!("trim.shuttle", "Dynamic Trim (Shuttle)", [], None, r#"{"direction":"forward|reverse","slow":bool?,"clock":seconds}"#, has_edit_points, |s, p| {
            crate::trim::shuttle(s, p)
        }),
        cmd!("trim.shuttleStop", "Dynamic Trim Stop", [], None, r#"{"clock":seconds?}"#, has_seq, |s, p| crate::trim::stop(s, p)),
        cmd!("trim.cancelDynamic", "Cancel Dynamic Trim", [], None, "{}", has_seq, |s, _| crate::trim::cancel(s)),
        cmd!("trim.playAround", "Play Around Edit", [], None, r#"{"clock":seconds,"loop":bool=true,"toggle":bool?}"#, has_seq, |s, p| {
            crate::trim::play_around(s, p)
        }),
        CommandSpec {
            id: "trim.tick",
            label: "Advance Trim Playback",
            menu: &[],
            shortcut: None,
            params: r#"{"clock":seconds}"#,
            enabled: has_seq,
            run: |s, p| crate::trim::tick(s, p.get("clock").and_then(Value::as_f64).unwrap_or(0.0)),
            journal: false,
        },
        query!("trim.monitor", "Trim Monitor State", "{}", |s, _| Ok(crate::trim::monitor_info(s))),
        cmd!("trim.nudge", "Trim by Frames", [], None, r#"{"frames":i64}"#, has_edit_points, |s, p| crate::trim::nudge(s, p)),
        cmd!("trim.extendToPlayhead", "Extend Selected Edit to Playhead", [], Some("E"), "{}", has_edit_points, |s, _| { crate::trim::extend_to_playhead(s) }),
        cmd!("trim.ripplePrevious", "Ripple Trim Previous Edit to Playhead", [], Some("Q"), "{}", has_seq, |s, _| crate::trim::to_playhead(
            s,
            &json!({"side": "previous", "ripple": true})
        )),
        cmd!("trim.rippleNext", "Ripple Trim Next Edit to Playhead", [], Some("W"), "{}", has_seq, |s, _| crate::trim::to_playhead(
            s,
            &json!({"side": "next", "ripple": true})
        )),
        cmd!("trim.previous", "Trim Previous Edit to Playhead", [], Some("Alt+Q"), "{}", has_seq, |s, _| crate::trim::to_playhead(
            s,
            &json!({"side": "previous", "ripple": false})
        )),
        cmd!("trim.next", "Trim Next Edit to Playhead", [], Some("Alt+W"), "{}", has_seq, |s, _| crate::trim::to_playhead(
            s,
            &json!({"side": "next", "ripple": false})
        )),
        cmd!("timeline.roll", "Rolling Edit", [], None, r#"{"left":id,"right":id,"delta":ticks|"deltaFrames":i64,"followLinks"?:bool}"#, has_seq, |s, p| {
            let (l, r) = (
                clip_p(p, "left").ok_or_else(|| bad("timeline.roll", "need `left`"))?,
                clip_p(p, "right").ok_or_else(|| bad("timeline.roll", "need `right`"))?,
            );
            let d = p
                .get("delta")
                .and_then(Value::as_i64)
                .map(Tick)
                .or_else(|| p.get("deltaFrames").and_then(Value::as_i64).map(|f| s.sequence_rate().tick_of(f)))
                .unwrap_or_default();
            // `followLinks: false` rolls only this pair: Trim mode already lists linked partners as edit points of their own
            let follow = s.state.linked_selection && p.get("followLinks").and_then(Value::as_bool).unwrap_or(true);
            let (x, n, linked) = s.edit_sequence("Rolling Edit", |q, ctx, _| {
                let pairs = linked_roll_pairs(q, l, r, follow);
                let linked = pairs.len() > 1;
                let mut d = d;
                for &(pl, pr) in &pairs {
                    let mut probe = q.clone();
                    let x = edit::roll(&mut probe, pl, pr, d, ctx)?;
                    if x.abs() < d.abs() {
                        d = x;
                    }
                }
                if d != Tick::ZERO {
                    for &(pl, pr) in &pairs {
                        edit::roll(q, pl, pr, d, ctx)?;
                    }
                }
                Ok((d, pairs.len(), linked))
            })?;
            Ok(json!({"delta": x.0, "pairs": n, "linked": linked}))
        }),
        cmd!("timeline.slip", "Slip", [], None, r#"{"clip":id,"delta":ticks|"deltaFrames":i64}"#, has_seq, |s, p| {
            let c = clip_p(p, "clip").ok_or_else(|| bad("timeline.slip", "need `clip`"))?;
            let d = p
                .get("delta")
                .and_then(Value::as_i64)
                .map(Tick)
                .or_else(|| p.get("deltaFrames").and_then(Value::as_i64).map(|f| s.sequence_rate().tick_of(f)))
                .unwrap_or_default();
            let clips = with_links(s, &[c]);
            s.edit_sequence("Slip", |q, ctx, _| {
                // clamp across all linked partners, then slip them by the common delta
                let deltas: Vec<(ClipId, Tick)> = clips.iter().map(|c| (*c, d)).collect();
                slip_together(q, ctx, &deltas)
            })?;
            Ok(Value::Null)
        }),
        cmd!("timeline.slide", "Slide", [], None, r#"{"clip":id,"delta":ticks|"deltaFrames":i64}"#, has_seq, |s, p| {
            let c = clip_p(p, "clip").ok_or_else(|| bad("timeline.slide", "need `clip`"))?;
            let d = p
                .get("delta")
                .and_then(Value::as_i64)
                .map(Tick)
                .or_else(|| p.get("deltaFrames").and_then(Value::as_i64).map(|f| s.sequence_rate().tick_of(f)))
                .unwrap_or_default();
            let clips = with_links(s, &[c]);
            // clamp across all linked partners, then slide them by the common delta
            s.edit_sequence("Slide", |q, ctx, _| {
                let unlocked: Vec<ClipId> =
                    clips.iter().copied().filter(|id| q.find_item(*id).is_some_and(|(t, _)| q.track(t).is_some_and(|tr| !tr.locked))).collect();
                if unlocked.is_empty() {
                    return Err(edit::EditError::Locked.into());
                }
                Ok(edit::slide_items(q, &unlocked, d, ctx)?)
            })?;
            Ok(Value::Null)
        }),
        cmd!("timeline.rateStretch", "Rate Stretch", [], None, r#"{"clip":id,"edge":"in|out","delta":ticks}"#, has_seq, |s, p| {
            let c = clip_p(p, "clip").ok_or_else(|| bad("timeline.rateStretch", "need `clip`"))?;
            let edge = if str_p(p, "edge") == Some("in") { Edge::In } else { Edge::Out };
            let d = p
                .get("delta")
                .and_then(Value::as_i64)
                .map(Tick)
                .or_else(|| p.get("deltaFrames").and_then(Value::as_i64).map(|f| s.sequence_rate().tick_of(f)))
                .unwrap_or_default();
            let clips = with_links(s, &[c]);
            let sp = s.edit_sequence("Rate Stretch", |q, ctx, _| Ok(edit::rate_stretch_items(q, &clips, edge, d, ctx)?))?;
            Ok(json!({"speed": sp}))
        }),
        cmd!("timeline.razor", "Razor", [], None, r#"{"time":ticks,"track":"V1"|id?,"clip":id?}"#, has_seq, |s, p| {
            let t = time_p(s, p, "").unwrap_or(s.playhead());
            let tr = track_p(s, p, "track", "timeline.razor")?;
            let clip = clip_p(p, "clip");
            let n = if let Some(c) = clip {
                let clips = with_links(s, &[c]);
                s.edit_sequence("Razor", |q, ctx, _| Ok(edit::razor_items(q, &clips, t, ctx)))?
            } else {
                let tracks: Vec<TrackId> = tr.into_iter().collect();
                s.edit_sequence("Razor", |q, ctx, _| Ok(edit::razor(q, &tracks, t, ctx)))?
            };
            Ok(json!({"cuts": n.len()}))
        }),
        cmd!(
            "timeline.setTrack",
            "Track Settings",
            [],
            None,
            r#"{"track":"V1"|id,"locked":bool?,"syncLock":bool?,"enabled":bool?,"muted":bool?,"solo":bool?,"name":str?,"volumeDb":f64?,"pan":f64?}"#,
            has_seq,
            |s, p| {
                let tr = track_p(s, p, "track", "timeline.setTrack")?.ok_or_else(|| bad("timeline.setTrack", "need `track`"))?;
                let p = p.clone();
                s.edit_sequence("Track Settings", |q, _, _| {
                    let t = q.track_mut(tr).ok_or(filmcraft_edit::EditError::NoTrack(tr))?;
                    if let Some(v) = bool_p(&p, "locked") {
                        t.locked = v;
                    }
                    if let Some(v) = bool_p(&p, "syncLock") {
                        t.sync_lock = v;
                    }
                    if let Some(v) = bool_p(&p, "enabled") {
                        t.enabled = v;
                    }
                    if let Some(v) = bool_p(&p, "muted") {
                        t.muted = v;
                    }
                    if let Some(v) = bool_p(&p, "solo") {
                        t.solo = v;
                    }
                    if let Some(v) = str_p(&p, "name") {
                        t.name = v.to_string();
                    }
                    if let Some(v) = f64_p(&p, "volumeDb") {
                        t.volume_db = v;
                    }
                    if let Some(v) = f64_p(&p, "pan") {
                        t.pan = v.clamp(-100.0, 100.0);
                    }
                    Ok(())
                })?;
                Ok(Value::Null)
            }
        ),
        cmd!("timeline.setTargeting", "Track Targeting", [], None, r#"{"track":"V1"|id,"targeted":bool?,"sourcePatch":bool?}"#, has_seq, |s, p| {
            let tr = track_p(s, p, "track", "timeline.setTargeting")?.ok_or_else(|| bad("timeline.setTargeting", "need `track`"))?;
            let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
            let mut tg = s.targeting();
            let is_video = s.active_sequence().is_some_and(|q| q.video_tracks.iter().any(|t| t.id == tr));
            if let Some(on) = bool_p(p, "targeted") {
                tg.targeted.retain(|t| *t != tr);
                if on {
                    tg.targeted.push(tr);
                }
            }
            if let Some(on) = bool_p(p, "sourcePatch") {
                let slot = if is_video { &mut tg.video_dest } else { &mut tg.audio_dest };
                *slot = if on {
                    Some(tr)
                } else if *slot == Some(tr) {
                    None
                } else {
                    *slot
                };
            }
            s.state.targeting.insert(seq_id, tg);
            Ok(Value::Null)
        }),
    ]
}

fn has_edit_points(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    if s.state.edit_points.is_empty() { Err("no edit points selected".into()) } else { Ok(()) }
}

/// `timeline.move`: move the listed clips to (track, time). With Linked Selection on (or
/// `linked: true`) the linked partners of a listed clip that are not listed themselves follow by
/// the same time offset on their own tracks, so picture and sound stay in sync; a partner on a
/// locked track, or one whose position would not change, stays as it is. Nothing moves before the sequence start: when a clip would,
/// every clip of the call lands later by the same amount, so the spacing asked for is kept.
/// `overwritten` lists every clip the move covered without being asked to move it: its length
/// before and after (ticks) and the clips what is left of it now lives on as (none = removed; a
/// clip a moved clip lands inside is split in two). With `copy: true` (Option-drag) the clips stay
/// where they are and copies land at the destinations instead; the copies are selected and listed
/// as `copied`.
fn move_clips(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "timeline.move";
    let mut listed: Vec<(ClipId, TrackId, Tick)> = Vec::new();
    let mut new_moves = Vec::new();
    for m in p.get("moves").and_then(Value::as_array).into_iter().flatten() {
        let track = if str_p(m, "track").is_some_and(|t| t.eq_ignore_ascii_case("new")) {
            let clip = clip_p(m, "clip").ok_or_else(|| bad(CMD, "new track needs a clip"))?;
            let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
            let (source, _) = q.find_item(clip).ok_or(edit::EditError::NoItem(clip))?;
            if q.track(source).is_some_and(|t| t.locked) {
                return Err(edit::EditError::Locked.into());
            }
            if m.get("time").and_then(Value::as_i64).is_none() {
                return Err(bad(CMD, "new track needs a time"));
            }
            new_moves.push((clip, source));
            Some(source)
        } else {
            track_p(s, m, "track", CMD)?
        };
        if let (Some(clip), Some(track), Some(time)) = (clip_p(m, "clip"), track, m.get("time").and_then(Value::as_i64)) {
            listed.push((clip, track, Tick(time)));
        }
    }
    if listed.is_empty() {
        return Err(bad(CMD, "need `moves`"));
    }
    let follow = bool_p(p, "linked").unwrap_or(s.state.linked_selection);
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    // (clip, destination track, new start, where a partner starts now). `time` is any i64: the
    // sums are done in i128, where a few i64 values cannot overflow, and checked on the way back.
    let mut plan: Vec<(ClipId, TrackId, i128, Option<i128>)> = Vec::new();
    for &(clip, track, time) in &listed {
        plan.push((clip, track, i128::from(time.0), None));
        let Some((_, it)) = q.find_item(clip) else { continue }; // `edit::move_items` reports it
        let Some(link) = it.link.filter(|_| follow) else { continue };
        let offset = i128::from(time.0) - i128::from(it.start.0);
        for t in q.all_tracks().filter(|t| !t.locked) {
            for partner in t.items.iter().filter(|i| i.link == Some(link)) {
                if !listed.iter().any(|m| m.0 == partner.id) && !plan.iter().any(|m| m.0 == partner.id) {
                    plan.push((partner.id, t.id, i128::from(partner.start.0) + offset, Some(i128::from(partner.start.0))));
                }
            }
        }
    }
    let late = plan.iter().map(|m| -m.2).max().unwrap_or(0).max(0);
    let mut moves: Vec<(ClipId, TrackId, Tick)> = Vec::with_capacity(plan.len());
    for (clip, track, start, was) in plan {
        // a partner that ends up where it is does not move (its listed clip only changed track):
        // re-placing it would drop the transitions at its edges
        if was == Some(start + late) {
            continue;
        }
        let start = i64::try_from(start + late).ok().map(Tick).filter(|t| *t <= Tick::MAX).ok_or_else(|| bad(CMD, "`time` is out of range"))?;
        moves.push((clip, track, start));
    }
    let ins = bool_p(p, "insert").unwrap_or(false);
    let copy = bool_p(p, "copy").unwrap_or(false);
    // every clip on the sequence now: id -> (track, start, end, project item)
    let spots = |s: &Session| -> Vec<(u64, TrackId, Tick, Tick, ItemId)> {
        let q = s.active_sequence();
        q.into_iter().flat_map(|q| q.all_tracks()).flat_map(|t| t.items.iter().map(move |i| (i.id.0, t.id, i.start, i.end(), i.item))).collect()
    };
    let before = spots(s);
    // Allocate destinations in the same transaction as the move: an invalid edit leaves
    // neither empty tracks nor history entries. Each source track keeps its clips together.
    let prepare = |q: &mut filmcraft_project::Sequence, ctx: &mut edit::EditCtx<'_>| -> Result<Vec<(ClipId, TrackId, Tick)>> {
        let sources: Vec<_> = q.all_tracks().filter(|t| new_moves.iter().any(|m| m.1 == t.id)).map(|t| (t.id, t.kind, t.channels)).collect();
        for kind in [TrackKind::Video, TrackKind::Audio] {
            let added = sources.iter().filter(|s| s.1 == kind).count();
            if added > 0 && q.tracks(kind).len().saturating_add(added) > crate::sequence_tools::MAX_TRACKS {
                return Err(bad(CMD, "new tracks exceed the sequence track limit"));
            }
        }
        let mut destinations = Vec::new();
        for (source, kind, channels) in sources {
            let tracks = if kind == TrackKind::Video { &mut q.video_tracks } else { &mut q.audio_tracks };
            let prefix = if kind == TrackKind::Video { "Video" } else { "Audio" };
            let id = TrackId(ctx.alloc());
            let mut track = Track::new(id, kind, format!("{prefix} {}", tracks.len() + 1));
            track.channels = channels;
            tracks.push(track);
            destinations.push((source, id));
        }
        Ok(moves
            .iter()
            .map(|&(clip, track, time)| {
                let dest = new_moves.iter().find(|m| m.0 == clip).and_then(|m| destinations.iter().find(|d| d.0 == m.1)).map_or(track, |d| d.1);
                (clip, dest, time)
            })
            .collect())
    };
    let copied = if copy {
        let label = if ins { "Copy (Insert)" } else { "Copy" };
        s.edit_sequence(label, |q, ctx, st| {
            let destinations = prepare(q, ctx)?;
            let ids = edit::copy_items(q, &destinations, ins, ctx)?;
            st.selection = ids.clone();
            st.transition_selection.clear();
            Ok(ids)
        })?
    } else {
        s.edit_sequence(if ins { "Move (Insert)" } else { "Move" }, |q, ctx, _| {
            let destinations = prepare(q, ctx)?;
            Ok(edit::move_items(q, &destinations, ins, ctx)?)
        })?;
        Vec::new()
    };
    // An overwrite shortens, splits or removes whatever already sits where a clip lands, and a
    // shortened clip may live on under a new id: name each clip the move changed without being
    // asked to, with what is left of it, so the caller hears about it instead of finding out later.
    let after = spots(s);
    let moved = |c: u64| moves.iter().any(|m| m.0.0 == c) && !copy;
    let new = |c: u64| !before.iter().any(|b| b.0 == c) && !copied.iter().any(|k| k.0 == c);
    let mut overwritten = Vec::new();
    for &(c, track, start, end, item) in before.iter().filter(|b| !moved(b.0)) {
        let kept = after.iter().find(|a| a.0 == c);
        if ins || kept.is_some_and(|k| (k.2, k.3) == (start, end)) {
            continue; // untouched (an insert only shifts clips, it never covers one)
        }
        let pieces: Vec<_> =
            after.iter().filter(|a| a.0 == c || (new(a.0) && !moved(a.0) && a.1 == track && a.4 == item && a.2 >= start && a.3 <= end)).collect();
        let now: i64 = pieces.iter().map(|a| (a.3 - a.2).0).sum();
        overwritten.push(json!({"clip": c, "was": (end - start).0, "now": now, "pieces": pieces.iter().map(|a| a.0).collect::<Vec<_>>()}));
    }
    if copy {
        return Ok(json!({"copied": copied.iter().map(|c| c.0).collect::<Vec<_>>(), "overwritten": overwritten}));
    }
    Ok(json!({"moved": moves.iter().map(|m| m.0.0).collect::<Vec<_>>(), "overwritten": overwritten}))
}

/// `timeline.select {"transitions": […]}`: select transitions (clicking one in the Timeline). It
/// replaces the clip, edit point and caption selections, as a clip selection replaces it.
fn select_transitions(s: &mut Session, ids: &Value, add: bool, toggle: bool) -> Result<Value> {
    const CMD: &str = "timeline.select";
    let ids = transition_ids_p(ids).ok_or_else(|| bad(CMD, "`transitions` is a list of transition ids"))?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    if let Some(missing) = ids.iter().find(|id| !q.all_tracks().any(|t| t.transitions.iter().any(|x| x.id == **id))) {
        return Err(bad(CMD, format!("no transition {}", missing.0)));
    }
    if toggle {
        for id in ids {
            match s.state.transition_selection.iter().position(|x| *x == id) {
                Some(i) => {
                    s.state.transition_selection.remove(i);
                }
                None => s.state.transition_selection.push(id),
            }
        }
    } else if add {
        for id in ids {
            if !s.state.transition_selection.contains(&id) {
                s.state.transition_selection.push(id);
            }
        }
    } else {
        s.state.transition_selection = ids;
    }
    s.state.selection.clear();
    s.state.caption_selection.clear();
    s.state.gap_selection = None;
    s.state.edit_points.clear();
    s.state.trim_shift = Default::default();
    Ok(json!({"selection": [], "transitionSelection": s.state.transition_selection.iter().map(|t| t.0).collect::<Vec<_>>()}))
}

/// `timeline.selectGap`: select the gap on `track` at `time` (clicking empty space between
/// clips, #648, #668). It replaces the clip, transition, caption and edit point selections, as
/// in Premiere Pro. Empty time after a track's last clip is not a gap.
fn select_gap(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "timeline.selectGap";
    let track = track_p(s, p, "track", CMD)?.ok_or_else(|| bad(CMD, "need `track`"))?;
    let t = time_p(s, p, "").ok_or_else(|| bad(CMD, "need `time`, `frame` or `seconds`"))?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let tr = q.track(track).ok_or_else(|| bad(CMD, format!("no track {}", track.0)))?;
    if tr.locked {
        return Err(EngineError::Edit(edit::EditError::Locked));
    }
    let range = edit::through::track_gaps(tr).into_iter().find(|g| g.contains(t)).ok_or_else(|| bad(CMD, "there is no gap there"))?;
    s.state.gap_selection = Some(crate::GapSelection { track, range });
    s.state.selection.clear();
    s.state.caption_selection.clear();
    s.state.transition_selection.clear();
    s.state.edit_points.clear();
    s.state.trim_shift = Default::default();
    Ok(json!({"track": track.0, "start": range.start.0, "end": range.end().0}))
}
