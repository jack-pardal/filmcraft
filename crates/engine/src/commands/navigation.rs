//! Playhead navigation and the Source monitor.

use super::*;

pub(super) fn commands() -> Vec<CommandSpec> {
    vec![
        cmd!("playhead.set", "Set Playhead", [], None, r#"{"time":ticks|"frame":i64|"seconds":f64|"timecode":str}"#, has_seq, |s, p| {
            let t = time_p(s, p, "").ok_or_else(|| bad("playhead.set", "need time/frame/seconds/timecode"))?;
            s.set_playhead(t);
            Ok(json!({"time": s.playhead().0}))
        }),
        cmd!("playhead.step", "Step Frames", [], None, r#"{"frames":i64}"#, has_seq, |s, p| {
            let n = p.get("frames").and_then(Value::as_i64).unwrap_or(1);
            let r = s.sequence_rate();
            let f = r.frame_at(s.playhead()).checked_add(n).ok_or_else(|| bad("playhead.step", "frame offset is too large"))?.max(0);
            if f > r.frame_at(Tick::MAX) {
                return Err(bad("playhead.step", "target frame exceeds the supported time range"));
            }
            s.set_playhead(r.tick_of(f));
            Ok(json!({"frame": r.frame_at(s.playhead())}))
        }),
        cmd!("playhead.stepForward", "Step Forward One Frame", [], Some("Right"), "{}", has_seq, |s, _| s.execute("playhead.step", json!({"frames": 1}))),
        cmd!("playhead.stepBack", "Step Back One Frame", [], Some("Left"), "{}", has_seq, |s, _| s.execute("playhead.step", json!({"frames": -1}))),
        // Settings ▸ Playback ▸ Step forward/back many
        cmd!("playhead.stepForward5", "Step Forward Five Frames", [], Some("Shift+Right"), "{}", has_seq, |s, _| {
            let n = s.prefs.playback.step_many_frames as i64;
            s.execute("playhead.step", json!({"frames": n}))
        }),
        cmd!("playhead.stepBack5", "Step Back Five Frames", [], Some("Shift+Left"), "{}", has_seq, |s, _| {
            let n = s.prefs.playback.step_many_frames as i64;
            s.execute("playhead.step", json!({"frames": -n}))
        }),
        // Up / Down: edit points on the targeted tracks (Shift+Up / Shift+Down: any track)
        cmd!("playhead.nextEdit", "Go to Next Edit Point", [], Some("Down"), "{}", has_seq, |s, _| crate::keyboard::go_to_edit(s, true, false)),
        cmd!("playhead.prevEdit", "Go to Previous Edit Point", [], Some("Up"), "{}", has_seq, |s, _| crate::keyboard::go_to_edit(s, false, false)),
        cmd!("playhead.start", "Go to Sequence Start", [], Some("Home"), "{}", has_seq, |s, _| {
            s.set_playhead(Tick::ZERO);
            Ok(Value::Null)
        }),
        cmd!("playhead.end", "Go to Sequence End", [], Some("End"), "{}", has_seq, |s, _| {
            let e = s.active_sequence().map(|q| q.duration()).unwrap_or_default();
            s.set_playhead(e);
            Ok(Value::Null)
        }),
        // ================= Source monitor =================
        cmd!("source.open", "Open in Source Monitor", [], None, r#"{"item":id?,"clip":id?}"#, always, |s, p| {
            if let Some(clip) = clip_p(p, "clip") {
                return crate::source_monitor::open_clip(s, clip);
            }
            let id = item_p(p, "item")
                .or(s.state.project_selection.first().copied())
                .ok_or_else(|| bad("source.open", "need `item` (or a Project panel selection)"))?;
            s.project.item(id).ok_or_else(|| bad("source.open", "no such item"))?;
            s.state.source_item = Some(id);
            // a subclip opens at its In point
            s.state.source_playhead = crate::clip_ops::source_view(s, id).map(|v| v.start).unwrap_or_default();
            s.events.push(crate::Event::OpenSource(id));
            Ok(Value::Null)
        }),
        // Source panel: Close / Close All, for the clip shown and the list of recent clips (#313)
        cmd!("source.close", "Close", [], None, "{}", crate::source_monitor::has_source_clip, |s, _| crate::source_monitor::close(s, false)),
        cmd!("source.closeAll", "Close All", [], None, "{}", crate::source_monitor::has_source_clip, |s, _| crate::source_monitor::close(s, true)),
        cmd!("source.setPlayhead", "Set Source Playhead", [], None, r#"{"time":ticks|"frame":i64|"seconds":f64}"#, always, |s, p| {
            let item = s.state.source_item.ok_or_else(|| EngineError::Other("no source clip".into()))?;
            let view = crate::clip_ops::source_view(s, item);
            let rate = view.as_ref().map(|v| v.rate).unwrap_or_default();
            let t = p
                .get("time")
                .and_then(Value::as_i64)
                .map(Tick)
                .or_else(|| p.get("frame").and_then(Value::as_i64).map(|f| rate.tick_of(f)))
                .or_else(|| f64_p(p, "seconds").map(Tick::from_seconds_f64))
                .unwrap_or_default();
            // the Source monitor's span: a subclip that restricts trims keeps the playhead inside it
            let (lo, hi) = view.map(|v| (v.start, v.end)).unwrap_or((Tick::ZERO, Tick::MAX));
            s.state.source_playhead = rate.snap(t.clamp(lo, (hi - rate.frame_duration()).max(lo))).max(lo);
            Ok(json!({"time": s.state.source_playhead.0}))
        }),
        query!("source.inspect", "Inspect Source Monitor", "{}", |s, _| {
            let Some(item) = s.state.source_item else { return Ok(json!({"item": null})) };
            let v = crate::clip_ops::source_view(s, item).ok_or_else(|| bad("source.inspect", "the source item is gone"))?;
            Ok(v.to_json(s.state.source_playhead))
        }),
        cmd!("source.insert", "Insert", ["Clip"], Some(","), "{}", has_source, |s, _| edit_at_source(s, true)),
        cmd!("source.overwrite", "Overwrite", ["Clip"], Some("."), "{}", has_source, |s, _| edit_at_source(s, false)),
    ]
}

fn edit_at_source(s: &mut Session, insert: bool) -> Result<Value> {
    let (item, range) = source_range(s).ok_or_else(|| EngineError::Other("no source clip".into()))?;
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    // Three-point editing: sequence In wins over the playhead.
    let at = seq.mark_in.unwrap_or(s.playhead());
    let tg = s.targeting();
    let label = if insert { "Insert" } else { "Overwrite" };
    // split points: each channel takes its own range, offset by the difference of the In points
    let (vr, ar) = crate::sequence_tools::split_source_ranges(s, item, range);
    let base = vr.start.min(ar.start);
    let (vat, aat) = (at + (vr.start - base), at + (ar.start - base));
    let ids = if vr == ar {
        place_item(s, item, range, at, tg.video_dest, tg.audio_dest, insert, label, None)?
    } else {
        place_item(s, item, vr, vat, tg.video_dest, tg.audio_dest, insert, label, Some((ar, aat)))?
    };
    let end =
        if vr == ar { at + s.sequence_rate().snap_nearest(range.duration) } else { s.sequence_rate().snap_nearest((vat + vr.duration).max(aat + ar.duration)) };
    s.set_playhead(end);
    Ok(json!({"clips": ids.iter().map(|c| c.0).collect::<Vec<_>>()}))
}

fn has_source(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    s.state.source_item.map(|_| ()).ok_or_else(|| "no clip in the Source monitor".into())
}
