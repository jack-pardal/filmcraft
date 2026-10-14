//! The command registry. Ids follow Premiere's menu structure; every menu item, panel button,
//! shortcut and timeline gesture maps to exactly one of these.

use std::sync::OnceLock;

use filmcraft_edit as edit;
use filmcraft_edit::{Edge, TrimMode};
use filmcraft_media::Generator;
use filmcraft_media::generators::GeneratorSource;
use filmcraft_project::{
    ClipId, ItemId, ItemKind, Label, Marker, MarkerId, MarkerKind, MediaClip, MediaRef, ParamValue, Project, SequenceSettings, Track, TrackId, TrackKind,
    Transition, TransitionId, resolve_auto_points,
};
use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick, TimeRange, parse_timecode};
use serde_json::{Value, json};

use crate::{EngineError, Result, Session};

type Run = fn(&mut Session, &Value) -> Result<Value>;
type Enabled = fn(&Session) -> std::result::Result<(), String>;

/// Metadata + implementation for one command.
pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// Menu placement, e.g. `["Sequence"]` or `["File", "New"]`. Empty = not in menus.
    pub menu: &'static [&'static str],
    /// Default shortcut (egui-style names, `Cmd` = ⌘ on macOS / Ctrl elsewhere).
    pub shortcut: Option<&'static str>,
    /// Parameter description (JSON-ish, for agents and the MCP schema).
    pub params: &'static str,
    pub enabled: Enabled,
    pub run: Run,
    /// Record in the journal (false for pure queries).
    pub journal: bool,
}

macro_rules! cmd {
    ($id:literal, $label:literal, [$($m:literal),*], $sc:expr, $params:literal, $en:expr, $run:expr) => {
        CommandSpec { id: $id, label: $label, menu: &[$($m),*], shortcut: $sc, params: $params, enabled: $en, run: $run, journal: true }
    };
}
macro_rules! query {
    ($id:literal, $label:literal, $params:literal, $run:expr) => {
        CommandSpec { id: $id, label: $label, menu: &[], shortcut: None, params: $params, enabled: always, run: $run, journal: false }
    };
}

mod clip;
mod edit_menu;
mod effects;
mod file;
mod links;
mod markers;
mod navigation;
mod nest;
mod placement;
mod project_items;
mod queries;
mod sequence;
mod timeline;
mod transitions;

pub use effects::audio_effect_targets;
pub use file::{import_image_sequence, import_streamed};
pub use links::*;
pub use nest::*;
pub use placement::*;
pub use queries::{inspect_project, inspect_sequence};
pub use transitions::*;

pub fn command_specs() -> &'static [CommandSpec] {
    static SPECS: OnceLock<Vec<CommandSpec>> = OnceLock::new();
    SPECS.get_or_init(build)
}

pub fn find(id: &str) -> Option<&'static CommandSpec> {
    command_specs().iter().find(|c| c.id == id)
}

// ---------- enablement ----------

