//! Read-only queries for agents.

use super::*;

pub(super) fn commands() -> Vec<CommandSpec> {
    vec![
        query!("command.list", "List Commands", "{}", |s, _| {
            Ok(Value::Array(
                command_specs()
                    .iter()
                    .map(|c| {
                        let all: Vec<&str> = s.shortcuts.for_command(c.id).iter().map(|b| b.keys.as_str()).collect();
                        json!({"id": c.id, "label": c.label, "menu": c.menu, "shortcut": s.shortcuts.primary(c.id), "shortcuts": all, "defaultShortcut": c.shortcut, "params": c.params, "enabled": (c.enabled)(s).is_ok()})
                    })
                    .collect(),
            ))
        }),
        query!("project.inspect", "Inspect Project", "{}", |s, _| Ok(crate::commands::inspect_project(s))),
        query!("sequence.inspect", "Inspect Sequence", r#"{"item":id?}"#, |s, p| {
            let id = item_p(p, "item").or(s.state.active_sequence).ok_or(EngineError::NoSequence)?;
            let q = s.project.sequence(id).ok_or(EngineError::NoSequence)?;
            Ok(inspect_sequence(s, id, q))
        }),
        query!("state.inspect", "Inspect Editor State", "{}", |s, _| Ok(serde_json::to_value(&s.state).unwrap_or_default())),
        query!(
            "effects.list",
            "List Effects",
            r#"{"kind":"Video"|"Audio"|"VideoTransition"|"AudioTransition"?,"folder":"Video Transitions/Wipe"?,"detail":bool?}"#,
            |_, p| Ok(list_effects(p))
        ),
        query!("history.list", "List History", "{}", |s, _| Ok(
            json!({"undo": s.history.undo.iter().map(|h| &h.0).collect::<Vec<_>>(), "redo": s.history.redo.iter().map(|h| &h.0).collect::<Vec<_>>()})
        )),
    ]
}

/// `effects.list`: every effect definition with its Effects-panel folder (optionally filtered by
/// kind or folder prefix); `detail` adds each parameter's label, type, default and choices.
fn list_effects(p: &Value) -> Value {
    let kind = str_p(p, "kind");
    let folder = str_p(p, "folder");
    let detail = p.get("detail").and_then(Value::as_bool).unwrap_or(false);
    let out = filmcraft_project::effect_defs()
        .iter()
        .filter(|d| kind.is_none_or(|k| format!("{:?}", d.kind).eq_ignore_ascii_case(k)))
        .filter(|d| folder.is_none_or(|f| d.category.join("/").starts_with(f)))
        .map(|d| {
            let mut v = json!({
                "id": d.id,
                "name": d.name,
                "kind": format!("{:?}", d.kind),
                "category": d.category,
                "folder": d.category.get(1),
                "path": d.category.join("/"),
                "params": d.params.iter().map(|p| p.id).collect::<Vec<_>>(),
                "badges": {"accelerated": d.accelerated, "float32": d.float32, "yuv": d.yuv},
            });
            if detail {
                v["paramInfo"] = Value::Array(d.params.iter().map(param_info).collect());
            }
            v
        })
        .collect();
    Value::Array(out)
}

fn param_info(p: &filmcraft_project::ParamDef) -> Value {
    use filmcraft_project::ParamKind as K;
    let mut j = json!({"id": p.id, "label": p.label, "animatable": p.animatable, "default": serde_json::to_value(&p.default).unwrap_or_default()});
    let ty = match &p.kind {
        K::Choice(opts) => {
            j["options"] = json!(opts);
            "choice"
        }
        K::Float { min, max, unit, .. } => {
            j["min"] = json!(min);
            j["max"] = json!(max);
            j["unit"] = json!(unit);
            "float"
        }
        K::Point => "point",
        K::Color => "color",
        K::Bool => "bool",
        K::Angle => "angle",
        K::Text => "text",
        K::Curve { .. } => "curve",
        K::Wheel => "wheel",
        K::Path => "path",
    };
    j["type"] = json!(ty);
    j
}

