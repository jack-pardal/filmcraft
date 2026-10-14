//! Effects, transitions and keyframes.

use super::*;

pub(super) fn commands() -> Vec<CommandSpec> {
    vec![
        cmd!("effects.apply", "Apply Effect", [], None, r#"{"clips":[id]?,"effect":"gaussian_blur|Gaussian Blur|…"}"#, has_seq, |s, p| {
            let name = str_p(p, "effect").ok_or_else(|| bad("effects.apply", "need `effect`"))?;
            let def = resolve_effect_name(s, p, name).ok_or_else(|| bad("effects.apply", format!("unknown effect `{name}`")))?;
            if filmcraft_project::graphic::is_layer_id(def.id) {
                return Err(bad("effects.apply", "graphic layers are added with graphics.newText / graphics.newShape"));
            }
            if matches!(def.kind, filmcraft_project::EffectKind::VideoTransition | filmcraft_project::EffectKind::AudioTransition) {
                let mut q = p.clone();
                q["effect"] = json!(def.id);
                return apply_transition(s, &q, if def.kind == filmcraft_project::EffectKind::VideoTransition { TrackKind::Video } else { TrackKind::Audio });
            }
            let clips = clips_p(s, p);
            if clips.is_empty() {
                return Err(bad("effects.apply", "select clips first"));
            }
            let is_audio = def.kind == filmcraft_project::EffectKind::Audio;
            // Audio effects belong on audio clips. A video clip in an A/V pair is retargeted to
            // its linked audio so double-click / drop on the picture still applies Parametric EQ.
            let clips = if is_audio { audio_effect_targets(s, &clips) } else { clips };
            if clips.is_empty() {
                return Err(bad("effects.apply", "audio effects need an audio clip (or a video clip with linked audio)"));
            }
            let (fw, fh) = s.active_sequence().map(|q| (q.settings.width, q.settings.height)).unwrap_or((1920, 1080));
            let label = format!("Apply {}", def.name);
            s.edit_sequence(&label, |q, _, _| {
                let mut n = 0;
                for t in q.all_tracks_mut() {
                    if (t.kind == TrackKind::Audio) != is_audio {
                        continue;
                    }
                    for i in t.items.iter_mut().filter(|i| clips.contains(&i.id)) {
                        let mut inst = def.instance();
                        resolve_auto_points(&mut inst, (fw, fh), (fw, fh));
                        // standard effects go before the intrinsic ones (render order)
                        let pos = i.effects.iter().position(|e| e.def().is_some_and(|d| d.intrinsic)).unwrap_or(i.effects.len());
                        let pos = if is_audio { i.effects.len() } else { pos };
                        i.effects.insert(pos, inst);
                        n += 1;
                    }
                }
                Ok(n)
            })?;
            Ok(Value::Null)
        }),
        cmd!("effects.remove", "Remove Effect", [], None, r#"{"clip":id,"index":n}"#, has_seq, |s, p| {
            let c = clip_p(p, "clip").ok_or_else(|| bad("effects.remove", "need `clip`"))?;
            let idx = u64_p(p, "index").ok_or_else(|| bad("effects.remove", "need `index`"))? as usize;
            // the selected effect: where it is, and its selection once the effect at `idx` is gone
            let effects = s.active_sequence().and_then(|q| q.find_item(c)).map(|(_, it)| it.effects.clone()).unwrap_or_default();
            let sel = s.state.selected_effect.clone().filter(|e| e.clip == c).and_then(|e| e.index(&effects));
            let after = sel.filter(|i| *i != idx).and_then(|i| {
                let mut rest = effects;
                if idx < rest.len() {
                    rest.remove(idx);
                }
                crate::EffectSel::of(c, &rest, if i > idx { i - 1 } else { i })
            });
            s.edit_sequence("Remove Effect", |q, _, _| {
                let (_, it) = q.find_item_mut(c).ok_or(filmcraft_edit::EditError::NoItem(c))?;
                if it.effects.get(idx).and_then(|e| e.def()).is_some_and(|d| d.intrinsic) {
                    return Err(EngineError::Other("intrinsic effects cannot be removed".into()));
                }
                if idx < it.effects.len() {
                    it.effects.remove(idx);
                }
                Ok(())
            })?;
            if sel.is_some() {
                s.state.selected_effect = after;
            }
            Ok(Value::Null)
        }),
        CommandSpec {
            id: "effects.select",
            label: "Select Effect",
            menu: &[],
            shortcut: None,
            params: r#"{"clip":id,"effect":index|id}|{"none":true}"#,
            enabled: has_seq,
            run: select_effect,
            journal: false,
        },
        cmd!(
            "effects.setParam",
            "Set Effect Parameter",
            [],
            None,
            r##"{"clip":id,"effect":"motion"|index,"param":str,"mask":n?,"value":num|[x,y]|"#rrggbb"|bool|path|"option name" (a choice takes its index or its name),"time":ticks? (where the keyframe goes when the parameter is animated, kept there even outside the clip; default: the playhead, pinned to the clip; a parameter that is not animated gets a new static value whatever the time),"keyframe":bool? (true: write a keyframe at `time` and animate the parameter),"merge":bool|key?,"begin":bool?}"##,
            has_seq,
            |s, p| {
                let c = clip_p(p, "clip").ok_or_else(|| bad("effects.setParam", "need `clip`"))?;
                let pid = str_p(p, "param").ok_or_else(|| bad("effects.setParam", "need `param`"))?.to_string();
                let val = p.get("value").cloned().ok_or_else(|| bad("effects.setParam", "need `value`"))?;
                let eff = p.get("effect").cloned().unwrap_or(json!("motion"));
                let keyframe = bool_p(p, "keyframe").unwrap_or(false);
                // an explicit `time` outside the clip keeps its keyframe there, before the start as after
                // the end, so a ramp across a cut keeps its slope (#1055); the playhead is pinned to the clip
                let at = time_p(s, p, "");
                let ph = s.playhead();
                let pq = p.clone();
                // `merge`: a drag of one parameter is one undo step (#201), and a key makes one of a
                // drag that changes several (the Program monitor's handles); `begin` starts a new one
                let merge = match p.get("merge") {
                    Some(Value::String(key)) => Some(format!("setParam:{}:{key}", c.0)),
                    _ => bool_p(p, "merge").unwrap_or(false).then(|| format!("setParam:{}:{eff}:{pid}:{}", c.0, p.get("mask").unwrap_or(&Value::Null))),
                };
                if bool_p(p, "begin").unwrap_or(false) {
                    s.history.merge_key = None;
                }
                let keyframes = s.edit_sequence_as("Change Effect Parameter", merge.as_deref(), |q, _, _| {
                    let (_, it) = q.find_item_mut(c).ok_or(filmcraft_edit::EditError::NoItem(c))?;
                    let mt = it.source_time_at(at.unwrap_or_else(|| ph.clamp(it.start, (it.end() - Tick(1)).max(it.start))));
                    let e = match &eff {
                        Value::Number(n) => it.effects.get_mut(n.as_u64().unwrap_or(0) as usize),
                        Value::String(sid) => it.effects.iter_mut().find(|e| &e.effect == sid),
                        _ => None,
                    }
                    .ok_or_else(|| bad("effects.setParam", "no such effect on clip"))?;
                    // the parameter's definition (a mask's own parameters have none)
                    let def = if pq.get("mask").is_none_or(Value::is_null) { e.def().and_then(|d| d.param(&pid)) } else { None };
                    // a choice takes its option name as well as its index, as transition parameters do
                    let val = match (def.map(|d| &d.kind), &val) {
                        (Some(filmcraft_project::ParamKind::Choice(opts)), Value::String(name)) => {
                            let i = opts
                                .iter()
                                .position(|o| o.eq_ignore_ascii_case(name))
                                .ok_or_else(|| bad("effects.setParam", format!("`{pid}` must be one of {opts:?}")))?;
                            json!(i)
                        }
                        _ => val.clone(),
                    };
                    if keyframe && def.is_some_and(|d| !d.animatable) {
                        return Err(bad("effects.setParam", format!("`{pid}` cannot be animated")));
                    }
                    if eff == json!("opacity") || eff == json!("motion") {
                        e.enabled = true;
                    }
                    // instances saved before a parameter existed get it from the definition
                    if !e.params.contains_key(&pid)
                        && let Some(d) = e.def().and_then(|d| d.param(&pid))
                    {
                        e.params.insert(pid.clone(), filmcraft_project::Param::new(d.default.clone()));
                    }
                    let (keyframes, v) = {
                        let prm = crate::masks::target_param(e, &pq, &pid).ok_or_else(|| bad("effects.setParam", format!("no param `{pid}`")))?;
                        let v = json_to_param(&prm.value, &val).ok_or_else(|| bad("effects.setParam", "value has the wrong type"))?;
                        let keyframes = if keyframe {
                            // the stopwatch and the value in one step
                            prm.put_keyframe(mt, v.clone());
                            prm.keyframes.len()
                        } else {
                            prm.set_at(mt, v.clone());
                            prm.keyframes.len()
                        };
                        (keyframes, v)
                    };
                    if e.effect == "ultra_key"
                        && pid == "setting"
                        && let ParamValue::Choice(setting) = v
                        && let Some(rows) = filmcraft_project::effect::ultra_key_setting(setting)
                    {
                        for (id, value) in rows {
                            if let Some(p) = e.param_mut(id) {
                                p.value = ParamValue::Float(*value);
                                p.keyframes.clear();
                            } else {
                                e.params.insert((*id).to_string(), filmcraft_project::Param::new(ParamValue::Float(*value)));
                            }
                        }
                    }
                    Ok(keyframes)
                })?;
                // 0 keyframes: the value is static (`time` was not used)
                Ok(json!({"keyframes": keyframes}))
            }
        ),
        cmd!("effects.toggleAnimation", "Toggle Animation", [], None, r#"{"clip":id,"effect":str|index,"param":str,"mask":n?}"#, has_seq, |s, p| {
            let c = clip_p(p, "clip").ok_or_else(|| bad("effects.toggleAnimation", "need `clip`"))?;
            let pid = str_p(p, "param").ok_or_else(|| bad("effects.toggleAnimation", "need `param`"))?.to_string();
            let eff = p.get("effect").cloned().unwrap_or(json!("motion"));
            let ph = s.playhead();
            s.edit_sequence("Toggle Animation", |q, _, _| {
                let (_, it) = q.find_item_mut(c).ok_or(filmcraft_edit::EditError::NoItem(c))?;
                let mt = it.source_time_at(ph.clamp(it.start, (it.end() - Tick(1)).max(it.start)));
                let e = match &eff {
                    Value::Number(n) => it.effects.get_mut(n.as_u64().unwrap_or(0) as usize),
                    Value::String(sid) => it.effects.iter_mut().find(|e| &e.effect == sid),
                    _ => None,
                }
                .ok_or_else(|| bad("effects.toggleAnimation", "no such effect"))?;
                crate::masks::target_param(e, p, &pid).ok_or_else(|| bad("effects.toggleAnimation", "no such param"))?.toggle_animation(mt);
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!("effects.toggleEnabled", "Toggle Effect", [], None, r#"{"clip":id,"index":n}"#, has_seq, |s, p| {
            let c = clip_p(p, "clip").ok_or_else(|| bad("effects.toggleEnabled", "need `clip`"))?;
            let idx = u64_p(p, "index").unwrap_or(0) as usize;
            s.edit_sequence("Toggle Effect", |q, _, _| {
                let (_, it) = q.find_item_mut(c).ok_or(filmcraft_edit::EditError::NoItem(c))?;
                if let Some(e) = it.effects.get_mut(idx) {
                    e.enabled = !e.enabled;
                }
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!("effects.reset", "Reset Effect", [], None, r#"{"clip":id,"index":n}"#, has_seq, |s, p| {
            let c = clip_p(p, "clip").ok_or_else(|| bad("effects.reset", "need `clip`"))?;
            let idx = u64_p(p, "index").unwrap_or(0) as usize;
            let frame = s.active_sequence().map(|q| (q.settings.width, q.settings.height)).unwrap_or((1920, 1080));
            let ph = s.playhead();
            s.edit_sequence("Reset Effect", |q, _, _| {
                let (_, it) = q.find_item_mut(c).ok_or(filmcraft_edit::EditError::NoItem(c))?;
                let mt = it.source_time_at(ph.clamp(it.start, (it.end() - Tick(1)).max(it.start)));
                if let Some(e) = it.effects.get_mut(idx) {
                    reset_params(e, None, frame, mt);
                }
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!("effects.resetParam", "Reset Parameter", [], None, r#"{"clip":id,"effect":str|index,"param":str}"#, has_seq, |s, p| {
            let c = clip_p(p, "clip").ok_or_else(|| bad("effects.resetParam", "need `clip`"))?;
            let pid = str_p(p, "param").ok_or_else(|| bad("effects.resetParam", "need `param`"))?.to_string();
            let eff = p.get("effect").cloned().unwrap_or(json!("motion"));
            let frame = s.active_sequence().map(|q| (q.settings.width, q.settings.height)).unwrap_or((1920, 1080));
            let ph = s.playhead();
            s.edit_sequence("Reset Parameter", |q, _, _| {
                let (_, it) = q.find_item_mut(c).ok_or(filmcraft_edit::EditError::NoItem(c))?;
                let mt = it.source_time_at(ph.clamp(it.start, (it.end() - Tick(1)).max(it.start)));
                let e = match &eff {
                    Value::Number(n) => it.effects.get_mut(n.as_u64().unwrap_or(0) as usize),
                    Value::String(sid) => it.effects.iter_mut().find(|e| &e.effect == sid),
                    _ => None,
                }
                .ok_or_else(|| bad("effects.resetParam", "no such effect on clip"))?;
                if !reset_params(e, Some(&pid), frame, mt) {
                    return Err(bad("effects.resetParam", format!("no param `{pid}`")));
                }
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        cmd!(
            "effects.addKeyframe",
            "Add/Remove Keyframe",
            [],
            None,
            r#"{"clip":id,"effect":str|index,"param":str,"mask":n?,"time":ticks?}"#,
            has_seq,
            |s, p| { keyframe_op(s, p, "add") }
        ),
        cmd!("effects.deleteKeyframe", "Delete Keyframe", [], None, r#"{"clip":id,"effect":str|index,"param":str,"mediaTime":ticks}"#, has_seq, |s, p| {
            keyframe_op(s, p, "delete")
        }),
        cmd!(
            "effects.moveKeyframe",
            "Move Keyframe",
            [],
            None,
            r#"{"clip":id,"effect":str|index,"param":str,"mediaTime":ticks,"to":ticks,"value":any?,"merge":bool?,"begin":bool?}"#,
            has_seq,
            |s, p| keyframe_op(s, p, "move")
        ),
        cmd!(
            "effects.moveKeyframes",
            "Move Keyframes",
            [],
            None,
            r#"{"keyframes":[{"clip":id,"effect":index,"param":str,"mask":n?,"time":mediaTicks}],"delta":timelineTicks}"#,
            has_seq,
            crate::keyframe_move::move_many
        ),
        cmd!(
            "effects.setInterpolation",
            "Keyframe Interpolation",
            [],
            None,
            r#"{"clip":id,"effect":str|index,"param":str,"mediaTime":ticks,"interpolation":"linear|bezier|autoBezier|continuousBezier|hold|easeIn|easeOut"}"#,
            has_seq,
            |s, p| keyframe_op(s, p, "interp")
        ),
        cmd!(
            "effects.setKeyframe",
            "Edit Keyframe",
            [],
            None,
            r#"{"clip":id,"effect":str|index,"param":str,"mediaTime":ticks,"value":any?,"inInfluence":0..1?,"outInfluence":0..1?,"merge":bool?,"begin":bool?}"#,
            has_seq,
            |s, p| keyframe_op(s, p, "set")
        ),
    ]
}

/// Effect by id or display name. Names shared by several definitions ("Invert", "Volume",
/// "Channel Volume") resolve to an applicable (non-intrinsic) effect whose kind matches the
/// target clips' tracks.
fn resolve_effect_name(s: &Session, p: &Value, name: &str) -> Option<&'static filmcraft_project::EffectDef> {
    if let Some(d) = filmcraft_project::find_effect(name) {
        return Some(d);
    }
    let n = name.to_ascii_lowercase();
    let cands: Vec<_> = filmcraft_project::effect_defs().iter().filter(|e| e.name.to_ascii_lowercase() == n).collect();
    if cands.len() <= 1 {
        return cands.first().copied().or_else(|| filmcraft_project::find_effect(&n));
    }
    let on_audio = s.active_sequence().is_some_and(|q| {
        let clips = clips_p(s, p);
        !clips.is_empty() && clips.iter().all(|c| q.find_item(*c).and_then(|(t, _)| q.track(t)).is_some_and(|t| t.kind == TrackKind::Audio))
    });
    let want = if on_audio { filmcraft_project::EffectKind::Audio } else { filmcraft_project::EffectKind::Video };
    cands.iter().find(|d| !d.intrinsic && d.kind == want).or_else(|| cands.iter().find(|d| !d.intrinsic)).or(cands.first()).copied()
}

/// Clips an audio effect should land on: audio ids as given, plus the linked audio of any video
/// clip in the list (so applying Parametric EQ to an A/V pair hits the sound).
pub fn audio_effect_targets(s: &Session, clips: &[ClipId]) -> Vec<ClipId> {
    let Some(seq) = s.active_sequence() else { return clips.to_vec() };
    let mut out = Vec::new();
    for c in clips {
        let Some((tid, it)) = seq.find_item(*c) else { continue };
        let kind = seq.track(tid).map(|t| t.kind);
        if kind == Some(TrackKind::Audio) {
            if !out.contains(c) {
                out.push(*c);
            }
            continue;
        }
        if kind != Some(TrackKind::Video) {
            continue;
        }
        let Some(link) = it.link else { continue };
        for t in &seq.audio_tracks {
            for i in &t.items {
                if i.link == Some(link) && !out.contains(&i.id) {
                    out.push(i.id);
                }
            }
        }
    }
    out
}

/// Put `e`'s parameters (all of them, or only `only`) back to their defaults, as Premiere's reset
/// buttons do: a parameter that is not animated gets its default as its static value, and an
/// animated one keeps its keyframes and gets the default as a keyframe at media time `mt`.
/// Masks are left alone. Returns whether any parameter was reset.
fn reset_params(e: &mut filmcraft_project::EffectInstance, only: Option<&str>, frame: (u32, u32), mt: Tick) -> bool {
    let Some(d) = e.def() else { return false };
    let mut fresh = d.instance();
    resolve_auto_points(&mut fresh, frame, frame);
    let mut any = false;
    for (id, prm) in fresh.params {
        if only.is_some_and(|o| o != id) {
            continue;
        }
        any = true;
        match e.params.get_mut(&id) {
            Some(old) if old.is_animated() => old.set_at(mt, prm.value),
            _ => {
                e.params.insert(id, prm);
            }
        }
    }
    any
}

fn keyframe_op(s: &mut Session, p: &Value, op: &str) -> Result<Value> {
    let c = clip_p(p, "clip").ok_or_else(|| bad("keyframe", "need `clip`"))?;
    let pid = str_p(p, "param").ok_or_else(|| bad("keyframe", "need `param`"))?.to_string();
    let eff = p.get("effect").cloned().unwrap_or(json!("motion"));
    let ph = time_p(s, p, "").unwrap_or(s.playhead());
    let mtime = p.get("mediaTime").and_then(Value::as_i64).map(Tick);
    let to = p.get("to").and_then(Value::as_i64).map(Tick);
    let interp = str_p(p, "interpolation").map(str::to_string);
    let label = match op {
        "add" => "Add Keyframe",
        "delete" => "Delete Keyframe",
        "move" => "Move Keyframe",
        "set" => "Edit Keyframe",
        _ => "Keyframe Interpolation",
    };
    // a keyframe dragged on the timeline is one undo step, like a dragged value
    let merge = bool_p(p, "merge").unwrap_or(false).then(|| format!("keyframe:{op}:{}:{eff}:{pid}:{}", c.0, p.get("mask").unwrap_or(&Value::Null)));
    if bool_p(p, "begin").unwrap_or(false) {
        s.history.merge_key = None;
    }
    s.edit_sequence_as(label, merge.as_deref(), |q, _, _| {
        let (_, it) = q.find_item_mut(c).ok_or(filmcraft_edit::EditError::NoItem(c))?;
        let mt_now = it.source_time_at(ph.clamp(it.start, (it.end() - Tick(1)).max(it.start)));
        let e = match &eff {
            Value::Number(n) => it.effects.get_mut(n.as_u64().unwrap_or(0) as usize),
            Value::String(sid) => it.effects.iter_mut().find(|e| &e.effect == sid),
            _ => None,
        }
        .ok_or_else(|| bad("keyframe", "no such effect"))?;
        let prm = crate::masks::target_param(e, p, &pid).ok_or_else(|| bad("keyframe", "no such param"))?;
        match op {
            "add" => {
                // toggle: remove if a keyframe sits at the playhead, else add
                if !prm.remove_keyframe_at(mt_now) {
                    prm.add_keyframe(mt_now);
                }
            }
            "delete" => {
                prm.remove_keyframe_at(mtime.ok_or_else(|| bad("keyframe", "need `mediaTime`"))?);
            }
            "move" => {
                let from = mtime.ok_or_else(|| bad("keyframe", "need `mediaTime`"))?;
                let to = to.ok_or_else(|| bad("keyframe", "need `to`"))?;
                if let Some(i) = prm.keyframes.iter().position(|k| k.time == from) {
                    let mut k = prm.keyframes.remove(i);
                    k.time = to;
                    if let Some(v) = p.get("value") {
                        k.value = json_to_param(&k.value, v).ok_or_else(|| bad("keyframe", "value has the wrong type"))?;
                    }
                    prm.keyframes.retain(|x| x.time != to);
                    let at = prm.keyframes.partition_point(|x| x.time < to);
                    prm.keyframes.insert(at, k);
                }
            }
            "set" => {
                let at = mtime.ok_or_else(|| bad("keyframe", "need `mediaTime`"))?;
                let k = prm.keyframes.iter_mut().find(|k| k.time == at).ok_or_else(|| bad("keyframe", "no keyframe at that time"))?;
                if let Some(v) = p.get("value") {
                    k.value = json_to_param(&k.value, v).ok_or_else(|| bad("keyframe", "value has the wrong type"))?;
                }
                if let Some(x) = f64_p(p, "inInfluence") {
                    k.in_influence = x.clamp(0.01, 1.0);
                }
                if let Some(x) = f64_p(p, "outInfluence") {
                    k.out_influence = x.clamp(0.01, 1.0);
                }
            }
            _ => {
                let at = mtime.unwrap_or(mt_now);
                let kind = match interp.as_deref().unwrap_or("linear").to_ascii_lowercase().as_str() {
                    "bezier" => filmcraft_project::Interpolation::Bezier,
                    "autobezier" => filmcraft_project::Interpolation::AutoBezier,
                    "continuousbezier" => filmcraft_project::Interpolation::ContinuousBezier,
                    "hold" => filmcraft_project::Interpolation::Hold,
                    "easein" => filmcraft_project::Interpolation::EaseIn,
                    "easeout" => filmcraft_project::Interpolation::EaseOut,
                    _ => filmcraft_project::Interpolation::Linear,
                };
                let k = prm.keyframes.iter_mut().find(|k| k.time == at).ok_or_else(|| bad("keyframe", "no keyframe at that time"))?;
                k.interp = kind;
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// `effects.select`: the effect selected in Effect Controls (`effect`: index, or the id of the
/// clip's first effect of that kind). It replaces the mask selection.
fn select_effect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "effects.select";
    if bool_p(p, "none") == Some(true) {
        s.state.selected_effect = None;
        return Ok(Value::Null);
    }
    let clip = clip_p(p, "clip").ok_or_else(|| bad(CMD, "need `clip` (or `none`)"))?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (_, it) = q.find_item(clip).ok_or_else(|| bad(CMD, "no such clip"))?;
    let effect = match p.get("effect") {
        Some(Value::Number(n)) => n.as_u64().and_then(|i| usize::try_from(i).ok()).filter(|i| *i < it.effects.len()),
        Some(Value::String(id)) => it.effects.iter().position(|e| &e.effect == id),
        _ => None,
    }
    .ok_or_else(|| bad(CMD, "no such effect on clip"))?;
    s.state.selected_effect = crate::EffectSel::of(clip, &it.effects, effect);
    s.state.selected_mask = None;
    Ok(Value::Null)
}
