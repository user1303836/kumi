//! Extension-backed Arrangement MIDI creation and explicit range clearing.
use super::*;
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
const KEPT: &str = "Kumi can't bring this back; Live's undo can.";
/// How long an apply waits for Live to show the clips Kumi's extension made, and how often it looks.
const LANDING_WAIT: std::time::Duration = std::time::Duration::from_millis(500);
const LANDING_LOOK: std::time::Duration = std::time::Duration::from_millis(25);
fn js_number(n: f64) -> String {
    kumi_common::js::number::to_string(n)
}
fn finite_at_least(v: &Value, min: f64) -> bool {
    v.as_f64().is_some_and(|n| n.is_finite() && n >= min)
}
fn number(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}
fn refs(clips: &[Value]) -> Vec<Value> {
    let mut refs = vec![];
    for clip in clips {
        if !refs.contains(&clip["trackRef"]) {
            refs.push(clip["trackRef"].clone());
        }
    }
    refs
}
fn clips_of(s: &LiveSnapshot, reference: &Value) -> Vec<Value> {
    let s = serde_json::to_value(s).unwrap();
    s["arrangement"]["clips"].as_array().into_iter().flatten().filter(|c| c.is_object() && c["trackRef"] == *reference).cloned().collect()
}
fn midi_args(p: &Value) -> Result<Vec<Value>, String> {
    if !p.is_object() {
        return Err("the clip is required".into());
    }
    let fields = ["trackRef", "start", "length", "notes", "name", "looping"];
    let rows = if p.get("clips").is_some() {
        if !has_only(p, &["clips"]) || !p["clips"].as_array().is_some_and(|a| !a.is_empty()) {
            return Err("give one clip's trackRef, start, length and notes, or clips: a list of them".into());
        }
        p["clips"].as_array().unwrap().clone()
    } else if has_only(p, &fields) {
        vec![p.clone()]
    } else {
        return Err("give one clip's trackRef, start, length and notes, or clips: a list of them".into());
    };
    let mut clips = vec![];
    for (i, row) in rows.iter().enumerate() {
        let at = if rows.len() > 1 { format!("clip {}: ", i + 1) } else { String::new() };
        let failure = if !has_only(row, &fields) || !is_non_empty_string(&row["trackRef"], 256) {
            Some("trackRef is required")
        } else if !finite_at_least(&row["start"], 0.) || !finite_at_least(&row["length"], 0.001) {
            Some("start (0 or later) and length (above 0) are beats")
        } else if row.get("name").is_some_and(|v| !is_non_empty_string(v, 256)) {
            Some("name is 1 to 256 characters")
        } else if row.get("looping").is_some_and(|v| !v.is_boolean()) {
            Some("looping is true or false")
        } else if !row["notes"].as_array().is_some_and(|a| {
            a.iter().all(|n| {
                has_only(n, &["pitch", "start", "duration", "velocity", "mute", "probability", "velocityDeviation", "releaseVelocity"])
                    && is_integer_in_range(&n["pitch"], 0., 127.)
                    && finite_at_least(&n["start"], 0.)
                    && number(&n["start"]) < number(&row["length"])
                    && finite_at_least(&n["duration"], 0.001)
                    && n.get("velocity").is_none_or(|v| finite_at_least(v, 1.) && number(v) <= 127.)
                    && n.get("mute").is_none_or(Value::is_boolean)
                    && n.get("probability").is_none_or(|v| finite_at_least(v, 0.) && number(v) <= 1.)
                    && n.get("velocityDeviation").is_none_or(|v| number(v).abs() <= 127.)
                    && n.get("releaseVelocity").is_none_or(|v| finite_at_least(v, 0.) && number(v) <= 127.)
            })
        }) {
            Some("notes are a list of {pitch 0-127, start (inside the clip), duration, velocity?, mute?, probability?, velocityDeviation?, releaseVelocity?}")
        } else {
            None
        };
        if let Some(message) = failure {
            return Err(format!("{at}{message}"));
        }
        let mut clip = json!({"trackRef":row["trackRef"],"start":row["start"],"length":row["length"],"notes":row["notes"]});
        for field in ["name", "looping"] {
            if let Some(v) = row.get(field) {
                clip[field] = v.clone();
            }
        }
        clips.push(clip);
    }
    Ok(clips)
}
impl McpHost {
    pub async fn dispatch_arrangement_midi_tool(
        &self,
        call: &ToolCall,
        signal: Option<&Signal>,
    ) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_arrangement_midi_clip_preview" => Some(self.live_arrangement_midi_clip_preview_async(&call.id, p).await),
            "live_arrangement_midi_clip_apply" => self.live_arrangement_midi_clip_apply_async(&call.id, p, signal).await,
            "live_clip_clear_range_preview" => Some(self.live_clip_clear_range_preview_async(&call.id, p).await),
            "live_clip_clear_range_apply" => self.live_clip_clear_range_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    fn midi_fence(&self, s: &LiveSnapshot, refs: &[Value]) -> Result<String, LiveError> {
        let tracks = s.tracks.as_deref().ok_or_else(|| LiveError::type_error("Cannot read properties of undefined (reading 'find')"))?;
        Ok(js_json::stringify(&json!({"tracks":refs.iter().map(|r|{
            let t=tracks.iter().find(|t|Some(t.ref_.as_str())==r.as_str());
            json!([r,t.and_then(|t|t.object_identity.as_ref()),t.map(|t|&t.name)])
        }).collect::<Vec<_>>(),"clips":self.arrangement_fence(s,refs)?})))
    }
    pub async fn live_arrangement_midi_clip_preview_async(&self, id: &Value, p: &Value) -> Value {
        let clips = match midi_args(p) {
            Ok(v) => v,
            Err(e) => return error(id, -32602, &e, None),
        };
        let result=async{
            let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.has_operation("arrangement.midi-clip.create"){return Err(LiveError::error("Arrangement MIDI clips with notes need Kumi's Live extension, which isn't connected"));}
            if clips.len()>1&&!status.has_operation("transaction.group"){return Err(LiveError::error("several clips at once need Kumi's Live extension to group them"));}
            let refs=refs(&clips);let context=LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS));
            let snapshot=self.views.view_for(Some(&context),&refs,None,&[]).await?;
            let tracks=refs.iter().map(|r|Ok((r.clone(),self.extension_track(&snapshot,r,TrackMedia::Midi,"make clips on")?))).collect::<Result<Vec<_>,LiveError>>()?;
            for (i,clip) in clips.iter().enumerate(){
                let end=number(&clip["start"])+number(&clip["length"]);let name=&tracks.iter().find(|(r,_)|*r==clip["trackRef"]).unwrap().1.name;
                if let Some(other)=clips_of(&snapshot,&clip["trackRef"]).iter().find(|o|number(&o["start"])<end&&arrangement_clip_end(o)>number(&clip["start"])){return Err(LiveError::error(format!("track \"{name}\" already has \"{}\" in the Arrangement between beat {} and {}; clear the range first (live_clip_clear_range) or choose another place",js_string(other.get("name").filter(|v|!v.is_null()).unwrap_or(&json!("a clip")))?,js_string(&clip["start"])?,kumi_common::js::number::to_string(end))));}
                if clips.iter().enumerate().any(|(j,o)|j!=i&&o["trackRef"]==clip["trackRef"]&&number(&o["start"])<end&&number(&o["start"])+number(&o["length"])>number(&clip["start"])){return Err(LiveError::error(format!("two of the new clips overlap on \"{name}\"")));}
            }
            let payload=json!({"clips":clips.iter().map(|c|{let mut c=c.clone();c["expectedName"]=json!(tracks.iter().find(|(r,_)|*r==c["trackRef"]).unwrap().1.name);c}).collect::<Vec<_>>()});
            let prior=json!({"clipIdentities":refs.iter().flat_map(|r|clips_of(&snapshot,r).into_iter().map(|c|c["objectIdentity"].clone())).collect::<Vec<_>>()});
            let t=json!({"id":tempo::transaction_id("arrmidi"),"epoch":status.epoch,"kind":"arrangement-midi-create","fence":self.midi_fence(&snapshot,&refs)?,"payload":payload,"prior":prior,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions,t.clone(),"Arrangement MIDI clip")?;
            Ok(success_text(id,&json!({"transactionId":t["id"],"epoch":t["epoch"],"clips":clips.iter().map(|c|json!({"trackRef":c["trackRef"],"trackName":tracks.iter().find(|(r,_)|*r==c["trackRef"]).unwrap().1.name,"start":c["start"],"length":c["length"],"name":c["name"],"notes":c["notes"].as_array().unwrap().len()})).collect::<Vec<_>>(),"impact":"creates-arrangement-midi-clips","confirmation":"apply","expiresAt":t["expiresAt"]})))
        }.await;
        result.unwrap_or_else(|e| {
            adapter_tool_error(id, &e, "No clip was made; discover the tracks again and preview from fresh references.")
        })
    }
    async fn created_arrangement_midi_clips(&self, t: &Value, context: &LiveOperationContext) -> Result<Vec<Option<Value>>, LiveError> {
        let mut found = vec![];
        let before = t["prior"]["clipIdentities"].as_array().cloned().unwrap_or_default();
        for c in t["payload"]["clips"].as_array().unwrap() {
            let rows = self
                .views
                .discover_all(
                    &serde_json::from_value(json!({"kind":"arrangement-clip","parent":c["trackRef"]})).unwrap(),
                    Some(context),
                    None,
                )
                .await?;
            found.push(rows.into_iter().map(Value::Object).find(|r| {
                !before.contains(&r["objectIdentity"])
                    && is_non_empty_string(&r["objectIdentity"], 256)
                    && (number(&r["start"]) - number(&c["start"])).abs() < 1e-6
                    && (number(&r["length"]) - number(&c["length"])).abs() < 1e-6
                    && c.get("name").is_none_or(|n| r.get("name") == Some(n))
            }));
        }
        Ok(found)
    }
    /// The clips the transaction made, each with its notes, once Live shows them whole. Live shows what
    /// Kumi's extension made a moment after the extension answers (about 0.1 s on a recent Mac), so this
    /// looks again every 25 ms, for up to half a second, until every clip is there with all its notes.
    /// A cancelled apply stops waiting and goes on with what the last look found.
    async fn landed_arrangement_midi_clips(
        &self,
        t: &Value,
        context: &LiveOperationContext,
    ) -> Result<(Vec<Option<Value>>, Vec<Option<Vec<Value>>>), LiveError> {
        let clips = t["payload"]["clips"].as_array().unwrap();
        let began = tokio::time::Instant::now();
        loop {
            let made = self.created_arrangement_midi_clips(t, context).await?;
            let mut notes = vec![];
            for row in &made {
                notes.push(match row {
                    Some(row) => Some(self.clip_notes_async(row["ref"].as_str().unwrap(), Some(context)).await?),
                    None => None,
                });
            }
            let landed = notes
                .iter()
                .zip(clips)
                .all(|(notes, clip)| notes.as_ref().is_some_and(|notes| notes.len() == clip["notes"].as_array().map_or(0, Vec::len)));
            let signal = context.signal.as_ref();
            if landed || began.elapsed() >= LANDING_WAIT || signal.is_some_and(Signal::is_cancelled) {
                return Ok((made, notes));
            }
            match signal {
                Some(signal) => tokio::select! {
                    _ = tokio::time::sleep(LANDING_LOOK) => {}
                    _ = signal.cancelled() => {}
                },
                None => tokio::time::sleep(LANDING_LOOK).await,
            }
        }
    }
    /// What Live shows where the transaction asked for its clips, when it can't find them: each new clip
    /// over the asked place, with its name, start, length and notes.
    async fn arrangement_midi_seen(&self, t: &Value, context: &LiveOperationContext) -> Result<String, LiveError> {
        let before = t["prior"]["clipIdentities"].as_array().cloned().unwrap_or_default();
        let mut said = vec![];
        for c in t["payload"]["clips"].as_array().unwrap() {
            let (start, end) = (number(&c["start"]), number(&c["start"]) + number(&c["length"]));
            let rows = self
                .views
                .discover_all(
                    &serde_json::from_value(json!({"kind":"arrangement-clip","parent":c["trackRef"]})).unwrap(),
                    Some(context),
                    None,
                )
                .await?;
            let new: Vec<_> = rows
                .into_iter()
                .map(Value::Object)
                .filter(|r| {
                    !before.contains(&r["objectIdentity"])
                        && number(&r["start"]) < end
                        && number(&r["start"]) + number(&r["length"]) > start
                })
                .collect();
            if new.is_empty() {
                said.push(format!("nothing new at beat {}", js_number(start)));
            }
            for row in new {
                let notes = match row["ref"].as_str() {
                    Some(reference) => js_number(self.clip_notes_async(reference, Some(context)).await?.len() as f64),
                    None => "?".into(),
                };
                let name = row["name"].as_str().filter(|n| !n.is_empty()).map_or("an unnamed clip".into(), |n| format!("“{n}”"));
                said.push(format!(
                    "{name} at beat {}, {} beats long, with {notes} of {} notes",
                    js_number(number(&row["start"])),
                    js_number(number(&row["length"])),
                    c["notes"].as_array().map_or(0, Vec::len)
                ));
            }
        }
        Ok(said.join("; "))
    }
    pub async fn live_arrangement_midi_clip_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()).filter(|r| {
            let t = r.borrow();
            t["kind"] == "arrangement-midi-create" && !(t["state"] == "previewed" && number(&t["expiresAt"]) <= now_ms_f64())
        }) else {
            return Some(transaction_error(id, "Unknown or expired Arrangement MIDI clip transaction"));
        };
        let t = record.borrow().clone();
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            return Some(success_text(
                id,
                &json!({"transactionId":t["id"],"state":"applied","clips":t["created"].get("clips").unwrap_or(&json!([])),"idempotent":true}),
            ));
        }
        let reconciliation = t["state"] == "uncertain" && t["applyKey"] == p["idempotencyKey"];
        if t["state"] != "previewed" && !reconciliation {
            return Some(transaction_error(id, "Transaction is no longer applicable"));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        let result=async{
            let status=if reconciliation{self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?}else{self.require_connected(None)?};
            if json!(status.epoch)!=t["epoch"]{return Ok(transaction_error(id,"Live connection epoch changed; preview again"));}
            let context=self.transaction_context(p,signal,reads::AUDITION_DEADLINE_MS);let clips=t["payload"]["clips"].as_array().unwrap();let refs=refs(clips);
            let mut made=if reconciliation{self.created_arrangement_midi_clips(&t,&context).await?}else{vec![]};
            // Each made clip's notes as they were read once they had landed, for its fence.
            let mut landed:Vec<Option<Vec<Value>>>=vec![];
            let mut partial=if reconciliation&&made.iter().any(Option::is_some)&&!made.iter().all(Option::is_some){Some("only some of the clips are there".to_owned())}else{None};
            if !made.iter().any(Option::is_some){
                let snapshot=self.views.view_for(Some(&context),&refs,None,&[]).await?;
                if self.midi_fence(&snapshot,&refs)?!=t["fence"]{return Ok(transaction_error(id,"the tracks or their Arrangement clips changed since the preview; preview again"));}
                record.borrow_mut()["state"]=json!("applying");record.borrow_mut()["applyKey"]=p["idempotencyKey"].clone();
                let invocation=if clips.len()==1{LiveInvocation::new("arrangement.midi-clip.create",clips[0].clone())}else{LiveInvocation::new("transaction.group",json!({"label":format!("Kumi: {} Arrangement MIDI clips",clips.len()),"ops":clips.iter().map(|c|json!({"operation":"arrangement.midi-clip.create","args":c})).collect::<Vec<_>>()}))};
                if let Err(e)=self.async_adapter().invoke_async(&invocation,Some(&context)).await{
                    let message=e.to_string();let step=message.strip_prefix("Kumi's Live extension: ").unwrap_or(&message);
                    let is_step=step.strip_prefix("step ").and_then(|s|s.split_once(" failed (")).is_some_and(|(n,_)|!n.is_empty()&&n.bytes().all(|b|b.is_ascii_digit()));
                    if clips.len()==1||signal.is_some_and(Signal::is_cancelled)||!is_step{return Err(e);}
                    made=self.created_arrangement_midi_clips(&t,&context).await?;if !made.iter().any(Option::is_some){return Err(e);}partial=Some(kumi_common::js::string::head(&message, 300));
                }
                if partial.is_none(){
                    (made,landed)=self.landed_arrangement_midi_clips(&t,&context).await?;
                    if !made.iter().any(Option::is_some){return Err(LiveError::error(format!("Live made the clips, but the Arrangement doesn't show them where they were asked; Live shows {}",self.arrangement_midi_seen(&t,&context).await?)));}
                    if !made.iter().all(Option::is_some){partial=Some("the Arrangement doesn't show every clip where it was asked".into());}
                    else if let Some((i,notes))=landed.iter().zip(clips).enumerate().find_map(|(i,(notes,clip))|notes.as_ref().filter(|n|n.len()!=clip["notes"].as_array().map_or(0,Vec::len)).map(|n|(i,n.len()))){partial=Some(format!("Live shows {notes} of the {} notes asked for in the clip at beat {}",clips[i]["notes"].as_array().map_or(0,Vec::len),js_number(number(&clips[i]["start"]))));}
                }
            }
            let mut fences=vec![];let mut created=vec![];let mut not_made=vec![];
            for (i,row) in made.iter().enumerate(){if let Some(row)=row{
                fences.push(json!({"objectIdentity":row["objectIdentity"],"name":row["name"],"start":row["start"],"end":arrangement_clip_end(row),"notesRevision":Self::notes_revision(&match landed.get(i).cloned().flatten(){Some(notes)=>notes,None=>self.clip_notes_async(row["ref"].as_str().unwrap(),Some(&context)).await?})?}));
                created.push(json!({"ref":row["ref"],"objectIdentity":row["objectIdentity"],"trackRef":clips[i]["trackRef"],"name":row["name"],"start":row["start"],"length":row["length"],"notes":clips[i]["notes"].as_array().unwrap().len()}));
            }else{let mut c=json!({"trackRef":clips[i]["trackRef"],"start":clips[i]["start"]});if let Some(n)=clips[i].get("name"){c["name"]=n.clone();}not_made.push(c);}}
            record.borrow_mut()["created"]=json!({"clips":created,"fences":fences});record.borrow_mut()["applyKey"]=p["idempotencyKey"].clone();record.borrow_mut()["state"]=json!("applied");
            let mut result=json!({"transactionId":t["id"],"state":"applied","clips":created});if let Some(reason)=partial{result["partial"]=json!({"made":created.len(),"of":clips.len(),"notMade":not_made,"reason":reason});}if reconciliation{result["reconciled"]=json!(true);}result["idempotent"]=json!(false);Ok(success_text(id,&result))
        }.await;
        Some(result.unwrap_or_else(|e| {
            if reconciliation {
                record.borrow_mut()["state"] = json!("uncertain");
            }
            apply_failed(
                id,
                &record,
                &e,
                "Whether the clips are there is uncertain: retry with the same key, which looks for them before making any.",
            )
        }))
    }
    pub async fn undo_arrangement_midi_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = self
            .clip_lifecycle_transactions
            .get(p["transactionId"].as_str().unwrap())
            .filter(|r| r.borrow()["kind"] == "arrangement-midi-create")
        else {
            return transaction_error(id, "Unknown or expired Arrangement MIDI clip transaction");
        };
        let t = record.borrow().clone();
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconciliation = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if (t["state"] != "applied" && !reconciliation) || t.get("created").is_none() {
            return transaction_error(id, "Only an applied or exact-key uncertain Arrangement MIDI clip transaction can be undone");
        }
        let result = async {
            let status = self.require_connected(None)?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(p, signal, reads::AUDITION_DEADLINE_MS);
            record.borrow_mut()["undoKey"] = p["idempotencyKey"].clone();
            let clips = t["created"]["clips"].as_array().unwrap();
            let refs = refs(clips);
            if !reconciliation {
                let snapshot = self.views.view_for(Some(&context), &refs, None, &[]).await?;
                if clips.iter().any(|c| !clips_of(&snapshot, &c["trackRef"]).iter().any(|r| r["objectIdentity"] == c["objectIdentity"])) {
                    return Ok(transaction_error(id, "a clip this change made isn't in the Arrangement any more; undo refused"));
                }
                for fence in t["created"]["fences"].as_array().into_iter().flatten() {
                    let Some(clip) = clips.iter().find(|c| c["objectIdentity"] == fence["objectIdentity"]) else { continue };
                    let snapshot = self.views.view_for(Some(&context), &[clip["trackRef"].clone()], None, &[]).await?;
                    let Some(row) =
                        clips_of(&snapshot, &clip["trackRef"]).into_iter().find(|r| r["objectIdentity"] == fence["objectIdentity"])
                    else {
                        continue;
                    };
                    if row.get("name") != fence.get("name")
                        || !same_live_value(row.get("start"), fence.get("start"))
                        || !same_live_value(Some(&json!(arrangement_clip_end(&row))), fence.get("end"))
                        || Self::notes_revision(&self.clip_notes_async(row["ref"].as_str().unwrap(), Some(&context)).await?)?
                            != fence["notesRevision"]
                    {
                        return Ok(reason_error(
                            id,
                            &format!(
                                "the clip \"{}\" Kumi made has been edited since (its notes, name or length): it stays, with those edits",
                                js_string(fence.get("name").filter(|v| !v.is_null()).unwrap_or(&json!("")))?
                            ),
                            "If it should go anyway, delete that clip.",
                        ));
                    }
                }
            }
            self.begin_undo_recovery(&record, p["idempotencyKey"].as_str().unwrap())?;
            record.borrow_mut()["state"] = json!("undoing");
            for clip in clips.iter().rev() {
                let snapshot = self.views.view_for(Some(&context), &[clip["trackRef"].clone()], None, &[]).await?;
                if let Some(row) =
                    clips_of(&snapshot, &clip["trackRef"]).into_iter().find(|r| r["objectIdentity"] == clip["objectIdentity"])
                {
                    let mut args = json!({"ref":row["ref"]});
                    for (k, v) in self.arrangement_clip_authority(&snapshot, row["ref"].as_str().unwrap())?.as_object().unwrap() {
                        args[k] = v.clone();
                    }
                    args["explicitDeletion"] = json!(true);
                    self.invoke_undo_recovery(&record, adapter.as_ref(), "arrangement.clip.delete", &args, &context).await?;
                }
            }
            let after = self.views.view_for(Some(&context), &refs, None, &[]).await?;
            if clips.iter().any(|c| clips_of(&after, &c["trackRef"]).iter().any(|r| r["objectIdentity"] == c["objectIdentity"])) {
                return Err(LiveError::error("a clip this change made is still in the Arrangement"));
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Arrangement MIDI clip undo is uncertain; look at the Arrangement, then retry with the same key.")
        })
    }
    fn clear_range_fence(&self, s: &LiveSnapshot, reference: &Value) -> Result<Option<String>, LiveError> {
        let Some(track) = s
            .tracks
            .as_deref()
            .ok_or_else(|| LiveError::type_error("Cannot read properties of undefined (reading 'find')"))?
            .iter()
            .find(|t| Some(t.ref_.as_str()) == reference.as_str())
        else {
            return Ok(None);
        };
        Ok(Some(js_json::stringify(
            &json!({"track":[track.ref_,track.object_identity,track.name],"clips":self.arrangement_fence(s,&[reference.clone()])?}),
        )))
    }
    pub async fn live_clip_clear_range_preview_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, &["trackRef", "fromBeat", "toBeat"])
            || !is_non_empty_string(&p["trackRef"], 256)
            || !finite_at_least(&p["fromBeat"], 0.)
            || !finite_at_least(&p["toBeat"], 0.)
            || number(&p["toBeat"]) <= number(&p["fromBeat"])
        {
            return error(id, -32602, "trackRef, fromBeat and a later toBeat are required", None);
        }
        let result=async{
            let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.has_operation("clip.clear-range"){return Err(LiveError::error("clearing a range needs Kumi's Live extension, which isn't connected"));}
            let context=LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS));let snapshot=self.views.view_for(Some(&context),&[p["trackRef"].clone()],None,&[]).await?;
            let track=snapshot.tracks.as_deref().ok_or_else(||LiveError::type_error("Cannot read properties of undefined (reading 'find')"))?.iter().find(|t|Some(t.ref_.as_str())==p["trackRef"].as_str()).filter(|t|t.object_identity.as_ref().is_some_and(|s|is_non_empty_string(&json!(s),256))).ok_or_else(||LiveError::error("track reference is not authoritative"))?;
            if matches!(track.kind,TrackKind::Return|TrackKind::Main)||track_media(track).is_none(){return Err(LiveError::error(format!("track \"{}\" has no Arrangement clips of its own",track.name)));}
            let from=number(&p["fromBeat"]);let to=number(&p["toBeat"]);let overlapping=clips_of(&snapshot,&p["trackRef"]).into_iter().filter(|c|number(&c["start"])<to&&arrangement_clip_end(c)>from).collect::<Vec<_>>();
            if overlapping.is_empty(){return Err(LiveError::error(format!("track \"{}\" has no Arrangement clips between beat {} and {}",track.name,js_string(&p["fromBeat"])?,js_string(&p["toBeat"])?)));}
            let mut removes=vec![];let mut cuts=vec![];for c in overlapping{let row=json!({"ref":c["ref"],"name":c["name"],"start":c["start"],"end":arrangement_clip_end(&c)});if number(&c["start"])>=from&&arrangement_clip_end(&c)<=to{removes.push(row)}else{cuts.push(row)}}
            let t=json!({"id":tempo::transaction_id("clearrange"),"epoch":status.epoch,"kind":"clip-clear-range","fence":self.clear_range_fence(&snapshot,&p["trackRef"])? ,"clipRef":p["trackRef"],"payload":{"trackRef":p["trackRef"],"fromBeat":p["fromBeat"],"toBeat":p["toBeat"],"expectedName":track.name},"prior":{"removes":removes,"cuts":cuts},"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions,t.clone(),"clear range")?;
            Ok(success_text(id,&json!({"transactionId":t["id"],"epoch":t["epoch"],"trackRef":p["trackRef"],"trackName":track.name,"fromBeat":p["fromBeat"],"toBeat":p["toBeat"],"removes":removes,"cuts":cuts,"impact":"clears-arrangement-range-no-undo","kept":KEPT,"confirmation":"apply","expiresAt":t["expiresAt"]})))
        }.await;
        result.unwrap_or_else(|e| {
            adapter_tool_error(id, &e, "Nothing was cleared; discover the track again and preview from fresh references.")
        })
    }
    pub async fn live_clip_clear_range_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()).filter(|r| {
            let t = r.borrow();
            t["kind"] == "clip-clear-range" && !(t["state"] == "previewed" && number(&t["expiresAt"]) <= now_ms_f64())
        }) else {
            return Some(transaction_error(id, "Unknown or expired clear-range transaction"));
        };
        let t = record.borrow().clone();
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            return Some(success_text(
                id,
                &json!({"transactionId":t["id"],"state":"applied","removed":t["created"].get("removed").unwrap_or(&json!([])),"kept":KEPT,"idempotent":true}),
            ));
        }
        if t["state"] != "previewed" {
            return Some(transaction_error(id, "Transaction is no longer applicable: look at the Arrangement, then preview again"));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        let result=async{
            let status=self.require_connected(None)?;if json!(status.epoch)!=t["epoch"]{return Ok(transaction_error(id,"Live connection epoch changed; preview again"));}
            let context=self.transaction_context(p,signal,reads::AUDITION_DEADLINE_MS);let payload=&t["payload"];let refs=[payload["trackRef"].clone()];let snapshot=self.views.view_for(Some(&context),&refs,None,&[]).await?;
            if self.clear_range_fence(&snapshot,&payload["trackRef"])? .as_deref()!=t["fence"].as_str(){return Ok(transaction_error(id,"the track or its Arrangement clips changed since the preview; preview again"));}
            record.borrow_mut()["state"]=json!("applying");record.borrow_mut()["applyKey"]=p["idempotencyKey"].clone();let result=self.async_adapter().invoke_async(&LiveInvocation::new("clip.clear-range",payload.clone()),Some(&context)).await?;
            let after=clips_of(&self.views.view_for(Some(&context),&refs,None,&[]).await?,&payload["trackRef"]);
            if after.iter().any(|c|number(&c["start"])<number(&payload["toBeat"])-1e-6&&arrangement_clip_end(c)>number(&payload["fromBeat"])+1e-6){return Err(LiveError::error("the range still holds a clip after clearing"));}
            if result.is_null(){return Err(LiveError::type_error("Cannot read properties of null (reading 'removed')"));}
            let removed=result.get("removed").filter(|v|!v.is_null()).cloned().unwrap_or(json!([]));record.borrow_mut()["created"]=json!({"removed":removed});record.borrow_mut()["state"]=json!("applied");
            Ok(success_text(id,&json!({"transactionId":t["id"],"state":"applied","removed":removed,"clipsBefore":result["clipsBefore"],"clipsAfter":result["clipsAfter"],"kept":KEPT,"idempotent":false})))
        }.await;
        Some(result.unwrap_or_else(|e| {
            apply_failed(id, &record, &e, "Whether the range is clear is uncertain: look at the Arrangement before trying again.")
        }))
    }
}
