//! Linked clips: which clips move, slip and trim together.

use super::*;

/// Slip clips together: the media delta of each clip is scaled down by the same factor so that the
/// most constrained clip (media end or start) still moves, and no linked partner slips further.
pub(crate) fn slip_together(q: &mut filmcraft_project::Sequence, ctx: &mut edit::EditCtx<'_>, deltas: &[(ClipId, Tick)]) -> Result<()> {
    let mut scale = 1.0_f64;
    let mut any = false;
    for (c, d) in deltas {
        if d.0 == 0 {
            continue;
        }
        match edit::slip(&mut q.clone(), *c, *d, ctx) {
            Err(edit::EditError::Locked) => continue,
            Err(e) => return Err(e.into()),
            Ok(x) => {
                any = true;
                scale = scale.min(x.0.abs() as f64 / d.0.abs() as f64);
            }
        }
    }
    if !any && deltas.iter().any(|(_, d)| d.0 != 0) {
        return Err(edit::EditError::Locked.into());
    }
    for (c, d) in deltas {
        let dd = if scale < 1.0 { Tick((d.0 as f64 * scale).round() as i64) } else { *d };
        match edit::slip(q, *c, dd, ctx) {
            Err(edit::EditError::Locked) => continue,
            Err(e) => return Err(e.into()),
            Ok(_) => {}
        }
    }
    Ok(())
}

/// Adjacent roll pairs that follow `left`/`right` when linked selection is on: the given cut, plus
/// through-edits between the same pieces' linked partners at that same time (Premiere linked
/// selection). Split edits (audio cut at another time) and unlinked clips are not added.
pub fn linked_roll_pairs(seq: &filmcraft_project::Sequence, left: ClipId, right: ClipId, follow_links: bool) -> Vec<(ClipId, ClipId)> {
    let mut pairs = vec![(left, right)];
    if !follow_links {
        return pairs;
    }
    let Some((_, l)) = seq.find_item(left) else {
        return pairs;
    };
    let Some((_, r)) = seq.find_item(right) else {
        return pairs;
    };
    let cut = l.end();
    let left_link = l.link;
    let right_link = r.link;
    if left_link.is_none() && right_link.is_none() {
        return pairs;
    }
    for tr in seq.all_tracks() {
        if tr.locked {
            continue;
        }
        for w in tr.items.windows(2) {
            if w[0].id == left && w[1].id == right {
                continue;
            }
            if w[0].end() != w[1].start || w[0].end() != cut {
                continue;
            }
            let left_ok = left_link.is_some_and(|g| w[0].link == Some(g));
            let right_ok = right_link.is_some_and(|g| w[1].link == Some(g));
            if left_ok && right_ok {
                pairs.push((w[0].id, w[1].id));
            }
        }
    }
    pairs
}

/// Expand a clip selection with linked partners (when linked selection is on).
pub fn with_links(s: &Session, clips: &[ClipId]) -> Vec<ClipId> {
    let Some(seq) = s.active_sequence() else { return clips.to_vec() };
    let mut out = clips.to_vec();
    if s.state.linked_selection {
        let links: Vec<u64> = clips.iter().filter_map(|c| seq.find_item(*c).and_then(|(_, i)| i.link)).collect();
        for t in seq.all_tracks() {
            for i in &t.items {
                if i.link.is_some_and(|l| links.contains(&l)) && !out.contains(&i.id) {
                    out.push(i.id);
                }
            }
        }
    }
    out
}

/// The delta a trim of `clips` (a clip and its linked partners) can apply together: `delta`
/// clamped by each one's media and neighbours.
pub(super) fn clamp_trim_linked(
    q: &filmcraft_project::Sequence,
    clips: &[ClipId],
    edge: Edge,
    mode: TrimMode,
    delta: Tick,
    ctx: &edit::EditCtx,
) -> Result<Tick> {
    let mut d = delta;
    let mut any = false;
    for c in clips {
        match edit::clamp_trim(q, *c, edge, mode, d, ctx) {
            Err(edit::EditError::Locked) => continue,
            Err(e) => return Err(e.into()),
            Ok(x) => {
                any = true;
                if x.abs() < d.abs() {
                    d = x;
                }
            }
        }
    }
    if !any {
        return Err(edit::EditError::Locked.into());
    }
    Ok(d)
}

/// The delta `timeline.trim` would apply for `delta`, without editing anything: the Timeline's
/// trim drag shows this, so the edge stops at the end of the media or the neighbouring clip
/// instead of following the pointer past it (#653).
pub fn trim_delta(s: &Session, clip: ClipId, edge: Edge, mode: TrimMode, delta: Tick) -> Result<Tick> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let durations = |id: ItemId| crate::media_duration(&s.project, &s.media, id);
    let starts = |id: ItemId| crate::media_start(&s.project, id);
    let mut next = s.project.next_id;
    let ctx = edit::EditCtx { next_id: &mut next, media_duration: &durations, media_start: &starts, min_duration: q.settings.frame_rate.frame_duration() };
    clamp_trim_linked(q, &with_links(s, &[clip]), edge, mode, delta, &ctx)
}