pub(crate) fn always(_: &Session) -> std::result::Result<(), String> {
    Ok(())
}
pub(crate) fn has_seq(s: &Session) -> std::result::Result<(), String> {
    s.active_sequence().map(|_| ()).ok_or_else(|| "no sequence is open".into())
}
pub(crate) fn has_selection(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    if s.state.selection.is_empty() { Err("no clips selected".into()) } else { Ok(()) }
}
fn can_undo(s: &Session) -> std::result::Result<(), String> {
    if s.history.can_undo() { Ok(()) } else { Err("nothing to undo".into()) }
}
fn can_redo(s: &Session) -> std::result::Result<(), String> {
    if s.history.can_redo() { Ok(()) } else { Err("nothing to redo".into()) }
}
pub(crate) fn has_project_selection(s: &Session) -> std::result::Result<(), String> {
    if s.state.project_selection.is_empty() { Err("select an item in the Project panel".into()) } else { Ok(()) }
}
/// The ids a caller named under `many` (a list) or `one` (a single id), when the command documents
/// that key as ids (`"clips":[id]`, `"item":id`); `None` when it named none.
fn named_ids(spec: &CommandSpec, p: &Value, many: &str, one: &str) -> Option<Vec<u64>> {
    let ids: Vec<u64> = if spec.params.contains(&format!("\"{many}\":[id]"))
        && let Some(a) = p.get(many).and_then(Value::as_array)
    {
        a.iter().filter_map(Value::as_u64).collect()
    } else if spec.params.contains(&format!("\"{one}\":id")) {
        u64_p(p, one).into_iter().collect()
    } else {
        Vec::new()
    };
    (!ids.is_empty()).then_some(ids)
}
/// Timeline clips named explicitly in `clips` / `clip` (see [`Session::execute`]): only clips of
/// the active sequence count, so an id of nothing does not stand in for a selection.
pub(crate) fn named_clips(s: &Session, spec: &CommandSpec, p: &Value) -> Option<Vec<ClipId>> {
    let seq = s.active_sequence()?;
    let clips: Vec<ClipId> = named_ids(spec, p, "clips", "clip")?.into_iter().map(ClipId).filter(|c| seq.find_item(*c).is_some()).collect();
    (!clips.is_empty()).then_some(clips)
}
/// Why the clips a caller named in `clips` / `clip` stand in for no selection (#591): a clip of
/// another sequence, an id of no clip (a track, a project item…) or a value that is not an id.
/// `None` when the caller named nothing or no sequence is open, so the selection's reason stands.
pub(crate) fn named_clips_missing(s: &Session, spec: &CommandSpec, p: &Value) -> Option<String> {
    s.active_sequence()?;
    let first = if spec.params.contains("\"clips\":[id]")
        && let Some(a) = p.get("clips").and_then(Value::as_array)
    {
        a.first()?
    } else if spec.params.contains("\"clip\":id") {
        p.get("clip").filter(|v| !v.is_null())?
    } else {
        return None;
    };
    let Some(id) = first.as_u64() else {
        return Some(format!("{first} is not a clip id (clip ids are the `clip` numbers of the items in sequence.inspect)"));
    };
    let elsewhere = s.project.sequences().find(|i| i.as_sequence().is_some_and(|q| q.find_item(ClipId(id)).is_some()));
    Some(match elsewhere {
        Some(i) => format!("clip {id} is in sequence \"{}\", not the active one: open it first with sequence.open {{\"item\":{}}}", i.name, i.id.0),
        None => format!("no clip {id} in the active sequence (clip ids are the `clip` numbers of the items in sequence.inspect)"),
    })
}
/// Project items named explicitly in `items` / `item`, likewise. The Project panel's selection
/// holds bins beside items (one id space, see `project.select`), so a bin named here counts too;
/// the project's own top bin does not, since no command acts on it (#244).
pub(crate) fn named_items(s: &Session, spec: &CommandSpec, p: &Value) -> Option<Vec<ItemId>> {
    let root = s.project.root.id;
    let exists = |i: &ItemId| {
        let b = filmcraft_project::BinId(i.0);
        s.project.item(*i).is_some() || (b != root && s.project.root.find_bin(b).is_some())
    };
    let items: Vec<ItemId> = named_ids(spec, p, "items", "item")?.into_iter().map(ItemId).filter(exists).collect();
    (!items.is_empty()).then_some(items)
}

// ---------- param helpers ----------

