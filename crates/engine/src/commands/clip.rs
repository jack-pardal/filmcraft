//! Clip menu.

use super::*;

pub(super) fn commands() -> Vec<CommandSpec> {
    vec![
        cmd!("clip.rename", "Rename…", ["Clip"], None, r#"{"clip":id?,"item":id?,"name":str}"#, always, |s, p| {
            let name = str_p(p, "name").ok_or_else(|| bad("clip.rename", "need `name`"))?.to_string();
            let item = item_p(p, "item");
            let clip = clip_p(p, "clip").or_else(|| s.state.selection.first().copied());
            let seq = s.state.active_sequence;
            s.edit("Rename", |pr, _| {
                if let Some(i) = item {
                    pr.item_mut(i).ok_or_else(|| bad("clip.rename", "no such item"))?.name = name;
                } else if let (Some(c), Some(q)) = (clip, seq) {
                    pr.sequence_mut(q).and_then(|q| q.find_item_mut(c)).ok_or_else(|| bad("clip.rename", "no such clip"))?.1.name = name;
                }
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!(
            "clip.speedDuration",
            "Speed/Duration…",
            ["Clip"],
            Some("Cmd+R"),
            r#"{"clips":[id]?,"speed":percent=100,"reverse":bool,"ripple":bool,"interpolation":"frameSampling|frameBlending|opticalFlow"?}"#,
            has_selection,
            |s, p| {
                let speed = f64_p(p, "speed").unwrap_or(100.0) / 100.0;
                let reverse = bool_p(p, "reverse").unwrap_or(false);
                let ripple = bool_p(p, "ripple").unwrap_or(false);
                let interp = match str_p(p, "interpolation") {
                    Some(m) => Some(filmcraft_project::TimeInterpolation::from_name(m).ok_or_else(|| bad("clip.speedDuration", "unknown interpolation"))?),
                    None => None,
                };
                let sel = with_links(s, &clips_p(s, p));
                s.edit_sequence("Speed/Duration", |q, ctx, _| {
                    // a clip and its linked partners on other tracks change as one edit, so a ripple
                    // moves the later clips once
                    let place = |q: &filmcraft_project::Sequence, c: ClipId| q.find_item(c).map(|(t, i)| (t, i.link));
                    let mut groups: Vec<Vec<ClipId>> = Vec::new();
                    for c in &sel {
                        // a clip that is not there goes on alone, so the edit fails with "no such item"
                        let Some((track, link)) = place(q, *c) else {
                            groups.push(vec![*c]);
                            continue;
                        };
                        let joins = |g: &Vec<ClipId>| link.is_some() && g.iter().all(|o| place(q, *o).is_some_and(|(t, l)| l == link && t != track));
                        match groups.iter_mut().find(|g| joins(g)) {
                            Some(g) => g.push(*c),
                            None => groups.push(vec![*c]),
                        }
                    }
                    for group in &groups {
                        let unlocked: Vec<ClipId> =
                            group.iter().copied().filter(|c| q.find_item(*c).is_some_and(|(t, _)| q.track(t).is_some_and(|tr| !tr.locked))).collect();
                        if unlocked.is_empty() {
                            return Err(edit::EditError::Locked.into());
                        }
                        edit::set_speed_group(q, &unlocked, speed, reverse, ripple, ctx)?;
                    }
                    for c in &sel {
                        if let (Some(m), Some((_, it))) = (interp, q.find_item_mut(*c)) {
                            it.time_interpolation = m;
                        }
                    }
                    Ok(())
                })?;
                Ok(Value::Null)
            }
        ),
        // Each clip flips on its own, as in Premiere (#484): with one enabled and one disabled clip
        // selected, Enable swaps them rather than first making both the same.
        cmd!("clip.enable", "Enable", ["Clip"], Some("Shift+E"), r#"{"clips":[id]?}"#, has_selection, |s, p| {
            let mut sel = with_links(s, &clips_p(s, p));
            // a clip named twice must still flip once
            sel.sort();
            sel.dedup();
            s.edit_sequence("Enable", |q, _, _| {
                for c in &sel {
                    if let Some((_, i)) = q.find_item_mut(*c) {
                        i.enabled = !i.enabled;
                    }
                }
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!("clip.link", "Link", ["Clip"], Some("Cmd+L"), r#"{"clips":[id]?}"#, has_selection, |s, p| {
            let sel = clips_p(s, p);
            let linked = sel.iter().all(|c| s.active_sequence().and_then(|q| q.find_item(*c)).is_some_and(|(_, i)| i.link.is_some()));
            s.edit_sequence(if linked { "Unlink" } else { "Link" }, |q, ctx, _| {
                if linked {
                    for c in &sel {
                        if let Some((_, i)) = q.find_item_mut(*c) {
                            i.link = None;
                        }
                    }
                } else {
                    // each selected clip with its track kind, worked out once
                    type Spec = (ClipId, filmcraft_project::ItemId, filmcraft_time::TimeRange);
                    let (mut v_clips, mut audio_specs): (Vec<Spec>, Vec<Spec>) = (Vec::new(), Vec::new());
                    for c in &sel {
                        if let Some((tid, i)) = q.find_item(*c) {
                            if q.video_tracks.iter().any(|t| t.id == tid) {
                                v_clips.push((*c, i.item, i.range()));
                            } else {
                                audio_specs.push((*c, i.item, i.range()));
                            }
                        }
                    }
                    if !audio_specs.is_empty() && v_clips.len() > 1 {
                        let mut linked_audios = std::collections::HashSet::new();
                        for (vid, item_id, vrange) in &v_clips {
                            // Pair with overlapping audio clips from the same item, or overlapping audio clips if no same-item audio
                            let matching: Vec<ClipId> = audio_specs
                                .iter()
                                .filter(|(aid, aitem, arange)| !linked_audios.contains(aid) && aitem == item_id && arange.overlaps(vrange))
                                .map(|(aid, _, _)| *aid)
                                .collect();
                            let to_link = if !matching.is_empty() {
                                matching
                            } else {
                                audio_specs
                                    .iter()
                                    .filter(|(aid, _, arange)| !linked_audios.contains(aid) && arange.overlaps(vrange))
                                    .map(|(aid, _, _)| *aid)
                                    .collect()
                            };
                            // a video clip with no audio partner stays unlinked: a one-member link would read as "linked"
                            let l = (!to_link.is_empty()).then(|| ctx.alloc());
                            if let Some((_, vi)) = q.find_item_mut(*vid) {
                                vi.link = l;
                            }
                            for aid in to_link {
                                linked_audios.insert(aid);
                                if let Some((_, ai)) = q.find_item_mut(aid) {
                                    ai.link = l;
                                }
                            }
                        }
                        // selected audio clips with no overlapping video stay unlinked
                        for (aid, _, _) in audio_specs {
                            if !linked_audios.contains(&aid)
                                && let Some((_, ai)) = q.find_item_mut(aid)
                            {
                                ai.link = None;
                            }
                        }
                    } else {
                        let l = ctx.alloc();
                        for c in &sel {
                            if let Some((_, i)) = q.find_item_mut(*c) {
                                i.link = Some(l);
                            }
                        }
                    }
                }
                Ok(())
            })?;
            Ok(json!({"linked": !linked}))
        }),
        cmd!("clip.group", "Group", ["Clip"], Some("Cmd+G"), "{}", has_selection, |s, _| {
            let sel = s.state.selection.clone();
            s.edit_sequence("Group", |q, ctx, _| {
                let g = ctx.alloc();
                for c in &sel {
                    if let Some((_, i)) = q.find_item_mut(*c) {
                        i.group = Some(g);
                    }
                }
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!("clip.ungroup", "Ungroup", ["Clip"], Some("Cmd+Shift+G"), "{}", has_selection, |s, _| {
            let sel = s.state.selection.clone();
            s.edit_sequence("Ungroup", |q, _, _| {
                for c in &sel {
                    if let Some((_, i)) = q.find_item_mut(*c) {
                        i.group = None;
                    }
                }
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!("clip.scaleToFrameSize", "Scale to Frame Size", ["Clip", "Video Options"], None, "{}", has_selection, |s, _| {
            let sel = s.state.selection.clone();
            s.edit_sequence("Scale to Frame Size", |q, _, _| {
                for c in &sel {
                    if let Some((_, i)) = q.find_item_mut(*c) {
                        i.scale_to_frame = !i.scale_to_frame;
                    }
                }
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!(
            "clip.audioGain",
            "Audio Gain…",
            ["Clip", "Audio Options"],
            Some("G"),
            r#"{"clips":[id]?,"mode":"set|adjust|normalizeMax|normalizeAll"?,"db":f64,"relative":bool?}"#,
            has_selection,
            |s, p| crate::mixer::audio_gain(s, p)
        ),
        query!("clip.audioPeak", "Audio Clip Peak Amplitude", r#"{"clips":[id]?}"#, |s, p| crate::mixer::audio_peak(s, p)),
        cmd!("clip.frameHold", "Add Frame Hold", ["Clip", "Video Options"], None, r#"{"clips":[id]?,"time":ticks?}"#, has_seq, |s, p| {
            crate::clip_ops::add_frame_hold(s, p)
        }),
        cmd!("clip.nest", "Nest…", ["Clip"], None, r#"{"name":str}"#, has_selection, |s, p| nest(s, p)),
        // Reveal in Project (clip context menu) and Reveal Sequence in Project (Timeline tab menu):
        // select the item in the Project panel and show it there
        cmd!("clip.revealInProject", "Reveal in Project", [], None, r#"{"clip":id?}"#, has_seq, |s, p| {
            let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
            let t = s.playhead();
            let tg = s.targeting().targeted;
            // the named or first selected clip, else the topmost clip under the playhead on a targeted track
            let item = clips_p(s, p)
                .iter()
                .find_map(|c| seq.find_item(*c).map(|(_, i)| i.item))
                .or_else(|| {
                    seq.video_tracks.iter().rev().chain(seq.audio_tracks.iter()).filter(|tr| tg.contains(&tr.id)).find_map(|tr| tr.item_at(t).map(|i| i.item))
                })
                .ok_or_else(|| EngineError::Other("select a clip to reveal".into()))?;
            reveal_in_project(s, item)
        }),
        cmd!("sequence.revealInProject", "Reveal Sequence in Project", [], None, r#"{"item":id?}"#, has_seq, |s, p| {
            let item = item_p(p, "item").or(s.state.active_sequence).ok_or(EngineError::NoSequence)?;
            reveal_in_project(s, item)
        }),
    ]
}

/// Select `item` in the Project panel and ask the UI to show it there.
fn reveal_in_project(s: &mut Session, item: ItemId) -> Result<Value> {
    s.project.item(item).ok_or_else(|| EngineError::Other("that clip's source is not in the project".into()))?;
    s.state.project_selection = vec![item];
    s.events.push(crate::Event::RevealInProject(item));
    Ok(json!({"item": item.0}))
}