pub fn inspect_sequence(s: &Session, id: ItemId, q: &filmcraft_project::Sequence) -> Value {
    let rate = q.settings.frame_rate;
    let tr = |t: &filmcraft_project::Track| {
        json!({
            "id": t.id.0, "name": t.name, "locked": t.locked, "syncLock": t.sync_lock, "enabled": t.enabled, "muted": t.muted, "solo": t.solo,
            "items": t.items.iter().map(|i| json!({
                "clip": i.id.0, "item": i.item.0, "name": i.name, "start": i.start.0, "duration": i.duration.0,
                "startFrame": rate.frame_at(i.start), "durationFrames": rate.frame_at(i.duration), "sourceIn": i.source_in.0, "speed": i.speed, "reverse": i.reverse,
                "end": i.end().0, "endFrame": rate.frame_at(i.end()), "sourceOut": i.source_out().0, "gainDb": i.gain_db,
                "enabled": i.enabled, "link": i.link, "label": i.label.name(),
                "effects": i.effects.iter().map(|e| json!({"effect": e.effect, "enabled": e.enabled, "masks": e.masks.len(), "params": e.params.iter().map(|(k, p)| (k.clone(), json!({"value": format!("{:?}", p.value), "keyframes": p.keyframes.len()}))).collect::<serde_json::Map<_, _>>()})).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "transitions": t.transitions.iter().map(|x| json!({"id": x.id.0, "effect": x.effect.effect, "start": x.start.0, "duration": x.duration.0, "from": x.from.map(|c| c.0), "to": x.to.map(|c| c.0), "reverse": x.reverse, "align": filmcraft_edit::transitions::bounds(t, x).map(|b| filmcraft_edit::transitions::alignment(x, b.1, rate.frame_duration()).id()), "params": x.effect.params.iter().map(|(k, v)| (k.clone(), serde_json::to_value(&v.value).unwrap_or_default())).collect::<serde_json::Map<_, _>>()})).collect::<Vec<_>>(),
        })
    };
    // the frames export renders, a partial last frame included
    let (f0, f1) = filmcraft_export::frame_span(rate, TimeRange::new(Tick::ZERO, q.duration()));
    json!({
        "id": id.0,
        "name": s.project.item(id).map(|i| i.name.clone()),
        "settings": q.settings,
        "duration": q.duration().0,
        "durationFrames": f1 - f0,
        "playhead": s.state.playheads.get(&id).map(|t| t.0),
        "in": q.mark_in.map(|t| t.0),
        "out": q.mark_out.map(|t| t.0),
        "split": q.split,
        "markers": q.markers.iter().map(|m| json!({"id": m.id.0, "start": m.start.0, "duration": m.duration.0, "kind": format!("{:?}", m.kind), "name": m.name, "comment": m.comment, "color": m.color.name()})).collect::<Vec<_>>(),
        "video": q.video_tracks.iter().map(tr).collect::<Vec<_>>(),
        "audio": q.audio_tracks.iter().map(tr).collect::<Vec<_>>(),
        "selection": s.state.selection.iter().map(|c| c.0).collect::<Vec<_>>(),
        "transitionSelection": s.state.transition_selection.iter().map(|t| t.0).collect::<Vec<_>>(),
        "gapSelection": s.state.gap_selection.map(|g| json!({"track": g.track.0, "start": g.range.start.0, "end": g.range.end().0})),
    })
}

/// JSON description of the project (bins, items) for agents.
pub fn inspect_project(s: &Session) -> Value {
    let p = &s.project;
    fn bin(b: &filmcraft_project::Bin, p: &filmcraft_project::Project) -> Value {
        json!({
            "id": b.id.0,
            "name": b.name,
            "children": b.children.iter().map(|c| match c {
                filmcraft_project::BinEntry::Item(i) => {
                    let it = p.item(*i);
                    json!({"item": i.0, "name": it.map(|x| x.name.clone()), "type": it.map(|x| x.type_label()), "duration": it.map(|x| x.duration().0), "label": it.map(|x| x.label.name())})
                }
                filmcraft_project::BinEntry::Bin(b) => bin(b, p),
            }).collect::<Vec<_>>()
        })
    }
    json!({
        "name": p.name,
        "path": s.path,
        "dirty": s.is_dirty(),
        "revision": s.revision,
        "root": bin(&p.root, p),
        "activeSequence": s.state.active_sequence.map(|i| i.0),
        "sourceItem": s.state.source_item.map(|i| i.0),
    })
}
