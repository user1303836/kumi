use super::*;
#[derive(Debug, Clone, Copy)]
pub struct Shortens {
    pub midi: bool,
    pub audio: bool,
}
pub fn is_part(p: &Placement) -> bool {
    p.beats < p.clip.beats - EPSILON
}
fn unique(name: &str, taken: &mut HashSet<String>) -> String {
    let mut candidate = name.to_owned();
    let mut count = 2;
    while taken.contains(&candidate) {
        candidate = format!("{name} {count}");
        count += 1;
    }
    taken.insert(candidate.clone());
    candidate
}
fn find(material: &Material, named: &str) -> Result<usize, String> {
    if let Some(index) = material.tracks.iter().position(|t| t.reference == named) {
        return Ok(index);
    }
    let wanted = trim(named).to_lowercase();
    let matches: Vec<_> = material.tracks.iter().enumerate().filter(|(_, t)| trim(&t.name).to_lowercase() == wanted).collect();
    if matches.len() == 1 {
        return Ok(matches[0].0);
    }
    let quoted = stringify(&json!(named));
    if matches.len() > 1 {
        return Err(format!(
            "Several tracks are called {quoted}: name each by its ref ({}).",
            matches.iter().map(|(_, t)| t.reference.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    let tracks = material.tracks.iter().filter(|t| !t.clips.is_empty()).take(32).map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ");
    Err(format!("There's no track called {quoted}. Tracks with clips: {}.", if tracks.is_empty() { "none" } else { &tracks }))
}
fn place(track: &SourceTrack, index: usize, clip: &SourceClip, at: f64, beats: f64, section: usize, role: Option<&str>) -> Placement {
    Placement { track: track.clone(), track_index: index, clip: clip.clone(), at, beats, section, role: role.map(str::to_owned) }
}
pub fn compile(material: &Material, request: &ArrangeRequest, shortens: Shortens) -> Result<Plan, String> {
    let bpb = material.beats_per_bar;
    let bar = |beats: f64| to_string(round((beats / bpb + 1.0) * 100.0) / 100.0);
    let with_clips: Vec<_> = material.tracks.iter().filter(|t| !t.clips.is_empty()).collect();
    let looped =
        material.loop_.as_ref().map(|l| format!("bars {}–{} of the Arrangement", to_string(l.from / bpb + 1.0), to_string(l.to / bpb)));
    if let Some(looped) = &looped {
        let audio = &material.loop_.as_ref().unwrap().audio;
        if !with_clips.iter().any(|t| t.clips.iter().any(|c| c.notes.is_some())) {
            return Err(format!(
                "There are no MIDI clips in {looped} to arrange from{}.",
                if audio.is_empty() {
                    String::new()
                } else {
                    format!(" ({} {} audio there, which Live's scripting can't copy within the Arrangement: drag those clips into Session slots, or arrange from scenes)",audio.join(", "),if audio.len()==1{"has"}else{"have"})
                }
            ));
        }
    }
    if with_clips.is_empty() {
        return Err("There are no clips in the Session to arrange: record or write the loop into Session clips first.".into());
    }
    let mut notes = vec![];
    if let Some(looped) = &looped {
        let audio = &material.loop_.as_ref().unwrap().audio;
        if !audio.is_empty() {
            notes.push(format!("{} {} audio in {looped}, which Live's scripting can't copy within the Arrangement, so {} in the arrangement: drag those clips into Session slots and name them as {{track, scene}} to bring them in.",audio.join(", "),if audio.len()==1{"has"}else{"have"},if audio.len()==1{"it isn't"}else{"they aren't"}));
        }
    }
    let default_scene = if looped.is_some() {
        -1.0
    } else {
        request.scene.unwrap_or_else(|| with_clips.iter().flat_map(|t| t.clips.iter().map(|c| c.scene)).fold(f64::INFINITY, f64::min))
    };
    let where_ = |scene: f64| {
        if scene == -1.0 {
            looped.clone().unwrap_or_else(|| "undefined".into())
        } else {
            format!("scene {}", to_string(scene + 1.0))
        }
    };
    let start = request.start_bar.map(|n| (n - 1.0) * bpb).unwrap_or_else(|| (material.end / bpb - EPSILON).ceil().max(0.0) * bpb);
    let mut placements = vec![];
    let mut sections = vec![];
    let mut taken = material.locators.iter().map(|l| l.name.clone()).collect();
    let mut from = start;
    for (index, section) in request.sections.iter().enumerate() {
        let to = from + section.bars * bpb;
        let scene = section.scene.unwrap_or(default_scene);
        let name = unique(&section.name, &mut taken);
        let mut plays: Vec<(usize, &SourceClip)> = vec![];
        let everything = section.tracks.is_none();
        if let Some(choices) = &section.tracks {
            for choice in choices {
                let track_index = find(material, &choice.track).map_err(|e| format!("{name}: {e}"))?;
                let track = &material.tracks[track_index];
                let clip = track.clips.iter().find(|c| c.scene == choice.scene.unwrap_or(scene)).or_else(|| {
                    if choice.scene.is_none() && track.clips.len() == 1 {
                        track.clips.first()
                    } else {
                        None
                    }
                });
                let Some(clip) = clip else {
                    notes.push(format!(
                        "{} has no clip in {}, so it doesn't play in {name}.",
                        track.name,
                        where_(choice.scene.unwrap_or(scene))
                    ));
                    continue;
                };
                if plays.iter().any(|(i, _)| *i == track_index) {
                    return Err(format!("{name}: {} is named twice; a track plays one clip at a time.", track.name));
                }
                plays.push((track_index, clip));
            }
        } else {
            for (track_index, track) in material.tracks.iter().enumerate() {
                if let Some(clip) = track.clips.iter().find(|c| c.scene == scene) {
                    plays.push((track_index, clip));
                }
            }
            if plays.is_empty() {
                let place = where_(scene);
                let mut chars = place.chars();
                let place = chars.next().unwrap().to_uppercase().to_string() + chars.as_str();
                return Err(format!("{place} has no clips, so {name} would be empty: name the tracks that play, or another scene."));
            }
        }
        let mut extras = vec![];
        let mut gapped = vec![];
        if let Some(gap) = &section.gap {
            if gap.beats >= section.bars * bpb {
                return Err(format!("{name}: its gap ({} beats) is as long as the section.", to_string(gap.beats)));
            }
            let which = gap.tracks.clone().unwrap_or_else(|| plays.iter().map(|(i, _)| material.tracks[*i].reference.clone()).collect());
            for which in which {
                let track = find(material, &which).map_err(|e| format!("{name}'s gap: {e}"))?;
                if !gapped.contains(&track) {
                    gapped.push(track);
                }
            }
            let who: Vec<_> =
                gapped.iter().filter(|i| plays.iter().any(|(index, _)| index == *i)).map(|i| material.tracks[*i].name.as_str()).collect();
            if !who.is_empty() {
                let duration = if gap.beats == bpb {
                    "bar".into()
                } else if gap.beats % bpb == 0.0 {
                    format!("{} bars", to_string(gap.beats / bpb))
                } else {
                    format!("{} beat{}", to_string(gap.beats), if gap.beats == 1.0 { "" } else { "s" })
                };
                extras.push(format!(
                    "{} out for the last {duration}",
                    if who.len() == plays.len() { "everything".into() } else { who.join(", ") }
                ));
            }
        }
        let mut fills = indexmap::IndexMap::new();
        for fill in &section.fill {
            let track_index = find(material, &fill.track).map_err(|e| format!("{name}'s fill: {e}"))?;
            let track = &material.tracks[track_index];
            let clip = track.clips.iter().find(|c| Some(c.scene) == fill.scene).ok_or_else(|| {
                format!("{name}'s fill: {} has no clip in scene {}.", track.name, to_string(fill.scene.unwrap_or(f64::NAN) + 1.0))
            })?;
            fills.insert(track_index, clip);
        }
        let mut tracks: Vec<_> = plays.iter().map(|(i, _)| *i).collect();
        for i in fills.keys() {
            if !tracks.contains(i) {
                tracks.push(*i);
            }
        }
        for track_index in tracks {
            let track = &material.tracks[track_index];
            let mut end = to - if gapped.contains(&track_index) { section.gap.as_ref().map(|g| g.beats).unwrap_or(0.0) } else { 0.0 };
            if let Some(fill) = fills.get(&track_index) {
                if fill.beats > end - from + EPSILON {
                    notes.push(format!(
                        "{}'s fill ({} beats) is longer than {name} leaves it, so Kumi left it out.",
                        track.name,
                        to_string(fill.beats)
                    ));
                } else {
                    end -= fill.beats;
                    placements.push(place(track, track_index, fill, end, fill.beats, index, Some("fill")));
                    extras.push(format!("{} fill at the end", track.name));
                }
            }
            let Some((_, clip)) = plays.iter().find(|(i, _)| *i == track_index) else { continue };
            let mut at = from;
            while at < end - EPSILON {
                let beats = clip.beats.min(end - at);
                if beats < clip.beats - EPSILON {
                    if beats < SHORTEST {
                        break;
                    }
                    if clip.notes.is_none() && (!clip.shortens || !(if clip.audio { shortens.audio } else { shortens.midi })) {
                        notes.push(format!("{}'s clip ({} beats) doesn't fit {name}'s end evenly and Live can't shorten {}, so its last {} beats are left empty.",track.name,to_string(clip.beats),if clip.audio{"this audio clip (it isn't warped)"}else{"its loop here"},to_string(beats)));
                        break;
                    }
                }
                placements.push(place(track, track_index, clip, at, beats, index, None));
                at += clip.beats;
            }
        }
        if let Some(riser) = &section.riser {
            let track_index = find(material, &riser.track).map_err(|e| format!("{name}'s riser: {e}"))?;
            let track = &material.tracks[track_index];
            let clip = track.clips.iter().find(|c| Some(c.scene) == riser.scene).ok_or_else(|| {
                format!("{name}'s riser: {} has no clip in scene {}.", track.name, to_string(riser.scene.unwrap_or(f64::NAN) + 1.0))
            })?;
            if to - clip.beats < start - EPSILON {
                notes.push(format!(
                    "{}'s riser ({} beats) would start before the arrangement does, so Kumi left it out of {name}.",
                    track.name,
                    to_string(clip.beats)
                ));
            } else {
                placements.push(place(track, track_index, clip, to - clip.beats, clip.beats, index, Some("riser")));
                extras.push(format!("{} rises into what follows", track.name));
            }
        }
        sections.push(PlannedSection {
            name,
            from,
            to,
            tracks: plays.iter().map(|(i, _)| material.tracks[*i].name.clone()).collect(),
            everything,
            extras,
        });
        from = to;
    }
    let end = from;
    for (track_index, track) in material.tracks.iter().enumerate() {
        let mut mine: Vec<&Placement> = placements.iter().filter(|p| p.track_index == track_index).collect();
        mine.sort_by(|a, b| a.at.total_cmp(&b.at));
        for (index, placement) in mine.iter().enumerate() {
            if let Some(before) = index.checked_sub(1).map(|i| mine[i]) {
                if placement.at < before.at + before.beats - EPSILON {
                    let what = |p: &Placement| p.role.as_ref().map(|s| format!("its {s}")).unwrap_or_else(|| "its loop".into());
                    return Err(format!("{} would play two clips at once at bar {} ({} and {}): give a riser a track of its own, and fills to tracks that play.",track.name,bar(placement.at),what(before),what(placement)));
                }
            }
            if let Some(clash) =
                track.busy.iter().find(|pair| placement.at < pair[1] - EPSILON && pair[0] < placement.at + placement.beats - EPSILON)
            {
                return Err(format!("{} already has a clip in the Arrangement at bar {}, where this arrangement would go: start it after what's there (start_bar {}), or clear that stretch first.",track.name,bar(clash[0]),to_string((material.end/bpb-EPSILON).ceil()+1.0)));
            }
        }
    }
    let marked = |position: f64| material.locators.iter().any(|l| (l.position - position).abs() < EPSILON);
    let mut locators: Vec<_> =
        sections.iter().filter(|s| !marked(s.from)).map(|s| Locator { name: s.name.clone(), position: s.from }).collect();
    if locators.len() < sections.len() {
        notes.push(format!(
            "Locators already mark {}, so Kumi left those as they are.",
            if sections.len() - locators.len() == 1 { "one of the sections' starts" } else { "some of the sections' starts" }
        ));
    }
    if locators.len() % 2 == 1 {
        if !marked(end) {
            locators.push(Locator { name: unique("End", &mut taken), position: end });
        } else {
            locators.pop();
        }
    }
    placements.sort_by(|a, b| a.at.total_cmp(&b.at).then(a.track_index.cmp(&b.track_index)));
    Ok(Plan {
        start,
        end,
        beats_per_bar: bpb,
        tempo: material.tempo.filter(|n| *n != 0.0 && !n.is_nan()),
        sections,
        placements,
        locators,
        notes,
    })
}
pub(super) fn bars_of(plan: &Plan, from: f64, to: f64) -> String {
    let first = round(from / plan.beats_per_bar) + 1.0;
    let last = round(to / plan.beats_per_bar);
    if first == last {
        format!("bar {}", to_string(first))
    } else {
        format!("bars {}–{}", to_string(first), to_string(last))
    }
}
pub fn section_lines(plan: &Plan) -> Vec<String> {
    plan.sections
        .iter()
        .map(|s| {
            let plays = if s.tracks.is_empty() {
                "silence".into()
            } else if s.everything && s.tracks.len() > 2 {
                "everything".into()
            } else if s.tracks.len() > 6 {
                format!("{} and {} more", s.tracks[..5].join(", "), s.tracks.len() - 5)
            } else {
                s.tracks.join(", ")
            };
            format!(
                "{}, {}: {plays}{}",
                s.name,
                bars_of(plan, s.from, s.to),
                if s.extras.is_empty() { String::new() } else { format!("; {}", s.extras.join("; ")) }
            )
        })
        .collect()
}
pub fn describe(material: &Material) -> JsonObject {
    let bpb = material.beats_per_bar;
    let length = |beats: f64| {
        if (beats / bpb - round(beats / bpb)).abs() < EPSILON {
            format!("{} bar{}", to_string(round(beats / bpb)), if round(beats / bpb) == 1.0 { "" } else { "s" })
        } else {
            format!("{} beats", to_string(round(beats * 100.0) / 100.0))
        }
    };
    let used: Vec<_> =
        material.scenes.iter().filter(|scene| material.tracks.iter().any(|t| t.clips.iter().any(|c| c.scene == scene.index))).collect();
    let scenes:Vec<Value>=used.iter().map(|scene|{let mut row=json!({"scene":scene.index,"clips":material.tracks.iter().flat_map(|track|track.clips.iter().filter(|c|c.scene==scene.index).map(|c|format!("{}: {}{}{}",track.name,if c.name.is_empty(){String::new()}else{format!("“{}”, ",c.name)},length(c.beats),if c.audio{", audio"}else{""}))).collect::<Vec<_>>()});if !scene.name.is_empty(){row["name"]=json!(scene.name);}ordered(row, &["scene", "name", "clips"])}).collect();
    let mut shown = scenes.len().min(64);
    while shown > 1 && utf16_len(&stringify(&json!(&scenes[..shown]))) > LISTING_BYTES {
        shown -= 1;
    }
    let mut value = json!({"beatsPerBar":bpb,"scenes":&scenes[..shown]});
    if let Some(tempo) = material.tempo.filter(|n| *n != 0.0 && !n.is_nan()) {
        value["tempo"] = json!(tempo);
    }
    if used.len() > shown {
        value["moreScenes"] = json!(used.len() - shown);
    }
    if material.unread > 0 {
        value["clipsNotRead"] = json!(material.unread);
    }
    if let Some(span) = &material.loop_ {
        let mut loop_ = json!({"bars":format!("{}–{}",to_string(span.from/bpb+1.0),to_string(span.to/bpb)),"tracks":material.tracks.iter().flat_map(|t|t.clips.iter().filter_map(|c|c.notes.as_ref().map(|notes|format!("{}: {} notes",t.name,notes.len())))).collect::<Vec<_>>()});
        if !span.audio.is_empty() {
            loop_["audioLeftOut"] =
                json!(format!("{}: audio, which Live's scripting can't copy within the Arrangement", span.audio.join(", ")));
        }
        value["loop"] = loop_;
    }
    let mut arrangement = if material.end > 0.0 {
        json!({"clipsEndAt":format!("bar {}",to_string(round(material.end/bpb*100.0)/100.0+1.0))})
    } else {
        json!({"empty":true})
    };
    if material.end > 0.0 && !material.locators.is_empty() {
        arrangement["locators"] = json!(material
            .locators
            .iter()
            .take(32)
            .map(|l| format!("{} at bar {}", l.name, to_string(round(l.position / bpb * 100.0) / 100.0 + 1.0)))
            .collect::<Vec<_>>());
    }
    value["arrangement"] = arrangement;
    if material.playing {
        value["playing"] = json!(true);
    }
    let value = ordered(value, &["tempo", "beatsPerBar", "scenes", "moreScenes", "clipsNotRead", "loop", "arrangement", "playing"]);
    json!({"material":value,"next":"Call arrange again with the sections: each one's name, bars and the scene or tracks that play, with gaps, fills and risers where they help; it starts after the Arrangement's clips unless start_bar says where."}).as_object().unwrap().clone()
}