pub(crate) fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}
/// A marker's `color` param: absent (or null) is `None`; anything but a label name is an error
/// that lists the accepted names, instead of silently becoming Green (#857).
pub(crate) fn marker_color_p(p: &Value, cmd: &str) -> Result<Option<Label>> {
    let v = match p.get("color") {
        None | Some(Value::Null) => return Ok(None),
        Some(v) => v,
    };
    v.as_str().and_then(Label::from_name).map(Some).ok_or_else(|| {
        let names: Vec<&str> = Label::ALL.iter().map(|l| l.name()).collect();
        bad(cmd, format!("unknown marker color {v}; use one of: {}", names.join(", ")))
    })
}
pub(crate) fn str_p<'a>(p: &'a Value, k: &str) -> Option<&'a str> {
    p.get(k).and_then(Value::as_str)
}
pub(crate) fn f64_p(p: &Value, k: &str) -> Option<f64> {
    p.get(k).and_then(Value::as_f64)
}
/// `fps` as a sequence frame rate; zero or negative rates are refused (all frame maths divides by them).
fn fps_p(p: &Value, cmd: &str) -> Result<Option<FrameRate>> {
    let Some(fps) = f64_p(p, "fps") else { return Ok(None) };
    let r = FrameRate::from_f64(fps);
    if r.num <= 0 || r.frame_duration() <= Tick::ZERO {
        return Err(bad(cmd, format!("fps must be a positive frame rate, got {fps}")));
    }
    Ok(Some(r))
}
pub(crate) fn bool_p(p: &Value, k: &str) -> Option<bool> {
    p.get(k).and_then(Value::as_bool)
}
pub(crate) fn u64_p(p: &Value, k: &str) -> Option<u64> {
    p.get(k).and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
}

/// Typed dimensions/counts must not truncate, wrap or silently accept negative/fractional values.
/// Integer-valued floats (`1920.0`, as JSON from many clients) are integers.
pub(crate) fn checked_u32_p(p: &Value, key: &str, cmd: &str) -> Result<Option<u32>> {
    let Some(value) = p.get(key).filter(|v| !v.is_null()) else { return Ok(None) };
    let exact = value
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .or_else(|| value.as_f64().filter(|f| f.is_finite() && f.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(f)).map(|f| f as u32));
    exact.map(Some).ok_or_else(|| bad(cmd, format!("`{key}` must be an unsigned 32-bit integer")))
}

/// Parse a time from params: `time` (ticks), `frame`, `seconds` or `timecode`, with `prefix`.
pub(crate) fn time_p(s: &Session, p: &Value, prefix: &str) -> Option<Tick> {
    let rate = s.sequence_rate();
    let df = s.active_sequence().map(|q| q.settings.drop_frame).unwrap_or(false);
    time_p_at(p, prefix, rate, df, s.playhead())
}

/// Same formats as [`time_p`], against an explicit rate (Source monitor, remix duration, …).
pub(crate) fn time_p_at(p: &Value, prefix: &str, rate: FrameRate, drop_frame: bool, current: Tick) -> Option<Tick> {
    let k = |n: &str| if prefix.is_empty() { n.to_string() } else { format!("{prefix}{}{}", n[..1].to_uppercase(), &n[1..]) };
    if let Some(t) = p.get(k("time")).and_then(Value::as_i64) {
        return Some(Tick(t));
    }
    if let Some(f) = p.get(k("frame")).and_then(Value::as_i64) {
        return Some(rate.tick_of(f));
    }
    if let Some(sec) = p.get(k("seconds")).and_then(Value::as_f64) {
        return Some(Tick::from_seconds_f64(sec));
    }
    if let Some(tc) = p.get(k("timecode")).and_then(Value::as_str) {
        return parse_timecode(tc, rate, drop_frame, rate.frame_at(current)).ok().map(|f| rate.tick_of(f));
    }
    None
}

/// In/Out / split time on the Source monitor: explicit `time`/`frame`/`seconds`/`timecode`, else the Source playhead.
pub(crate) fn source_mark_time(s: &Session, p: &Value, item: ItemId) -> Tick {
    let view = crate::clip_ops::source_view(s, item);
    let rate = view.as_ref().map(|v| v.rate).unwrap_or_else(|| s.project.item(item).map(|i| i.frame_rate()).unwrap_or_else(|| s.sequence_rate()));
    let df = s.project.sequence(item).map(|q| q.settings.drop_frame).unwrap_or(false);
    time_p_at(p, "", rate, df, s.state.source_playhead).unwrap_or(s.state.source_playhead)
}

