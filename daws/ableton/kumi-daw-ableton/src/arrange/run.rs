use super::compile::{bars_of, Shortens};
use super::*;
const WRITTEN_AT_ONCE: usize = 32;
fn message(error: RuntimeError) -> String {
    head(&error.to_string(), 400)
}
async fn take_back(scratch: &mut Vec<(String, String)>, built: &mut Built, host: &dyn ArrangeHost) {
    for (id, what) in std::mem::take(scratch).into_iter().rev() {
        if !host.undo(&id, abort::timeout(30_000)).await.unwrap_or(false) {
            built.notes.push(what);
        }
    }
}
fn progress(placement: &Placement, current: &mut Option<usize>, plan: &Plan, host: &dyn ArrangeHost) {
    if *current == Some(placement.section) {
        return;
    }
    *current = Some(placement.section);
    let section = &plan.sections[placement.section];
    host.tell(&format!("Arranging · {}, bar {}", section.name, to_string(round(section.from / plan.beats_per_bar) + 1.0)));
}
pub async fn build(plan: &Plan, material: &Material, host: &dyn ArrangeHost, signal: Signal) -> Result<Built, RuntimeError> {
    let mut built = Built::default();
    let step = host.undo_step().await?;
    built.opened = step.opened;
    let mut scratch: Vec<(String, String)> = vec![];
    let mut scene: Option<(String, usize)> = None;
    let mut current = None;
    let result = async {
        for placement in plan.placements.iter().filter(|p| !is_part(p) && p.clip.notes.is_none()) {
            signal.check()?;
            progress(placement, &mut current, plan, host);
            host.change(
                "duplicate_clip",
                json!({"clipRef":placement.clip.reference,"arrangementPosition":placement.at}).as_object().unwrap().clone(),
                signal.clone(),
            )
            .await?;
            built.copies += 1;
        }
        let mut groups = indexmap::IndexMap::<String, Vec<&Placement>>::new();
        for placement in plan.placements.iter().filter(|p| is_part(p) && p.clip.notes.is_none()) {
            groups.entry(format!("{}|{}", placement.clip.reference, to_string(placement.beats))).or_default().push(placement);
        }
        if !groups.is_empty() {
            host.tell("Arranging · the shorter parts");
        }
        for group in groups.values() {
            signal.check()?;
            let (track, clip, beats) = (&group[0].track, &group[0].clip, group[0].beats);
            let slot = if let Some(slot) = track.empty.iter().find(|index| scene.as_ref().is_none_or(|(_, i)| **index != *i as f64)) {
                *slot
            } else {
                if scene.is_none() {
                    let made = host
                        .change(
                            "add_tracks_and_scenes",
                            json!({"tracks":[],"scenes":[{"name":"Kumi parts"}]}).as_object().unwrap().clone(),
                            signal.clone(),
                        )
                        .await?;
                    scene = Some((made.id, material.scenes.len()));
                }
                scene.as_ref().unwrap().1 as f64
            };
            let copy = host
                .change(
                    "duplicate_clip",
                    json!({"clipRef":clip.reference,"targetTrackRef":track.reference,"targetSceneIndex":slot}).as_object().unwrap().clone(),
                    signal.clone(),
                )
                .await?;
            let what = if clip.name.is_empty() { format!("{}'s clip", track.name) } else { format!("“{}”", clip.name) };
            scratch.push((
                copy.id,
                format!("A copy of {what} Kumi shortened is still in {}, scene {}: delete it in Live.", track.name, to_string(slot + 1.0)),
            ));
            let reference = copy.reference.ok_or_else(|| RuntimeError::plain("Live didn't say where the copy went"))?;
            let shortened = host
                .change(
                    if clip.audio { "set_audio_clip" } else { "set_clip" },
                    json!({"clipRef":reference,"loopEnd":clip.loop_start+beats}).as_object().unwrap().clone(),
                    signal.clone(),
                )
                .await?;
            scratch.push((
                shortened.id,
                format!("A copy of {what} is still shortened in {}, scene {}: delete it in Live.", track.name, to_string(slot + 1.0)),
            ));
            for placement in group {
                signal.check()?;
                progress(placement, &mut current, plan, host);
                host.change(
                    "duplicate_clip",
                    json!({"clipRef":reference,"arrangementPosition":placement.at}).as_object().unwrap().clone(),
                    signal.clone(),
                )
                .await?;
                built.parts += 1;
            }
            take_back(&mut scratch, &mut built, host).await;
        }
        if let Some((id, _)) = &scene {
            if host.undo(id, abort::timeout(30_000)).await.unwrap_or(false) {
                scene = None;
            }
        }
        let written: Vec<_> = plan.placements.iter().filter(|p| p.clip.notes.is_some()).collect();
        if !written.is_empty() {
            host.tell("Arranging · writing the loop's clips");
        }
        for group in written.chunks(WRITTEN_AT_ONCE) {
            signal.check()?;
            let clips: Vec<_> = group
                .iter()
                .map(|item| {
                    let notes: Vec<_> = item
                        .clip
                        .notes
                        .as_ref()
                        .unwrap()
                        .iter()
                        .filter(|n| n.start < item.beats - EPSILON)
                        .map(|n| {
                            let mut note = n.clone();
                            note.duration = note.duration.min(item.beats - note.start);
                            note
                        })
                        .collect();
                    json!({"trackRef":item.track.reference,"start":item.at,"length":item.beats,"name":item.clip.name,"notes":notes})
                })
                .collect();
            host.change("write_arrangement_clip", json!({"clips":clips}).as_object().unwrap().clone(), signal.clone()).await?;
            for item in group {
                if is_part(item) {
                    built.parts += 1;
                } else {
                    built.copies += 1;
                }
            }
        }
        if !plan.locators.is_empty() && material.playing {
            built.notes.push("Live was playing, so the sections aren't marked with locators: stop, and ask Kumi to mark them.".into());
        } else if !plan.locators.is_empty() && !host.offers("set_locators") {
            built.notes.push("Live doesn't offer adding locators for this Set, so the sections aren't marked.".into());
        } else if !plan.locators.is_empty() {
            host.tell("Arranging · naming the sections");
            for pair in plan.locators.chunks_exact(2) {
                let (first, second) = (&pair[0], &pair[1]);
                match host
                    .change(
                        "set_locators",
                        json!({"start":first.position,"end":second.position,"startName":first.name,"endName":second.name})
                            .as_object()
                            .unwrap()
                            .clone(),
                        signal.clone(),
                    )
                    .await
                {
                    Ok(_) => built.locators += 2,
                    Err(e) => {
                        signal.check()?;
                        built.notes.push(format!("Live didn't add the locators {} and {}: {}", first.name, second.name, message(e)));
                    }
                }
            }
        }
        if !material.playing && host.offers("set_transport") {
            match host.change("set_transport", json!({"position":plan.start}).as_object().unwrap().clone(), signal.clone()).await {
                Ok(made) => built.playhead = Some(made.id),
                Err(_) => signal.check()?,
            }
        }
        Ok::<_, RuntimeError>(())
    }
    .await;
    if let Err(error) = result {
        built.stopped = Some(if signal.is_cancelled() { "Stopped before it finished.".into() } else { message(error) });
        built.stopped_in = current.and_then(|i| plan.sections.get(i)).map(|s| s.name.clone());
    }
    take_back(&mut scratch, &mut built, host).await;
    if let Some((id, index)) = scene {
        if !host.undo(&id, abort::timeout(30_000)).await.unwrap_or(false) {
            built.notes.push(format!(
                "The scene Kumi added for its working copies (scene {}, “Kumi parts”) is still there: delete it in Live.",
                index + 1
            ));
        }
    }
    (step.close)().await?;
    Ok(built)
}
fn clock(seconds: f64) -> String {
    // Whole seconds first: 59.6 s is 1:00, not 0:60.
    let total = round(seconds);
    format!("{}:{:0>2}", to_string((total / 60.0).floor()), to_string(total % 60.0))
}
pub async fn arrange(input: JsonObject, host: &dyn ArrangeHost, signal: Signal) -> Result<ToolResult, RuntimeError> {
    let request = match arrange_request(&input) {
        Ok(r) => r,
        Err(e) => return Ok(ToolResult::error(e)),
    };
    let material = match read_material(host, signal.clone(), request.loop_.as_ref(), Some(&request)).await {
        Ok(m) => m,
        Err(e) => {
            signal.check()?;
            return Ok(ToolResult::error(format!("Kumi couldn't read the Set's clips: {}", message(e))));
        }
    };
    if request.sections.is_empty() {
        return Ok(ToolResult::text(stringify(&Value::Object(describe(&material)))));
    }
    let plan = match compile(&material, &request, Shortens { midi: host.offers("set_clip"), audio: host.offers("set_audio_clip") }) {
        Ok(p) => p,
        Err(e) => return Ok(ToolResult::error(e)),
    };
    if plan.placements.is_empty() {
        return Ok(ToolResult::error("Those sections place no clips: name tracks with clips in the scenes they play."));
    }
    if plan.placements.iter().any(|p| p.clip.notes.is_none()) && !host.offers("duplicate_clip") {
        return Ok(ToolResult::error("Live doesn't offer copying clips into the Arrangement for this Set right now."));
    }
    if plan.placements.iter().any(|p| p.clip.notes.is_some()) && !host.offers("write_arrangement_clip") {
        return Ok(ToolResult::error("A loop in the Arrangement is copied by writing its clips with their notes, through Kumi's Live extension (Live 12.4 or later), which isn't running for this Set: arrange from Session scenes instead (drag the loop's clips into slots)."));
    }
    let began = kumi_common::time::now_ms();
    host.tell(&format!("Arranging {} sections, {}", plan.sections.len(), bars_of(&plan, plan.start, plan.end)));
    let copy = host.keep_copy(signal.clone()).await.ok().flatten();
    let QuietBuilt { value: built, ids } = host.quietly(build(&plan, &material, host, signal).boxed_local()).await?;
    let bars = round((plan.end - plan.start) / plan.beats_per_bar);
    let first = round(plan.start / plan.beats_per_bar) + 1.0;
    let title = if built.stopped.is_some() {
        format!(
            "Arrangement, stopped{} · {} clips from bar {}",
            built.stopped_in.as_ref().map(|s| format!(" in {s}")).unwrap_or_default(),
            built.copies + built.parts,
            to_string(first)
        )
    } else {
        format!("Arrangement · {} sections, {}", plan.sections.len(), bars_of(&plan, plan.start, plan.end))
    };
    let change = host.record(&title, &ids, &built.playhead.iter().cloned().collect::<Vec<_>>());
    let lines = section_lines(&plan);
    let length = plan
        .tempo
        .filter(|n| *n != 0.0 && !n.is_nan())
        .map(|tempo| format!("{} at {} BPM", clock((plan.end - plan.start) * 60.0 / tempo), to_string(round(tempo * 100.0) / 100.0)));
    let mut notes = plan.notes.clone();
    notes.extend(built.notes);
    if material.session_playing {
        notes.push("Some tracks are playing Session clips, which keeps them from playing the Arrangement: press Back to Arrangement in Live to hear it.".into());
    }
    let mut arranged = json!({"from":format!("bar {}",to_string(first)),"bars":bars,"sections":lines});
    if let Some(length) = &length {
        arranged["length"] = json!(length);
    }
    let arranged = ordered(arranged, &["from", "bars", "length", "sections"]);
    let mut made = json!({"clips":built.copies+built.parts,"locators":built.locators});
    if built.playhead.is_some() {
        made["playhead"] = json!(format!("bar {}", to_string(first)));
    }
    let mut result = json!({"arranged":arranged,"made":made,"seconds":round((kumi_common::time::now_ms()-began)as f64/100.0)/10.0});
    if let Some(change) = change.filter(|s| !s.is_empty()) {
        result["change"] = json!(change);
    }
    if !notes.is_empty() {
        result["notes"] = json!(notes);
    }
    let copy = copy.filter(|s| !s.is_empty());
    if let Some(copy) = &copy {
        result["copy"] = json!(copy);
        result["copyNote"] = json!(
            "Before this, Kumi kept a copy of the Set as last saved, next to it. Tell the producer in a few words, with the file's name."
        );
    }
    let mut result = ordered(result, &["arranged", "made", "change", "seconds", "notes", "copy", "copyNote"]);
    if let Some(stopped) = built.stopped {
        result["stopped"] = json!(stopped);
        result["note"] = json!("What was made stays, as one change in HISTORY whose undo takes it back. Say where it stopped and why.");
        return Ok(ToolResult::error(stringify(&result)));
    }
    if !request.r#final {
        return Ok(ToolResult::text(stringify(&result)));
    }
    let undo = format!("Undo in HISTORY takes it all back{}.", if built.opened { " (or one Cmd-Z in Live)" } else { "" });
    let marked = if built.locators > 0 {
        format!(
            "Locators mark the sections{}. ",
            if built.playhead.is_some() { format!(", and the playhead is at bar {}", to_string(first)) } else { String::new() }
        )
    } else if built.playhead.is_some() {
        format!("The playhead is at bar {}. ", to_string(first))
    } else {
        String::new()
    };
    let mut reply = vec![format!(
        "Arranged {} bars from bar {}{}:",
        to_string(bars),
        to_string(first),
        length.map(|s| format!(" ({s})")).unwrap_or_default()
    )];
    reply.extend(lines.iter().map(|s| format!("- {s}")));
    reply.push(String::new());
    reply.push(format!("{marked}{undo}"));
    reply.extend(notes);
    if let Some(copy) = copy {
        reply.push(format!("First, Kumi kept a copy of your Set as last saved, next to it: {}", copy.rsplit(['\\', '/']).next().unwrap()));
    }
    let mut result = ToolResult::text(stringify(&result));
    result.reply = Some(reply.join("\n"));
    Ok(result)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_length_rounds_to_whole_seconds_before_it_reads_as_minutes() {
        assert_eq!(super::clock(59.6), "1:00");
        assert_eq!(super::clock(119.4), "1:59");
        assert_eq!(super::clock(0.4), "0:00");
        assert_eq!(super::clock(125.0), "2:05");
    }
}
