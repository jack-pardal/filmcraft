//! Markers menu and In / Out marks.

use super::*;

pub(super) fn commands() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "markers.markIn",
            "Mark In",
            ["Markers"],
            Some("I"),
            r#"{"time":ticks?,"frame":i64?,"seconds":f64?,"timecode":str?,"target":"program|source"}"#,
            always,
            |s, p| mark(s, p, true)
        ),
        cmd!(
            "markers.markOut",
            "Mark Out",
            ["Markers"],
            Some("O"),
            r#"{"time":ticks?,"frame":i64?,"seconds":f64?,"timecode":str?,"target":"program|source"}"#,
            always,
            |s, p| mark(s, p, false)
        ),
        cmd!("markers.markClip", "Mark Clip", ["Markers"], Some("X"), "{}", has_seq, |s, _| {
            let t = s.playhead();
            let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
            let tg = s.targeting().targeted;
            let r = seq
                .all_tracks()
                .filter(|tr| tg.contains(&tr.id))
                .find_map(|tr| tr.item_at(t).map(|i| i.range()))
                .ok_or_else(|| EngineError::Other("no clip at the playhead".into()))?;
            let fd = s.sequence_rate().frame_duration();
            s.edit_sequence("Mark Clip", |q, _, _| {
                q.mark_in = Some(r.start);
                q.mark_out = Some(r.end() - fd);
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!("markers.markSelection", "Mark Selection", ["Markers"], Some("/"), "{}", has_selection, |s, _| {
            let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
            let sel = s.state.selection.clone();
            let items: Vec<_> = sel.iter().filter_map(|c| seq.find_item(*c).map(|(_, i)| i.range())).collect();
            let a = items.iter().map(|r| r.start).min().unwrap_or_default();
            let b = items.iter().map(|r| r.end()).max().unwrap_or_default();
            let fd = s.sequence_rate().frame_duration();
            s.edit_sequence("Mark Selection", |q, _, _| {
                q.mark_in = Some(a);
                q.mark_out = Some(b - fd);
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        // With no In / Out mark, like Premiere: the start / end of the sequence (#208).
        cmd!("markers.goToIn", "Go to In", ["Markers"], Some("Shift+I"), r#"{"target":"program|source"}"#, always, |s, p| {
            if let Some(v) = crate::source_monitor::route(s, "markers.goToIn", p)? {
                return Ok(v);
            }
            s.active_sequence().ok_or(EngineError::NoSequence)?;
            if let Some(i) = s.active_sequence().map(|q| q.mark_in.unwrap_or(Tick::ZERO)) {
                s.set_playhead(i);
            }
            Ok(Value::Null)
        }),
        cmd!("markers.goToOut", "Go to Out", ["Markers"], Some("Shift+O"), r#"{"target":"program|source"}"#, always, |s, p| {
            if let Some(v) = crate::source_monitor::route(s, "markers.goToOut", p)? {
                return Ok(v);
            }
            s.active_sequence().ok_or(EngineError::NoSequence)?;
            if let Some(o) = s.active_sequence().map(|q| q.mark_out.unwrap_or_else(|| q.duration())) {
                s.set_playhead(o);
            }
            Ok(Value::Null)
        }),
        cmd!("markers.clearIn", "Clear In", ["Markers"], Some("Cmd+Shift+I"), r#"{"target":"program|source"}"#, always, |s, p| {
            if let Some(v) = crate::source_monitor::route(s, "markers.clearIn", p)? {
                return Ok(v);
            }
            s.edit_sequence("Clear In", |q, _, _| {
                q.mark_in = None;
                q.split.video_in = None;
                q.split.audio_in = None;
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!("markers.clearOut", "Clear Out", ["Markers"], Some("Cmd+Shift+O"), r#"{"target":"program|source"}"#, always, |s, p| {
            if let Some(v) = crate::source_monitor::route(s, "markers.clearOut", p)? {
                return Ok(v);
            }
            s.edit_sequence("Clear Out", |q, _, _| {
                q.mark_out = None;
                q.split.video_out = None;
                q.split.audio_out = None;
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!("markers.clearInOut", "Clear In and Out", ["Markers"], Some("Cmd+Shift+X"), r#"{"target":"program|source"}"#, always, |s, p| {
            if let Some(v) = crate::source_monitor::route(s, "markers.clearInOut", p)? {
                return Ok(v);
            }
            s.edit_sequence("Clear In and Out", |q, _, _| {
                q.mark_in = None;
                q.mark_out = None;
                q.split = Default::default();
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!(
            "markers.add",
            "Add Marker",
            ["Markers"],
            Some("M"),
            r#"{"target":"program|source","time":ticks?,"name":str?,"comment":str?,"color":label?,"durationFrames":i64?}"#,
            always,
            |s, p| {
                if let Some(v) = crate::source_monitor::route(s, "markers.add", p)? {
                    return Ok(v);
                }
                let (_seq, ph) = marker_list_mut(s)?;
                let t = sequence_time_p(s, p, "", "markers.add")?.unwrap_or(ph);
                let name = str_p(p, "name").unwrap_or("").to_string();
                let comment = str_p(p, "comment").unwrap_or("").to_string();
                let color = marker_color_p(p, "markers.add")?.unwrap_or(Label::Green);
                let dur = p.get("durationFrames").and_then(Value::as_i64).map(|f| s.sequence_rate().tick_of(f)).unwrap_or_default();
                let id = s.edit_sequence("Add Marker", |q, ctx, _| {
                    let id = MarkerId(ctx.alloc());
                    q.markers.push(Marker { id, start: t, duration: dur, name, comment, kind: MarkerKind::Comment, color });
                    q.markers.sort_by_key(|m| m.start);
                    Ok(id)
                })?;
                Ok(json!({"marker": id.0}))
            }
        ),
        cmd!("markers.goNext", "Go to Next Marker", ["Markers"], Some("Shift+M"), r#"{"target":"program|source"}"#, always, |s, p| {
            if let Some(v) = crate::source_monitor::route(s, "markers.goNext", p)? {
                return Ok(v);
            }
            let t = s.playhead();
            s.active_sequence().ok_or(EngineError::NoSequence)?;
            if let Some(m) = s.active_sequence().and_then(|q| q.markers.iter().map(|m| m.start).find(|m| *m > t)) {
                s.set_playhead(m);
            }
            Ok(Value::Null)
        }),
        cmd!("markers.goPrev", "Go to Previous Marker", ["Markers"], Some("Cmd+Shift+M"), r#"{"target":"program|source"}"#, always, |s, p| {
            if let Some(v) = crate::source_monitor::route(s, "markers.goPrev", p)? {
                return Ok(v);
            }
            let t = s.playhead();
            s.active_sequence().ok_or(EngineError::NoSequence)?;
            if let Some(m) = s.active_sequence().and_then(|q| q.markers.iter().rev().map(|m| m.start).find(|m| *m < t)) {
                s.set_playhead(m);
            }
            Ok(Value::Null)
        }),
        cmd!("markers.clearCurrent", "Clear Selected Marker", ["Markers"], Some("Cmd+Alt+M"), r#"{"target":"program|source"}"#, always, |s, p| {
            if let Some(v) = crate::source_monitor::route(s, "markers.clearCurrent", p)? {
                return Ok(v);
            }
            let t = s.playhead();
            s.edit_sequence("Clear Marker", |q, _, _| {
                q.markers.retain(|m| m.start != t);
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!(
            "markers.exportCsv",
            "Export Markers as CSV…",
            ["Markers"],
            None,
            r#"{"path":str} (all markers in the active sequence)"#,
            has_seq,
            crate::marker_export::export
        ),
        cmd!("markers.clearAll", "Clear Markers", ["Markers"], Some("Cmd+Alt+Shift+M"), r#"{"target":"program|source"}"#, always, |s, p| {
            if let Some(v) = crate::source_monitor::route(s, "markers.clearAll", p)? {
                return Ok(v);
            }
            s.edit_sequence("Clear All Markers", |q, _, _| {
                q.markers.clear();
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!(
            "markers.edit",
            "Edit Marker…",
            [],
            None,
            r#"{"target":"program|source","marker":id,"name":str?,"comment":str?,"color":label?,"durationFrames":i64?}"#,
            always,
            |s, p| {
                if let Some(v) = crate::source_monitor::route(s, "markers.edit", p)? {
                    return Ok(v);
                }
                let id = u64_p(p, "marker").map(MarkerId).ok_or_else(|| bad("markers.edit", "need `marker`"))?;
                let rate = s.sequence_rate();
                let color = marker_color_p(p, "markers.edit")?;
                let p = p.clone();
                s.edit_sequence("Edit Marker", |q, _, _| {
                    let m = q.markers.iter_mut().find(|m| m.id == id).ok_or_else(|| bad("markers.edit", "no such marker"))?;
                    if let Some(n) = str_p(&p, "name") {
                        m.name = n.into();
                    }
                    if let Some(c) = str_p(&p, "comment") {
                        m.comment = c.into();
                    }
                    if let Some(c) = color {
                        m.color = c;
                    }
                    if let Some(f) = p.get("durationFrames").and_then(Value::as_i64) {
                        m.duration = rate.tick_of(f);
                    }
                    Ok(())
                })?;
                Ok(Value::Null)
            }
        ),
    ]
}

fn marker_list_mut(s: &mut Session) -> Result<(ItemId, Tick)> {
    let seq = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    Ok((seq, s.playhead()))
}

fn mark(s: &mut Session, p: &Value, is_in: bool) -> Result<Value> {
    let target_source = str_p(p, "target") == Some("source");
    if target_source {
        let item = s.state.source_item.ok_or_else(|| EngineError::Other("no source clip".into()))?;
        let t = source_mark_time(s, p, item);
        let key = if is_in { "in" } else { "out" };
        return s.execute("project.setMarks", json!({"item": item.0, key: t.0}));
    }
    let t = time_p(s, p, "").unwrap_or(s.playhead());
    s.edit_sequence(if is_in { "Mark In" } else { "Mark Out" }, |q, _, _| {
        if is_in {
            q.mark_in = Some(t);
            q.split.video_in = None;
            q.split.audio_in = None;
            if q.mark_out.is_some_and(|o| o < t) {
                q.mark_out = None;
            }
        } else {
            q.mark_out = Some(t);
            q.split.video_out = None;
            q.split.audio_out = None;
            if q.mark_in.is_some_and(|i| i > t) {
                q.mark_in = None;
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}
