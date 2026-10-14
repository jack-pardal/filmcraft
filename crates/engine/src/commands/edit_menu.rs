//! Edit menu: undo / redo, clipboard, selection.

use super::*;

pub(super) fn commands() -> Vec<CommandSpec> {
    vec![
        cmd!("edit.undo", "Undo", ["Edit"], Some("Cmd+Z"), "{}", can_undo, |s, _| Ok(json!({"undone": s.undo()}))),
        cmd!("edit.redo", "Redo", ["Edit"], Some("Cmd+Shift+Z"), "{}", can_redo, |s, _| Ok(json!({"redone": s.redo()}))),
        cmd!("edit.cut", "Cut", ["Edit"], Some("Cmd+X"), "{}", has_selection, |s, _| {
            copy_selection(s);
            let sel = with_links(s, &s.state.selection.clone());
            s.edit_sequence("Cut", |q, _, st| {
                edit::delete_items(q, &sel);
                st.selection.clear();
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!("edit.copy", "Copy", ["Edit"], Some("Cmd+C"), "{}", has_selection, |s, _| {
            copy_selection(s);
            Ok(json!({"copied": s.state.clipboard.len()}))
        }),
        cmd!("edit.paste", "Paste", ["Edit"], Some("Cmd+V"), "{}", has_clipboard, |s, _| paste(s, false)),
        cmd!("edit.pasteInsert", "Paste Insert", ["Edit"], Some("Cmd+Shift+V"), "{}", has_clipboard, |s, _| paste(s, true)),
        cmd!("edit.clear", "Clear", ["Edit"], Some("Backspace"), r#"{"clips":[id]?}"#, has_any_selection, |s, p| {
            if p.get("clips").is_none() && p.get("clip").is_none() && s.state.selection.is_empty() {
                // a selected transition (clicked in the Timeline) goes, as a clip would
                if !s.state.transition_selection.is_empty() {
                    let ids = s.state.transition_selection.clone();
                    return remove_transitions(s, &ids, "Clear");
                }
                // a selected gap closes, as in Premiere Pro (#648, #668)
                if s.state.caption_selection.is_empty()
                    && let Some(g) = s.state.gap_selection
                {
                    return ripple_delete_gap(s, g);
                }
                let caps = s.state.caption_selection.clone();
                return crate::captions::delete(s, &caps, false);
            }
            let sel = with_links(s, &clips_p(s, p));
            s.edit_sequence("Clear", |q, _, st| {
                edit::delete_items(q, &sel);
                st.selection.clear();
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!("edit.rippleDelete", "Ripple Delete", ["Edit"], Some("Shift+Delete"), r#"{"clips":[id]?}"#, has_clip_or_caption_selection, |s, p| {
            if p.get("clips").is_none() && p.get("clip").is_none() && s.state.selection.is_empty() {
                if s.state.caption_selection.is_empty()
                    && let Some(g) = s.state.gap_selection
                {
                    return ripple_delete_gap(s, g);
                }
                let caps = s.state.caption_selection.clone();
                return crate::captions::delete(s, &caps, true);
            }
            let sel = with_links(s, &clips_p(s, p));
            s.edit_sequence("Ripple Delete", |q, _, st| {
                let spans = edit::ripple_delete_items(q, &sel)?;
                if st.ripple_sequence_markers {
                    for r in spans.iter().rev() {
                        crate::sequence_tools::ripple_markers(&mut q.markers, r.end(), -r.duration);
                    }
                }
                st.selection.clear();
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!("edit.rippleDeleteHead", "Ripple Delete Head Frame", ["Edit"], None, r#"{"clips":[id]?,"clip":id?}"#, has_selection, |s, p| {
            // #1357: remove the head — from each selected clip's in point through the playhead —
            // and ripple the sequence closed. Linked clips ripple as one group (the engine's
            // group trim moves every member's in point by one delta and shifts the synced
            // tracks once). The playhead past every member's out point deletes the group whole.
            let ph = s.playhead();
            let clips = with_links(s, &clips_p(s, p));
            // groups keyed by link id (unlinked clips: their own id)
            let mut groups: Vec<(u64, Vec<ClipId>)> = Vec::new();
            {
                let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
                for c in &clips {
                    let (_, item) = q.find_item(*c).ok_or_else(|| EngineError::Other("clip not in the active sequence".into()))?;
                    if (ph - item.start) <= Tick::ZERO {
                        return Err(EngineError::Other("the playhead is at or before the clip's in point".into()));
                    }
                    let key = item.link.unwrap_or(c.0);
                    match groups.iter_mut().find(|(k, _)| *k == key) {
                        Some((_, members)) => members.push(*c),
                        None => groups.push((key, vec![*c])),
                    }
                }
            }
            s.edit_sequence("Ripple Delete Head Frame", |q, ctx, st| {
                for (_, members) in &groups {
                    let live: Vec<(ClipId, Tick, Tick)> = members.iter().filter_map(|c| q.find_item(*c).map(|(_, i)| (*c, i.start, i.duration))).collect();
                    if live.is_empty() {
                        continue;
                    }
                    let head = live.iter().map(|(_, s, _)| (ph - *s).max(Tick::ZERO)).max().unwrap_or(Tick::ZERO);
                    let all_whole = live.iter().all(|(_, _, d)| head >= *d);
                    let ids: Vec<ClipId> = live.iter().map(|(c, _, _)| *c).collect();
                    if all_whole {
                        let spans = edit::ripple_delete_items(q, &ids)?;
                        if st.ripple_sequence_markers {
                            for r in spans.iter().rev() {
                                crate::sequence_tools::ripple_markers(&mut q.markers, r.end(), -r.duration);
                            }
                        }
                        continue;
                    }
                    let start = live.iter().map(|(_, s, _)| *s).min().unwrap_or(ph);
                    edit::ripple_trim_group(q, &ids, Edge::In, head, ctx)?;
                    if st.ripple_sequence_markers {
                        // the head [start, start + head) closes: later markers move back by it
                        crate::sequence_tools::ripple_markers(&mut q.markers, start + head, Tick::ZERO - head);
                    }
                }
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!("edit.selectAll", "Select All", ["Edit"], Some("Cmd+A"), "{}", has_seq, |s, _| {
            let all: Vec<ClipId> = s.active_sequence().map(|q| q.all_tracks().flat_map(|t| t.items.iter().map(|i| i.id)).collect()).unwrap_or_default();
            s.state.selection = all;
            s.state.gap_selection = None;
            Ok(json!({"selected": s.state.selection.len()}))
        }),
        cmd!("edit.deselectAll", "Deselect All", ["Edit"], Some("Cmd+Shift+A"), "{}", always, |s, _| {
            s.state.selection.clear();
            s.state.caption_selection.clear();
            s.state.gap_selection = None;
            Ok(Value::Null)
        }),
        cmd!("edit.duplicate", "Duplicate", ["Edit"], Some("Cmd+Shift+/"), "{}", has_project_selection, |s, _| {
            let sel = s.state.project_selection.clone();
            let ids = s.edit("Duplicate", |p, st| {
                let mut out = Vec::new();
                for id in sel {
                    if let Some(it) = p.item(id).cloned() {
                        let nid = p.add_item(&format!("{} Copy", it.name), it.label, it.kind.clone(), None);
                        out.push(nid);
                    }
                }
                st.project_selection = out.clone();
                Ok(out)
            })?;
            Ok(json!({"items": ids.iter().map(|i| i.0).collect::<Vec<_>>()}))
        }),
        cmd!("edit.label", "Label", [], None, r#"{"label":"Violet|Iris|…"}"#, always, |s, p| {
            let l = str_p(p, "label").and_then(Label::from_name).ok_or_else(|| bad("edit.label", "unknown label"))?;
            let clips = s.state.selection.clone();
            let items = s.state.project_selection.clone();
            let seq = s.state.active_sequence;
            s.edit("Label", |pr, _| {
                for i in &items {
                    if let Some(it) = pr.item_mut(*i) {
                        it.label = l;
                    }
                }
                if let Some(q) = seq.and_then(|q| pr.sequence_mut(q)) {
                    for c in &clips {
                        if let Some((_, it)) = q.find_item_mut(*c) {
                            it.label = l;
                        }
                    }
                }
                Ok(())
            })?;
            Ok(Value::Null)
        }),
    ]
}

/// Clips or captions selected (Clear / Ripple Delete work on either).
fn has_any_selection(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    if s.state.selection.is_empty() && s.state.caption_selection.is_empty() && s.state.transition_selection.is_empty() && s.state.gap_selection.is_none() {
        Err("nothing selected".into())
    } else {
        Ok(())
    }
}

/// Clips, captions or a gap selected (Ripple Delete: a transition has no length of its own to close).
fn has_clip_or_caption_selection(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    if s.state.selection.is_empty() && s.state.caption_selection.is_empty() && s.state.gap_selection.is_none() {
        Err("nothing selected".into())
    } else {
        Ok(())
    }
}

fn has_clipboard(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    if s.state.clipboard.is_empty() { Err("clipboard is empty".into()) } else { Ok(()) }
}

fn copy_selection(s: &mut Session) {
    let Some(q) = s.active_sequence() else { return };
    let sel = with_links(s, &s.state.selection);
    let min = sel.iter().filter_map(|c| q.find_item(*c).map(|(_, i)| i.start)).min().unwrap_or_default();
    let max = sel.iter().filter_map(|c| q.find_item(*c).map(|(_, i)| i.end())).max().unwrap_or_default();
    // Copy Paste Includes Sequence Markers: the markers inside the copied span go along
    let markers: Vec<Marker> = if s.state.copy_paste_sequence_markers {
        q.markers.iter().filter(|m| m.start >= min && m.start < max).map(|m| Marker { start: m.start - min, ..m.clone() }).collect()
    } else {
        Vec::new()
    };
    let mut clip = Vec::new();
    for (kind, tracks) in [(TrackKind::Video, &q.video_tracks), (TrackKind::Audio, &q.audio_tracks)] {
        for (ti, t) in tracks.iter().enumerate() {
            for i in t.items.iter().filter(|i| sel.contains(&i.id)) {
                let mut c = i.clone();
                c.start -= min;
                clip.push((kind, ti, c));
            }
        }
    }
    s.state.clipboard = clip;
    s.state.clipboard_markers = markers;
}

fn paste(s: &mut Session, insert: bool) -> Result<Value> {
    let at = s.playhead();
    let clip = s.state.clipboard.clone();
    let markers = if s.state.copy_paste_sequence_markers { s.state.clipboard_markers.clone() } else { Vec::new() };
    let end = s.edit_sequence(if insert { "Paste Insert" } else { "Paste" }, |q, ctx, st| {
        let mut placements = Vec::new();
        let mut links = std::collections::HashMap::new();
        for (kind, ti, item) in clip {
            let tracks = q.tracks(kind);
            let Some(t) = tracks.get(ti).or(tracks.last()) else { continue };
            let mut it = item.clone();
            it.id = ClipId(ctx.alloc());
            it.start += at;
            if let Some(l) = it.link {
                it.link = Some(*links.entry(l).or_insert_with(|| ctx.alloc()));
            }
            placements.push((t.id, it));
        }
        let end = placements.iter().map(|p| p.1.end()).max().unwrap_or(at);
        let ids = if insert { edit::insert(q, placements, ctx)? } else { edit::overwrite(q, placements, ctx)? };
        if insert && st.ripple_sequence_markers {
            crate::sequence_tools::ripple_markers(&mut q.markers, at, end - at);
        }
        for m in &markers {
            q.markers.push(Marker { id: MarkerId(ctx.alloc()), start: m.start + at, ..m.clone() });
        }
        q.markers.sort_by_key(|m| m.start);
        st.selection = ids;
        Ok(end)
    })?;
    s.set_playhead(end);
    Ok(Value::Null)
}
