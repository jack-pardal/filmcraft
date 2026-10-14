//! File menu: projects, sequences, import, generators and project files.

use super::*;

pub(super) fn commands() -> Vec<CommandSpec> {
    vec![
        cmd!("file.newProject", "Project…", ["File", "New"], Some("Cmd+Alt+N"), r#"{"name":str}"#, always, |s, p| {
            let name = str_p(p, "name").unwrap_or("Untitled").to_string();
            s.project = std::sync::Arc::new(filmcraft_project::Project::new(&name));
            s.history = Default::default();
            s.history.limit = 200;
            s.state = crate::Session::default().state;
            if s.path.take().is_some() {
                s.previews.reset_temp();
            }
            s.revision += 1;
            s.saved_revision = s.revision;
            s.events.push(crate::Event::ProjectChanged { revision: s.revision });
            Ok(Value::Null)
        }),
        cmd!("file.openDemoProject", "Demo Project", ["File", "New"], None, "{}", always, |s, _| {
            let (p, seq) = crate::demo::demo_project(&s.media);
            s.project = std::sync::Arc::new(p);
            s.history = Default::default();
            s.history.limit = 200;
            s.state = crate::Session::default().state;
            s.state.active_sequence = Some(seq);
            s.state.open_sequences = vec![seq];
            s.state.playheads.insert(seq, FrameRate::FPS_23_976.tick_of(4 * 24 + 6));
            if s.path.take().is_some() {
                s.previews.reset_temp();
            }
            s.revision += 1;
            s.saved_revision = s.revision;
            s.events.push(crate::Event::ProjectChanged { revision: s.revision });
            s.events.push(crate::Event::OpenSequence(seq));
            Ok(json!({"sequence": seq.0}))
        }),
        cmd!(
            "file.newSequence",
            "Sequence…",
            ["File", "New"],
            Some("Cmd+N"),
            r#"{"name":str,"width":u32=1920,"height":u32=1080,"fps":f64=23.976,"sampleRate":u32=48000,"video":n=3,"audio":n=3,"mix":"Stereo|Mono|5.1|Adaptive"?,"trackType":"Standard|Mono|5.1|Adaptive"?,"fromItem":itemId?,"fromItems":[itemId]?}"#,
            always,
            |s, p| s.edit_group("New Sequence", |s| {
                // `fromItems` is `fromItem` for a list: the first sets the sequence, the rest follow it back to back
                let mut from_ids: Vec<ItemId> = match p.get("fromItems") {
                    None => Vec::new(),
                    Some(l) => l
                        .as_array()
                        .and_then(|a| a.iter().map(|v| v.as_u64().map(ItemId)).collect())
                        .ok_or_else(|| bad("file.newSequence", "`fromItems` must be a list of item ids"))?,
                };
                if let Some(one) = item_p(p, "fromItem") {
                    from_ids.insert(0, one);
                }
                let mut st = SequenceSettings::default();
                if let Some(from) = from_ids.first().and_then(|i| s.project.item(*i)).and_then(|i| i.as_media()) {
                    st = default_seq_settings_for(from);
                }
                if let Some(w) = checked_u32_p(p, "width", "file.newSequence")? {
                    st.width = w;
                }
                if let Some(h) = checked_u32_p(p, "height", "file.newSequence")? {
                    st.height = h;
                }
                if let Some(fps) = fps_p(p, "file.newSequence")? {
                    st.frame_rate = fps;
                }
                if let Some(sr) = checked_u32_p(p, "sampleRate", "file.newSequence")? {
                    st.sample_rate = sr;
                }
                if let Some(m) = str_p(p, "mix") {
                    st.audio_master =
                        crate::mixer::channels_from(m).ok_or_else(|| bad("file.newSequence", format!("unknown mix `{m}` (Stereo, Mono, 5.1, Adaptive)")))?;
                }
                let track_type = match str_p(p, "trackType") {
                    Some(t) => Some(
                        crate::mixer::channels_from(t)
                            .ok_or_else(|| bad("file.newSequence", format!("unknown track type `{t}` (Standard, Mono, 5.1, Adaptive)")))?,
                    ),
                    None => None,
                };
                st.preset = format!("{}x{} {}", st.width, st.height, st.frame_rate.label());
                st.validate().map_err(|e| bad("file.newSequence", e))?;
                let nv = checked_u32_p(p, "video", "file.newSequence")?.unwrap_or(3);
                let na = checked_u32_p(p, "audio", "file.newSequence")?.unwrap_or(3);
                if nv > 256 || na > 256 {
                    return Err(bad("file.newSequence", "at most 256 video and 256 audio tracks are supported"));
                }
                let n = s.project.sequences().count() + 1;
                let name = str_p(p, "name").map(str::to_string).unwrap_or_else(|| format!("Sequence {n:02}"));
                let seq_label = s.prefs.labels.defaults.sequence;
                let id = s.edit("New Sequence", |pr, st2| {
                    let id = pr.new_sequence(&name, st, nv as usize, na as usize, None);
                    if let Some(it) = pr.item_mut(id) {
                        it.label = seq_label;
                    }
                    if let (Some(c), Some(q)) = (track_type, pr.sequence_mut(id)) {
                        q.audio_tracks.iter_mut().for_each(|t| t.channels = c);
                    }
                    st2.active_sequence = Some(id);
                    if !st2.open_sequences.contains(&id) {
                        st2.open_sequences.push(id);
                    }
                    Ok(id)
                })?;
                if let Some((&from, rest)) = from_ids.split_first() {
                    let dur = s.project.item(from).map(|i| i.duration()).unwrap_or_default();
                    let tg = s.targeting();
                    let ids =
                        place_item(s, from, TimeRange::new(Tick::ZERO, dur), Tick::ZERO, tg.video_dest, tg.audio_dest, false, "New Sequence From Clip", None)?;
                    if !rest.is_empty() {
                        let end = s.active_sequence().and_then(|q| ids.iter().filter_map(|c| q.find_item(*c).map(|(_, i)| i.end())).max()).unwrap_or_default();
                        place_items(s, &json!({"time": end.0}), &json!(rest.iter().map(|i| i.0).collect::<Vec<_>>()))?;
                    }
                }
                s.events.push(crate::Event::OpenSequence(id));
                Ok(json!({"sequence": id.0}))
            })
        ),
        cmd!("file.newBin", "Bin", ["File", "New"], Some("Cmd+B"), r#"{"name":str,"parent":binId?}"#, always, |s, p| {
            let n = str_p(p, "name").unwrap_or("New Bin").to_string();
            let parent = u64_p(p, "parent").map(filmcraft_project::BinId);
            let id = s.edit("New Bin", |pr, _| Ok(pr.add_bin(&n, parent)))?;
            Ok(json!({"bin": id.0}))
        }),
        cmd!("file.newAdjustmentLayer", "Adjustment Layer…", ["File", "New"], None, r#"{"seconds":f64=5}"#, always, |s, p| {
            let st = s.active_sequence().map(|q| q.settings.clone()).unwrap_or_default();
            let secs = f64_p(p, "seconds").unwrap_or(5.0);
            let id = s.edit("New Adjustment Layer", |pr, st2| {
                let id = pr.add_item(
                    "Adjustment Layer",
                    Label::Lavender,
                    ItemKind::AdjustmentLayer { width: st.width, height: st.height, rate: st.frame_rate, duration: Tick::from_seconds_f64(secs) },
                    None,
                );
                st2.project_selection = vec![id];
                Ok(id)
            })?;
            Ok(json!({"item": id.0}))
        }),
        cmd!("file.newBarsAndTone", "Bars and Tone…", ["File", "New"], None, r#"{"seconds":f64=10}"#, always, |s, p| new_generator(
            s,
            Generator::BarsAndTone,
            "Bars and Tone",
            Label::Lavender,
            p
        )),
        cmd!("file.newBlackVideo", "Black Video…", ["File", "New"], None, r#"{"seconds":f64}"#, always, |s, p| new_generator(
            s,
            Generator::BlackVideo,
            "Black Video",
            Label::Lavender,
            p
        )),
        cmd!("file.newColorMatte", "Color Matte…", ["File", "New"], None, r##"{"color":"#rrggbb","seconds":f64}"##, always, |s, p| {
            let c = str_p(p, "color").and_then(filmcraft_color::parse_hex).unwrap_or([0.1, 0.1, 0.1, 1.0]);
            new_generator(s, Generator::ColorMatte { color: c }, "Color Matte", Label::Lavender, p)
        }),
        cmd!("project.matteColor", "Color Matte Color…", [], None, r##"{"item":id?,"color":"#rrggbb"}"##, always, set_matte_color),
        cmd!("file.newCountingLeader", "Universal Counting Leader…", ["File", "New"], None, "{}", always, |s, p| new_generator(
            s,
            Generator::CountingLeader,
            "Universal Counting Leader",
            Label::Lavender,
            p
        )),
        cmd!("file.newTransparentVideo", "Transparent Video…", ["File", "New"], None, "{}", always, |s, p| new_generator(
            s,
            Generator::TransparentVideo,
            "Transparent Video",
            Label::Lavender,
            p
        )),
        cmd!(
            "file.importDemoFootage",
            "Demo Footage",
            ["File", "Import From"],
            None,
            r#"{"scene":"OceanSunset|Aurora|CityNight|Dunes|Plasma|Forest"}"#,
            always,
            |s, p| {
                let scenes: Vec<filmcraft_media::DemoScene> = match str_p(p, "scene") {
                    Some(n) => filmcraft_media::DemoScene::ALL.iter().copied().filter(|d| format!("{d:?}").eq_ignore_ascii_case(n)).collect(),
                    None => filmcraft_media::DemoScene::ALL.to_vec(),
                };
                let pool = s.media.clone();
                let ids = s.edit("Import Demo Footage", |pr, _| {
                    Ok(scenes
                        .iter()
                        .map(|sc| crate::demo::add_generator(pr, &pool, GeneratorSource::demo(*sc), sc.file_name(), Label::Iris, None))
                        .collect::<Vec<_>>())
                })?;
                Ok(json!({"items": ids.iter().map(|i| i.0).collect::<Vec<_>>()}))
            }
        ),
        cmd!("file.import", "Import…", ["File"], Some("Cmd+I"), r#"{"paths":[str],"bin":binId?,"imageSequence":bool?,"replace":bool?}"#, always, |s, p| {
            let bin = u64_p(p, "bin").map(filmcraft_project::BinId);
            let paths: Vec<String> = match p.get("paths").and_then(Value::as_array) {
                Some(a) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
                None => str_p(p, "path").map(|x| vec![x.to_string()]).unwrap_or_default(),
            };
            if paths.is_empty() {
                return Err(bad("file.import", "need `paths`"));
            }
            if bool_p(p, "replace").unwrap_or(false) {
                let name = s.project.name.clone();
                s.execute("file.newProject", json!({ "name": name }))?;
            }
            let mut ids = Vec::new();
            let mut errors = Vec::new();
            let mut sequences = Vec::new();
            let mut reports = Vec::new();
            let mut image_sequences = Vec::new();
            // Image Sequence: each path is the first frame of a numbered still sequence. With
            // Settings ▸ Media ▸ Import image sequences on, a single numbered still is detected.
            let as_sequence = p.get("imageSequence").and_then(Value::as_bool).unwrap_or(false);
            let detect = !as_sequence && s.prefs.media.import_image_sequences && paths.len() == 1;
            for path in paths {
                if as_sequence || (detect && detect_image_sequence(s, &path)) {
                    match import_image_sequence(s, &path, bin) {
                        Ok((id, frames, missing)) => {
                            ids.push(id.0);
                            image_sequences.push(json!({"item": id.0, "frames": frames, "missing": missing}));
                        }
                        Err(e) => errors.push(format!("{path}: {e}")),
                    }
                    continue;
                }
                // Media files through the host's reader (no whole-file read) when it has one.
                let streamed = filmcraft_media::is_importable(std::path::Path::new(&path)) && s.services.reader(&path).is_some();
                let read = if streamed { Ok(Vec::new()) } else { s.services.read_file(&path) };
                match read {
                    Ok(_) if streamed => match import_streamed(s, &path, bin) {
                        Ok(id) => ids.push(id.0),
                        Err(e) => errors.push(format!("{path}: {e}")),
                    },
                    Ok(b) => {
                        if let Some(fmt) = crate::captions::detect(&path, &b) {
                            match crate::captions::import(s, &path, &b, fmt, None) {
                                Ok(r) => reports.push(r),
                                Err(e) => errors.push(format!("{path}: {e}")),
                            }
                        } else if let Some(fmt) = crate::interchange::detect(&path, &b) {
                            match crate::interchange::import(s, &path, &b, fmt) {
                                Ok(r) => {
                                    sequences.extend(r["sequences"].as_array().cloned().unwrap_or_default());
                                    reports.push(r);
                                }
                                Err(e) => errors.push(format!("{path}: {e}")),
                            }
                        } else {
                            match import_bytes(s, &path, b.into(), bin) {
                                Ok(id) => ids.push(id.0),
                                Err(e) => errors.push(format!("{path}: {e}")),
                            }
                        }
                    }
                    Err(e) => errors.push(format!("{path}: {e}")),
                }
            }
            if ids.is_empty() && sequences.is_empty() && reports.is_empty() && !errors.is_empty() {
                return Err(EngineError::Other(errors.join("; ")));
            }
            // Project Settings ▸ Ingest: copy / transcode / create proxies
            // (image sequences are many files: ingest copies and transcodes single files only)
            let item_ids: Vec<ItemId> = ids.iter().filter(|i| !image_sequences.iter().any(|q| q["item"].as_u64() == Some(**i))).map(|i| ItemId(*i)).collect();
            // Settings ▸ Media Analysis & Transcription ▸ Automatically transcribe clips
            let ma = &s.prefs.media_analysis;
            if ma.auto_transcribe && ma.auto_transcribe_scope == "allImported" && !ids.is_empty() {
                let audio: Vec<u64> = ids.iter().copied().filter(|i| s.project.item(ItemId(*i)).is_some_and(|it| it.has_audio())).collect();
                if !audio.is_empty() {
                    // without speech-to-text the setting can't act: say so instead of importing
                    // silently untranscribed (#89)
                    let r = crate::transcript::can_transcribe(s)
                        .map_err(EngineError::Other)
                        .and_then(|()| s.execute("transcript.generate", json!({"items": audio})));
                    if let Err(e) = r {
                        errors.push(format!("transcription: {e}"));
                    }
                }
            }
            let ingest = match crate::proxies::ingest(s, &item_ids) {
                Ok(v) => v,
                Err(e) => {
                    errors.push(format!("ingest: {e}"));
                    Value::Null
                }
            };
            let mut out = if !ingest.is_null() {
                json!({"items": ids, "sequences": sequences, "documents": reports, "errors": errors, "ingest": ingest})
            } else if reports.is_empty() {
                json!({"items": ids, "errors": errors})
            } else {
                json!({"items": ids, "sequences": sequences, "documents": reports, "errors": errors})
            };
            if !image_sequences.is_empty() {
                out["imageSequences"] = json!(image_sequences);
            }
            // Newly imported files may resolve paths the project already listed as offline
            // (issue #110): rebuild s.offline.missing so the "Media missing" badge clears, and drop
            // the cached slate of every item that came back so the monitors show the file again.
            let was_missing = s.offline.missing.clone();
            crate::relink::refresh(s);
            for item in was_missing.iter().filter(|i| !s.offline.missing.contains(i)) {
                s.media.remove(*item);
            }
            Ok(out)
        }),
        cmd!(
            "file.exportInterchange",
            "Export Interchange",
            [],
            None,
            r#"{"format":"edl|xml|fcpxml|otio|aaf|omf"=xml,"path":str,"sequence":id?}"#,
            has_seq,
            crate::interchange::export
        ),
        cmd!("file.exportEdl", "EDL…", ["File", "Export"], None, r#"{"path":str}"#, has_seq, |s, p| {
            let mut q = p.clone();
            q["format"] = json!("edl");
            crate::interchange::export(s, &q)
        }),
        cmd!("file.exportFcp7Xml", "Final Cut Pro XML…", ["File", "Export"], None, r#"{"path":str}"#, has_seq, |s, p| {
            let mut q = p.clone();
            q["format"] = json!("xml");
            crate::interchange::export(s, &q)
        }),
        cmd!("file.exportFcpxml", "FCPXML…", ["File", "Export"], None, r#"{"path":str}"#, has_seq, |s, p| {
            let mut q = p.clone();
            q["format"] = json!("fcpxml");
            crate::interchange::export(s, &q)
        }),
        cmd!("file.exportOtio", "OpenTimelineIO…", ["File", "Export"], None, r#"{"path":str}"#, has_seq, |s, p| {
            let mut q = p.clone();
            q["format"] = json!("otio");
            crate::interchange::export(s, &q)
        }),
        cmd!(
            "file.exportAaf",
            "AAF…",
            ["File", "Export"],
            None,
            r#"{"path":str,"sequence":id?,"mixdownVideo":bool?,"mixdownFormat":"mov|mxf"?,"breakoutToMono":bool?,"audio":"embedded|separate|linked"?,"audioFormat":"wav|aiff|mxf"?,"sampleRate":int?,"bitDepth":"16|24"?,"trimAudio":bool?,"handles":frames?,"renderAudioEffects":bool?,"smallSectors":bool?}"#,
            has_seq,
            |s, p| crate::aaf_omf::export_aaf(s, p)
        ),
        cmd!(
            "file.exportOmf",
            "OMF…",
            ["File", "Export"],
            None,
            r#"{"path":str,"sequence":id?,"title":str?,"audio":"embedded|separate"?,"audioFormat":"wav|aiff"?,"sampleRate":int?,"bitDepth":"16|24"?,"trimAudio":bool?,"handles":frames?,"renderAudioEffects":bool?,"breakoutToMono":bool?}"#,
            has_seq,
            |s, p| crate::aaf_omf::export_omf(s, p)
        ),
        cmd!("file.importAaf", "Import AAF…", [], None, r#"{"path":str}"#, always, |s, p| crate::aaf_omf::import_document(s, p, "file.importAaf")),
        cmd!("file.save", "Save", ["File"], Some("Cmd+S"), r#"{"path":str?}"#, always, |s, p| {
            let path = str_p(p, "path").map(str::to_string).or_else(|| s.path.clone()).ok_or_else(|| bad("file.save", "no path (use Save As)"))?;
            write_project(s, &path, true)
        }),
        cmd!("file.saveAs", "Save As…", ["File"], Some("Cmd+Shift+S"), r#"{"path":str}"#, always, |s, p| {
            let path = str_p(p, "path").ok_or_else(|| bad("file.saveAs", "need `path`"))?.to_string();
            write_project(s, &path, true)
        }),
        cmd!("file.saveCopy", "Save a Copy…", ["File"], Some("Cmd+Alt+S"), r#"{"path":str}"#, always, |s, p| {
            let path = str_p(p, "path").ok_or_else(|| bad("file.saveCopy", "need `path`"))?.to_string();
            write_project(s, &path, false)
        }),
        cmd!("file.revert", "Revert", ["File"], None, "{}", has_saved_path, |s, _| {
            let path = s.path.clone().ok_or_else(|| bad("file.revert", "project was never saved"))?;
            let keep = s.state.active_sequence;
            let r = open_project(s, &path)?;
            if let Some(k) = keep.filter(|k| s.project.sequence(*k).is_some()) {
                s.state.active_sequence = Some(k);
                s.state.open_sequences = vec![k];
            }
            Ok(r)
        }),
        cmd!("file.open", "Open Project…", ["File"], Some("Cmd+O"), r#"{"path":str}"#, always, |s, p| {
            let path = str_p(p, "path").ok_or_else(|| bad("file.open", "need `path`"))?.to_string();
            open_project(s, &path)
        }),
        cmd!("file.recover", "Recover Unsaved Changes…", ["File"], None, r#"{"id":str?}"#, has_recovery, |s, p| recover(s, str_p(p, "id"))),
        cmd!("file.discardRecovery", "Discard Unsaved Changes", [], None, r#"{"id":str?,"all":bool?}"#, has_recovery, |s, p| {
            let all = bool_p(p, "all").unwrap_or(false);
            let id = str_p(p, "id").map(str::to_string);
            let Some(per) = s.persistence.as_mut() else { return Ok(json!({"discarded": 0})) };
            let mut n = 0;
            let mut keep = Vec::new();
            for (i, c) in std::mem::take(&mut per.candidates).into_iter().enumerate() {
                let hit = all || id.as_deref().map_or(i == 0, |want| want == c.id);
                if hit {
                    crate::autosave::discard_candidate(&c).map_err(|e| EngineError::Other(e.to_string()))?;
                    n += 1;
                } else {
                    keep.push(c);
                }
            }
            per.candidates = keep;
            if n == 0 {
                return Err(bad("file.discardRecovery", "no such recovery session"));
            }
            Ok(json!({"discarded": n}))
        }),
        query!("file.recoveryList", "List Recoverable Sessions", "{}", |s, _| {
            Ok(s.persistence.as_ref().map(|p| p.candidates_json()).unwrap_or_else(|| json!([])))
        }),
        cmd!("file.autoSaveNow", "Auto Save Now", [], None, "{}", always, |s, _| {
            let per = s.persistence.as_ref().ok_or_else(|| EngineError::Other("auto-save is not running in this session".into()))?;
            let r = per.auto_save_now().map_err(EngineError::Other)?;
            s.poll_persistence();
            Ok(json!({"path": r.map(|p| p.to_string_lossy().into_owned())}))
        }),
        query!("file.autoSaveStatus", "Auto Save Status", "{}", |s, _| {
            let Some(per) = s.persistence.as_ref() else { return Ok(json!({"running": false, "prefs": s.prefs.auto_save})) };
            Ok(json!({
                "running": true,
                "prefs": s.prefs.auto_save,
                "dataDir": per.data_dir,
                "sessionDir": per.session_dir,
                "autoSaveDir": auto_save_dir(s),
                "status": per.status,
                "lastAutoSaveAt": per.status.last_auto_save_unix.map(|u| per.local_time(u)),
                "lastJournalAt": per.status.last_journal_unix.map(|u| per.local_time(u)),
                "recoverable": per.candidates.len(),
            }))
        }),
        query!("file.listAutoSaves", "Browse Auto-Saves", "{}", |s, _| {
            let dir = auto_save_dir(s);
            let files = filmcraft_format::autosave::list_auto_saves(&dir, &auto_save_name(s));
            Ok(json!({"dir": dir, "files": files.iter().rev().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>()}))
        }),
        query!("prefs.get", "Get Preferences", r#"{"key":str?}"#, |s, p| match str_p(p, "key") {
            Some(k) => s.prefs.get(k).ok_or_else(|| bad("prefs.get", format!("unknown preference `{k}`"))),
            None => Ok(json!({"values": s.prefs.to_value(), "keys": s.prefs.keys()})),
        }),
        cmd!("prefs.set", "Set Preferences", [], None, r#"{"key":str,"value":any}|{"values":{key:value}}"#, always, |s, p| {
            let mut next = s.prefs.clone();
            if let Some(k) = str_p(p, "key") {
                next.set(k, p.get("value").cloned().ok_or_else(|| bad("prefs.set", "need `value`"))?).map_err(|e| bad("prefs.set", e))?;
            }
            if let Some(m) = p.get("values").and_then(Value::as_object) {
                for (k, v) in m {
                    next.set(k, v.clone()).map_err(|e| bad("prefs.set", e))?;
                }
            }
            s.set_prefs(next).map_err(|e| EngineError::Other(format!("saving preferences: {e}")))?;
            Ok(s.prefs.to_value())
        }),
        cmd!("prefs.reset", "Reset Preferences", [], None, r#"{"category":str?}"#, always, |s, p| {
            let next = match str_p(p, "category") {
                Some(c) => {
                    let mut n = s.prefs.clone();
                    n.reset_category(c).map_err(|e| bad("prefs.reset", e))?;
                    n
                }
                None => Default::default(),
            };
            s.set_prefs(next).map_err(|e| EngineError::Other(format!("saving preferences: {e}")))?;
            Ok(s.prefs.to_value())
        }),
        cmd!(
            "file.exportMedia",
            "Media…",
            ["File", "Export"],
            None,
            r#"{"path":str,"preset":str?,"settings":ExportSettings?,"format":"h264|hevc|prores|dnxhr|apv|mjpeg|mxf-op1a|mxf-opatom|png|tiff|bmp|jpg|tga|dpx|exr|gif|wav|aiff|aac|flac"?,"audioCodec":"auto|aac|pcm|flac"?,"flacLevel":0..8=5,"width":u32?,"height":u32?,"fps":f64?,"bitrateKbps":u32?,"bitrateMode":"cbr|vbr1Pass|vbr2Pass|crf"?,"crf":0..51=23,"hardwareEncoding":"off|auto"?,"scale":f32=1,"audio":bool=true,"quality":0..100,"burnCaptions":bool=false,"alpha":bool=false,"exrCompression":"none|rle|zips|zip"?,"captionSidecar":"srt|vtt"?,"loudnessLufs":f64?,"proresProfile":"proxy|lt|standard|hq|4444|4444xq"?,"dnxProfile":"lb|sq|hq|hqx"?,"apvProfile":"422-10|422-12|444-10|444-12"?,"mxfVideoCodec":"dnxhr|proRes|h264"?,"sequence":id?,"range":"entire|inOut|workArea|custom"?,"startSeconds":f64?,"endSeconds":f64?,"wait":bool=false}"#,
            has_seq,
            crate::export_tools::export_media
        ),
        query!("jobs.list", "List Jobs", "{}", |s, _| Ok(Value::Array(s.jobs.iter().map(|j| crate::export_tools::job_json(s, j)).collect()))),
        query!("perf.stats", "Performance Statistics", "{}", |s, _| Ok(crate::perf::stats(s))),
        cmd!("jobs.cancel", "Cancel Job", [], None, r#"{"job":id}"#, always, |s, p| {
            let id = u64_p(p, "job").ok_or_else(|| bad("jobs.cancel", "need `job`"))?;
            let j = s.jobs.iter().find(|j| j.id == id).ok_or_else(|| bad("jobs.cancel", "no such job"))?;
            j.progress.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(Value::Null)
        }),
    ]
}