/// Sequence time from params. Times before 0 are unreachable in the GUI and cut off on export.
pub(crate) fn sequence_time_p(s: &Session, p: &Value, prefix: &str, cmd: &str) -> Result<Option<Tick>> {
    let Some(t) = time_p(s, p, prefix) else { return Ok(None) };
    if t < Tick::ZERO {
        return Err(bad(cmd, "time must not be negative"));
    }
    Ok(Some(t))
}

pub(crate) fn clip_p(p: &Value, k: &str) -> Option<ClipId> {
    u64_p(p, k).map(ClipId)
}
pub(crate) fn clips_p(s: &Session, p: &Value) -> Vec<ClipId> {
    match p.get("clips").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(|v| v.as_u64().map(ClipId)).collect(),
        None => clip_p(p, "clip").map(|c| vec![c]).unwrap_or_else(|| s.state.selection.clone()),
    }
}
/// The track parameter `k` names: a track id, or "V1" / "A2" (kind and 1-based number).
/// `Ok(None)` when the parameter is absent. Naming a track the active sequence does not have is
/// an error (bad parameters of `cmd`): callers fall back to a default track only when no track
/// was asked for.
pub(crate) fn track_p(s: &Session, p: &Value, k: &str, cmd: &str) -> Result<Option<TrackId>> {
    let v = match p.get(k) {
        None | Some(Value::Null) => return Ok(None),
        Some(v) => v,
    };
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let by_name = |name: &str| {
        let mut rest = name.chars();
        let tracks = match rest.next()? {
            'v' | 'V' => &seq.video_tracks,
            'a' | 'A' => &seq.audio_tracks,
            _ => return None,
        };
        let i: usize = rest.as_str().parse().ok()?;
        tracks.get(i.checked_sub(1)?).map(|t| t.id)
    };
    let found = match v.as_u64() {
        Some(id) => seq.track(TrackId(id)).map(|t| t.id),
        None => v.as_str().and_then(by_name),
    };
    let named = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
    found.map(Some).ok_or_else(|| bad(cmd, format!("no track {named} in this sequence (`{k}`)")))
}
pub(crate) fn item_p(p: &Value, k: &str) -> Option<ItemId> {
    u64_p(p, k).map(ItemId)
}

/// One audio clip of a placement: how many tracks below the destination it goes, and the source
/// channels it plays (`None` = the clip's own default, used for the first one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AudioPlacementSpec {
    pub track_offset: usize,
    pub audio_stream: usize,
    pub source_channels: Option<Vec<u16>>,
}

fn build() -> Vec<CommandSpec> {
    let mut v = file::commands();
    v.extend(edit_menu::commands());
    v.extend(clip::commands());
    v.extend(sequence::commands());
    v.extend(markers::commands());
    v.extend(navigation::commands());
    v.extend(timeline::commands());
    v.extend(effects::commands());
    v.extend(project_items::commands());
    v.extend(queries::commands());
    crate::sequence_tools::splice(&mut v);
    v.extend(crate::captions::commands());
    v.extend(crate::settings::commands());
    v.extend(crate::mixer::commands());
    v.extend(crate::multicam::commands());
    v.extend(crate::essential_sound::commands());
    v.extend(crate::color::commands());
    v.extend(crate::graphics::commands());
    v.extend(crate::graphic_templates::commands());
    v.extend(crate::shortcuts::commands());
    v.extend(crate::relink::commands());
    v.extend(crate::proxies::commands());
    v.extend(crate::project_manager::commands());
    v.extend(crate::masks::commands());
    v.extend(crate::presets::commands());
    v.extend(crate::export_tools::commands());
    v.extend(crate::transcript::commands());
    v.extend(crate::panels::commands());
    v.extend(crate::scopes::commands());
    v.extend(crate::remix::commands());
    v.extend(crate::voiceover::commands());
    v.extend(crate::narration::commands());
    v.extend(crate::keyboard::commands());
    v.extend(crate::project_panel::commands());
    v.extend(crate::media_browser::commands());
    v.extend(crate::paste_media::commands());
    // Edit ▸ Label ▸ <colour>, Paste Attributes, subclips, Video / Audio Options, Replace With Clip…
    // and their menu order
    crate::clip_ops::apply_layout(&mut v);
    // Search Bin, Find, Project Settings, Scene Edit Detection, Normalize Mix Track… (M3.11)
    crate::project_tools::apply_layout(&mut v);
    v.shrink_to_fit();
    v
}

