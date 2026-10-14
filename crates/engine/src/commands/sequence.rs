//! Sequence menu.

use super::*;

pub(super) fn commands() -> Vec<CommandSpec> {
    vec![
        cmd!("sequence.open", "Open in Timeline", [], None, r#"{"item":id}"#, always, |s, p| {
            let id = item_p(p, "item").ok_or_else(|| bad("sequence.open", "need `item`"))?;
            s.project.sequence(id).ok_or_else(|| bad("sequence.open", "not a sequence"))?;
            s.state.active_sequence = Some(id);
            if !s.state.open_sequences.contains(&id) {
                s.state.open_sequences.push(id);
            }
            s.state.selection.clear();
            s.events.push(crate::Event::OpenSequence(id));
            Ok(Value::Null)
        }),
        cmd!("sequence.close", "Close Sequence", [], None, r#"{"item":id?}"#, has_seq, |s, p| {
            let id = item_p(p, "item").or(s.state.active_sequence).ok_or(EngineError::NoSequence)?;
            s.state.open_sequences.retain(|x| *x != id);
            if s.state.active_sequence == Some(id) {
                s.state.active_sequence = s.state.open_sequences.last().copied();
            }
            Ok(Value::Null)
        }),
        cmd!("sequence.closeOthers", "Close Other Timeline Panels", [], None, r#"{"item":id?}"#, has_seq, |s, p| {
            let id = item_p(p, "item").or(s.state.active_sequence).ok_or(EngineError::NoSequence)?;
            if !s.state.open_sequences.contains(&id) {
                return Err(bad("sequence.closeOthers", "that sequence is not open"));
            }
            let closed = s.state.open_sequences.len().saturating_sub(1);
            s.state.open_sequences = vec![id];
            if s.state.active_sequence != Some(id) {
                s.state.active_sequence = Some(id);
                s.state.selection.clear();
                s.events.push(crate::Event::OpenSequence(id));
            }
            Ok(json!({"closed": closed}))
        }),
        cmd!("sequence.moveTab", "Move Sequence Tab", [], None, r#"{"item":id?,"index":int}"#, has_seq, |s, p| {
            let id = item_p(p, "item").or(s.state.active_sequence).ok_or(EngineError::NoSequence)?;
            let to = u64_p(p, "index").ok_or_else(|| bad("sequence.moveTab", "need `index`"))?;
            let from = s.state.open_sequences.iter().position(|x| *x == id).ok_or_else(|| bad("sequence.moveTab", "that sequence is not open"))?;
            // past the end is the end
            let to = usize::try_from(to).unwrap_or(usize::MAX).min(s.state.open_sequences.len().saturating_sub(1));
            let tab = s.state.open_sequences.remove(from);
            s.state.open_sequences.insert(to, tab);
            Ok(json!({"index": to}))
        }),
        cmd!("sequence.addEdit", "Add Edit", ["Sequence"], Some("Cmd+K"), r#"{"time":ticks?}"#, has_seq, |s, p| {
            let t = time_p(s, p, "").unwrap_or(s.playhead());
            // Like Premiere: selected clips under the playhead are cut, and only they (#164);
            // with none, the targeted tracks are.
            let sel = s.state.selection.clone();
            let selected_here =
                s.active_sequence().is_some_and(|q| sel.iter().any(|c| q.find_item(*c).is_some_and(|(_, it)| it.start < t && t < it.start + it.duration)));
            let n = if selected_here {
                s.edit_sequence("Add Edit", |q, ctx, _| Ok(edit::razor_items(q, &sel, t, ctx)))?
            } else {
                let tg = s.targeting().targeted;
                s.edit_sequence("Add Edit", |q, ctx, _| Ok(edit::razor(q, &tg, t, ctx)))?
            };
            Ok(json!({"cuts": n.len()}))
        }),
        cmd!("sequence.addEditAllTracks", "Add Edit to All Tracks", ["Sequence"], Some("Cmd+Shift+K"), r#"{"time":ticks?}"#, has_seq, |s, p| {
            let t = time_p(s, p, "").unwrap_or(s.playhead());
            let n = s.edit_sequence("Add Edit to All Tracks", |q, ctx, _| {
                let mut n = edit::razor(q, &[], t, ctx);
                n.extend(edit::captions::split_captions_at(q, &[], t, ctx));
                Ok(n)
            })?;
            Ok(json!({"cuts": n.len()}))
        }),
        cmd!("sequence.lift", "Lift", ["Sequence"], Some(";"), "{}", has_in_out, |s, _| {
            let r = in_out_range(s)?;
            let tg = s.targeting().targeted;
            s.edit_sequence("Lift", |q, ctx, _| Ok(edit::lift(q, &tg, r, ctx)))?;
            Ok(Value::Null)
        }),
        cmd!("sequence.extract", "Extract", ["Sequence"], Some("'"), "{}", has_in_out, |s, _| {
            let r = in_out_range(s)?;
            let tg = s.targeting().targeted;
            s.edit_sequence("Extract", |q, ctx, st| {
                edit::extract(q, &tg, r, ctx);
                if st.ripple_sequence_markers {
                    crate::sequence_tools::ripple_markers(&mut q.markers, r.end(), -r.duration);
                }
                q.mark_in = None;
                q.mark_out = None;
                q.split = Default::default();
                Ok(())
            })?;
            s.set_playhead(r.start);
            Ok(Value::Null)
        }),
        cmd!(
            "sequence.applyVideoTransition",
            "Apply Video Transition",
            ["Sequence"],
            Some("Cmd+D"),
            r#"{"clip":id?,"effect":"cross_dissolve|Cross Dissolve|…"?,"frames":i64?,"edge":"in"|"out"?,"params":{param:value}?,"reverse":bool?}"#,
            has_seq,
            |s, p| apply_transition(s, p, TrackKind::Video)
        ),
        cmd!(
            "sequence.applyAudioTransition",
            "Apply Audio Transition",
            ["Sequence"],
            Some("Cmd+Shift+D"),
            r#"{"clip":id?,"effect":"constant_power|Constant Power|…"?,"frames":i64?,"edge":"in"|"out"?,"params":{param:value}?,"reverse":bool?}"#,
            has_seq,
            |s, p| apply_transition(s, p, TrackKind::Audio)
        ),
        cmd!(
            "sequence.setTransition",
            "Edit Transition Settings",
            [],
            None,
            r#"{"transition":id,"params":{param:value}?,"reverse":bool?,"reset":bool?,"duration":ticks?|"frames":n?,"align":"center"|"start"|"end"?|"start":ticks?,"merge":bool?,"begin":bool?}"#,
            has_seq,
            set_transition
        ),
        cmd!("sequence.removeTransition", "Clear Transition", [], None, r#"{"transitions":[id]?}"#, has_seq, |s, p| {
            let ids: Vec<TransitionId> = match p.get("transitions").or_else(|| p.get("transition")) {
                Some(v) => transition_ids_p(v).ok_or_else(|| bad("sequence.removeTransition", "`transitions` is a list of transition ids"))?,
                None => s.state.transition_selection.clone(),
            };
            if ids.is_empty() {
                return Err(bad("sequence.removeTransition", "no transition given or selected"));
            }
            remove_transitions(s, &ids, "Clear Transition")
        }),
        cmd!("sequence.closeGap", "Close Gap", ["Sequence"], None, r#"{"track":"V1"|id?,"time":ticks?}"#, has_seq, |s, p| {
            // Shortcuts and menus have no parameters. Resolve that form from the selected gap
            // (#1135); explicit requests keep their existing track/time validation.
            if p.as_object().is_some_and(|p| p.is_empty()) {
                let gap = s.state.gap_selection.ok_or_else(|| bad("sequence.closeGap", "select a gap or provide `track`"))?;
                ripple_delete_gap(s, gap)?;
                return Ok(Value::Null);
            }
            let tr = track_p(s, p, "track", "sequence.closeGap")?.ok_or_else(|| bad("sequence.closeGap", "need `track`"))?;
            let t = time_p(s, p, "").unwrap_or(s.playhead());
            close_gap(s, tr, t)?;
            Ok(Value::Null)
        }),
        cmd!("sequence.snap", "Snap in Timeline", ["Sequence"], Some("S"), r#"{"on":bool?}"#, always, |s, p| {
            s.state.snapping = bool_p(p, "on").unwrap_or(!s.state.snapping);
            Ok(json!({"snapping": s.state.snapping}))
        }),
        cmd!("sequence.linkedSelection", "Linked Selection", ["Sequence"], None, r#"{"on":bool?}"#, always, |s, p| {
            s.state.linked_selection = bool_p(p, "on").unwrap_or(!s.state.linked_selection);
            Ok(json!({"linkedSelection": s.state.linked_selection}))
        }),
        cmd!("sequence.nestSequences", "Insert and overwrite sequences as nests or individual clips", [], None, r#"{"on":bool?}"#, always, |s, p| {
            let nest = bool_p(p, "on").unwrap_or(s.state.sequences_as_clips);
            s.state.sequences_as_clips = !nest;
            Ok(json!({"nest": nest}))
        }),
        cmd!(
            "sequence.addTracks",
            "Add Tracks…",
            ["Sequence"],
            None,
            r#"{"video":n=1,"audio":n=0,"submix":n=0,"videoAfter":"first"|"V2"|n?,"audioAfter":"first"|"A2"|n?,"submixAfter":"first"|"S1"|n?,"audioType":"standard|5.1|adaptive|mono"?,"submixType":"stereo|5.1|adaptive|mono"?}"#,
            has_seq,
            crate::sequence_tools::add_tracks
        ),
        cmd!("sequence.deleteTrack", "Delete Track", [], None, r#"{"track":"V3"|id}"#, has_seq, |s, p| {
            let tr = track_p(s, p, "track", "sequence.deleteTrack")?.ok_or_else(|| bad("sequence.deleteTrack", "need `track`"))?;
            s.edit_sequence("Delete Track", |q, _, _| {
                q.video_tracks.retain(|t| t.id != tr);
                q.audio_tracks.retain(|t| t.id != tr);
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!(
            "sequence.settings",
            "Sequence Settings…",
            ["Sequence"],
            None,
            r#"{"width":u32?,"height":u32?,"fps":f64?,"name":str?,"sampleRate":u32?,"mix":"Stereo|Mono|5.1|Adaptive"?,"dropFrame":bool?,"maxRenderQuality":bool?,"workingSpace":"rec709"|"rec2100-pq"|"rec2100-hlg"?,"wideGamut":bool?,"autoToneMap":bool?,"scaleMotion":bool=false}"#,
            has_seq,
            |s, p| {
                const CMD: &str = "sequence.settings";
                let id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
                let fps = fps_p(p, CMD)?;
                let mix = match str_p(p, "mix") {
                    Some(m) => Some(crate::mixer::channels_from(m).ok_or_else(|| bad(CMD, format!("unknown mix `{m}` (Stereo, Mono, 5.1, Adaptive)")))?),
                    None => None,
                };
                let working = match str_p(p, "workingSpace") {
                    Some(w) => Some(filmcraft_color::WorkingSpace::parse(w).ok_or_else(|| bad(CMD, format!("unknown working space `{w}`")))?),
                    None => None,
                };
                let drop_frame = bool_p(p, "dropFrame");
                let rate = fps.or_else(|| s.active_sequence().map(|q| q.settings.frame_rate)).unwrap_or(FrameRate::FPS_23_976);
                if drop_frame == Some(true) && !rate.supports_drop_frame() {
                    return Err(bad(CMD, format!("drop-frame timecode needs a 29.97, 59.94 or 119.88 fps timebase, not {rate}")));
                }
                let scale_motion = bool_p(p, "scaleMotion").unwrap_or(false);
                let p = p.clone();
                let scaled = s.edit("Sequence Settings", |pr, _| {
                    if let Some(n) = str_p(&p, "name") {
                        pr.item_mut(id).ok_or(EngineError::NoSequence)?.name = n.to_string();
                    }
                    let q = pr.sequence_mut(id).ok_or(EngineError::NoSequence)?;
                    let old_size = (q.settings.width, q.settings.height);
                    if let Some(w) = checked_u32_p(&p, "width", CMD)? {
                        q.settings.width = w;
                    }
                    if let Some(h) = checked_u32_p(&p, "height", CMD)? {
                        q.settings.height = h;
                    }
                    if let Some(f) = fps {
                        q.settings.frame_rate = f;
                    }
                    if let Some(sr) = checked_u32_p(&p, "sampleRate", CMD)? {
                        q.settings.sample_rate = sr;
                    }
                    if let Some(m) = mix {
                        q.settings.audio_master = m;
                    }
                    if let Some(df) = drop_frame {
                        q.settings.drop_frame = df;
                    }
                    if let Some(v) = bool_p(&p, "maxRenderQuality") {
                        q.settings.max_render_quality = v;
                    }
                    let c = &mut q.settings.color;
                    if let Some(w) = working {
                        c.working = w;
                    }
                    if let Some(v) = bool_p(&p, "wideGamut") {
                        c.wide_gamut = v;
                    }
                    if let Some(v) = bool_p(&p, "autoToneMap") {
                        c.auto_tone_map = v;
                    }
                    q.settings.working_space = q.settings.color.working.label().into();
                    q.settings.validate().map_err(|e| bad(CMD, e))?;
                    let new_size = (q.settings.width, q.settings.height);
                    Ok(if scale_motion { edit::frame_size::scale_motion(q, old_size, new_size) } else { 0 })
                })?;
                Ok(json!({"scaledClips": scaled}))
            }
        ),
        cmd!("sequence.renderEffectsInToOut", "Render Effects In to Out", ["Sequence"], Some("Enter"), r#"{"wait":bool=false}"#, has_seq, |s, p| {
            crate::previews::render(s, crate::previews::RenderMode::EffectsInToOut, p)
        }),
        cmd!("sequence.renderInToOut", "Render In to Out", ["Sequence"], None, r#"{"wait":bool=false}"#, has_seq, |s, p| {
            crate::previews::render(s, crate::previews::RenderMode::InToOut, p)
        }),
        cmd!("sequence.renderSelection", "Render Selection", ["Sequence"], None, r#"{"wait":bool=false}"#, has_selection, |s, p| {
            crate::previews::render(s, crate::previews::RenderMode::Selection, p)
        }),
        cmd!("sequence.renderAudio", "Render Audio", ["Sequence"], None, r#"{"wait":bool=false}"#, has_seq, |s, p| crate::previews::render_audio(s, p)),
        cmd!("sequence.deleteRenderFiles", "Delete Render Files", ["Sequence"], None, "{}", has_previews, |s, _| crate::previews::delete(s, false)),
        cmd!("sequence.deleteRenderFilesInToOut", "Delete Render Files In to Out", ["Sequence"], None, "{}", has_in_out, |s, _| {
            crate::previews::delete(s, true)
        }),
        query!("sequence.renderBar", "Render Bar", "{}", |s, _| crate::previews::bar_json(s)),
        cmd!("sequence.matchFrame", "Match Frame", ["Sequence"], Some("F"), "{}", has_seq, |s, _| {
            let t = s.playhead();
            let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
            let hit = seq.video_tracks.iter().rev().chain(seq.audio_tracks.iter()).find_map(|tr| tr.item_at(t).map(|i| (i.item, i.source_time_at(t))));
            let (item, st) = hit.ok_or_else(|| EngineError::Other("no clip at the playhead".into()))?;
            s.state.source_item = Some(item);
            s.state.source_playhead = st;
            s.events.push(crate::Event::OpenSource(item));
            Ok(json!({"item": item.0}))
        }),
    ]
}