fn has_saved_path(s: &Session) -> std::result::Result<(), String> {
    if s.path.is_some() { Ok(()) } else { Err("the project has not been saved yet".into()) }
}

fn has_recovery(s: &Session) -> std::result::Result<(), String> {
    if s.recovery_candidates().is_empty() { Err("there are no unsaved changes to recover".into()) } else { Ok(()) }
}

/// Import a media file through the host's random-access reader ([`crate::Services::reader`]):
/// only the container index is read now (web `Blob`s are never copied whole for containers).
pub fn import_streamed(s: &mut Session, path: &str, bin: Option<filmcraft_project::BinId>) -> Result<ItemId> {
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
    let identity = crate::relink::identity_of(&*s.services, path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let src = s.media.open_file(path, &*s.services)?;
    import_source(s, path, name, src, identity, bin)
}

/// Import the image sequence that the numbered still `path` starts (File ▸ Import with Image
/// Sequence): one movie item at Settings ▸ Media ▸ Indeterminate Media Timebase. Returns the item,
/// the frame count and the missing frame numbers (relative to the first).
pub fn import_image_sequence(s: &mut Session, path: &str, bin: Option<filmcraft_project::BinId>) -> Result<(ItemId, usize, Vec<usize>)> {
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
    let rate = crate::settings::timebase_rate(&s.prefs.media.indeterminate_timebase);
    let frames = crate::media_pool::image_sequence_frames(path, &*s.services)?;
    let missing: Vec<usize> = frames.iter().enumerate().filter(|(_, f)| f.is_none()).map(|(i, _)| i).collect();
    let src = crate::media_pool::open_image_sequence(path, &*s.services, rate, &name)?;
    let identity = crate::relink::identity_of(&*s.services, path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let id = import_source(s, path, name, src, identity, bin)?;
    Ok((id, frames.len(), missing))
}

/// Settings ▸ Media ▸ Import image sequences: a single numbered still with later frames beside it.
fn detect_image_sequence(s: &Session, path: &str) -> bool {
    filmcraft_media::sequence::Numbered::parse(path).is_some() && crate::media_pool::image_sequence_frames(path, &*s.services).is_ok_and(|f| f.len() > 1)
}

fn new_generator(s: &mut Session, g: Generator, name: &str, label: Label, p: &Value) -> Result<Value> {
    let st = s.active_sequence().map(|q| q.settings.clone()).unwrap_or_default();
    let w = u64_p(p, "width").map(|v| v as u32).unwrap_or(st.width);
    let h = u64_p(p, "height").map(|v| v as u32).unwrap_or(st.height);
    let secs = f64_p(p, "seconds").unwrap_or(5.0);
    let name = str_p(p, "name").unwrap_or(name).to_string();
    let src = GeneratorSource::new(g, w, h, st.frame_rate, Tick::from_seconds_f64(secs));
    let pool = s.media.clone();
    let par = filmcraft_project::sane_par(st.par);
    let id = s.edit(&format!("New {name}"), |pr, st| {
        let id = crate::demo::add_generator(pr, &pool, src, &name, label, None);
        // made for the sequence: its pixels have the sequence's aspect, as in Premiere
        if let Some(ItemKind::Media(m)) = pr.item_mut(id).map(|i| &mut i.kind)
            && let Some(v) = m.info.video.as_mut()
        {
            v.par = par;
        }
        st.project_selection = vec![id];
        Ok(id)
    })?;
    Ok(json!({"item": id.0}))
}

/// `project.matteColor`: change a Color Matte's color (`item`, or the one selected in the Project
/// panel). Every clip of the matte follows; undo brings the old color back.
fn set_matte_color(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "project.matteColor";
    let color = str_p(p, "color").and_then(filmcraft_color::parse_hex).ok_or_else(|| bad(CMD, "`color` must be #rrggbb"))?;
    let is_matte = |s: &Session, id: ItemId| {
        s.project.item(id).is_some_and(|it| matches!(&it.kind, ItemKind::Media(m) if matches!(m.media, MediaRef::Generator(Generator::ColorMatte { .. }))))
    };
    let item = match item_p(p, "item") {
        Some(id) => id,
        None => match s.state.project_selection.as_slice() {
            [id] => *id,
            _ => return Err(bad(CMD, "select one Color Matte (or pass `item`)")),
        },
    };
    if !is_matte(s, item) {
        return Err(bad(CMD, format!("item {} is not a Color Matte", item.0)));
    }
    s.edit("Color Matte Color", |pr, _| {
        if let Some(ItemKind::Media(m)) = pr.item_mut(item).map(|it| &mut it.kind)
            && let MediaRef::Generator(Generator::ColorMatte { color: c }) = &mut m.media
        {
            *c = color;
        }
        Ok(())
    })?;
    Ok(json!({"item": item.0, "color": filmcraft_color::to_hex(color)}))
}

fn write_project(s: &mut Session, path: &str, adopt: bool) -> Result<Value> {
    let t0 = web_time::Instant::now();
    // Save As renames the project after its new file (not an edit: no undo step, not dirty).
    if adopt
        && let Some(name) = project_name_for(path)
        && s.project.name != name
    {
        name_project(std::sync::Arc::make_mut(&mut s.project), name);
    }
    // what is open (sequence tabs, how each is shown) goes into the file beside the project
    let view = s.project_view();
    let bytes = filmcraft_format::encode_with_view(&s.project, Some(&view), false);
    let mut backup = None;
    if adopt && s.path.as_deref() == Some(path) && s.loaded_schema < filmcraft_format::SCHEMA_VERSION {
        let b = schema_backup_path(path, s.loaded_schema);
        if s.services.read_file(&b).is_err()
            && let Ok(old) = s.services.read_file(path)
        {
            s.services.write_file(&b, &old).map_err(|e| EngineError::Other(format!("{b}: {e}")))?;
            backup = Some(b);
        }
    }
    s.services.write_file(path, &bytes).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    if adopt {
        s.path = Some(path.to_string());
        s.previews_follow_path();
        s.note_recent_project();
        s.saved_revision = s.revision;
        s.loaded_schema = filmcraft_format::SCHEMA_VERSION;
    }
    Ok(json!({"path": path, "bytes": bytes.len(), "ms": t0.elapsed().as_secs_f64() * 1000.0, "backup": backup}))
}

fn open_project(s: &mut Session, path: &str) -> Result<Value> {
    let bytes = s.services.read_file(path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let loaded = filmcraft_format::decode(&bytes).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let (from, migrated) = (loaded.schema_version, loaded.migrated());
    install_project(s, loaded.project, Some(path.to_string()), true);
    if let Some(view) = loaded.view.filter(|_| s.prefs.timeline.restore_open_sequences) {
        s.restore_project_view(view);
    }
    // the frontend shows the project's active sequence (with its own view, when one was restored)
    if let Some(seq) = s.state.active_sequence {
        s.events.push(crate::Event::OpenSequence(seq));
    }
    s.note_recent_project();
    s.loaded_schema = from;
    if migrated {
        s.toast(format!("Upgraded project from schema v{from} to v{}; the original is kept as a backup when you save", filmcraft_format::SCHEMA_VERSION));
    }
    let missing = crate::relink::on_open(s);
    Ok(json!({"path": path, "schemaVersion": from, "migrated": migrated, "missingMedia": missing}))
}

fn auto_save_name(s: &Session) -> String {
    s.path.as_deref().and_then(|p| std::path::Path::new(p).file_stem()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| s.project.name.clone())
}

/// Serialize the project in the current schema and write it (atomically) to `path`. `adopt` =
/// Save / Save As (the file becomes the project's path and the project is clean); otherwise Save a
/// Copy. The first save over a file upgraded from an older schema keeps the original as
/// `<name> (schema vN backup).fcproj`.
/// A project is named after its file (as in Premiere): `/a/b/Trailer v2.fcproj` → `Trailer v2`.
fn project_name_for(path: &str) -> Option<String> {
    let stem = std::path::Path::new(path).file_stem()?.to_string_lossy().to_string();
    (!stem.is_empty()).then_some(stem)
}

fn name_project(p: &mut filmcraft_project::Project, name: String) {
    p.root.name.clone_from(&name);
    p.name = name;
}

fn schema_backup_path(path: &str, schema: u32) -> String {
    let stem = path.strip_suffix(".fcproj").unwrap_or(path);
    format!("{stem} (schema v{schema} backup).fcproj")
}

/// Make `proj` the session's project (fresh history, media pool and editor state).
fn install_project(s: &mut Session, proj: filmcraft_project::Project, path: Option<String>, clean: bool) {
    let proxies = s.media.use_proxies();
    s.media = std::sync::Arc::new(crate::MediaPool::default());
    s.media.set_use_proxies(proxies);
    s.offline = Default::default();
    let first = proj.sequences().next().map(|i| i.id);
    s.project = std::sync::Arc::new(proj);
    s.history = Default::default();
    s.history.limit = 200;
    s.state = crate::Session::default().state;
    s.state.active_sequence = first;
    s.state.open_sequences = first.into_iter().collect();
    if let Some(p) = &path
        && !cfg!(target_arch = "wasm32")
    {
        s.previews.set_dir(Some(crate::project_tools::previews_dir_for(&s.project, p)));
    }
    s.path = path;
    s.revision += 1;
    // A recovered project is unsaved: its saved revision is one that never existed.
    s.saved_revision = if clean { s.revision } else { 0 };
    s.loaded_schema = filmcraft_format::SCHEMA_VERSION;
    s.events.push(crate::Event::ProjectChanged { revision: s.revision });
}

/// Load a recovery candidate (newest when `id` is None) as the current, unsaved project.
fn recover(s: &mut Session, id: Option<&str>) -> Result<Value> {
    let per = s.persistence.as_ref().ok_or_else(|| EngineError::Other("recovery is not running".into()))?;
    let idx = match id {
        Some(id) => per.candidates.iter().position(|c| c.id == id).ok_or_else(|| bad("file.recover", format!("no recovery session `{id}`")))?,
        None => 0,
    };
    let c = per.candidates[idx].clone();
    let saved_at = per.local_time(c.meta.saved_unix);
    let loaded = crate::autosave::load_candidate(&c).map_err(EngineError::Other)?;
    install_project(s, loaded.project, c.meta.project_path.clone(), false);
    if let Some(seq) = s.state.active_sequence {
        s.events.push(crate::Event::OpenSequence(seq));
    }
    crate::relink::on_open(s);
    // Our own journal must hold the recovered state before the old one is deleted.
    s.sync_persistence();
    if let Some(per) = s.persistence.as_mut() {
        per.flush();
        per.candidates.remove(idx);
    }
    crate::autosave::discard_candidate(&c).map_err(|e| EngineError::Other(e.to_string()))?;
    s.toast(format!("Recovered unsaved changes to {} from {saved_at}", c.meta.project_name));
    Ok(json!({"id": c.id, "name": c.meta.project_name, "path": c.meta.project_path, "savedAt": saved_at, "revision": c.meta.revision}))
}

/// Where auto-saves of the current project go.
fn auto_save_dir(s: &Session) -> std::path::PathBuf {
    if let Some(d) = s.project.settings.scratch.auto_save.as_deref().filter(|d| !d.is_empty() && s.path.is_some()) {
        return std::path::PathBuf::from(d);
    }
    match (&s.path, &s.persistence) {
        (Some(p), _) => filmcraft_format::autosave::auto_save_dir(std::path::Path::new(p)),
        (None, Some(per)) => per.data_dir.join(filmcraft_format::autosave::AUTO_SAVE_DIR),
        (None, None) => std::path::PathBuf::from(filmcraft_format::autosave::AUTO_SAVE_DIR),
    }
}