pub(crate) fn json_to_param(template: &ParamValue, v: &Value) -> Option<ParamValue> {
    Some(match template {
        ParamValue::Float(_) => ParamValue::Float(v.as_f64()?),
        ParamValue::Vec2(_) => {
            let a = v.as_array()?;
            ParamValue::Vec2(filmcraft_geom::Vec2::new(a.first()?.as_f64()?, a.get(1)?.as_f64()?))
        }
        ParamValue::Color(_) => match v {
            Value::String(s) => ParamValue::Color(filmcraft_color::parse_hex(s)?),
            Value::Array(a) => ParamValue::Color([
                a.first()?.as_f64()? as f32,
                a.get(1)?.as_f64()? as f32,
                a.get(2)?.as_f64()? as f32,
                a.get(3).and_then(Value::as_f64).unwrap_or(1.0) as f32,
            ]),
            _ => return None,
        },
        ParamValue::Bool(_) => ParamValue::Bool(v.as_bool()?),
        ParamValue::Choice(_) => ParamValue::Choice(v.as_u64()? as u32),
        ParamValue::Text(_) => ParamValue::Text(v.as_str()?.to_string()),
        ParamValue::Path(_) => ParamValue::Path(crate::masks::path_from_json(v)?),
        ParamValue::Curve(_) => {
            ParamValue::Curve(v.as_array()?.iter().filter_map(|p| Some([p.get(0)?.as_f64()? as f32, p.get(1)?.as_f64()? as f32])).collect())
        }
    })
}

/// Close the selected gap (Delete, Backspace or Ripple Delete on a gap).
fn ripple_delete_gap(s: &mut Session, g: crate::GapSelection) -> Result<Value> {
    let still = s.active_sequence().and_then(|q| q.track(g.track)).is_some_and(|tr| edit::through::track_gaps(tr).contains(&g.range));
    if !still {
        s.state.gap_selection = None;
        return Err(EngineError::Other("the selected gap is no longer there".into()));
    }
    close_gap(s, g.track, g.range.start)?;
    s.state.gap_selection = None;
    Ok(json!({"closed": g.range.duration.0}))
}

/// `sequence.closeGap`: ripple the gap on `track` at `t` closed (one undo step, "Ripple Delete").
fn close_gap(s: &mut Session, tr: TrackId, t: Tick) -> Result<()> {
    s.edit_sequence("Ripple Delete", |q, _, st| {
        let before: Vec<(ClipId, Tick)> = q.track(tr).map(|x| x.items.iter().map(|i| (i.id, i.start)).collect()).unwrap_or_default();
        edit::close_gap(q, tr, t)?;
        if st.ripple_sequence_markers {
            // the gap ended where the first clip that moved started
            let moved = before
                .iter()
                .filter_map(|(id, old)| q.find_item(*id).filter(|(_, i)| i.start != *old).map(|(_, i)| (*old, *old - i.start)))
                .min_by_key(|m| m.0);
            if let Some((end, len)) = moved {
                crate::sequence_tools::ripple_markers(&mut q.markers, end, -len);
            }
        }
        Ok(())
    })
}

/// Seconds → ticks helper for callers.
pub fn secs(s: f64) -> Tick {
    Tick((s * TICKS_PER_SECOND as f64).round() as i64)
}