fn has_in_out(s: &Session) -> std::result::Result<(), String> {
    let seq = s.active_sequence().ok_or("no sequence is open")?;
    if seq.mark_in.is_some() || seq.mark_out.is_some() { Ok(()) } else { Err("mark an In or Out point first".into()) }
}

fn has_previews(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    if s.previews.count() == 0 { Err("there are no render files".into()) } else { Ok(()) }
}

fn in_out_range(s: &Session) -> Result<TimeRange> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let fd = q.settings.frame_rate.frame_duration();
    let a = q.mark_in.unwrap_or(Tick::ZERO);
    let b = q.mark_out.map(|o| o + fd).unwrap_or(q.duration());
    if b <= a {
        return Err(EngineError::Other("In point is after Out point".into()));
    }
    Ok(TimeRange::from_bounds(a, b))
}

/// `sequence.setTransition`: edit an applied transition (Effect Controls, the Timeline): its
/// settings, Reverse, and where it sits. `duration` / `frames` resize it the way its alignment
/// says (Center at Cut and Custom Start: both ends; Start at Cut: the end; End at Cut: the
/// beginning), `align` re-aligns it on its cut, `start` moves it (dragging it over the cut; with
/// `duration`, dragging one edge). One undo step; with `merge`, a drag's steps share one.
fn set_transition(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "sequence.setTransition";
    let id = p.get("transition").and_then(Value::as_u64).ok_or_else(|| bad(CMD, "need `transition`"))?;
    let tid = TransitionId(id);
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let rate = q.settings.frame_rate.sane();
    let frame = rate.frame_duration();
    let found = q.all_tracks().find_map(|tr| tr.transitions.iter().find(|x| x.id == tid).map(|x| (tr, x)));
    let (track, cur) = found.ok_or_else(|| bad(CMD, format!("no transition {id}")))?;
    let mut cur = cur.clone();
    let (lo, cut, hi) = edit::transitions::bounds(track, &cur).ok_or_else(|| bad(CMD, "the transition's clips are gone"))?;
    // times outside the clips it joins are refused before any frame arithmetic (hostile input)
    let room = Tick(hi.0.saturating_sub(lo.0));
    let duration = match (p.get("frames").and_then(Value::as_i64), p.get("duration").and_then(Value::as_i64)) {
        (Some(_), Some(_)) => return Err(bad(CMD, "give `duration` or `frames`, not both")),
        (Some(f), None) if (1..=10_000_000).contains(&f) && rate.tick_of(f) <= room => Some(rate.tick_of(f)),
        (None, Some(d)) if d > 0 && d <= room.0 => Some(rate.snap_nearest(Tick(d)).max(frame)),
        (Some(_), None) | (None, Some(_)) => return Err(bad(CMD, "a transition lasts at least one frame and can't extend past the clips it joins")),
        (None, None) => None,
    };
    let align = match str_p(p, "align") {
        Some(a) => Some(edit::transitions::Alignment::parse(a).ok_or_else(|| bad(CMD, format!("`align` is center, start or end, not `{a}`")))?),
        None => None,
    };
    let start = match p.get("start").and_then(Value::as_i64) {
        Some(t) if (lo.0..=hi.0).contains(&t) => Some(rate.snap_nearest(Tick(t))),
        Some(_) => return Err(bad(CMD, "a transition can't extend past the clips it joins")),
        None => None,
    };
    if start.is_some() && align.is_some() {
        return Err(bad(CMD, "give `start` or `align`, not both"));
    }
    let span = if duration.is_some() || align.is_some() || start.is_some() {
        let d = duration.unwrap_or(cur.duration);
        let st = match start {
            Some(st) => st,
            None => {
                let al = align.unwrap_or_else(|| edit::transitions::alignment(&cur, cut, frame));
                edit::transitions::start_for(q, tid, d, al, frame, |t| rate.snap(t)).ok_or_else(|| bad(CMD, format!("no transition {id}")))?
            }
        };
        Some((st, d))
    } else {
        None
    };
    if p.get("reset").and_then(Value::as_bool).unwrap_or(false)
        && let Some(def) = cur.effect.def()
    {
        cur.effect = def.instance();
    }
    if let Some(params) = p.get("params") {
        set_transition_params(&mut cur.effect, params, CMD)?;
    }
    if let Some(r) = p.get("reverse").and_then(Value::as_bool) {
        cur.reverse = r;
    }
    let name = cur.effect.def().map_or("Transition", |d| d.name).to_string();
    if bool_p(p, "begin").unwrap_or(false) {
        s.history.merge_key = None;
    }
    let merge = bool_p(p, "merge").unwrap_or(false).then(|| format!("setTransition:{id}"));
    let after = s.edit_sequence_as(&format!("Edit {name}"), merge.as_deref(), |q, _, _| {
        if let Some((st, d)) = span {
            edit::transitions::set_span(q, tid, st, d, frame, frame)?;
        }
        let mut after = None;
        for tr in q.video_tracks.iter_mut().chain(q.audio_tracks.iter_mut()) {
            if let Some(x) = tr.transitions.iter_mut().find(|x| x.id == tid) {
                x.effect = cur.effect.clone();
                x.reverse = cur.reverse;
                after = Some((x.start, x.duration));
            }
        }
        Ok(after)
    })?;
    let (st, d) = after.unwrap_or((cur.start, cur.duration));
    let al = s
        .active_sequence()
        .and_then(|q| {
            q.all_tracks().find_map(|tr| tr.transitions.iter().find(|x| x.id == tid).and_then(|x| edit::transitions::bounds(tr, x).map(|b| (x.clone(), b.1))))
        })
        .map(|(x, cut)| edit::transitions::alignment(&x, cut, frame).id());
    Ok(json!({"transition": id, "effect": cur.effect.effect, "reverse": cur.reverse, "start": st.0, "duration": d.0, "align": al}))
}
