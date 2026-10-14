//! Applying, selecting and previewing transitions.

use super::*;

/// Set transition parameters from a JSON object (`{"direction": 1}` or `{"direction": "From East"}`,
/// colours as `"#rrggbb"` or `[r,g,b]`, points as `[x,y]`). Floats are clamped to their range.
pub(super) fn set_transition_params(e: &mut filmcraft_project::EffectInstance, params: &Value, cmd: &str) -> Result<()> {
    use filmcraft_project::{ParamKind as K, ParamValue as V};
    let Some(obj) = params.as_object() else { return Err(bad(cmd, "`params` must be an object")) };
    let def = e.def().ok_or_else(|| bad(cmd, "unknown transition"))?;
    for (k, v) in obj {
        let pd = def.param(k).ok_or_else(|| bad(cmd, format!("`{}` has no param `{k}`", def.name)))?;
        let val = match (&pd.kind, v) {
            (K::Choice(opts), Value::String(sv)) => {
                V::Choice(opts.iter().position(|o| o.eq_ignore_ascii_case(sv)).ok_or_else(|| bad(cmd, format!("`{k}` must be one of {opts:?}")))? as u32)
            }
            _ => json_to_param(&pd.default, v).ok_or_else(|| bad(cmd, format!("`{k}`: value has the wrong type")))?,
        };
        let val = match (&pd.kind, val) {
            (K::Choice(opts), V::Choice(c)) if c as usize >= opts.len() => return Err(bad(cmd, format!("`{k}` must be < {}", opts.len()))),
            (K::Float { min, max, .. }, V::Float(f)) => V::Float(f.clamp(*min, *max)),
            (_, v) => v,
        };
        match e.param_mut(k) {
            Some(prm) => prm.value = val,
            None => {
                e.params.insert(k.clone(), filmcraft_project::Param::new(val));
            }
        }
    }
    Ok(())
}

/// A JSON list of transition ids (or one id).
pub(super) fn transition_ids_p(v: &Value) -> Option<Vec<TransitionId>> {
    match v {
        Value::Array(a) => a.iter().map(|x| x.as_u64().map(TransitionId)).collect(),
        x => x.as_u64().map(|i| vec![TransitionId(i)]),
    }
}

/// Remove transitions in one undo step and drop them from the selection.
pub(super) fn remove_transitions(s: &mut Session, ids: &[TransitionId], label: &str) -> Result<Value> {
    let ids = ids.to_vec();
    let n = s.edit_sequence(label, |q, _, st| {
        let n = edit::transitions::remove(q, &ids)?;
        st.transition_selection.retain(|x| !ids.contains(x));
        Ok(n)
    })?;
    Ok(json!({"removed": n}))
}

pub(super) fn apply_transition(s: &mut Session, p: &Value, kind: TrackKind) -> Result<Value> {
    if p.get("clip").is_none()
        && let Some(r) = crate::sequence_extras::apply_to_selection(s, p, kind)?
    {
        return Ok(r);
    }
    let (track, tr) = preview_transition(s, p, kind)?;
    let label = format!("Apply {}", tr.effect.def().map_or("Transition", |d| d.name));
    let id = s.edit_sequence(&label, |q, ctx, _| Ok(edit::add_transition(q, track, tr, ctx)?))?;
    Ok(json!({"transition": id.0}))
}

/// Plan an applied transition without editing the project or allocating an id.
/// Uses the same `effect`, `clip`, `edge`, `frames` and settings as transition application,
/// so a drag preview shows the exact range that release will commit.
pub fn preview_transition(s: &Session, p: &Value, kind: TrackKind) -> Result<(TrackId, Transition)> {
    let eff_id = str_p(p, "effect")
        .map(str::to_string)
        .unwrap_or_else(|| if kind == TrackKind::Video { s.state.default_video_transition.clone() } else { s.state.default_audio_transition.clone() });
    let ekind = if kind == TrackKind::Video { filmcraft_project::EffectKind::VideoTransition } else { filmcraft_project::EffectKind::AudioTransition };
    let def = filmcraft_project::vtransition::find_transition(&eff_id, ekind).ok_or_else(|| bad("transition", format!("unknown transition `{eff_id}`")))?;
    let mut instance = def.instance();
    if let Some(params) = p.get("params") {
        set_transition_params(&mut instance, params, "transition")?;
    }
    let reverse = p.get("reverse").and_then(Value::as_bool).unwrap_or(false);
    let rate = s.sequence_rate();
    // Settings ▸ Timeline ▸ Video / Audio Transition Default Duration
    let dur = match p.get("frames").and_then(Value::as_i64) {
        Some(frames) => rate.tick_of(frames),
        None if kind == TrackKind::Video => s.prefs.timeline.video_transition_duration(rate),
        None => s.prefs.timeline.audio_transition_duration(rate),
    };
    let t = s.playhead();
    let clip = clip_p(p, "clip");
    let edge = str_p(p, "edge").map(str::to_string);
    let targeted = s.targeting().targeted;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    // Find (track, from, to, cut) at the playhead (nearest edit on targeted tracks) or at a clip edge.
    let mut found = None;
    for tr in q.tracks(kind).iter().filter(|tr| clip.is_some() || targeted.contains(&tr.id)) {
        let items = &tr.items;
        for (i, it) in items.iter().enumerate() {
            if let Some(c) = clip
                && it.id != c
            {
                continue;
            }
            let prev = i.checked_sub(1).map(|j| &items[j]).filter(|pv| pv.end() == it.start);
            let next = items.get(i + 1).filter(|nx| nx.start == it.end());
            let use_in = match edge.as_deref() {
                Some("in") => true,
                Some("out") => false,
                _ => (t - it.start).abs() <= (t - it.end()).abs(),
            };
            if clip.is_none() && !(it.start == t || it.end() == t || (it.start - t).abs() < rate.tick_of(3) || (it.end() - t).abs() < rate.tick_of(3)) {
                continue;
            }
            found = Some(if use_in { (tr.id, prev.map(|x| x.id), Some(it.id), it.start) } else { (tr.id, Some(it.id), next.map(|x| x.id), it.end()) });
            break;
        }
        if found.is_some() {
            break;
        }
    }
    let (track, from, to, cut) = found.ok_or_else(|| EngineError::Other("no edit point at the playhead on targeted tracks".into()))?;
    if q.track(track).is_some_and(|tr| tr.locked) {
        return Err(edit::EditError::Locked.into());
    }
    let start = if from.is_some() && to.is_some() {
        cut - dur.mul_ratio(1, 2)
    } else if to.is_some() {
        cut
    } else {
        cut - dur
    };
    let tr = Transition { id: TransitionId(0), effect: instance, start: rate.snap(start), duration: dur, from, to, align: Default::default(), reverse };
    Ok((track, tr))
}
