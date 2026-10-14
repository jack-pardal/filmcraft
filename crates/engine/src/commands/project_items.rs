//! Project panel commands.

use super::*;

pub(super) fn commands() -> Vec<CommandSpec> {
    vec![
        cmd!("project.select", "Select Project Items", [], None, r#"{"items":[id]}"#, always, |s, p| {
            s.state.project_selection =
                p.get("items").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_u64().map(ItemId)).collect()).unwrap_or_default();
            Ok(Value::Null)
        }),
        cmd!("project.delete", "Clear", [], Some("Delete"), r#"{"items":[id]?}"#, has_project_selection, |s, p| {
            let asked: Vec<ItemId> = p
                .get("items")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_u64().map(ItemId)).collect())
                .unwrap_or_else(|| s.state.project_selection.clone());
            // A bin named here goes with everything in it, as in the Project panel of any editor.
            let root = s.project.root.id;
            let named: Vec<filmcraft_project::BinId> =
                asked.iter().map(|i| filmcraft_project::BinId(i.0)).filter(|b| *b != root && s.project.root.find_bin(*b).is_some()).collect();
            // a bin inside another named bin goes with that one
            let bins: Vec<filmcraft_project::BinId> = named
                .iter()
                .copied()
                .filter(|b| !named.iter().any(|o| o != b && s.project.root.find_bin(*o).is_some_and(|o| o.find_bin(*b).is_some())))
                .collect();
            let mut items: Vec<ItemId> = asked.iter().copied().filter(|i| !named.contains(&filmcraft_project::BinId(i.0))).collect();
            for b in &bins {
                if let Some(bin) = s.project.root.find_bin(*b) {
                    bin.all_items(&mut items);
                }
            }
            items.sort_by_key(|i| i.0);
            items.dedup();
            let mut used = std::collections::HashSet::new();
            for it in s.project.sequences().filter(|it| !items.contains(&it.id)) {
                if let Some(q) = it.as_sequence() {
                    for t in q.all_tracks() {
                        for clip in t
                            .items
                            .iter()
                            .filter(|clip| !items.contains(&clip.item) || s.project.item(clip.item).is_some_and(|item| matches!(item.kind, ItemKind::Media(_))))
                        {
                            // Preserve the whole valid file-backed chain, not just its root.
                            if !s.project.resolve_media(clip.item).is_some_and(|(_, media, _)| matches!(media.media, MediaRef::File { .. })) {
                                continue;
                            }
                            let mut id = clip.item;
                            for _ in 0..16 {
                                used.insert(id);
                                match s.project.item(id).map(|item| &item.kind) {
                                    Some(ItemKind::Subclip { parent, .. }) => id = *parent,
                                    _ => break,
                                }
                            }
                        }
                    }
                }
            }
            let mut gone = Vec::new();
            let mut offlined = Vec::new();
            let mut retained_subclips = Vec::new();
            for i in items {
                let keep = used.contains(&i)
                    && s.project.item(i).and_then(filmcraft_project::ProjectItem::as_media).is_some_and(|m| matches!(m.media, MediaRef::File { .. }));
                if keep {
                    offlined.push(i);
                } else if used.contains(&i) && s.project.item(i).is_some_and(|item| matches!(item.kind, ItemKind::Subclip { .. })) {
                    retained_subclips.push(i);
                } else {
                    gone.push(i);
                }
            }
            s.edit("Clear", |pr, st| {
                for i in &offlined {
                    if let Some(m) = pr.item_mut(*i).and_then(filmcraft_project::ProjectItem::as_media_mut) {
                        m.offline = true;
                    }
                }
                for b in &bins {
                    pr.root.remove_bin(*b);
                }
                for i in offlined.iter().chain(&retained_subclips) {
                    if pr.root.parent_of(*i).is_none() {
                        pr.root.children.push(filmcraft_project::BinEntry::Item(*i));
                    }
                }
                for i in &gone {
                    pr.items.remove(i);
                    pr.root.remove_item(*i);
                }
                let seq_ids: Vec<ItemId> = pr.sequences().map(|q| q.id).collect();
                for sid in seq_ids {
                    if let Some(q) = pr.sequence_mut(sid) {
                        for t in q.all_tracks_mut() {
                            t.items.retain(|x| !gone.contains(&x.item));
                            filmcraft_edit::remove_orphan_transitions(t);
                        }
                    }
                }
                st.project_selection.clear();
                Ok(())
            })?;
            for i in gone.iter().chain(&offlined) {
                s.media.remove(*i);
            }
            crate::relink::refresh(s);
            s.fix_state();
            Ok(json!({"items": gone.len(), "bins": bins.len(), "offline": offlined.len()}))
        }),
        cmd!("project.moveToBin", "Move to Bin", [], None, r#"{"items":[id|binId]?,"bin":binId|null}"#, always, |s, p| {
            let items: Vec<ItemId> = match p.get("items").and_then(Value::as_array) {
                Some(a) => a.iter().filter_map(|v| v.as_u64().map(ItemId)).collect(),
                None => s.state.project_selection.clone(),
            };
            if items.is_empty() {
                return Err(bad("project.moveToBin", "need `items` (or a Project panel selection)"));
            }
            let bin = u64_p(p, "bin").map(filmcraft_project::BinId);
            let moved = s.edit("Move to Bin", |pr, _| pr.move_to_bin(&items, bin).ok_or_else(|| bad("project.moveToBin", "no such bin")))?;
            Ok(json!({"moved": moved}))
        }),
        cmd!("project.setMarks", "Set Source In/Out", [], None, r#"{"item":id,"in":ticks?|null,"out":ticks?|null}"#, always, |s, p| {
            let id = item_p(p, "item").or(s.state.source_item).ok_or_else(|| bad("project.setMarks", "need `item`"))?;
            let p = p.clone();
            s.edit("Mark", |pr, _| {
                let it = pr.item_mut(id).ok_or_else(|| bad("project.setMarks", "no such item"))?;
                // an ordinary In/Out replaces that side's split points
                if p.get("in").is_some() {
                    it.split.video_in = None;
                    it.split.audio_in = None;
                }
                if p.get("out").is_some() {
                    it.split.video_out = None;
                    it.split.audio_out = None;
                }
                if let Some(m) = it.as_media_mut() {
                    if let Some(v) = p.get("in") {
                        m.mark_in = v.as_i64().map(Tick);
                    }
                    if let Some(v) = p.get("out") {
                        m.mark_out = v.as_i64().map(Tick);
                    }
                } else if let Some(q) = it.as_sequence_mut() {
                    // a sequence loaded in the Source Monitor is marked like a clip: these are the
                    // sequence's own In and Out
                    if let Some(v) = p.get("in") {
                        q.mark_in = v.as_i64().map(Tick);
                        q.split.video_in = None;
                        q.split.audio_in = None;
                    }
                    if let Some(v) = p.get("out") {
                        q.mark_out = v.as_i64().map(Tick);
                        q.split.video_out = None;
                        q.split.audio_out = None;
                    }
                }
                Ok(())
            })?;
            Ok(Value::Null)
        }),
    ]
}
