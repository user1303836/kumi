//! Ranked Browser discovery and offline renders through Live's extension.
use super::*;
use kumi_common::{
    abort::Signal,
    js::{
        number::round,
        string::{trim, utf16_len},
    },
    time::now_ms_f64,
};
use serde::Serialize;
const CANDIDATES: usize = 10_000;
/// How long a walk of Live's Browser is kept. A walk holds Live's main thread (0.6–0.75 s for Live's
/// own library, more with packs and plug-ins), and the Browser rarely changes: when nothing kept has the
/// whole query in its name, the Browser is walked again, once, for what's new (a pack, a device Kumi
/// made), unless what's kept is under a minute old: a search for something that isn't there walks at
/// most once a minute.
const CACHE_MS: f64 = 600_000.;
const MISS_WALK_MS: f64 = 60_000.;
const ROOTS: &[&str] =
    &["instruments", "audio_effects", "midi_effects", "modulators", "drums", "plugins", "packs", "max_for_live", "clips"];
#[derive(Clone)]
pub(super) struct BrowserCache {
    /// Shared, so a cache hit (the start of every device or preset load) doesn't copy 10k rows.
    items: std::rc::Rc<Vec<Value>>,
    at: f64,
    epoch: i64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Ranked {
    score: u64,
    matched_tokens: Vec<String>,
    exact_name_match: bool,
}
fn words(value: &str) -> impl Iterator<Item = &str> {
    value.split(|c: char| !c.is_ascii_lowercase() && !c.is_ascii_digit()).filter(|s| !s.is_empty())
}
fn rank(item: &Value, query: &str, tokens: &[String]) -> Ranked {
    let name = item["name"].as_str().unwrap().to_lowercase();
    let path = item["path"].as_str().unwrap().to_lowercase();
    let words: Vec<_> = words(&name).collect();
    let mut score = 0;
    let mut exact = false;
    if !query.is_empty() && name.contains(query) {
        score += 100;
        exact = true;
    } else if !query.is_empty() && path.contains(query) {
        score += 40;
    }
    let mut matched = Vec::new();
    for token in tokens {
        let mut points = if words.contains(&token.as_str()) {
            30
        } else if words.iter().any(|w| w.starts_with(token)) {
            18
        } else if name.contains(token) {
            10
        } else {
            0
        };
        if path.contains(token) {
            points += 4;
        }
        if points > 0 {
            matched.push(token.clone());
            score += points;
        }
    }
    if !tokens.is_empty() && matched.len() == tokens.len() {
        score += 25;
    }
    Ranked { score, matched_tokens: matched, exact_name_match: exact }
}
impl McpHost {
    pub async fn dispatch_browser_render_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let params = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(match call.name.as_str() {
            "live_browser_search" => self.live_browser_search_async(&call.id, params).await.map(Some),
            "live_render_offline" => Ok(self.live_render_offline_async(&call.id, params, signal).await),
            _ => return None,
        })
    }
    fn browser_now(&self) -> f64 {
        self.options.now.as_ref().map_or_else(now_ms_f64, |now| now())
    }
    pub async fn live_browser_search_async(&self, id: &Value, params: &Value) -> Result<Value, LiveError> {
        if !has_only(params, &["category", "query", "limit", "matchMode", "refresh"])
            || (params.get("category").is_some() && !ROOTS.contains(&js_string(&params["category"])?.as_str()))
            || params.get("query").is_some_and(|v| !is_non_empty_string(v, 256) && v != "")
            || params.get("limit").is_some_and(|v| !is_integer_in_range(v, 1., 100.))
            || params.get("matchMode").is_some_and(|v| v != "ranked" && v != "substring")
            || params.get("refresh").is_some_and(|v| !v.is_boolean())
        {
            return Ok(error(id, -32602, "category, query, limit, matchMode, and refresh are invalid", None));
        }
        let result=async{
            let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected||!status.capabilities.iter().any(|c|c.as_str()=="session.read"){return Err(LiveError::error("session read capability is unavailable"));}
            if !status.has_operation("browser.search"){return Err(LiveError::error("the Live Browser is unavailable"));}
            let adapter=self.async_adapter();
            if params["matchMode"]=="substring"{
                let mut args=json!({});for key in ["category","query","limit"]{if let Some(value)=params.get(key){args[key]=value.clone();}}
                let result=adapter.invoke_async(&LiveInvocation::new("browser.search",args),Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
                if result.is_null(){return Err(LiveError::type_error("Cannot read properties of null (reading 'items')"));}
                return Ok(success_text(id,&json!({"items":result["items"].as_array().cloned().unwrap_or_default()})));
            }
            let epoch=status.epoch.unwrap_or(0);let key=params["category"].as_str().unwrap_or("all");
            let query=params["query"].as_str().unwrap_or("");let lower=trim(query).to_lowercase();let mut seen=HashSet::new();
            let tokens:Vec<_>=words(&lower).filter(|w|w.len()<=64&&seen.insert(w.to_string())).take(8).map(str::to_owned).collect();
            let mut refresh=params["refresh"]==true;
            let (entry,from_cache)=loop{
            let prior=self.browser_search_cache.borrow().iter().find(|(name,_)|name==key).map(|(_,v)|v.clone());
            let (entry,from_cache)=if let Some(prior)=prior.filter(|c|c.epoch==epoch&&self.browser_now()-c.at<CACHE_MS&&!refresh){
                self.browser_search_cache.borrow_mut().retain(|(name,_)|name!=key);self.browser_search_cache.borrow_mut().push_back((key.into(),prior.clone()));(prior,true)
            }else{
                let mut args=json!({"query":"","limit":CANDIDATES});if let Some(category)=params.get("category"){args["category"]=category.clone();}
                let result=adapter.invoke_async(&LiveInvocation::new("browser.search",args),Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
                if result.is_null(){return Err(LiveError::type_error("Cannot read properties of null (reading 'items')"));}
                let Some(rows)=result["items"].as_array().filter(|a|a.len()<=CANDIDATES)else{return Err(LiveError::error("browser search returned an unbounded or malformed candidate set"))};
                let mut items=Vec::with_capacity(rows.len());
                for row in rows{
                    if !row.is_object()||!is_non_empty_string(&row["id"],256)||!is_non_empty_string(&row["objectIdentity"],256)
                        ||!row["name"].as_str().is_some_and(|s|utf16_len(s)<=256)||!is_non_empty_string(&row["category"],64)
                        ||!row["path"].as_str().is_some_and(|s|utf16_len(s)<=512)||!row["isDevice"].is_boolean(){return Err(LiveError::error("browser search returned a malformed candidate set"));}
                    items.push(json!({"id":row["id"],"objectIdentity":row["objectIdentity"],"name":row["name"],"category":row["category"],"path":row["path"],"isDevice":row["isDevice"]}));
                }
                let entry=BrowserCache{items:std::rc::Rc::new(items),at:self.browser_now(),epoch};let mut cache=self.browser_search_cache.borrow_mut();
                if let Some((_,old))=cache.iter_mut().find(|(name,_)|name==key){*old=entry.clone();}else{cache.push_back((key.into(),entry.clone()));}
                while cache.len()>16{cache.pop_front();}(entry,false)
            };
            // Nothing kept has the whole query in its name (a new "Gritty Reese Bass" besides an old "Bass"):
            // walk again, once, for what's new since.
            if from_cache&&!tokens.is_empty()&&self.browser_now()-entry.at>=MISS_WALK_MS&&!entry.items.iter().any(|item|rank(item,&lower,&tokens).exact_name_match){refresh=true;continue;}
            break (entry,from_cache);
            };
            let mut ranked:Vec<_>=entry.items.iter().map(|item|(item,rank(item,&lower,&tokens))).filter(|(_,r)|tokens.is_empty()||!r.matched_tokens.is_empty()).collect();
            ranked.sort_by(|(a,ar),(b,br)|br.score.cmp(&ar.score).then_with(||a["name"].as_str().unwrap().encode_utf16().cmp(b["name"].as_str().unwrap().encode_utf16())).then_with(||a["id"].as_str().unwrap().encode_utf16().cmp(b["id"].as_str().unwrap().encode_utf16())));
            let limit=params["limit"].as_f64().unwrap_or(50.) as usize;
            let page:Vec<_>=ranked.iter().take(limit).map(|(item,r)|{let mut row=(*item).clone();row["score"]=json!(r.score);row["match"]=json!({"matchedTokens":r.matched_tokens,"exactNameMatch":r.exact_name_match});row}).collect();
            let mut roots:Vec<_>=entry.items.iter().map(|i|i["category"].as_str().unwrap()).collect::<HashSet<_>>().into_iter().collect();roots.sort_by(|a,b|a.encode_utf16().cmp(b.encode_utf16()));
            Ok(success_text(id,&json!({"items":page,"matchMode":"ranked","query":query,"tokens":tokens,"searchedRoots":roots,"candidates":entry.items.len(),"candidateBound":CANDIDATES,"candidateBoundReached":entry.items.len()>=CANDIDATES,"truncated":ranked.len()>page.len(),"fromCache":from_cache,"cacheAgeSeconds":round((self.browser_now()-entry.at)/1000.).max(0.),"cacheTtlSeconds":CACHE_MS/1000.,"epoch":epoch,"note":"Ranked host-side over one bounded candidate traversal per root; searchedRoots are the roots that contributed candidates, so a bound-limited traversal may not have reached every requested root. Substring-exact matching remains available with matchMode=substring. Load an item straight from these results."})))
        }.await;
        Ok(result.unwrap_or_else(|cause| adapter_tool_error(id, &cause, "Browser search requires an available Live Browser.")))
    }
    pub(super) fn extension_track<'a>(
        &self,
        snapshot: &'a LiveSnapshot,
        reference: &Value,
        kind: TrackMedia,
        what: &str,
    ) -> Result<&'a Track, LiveError> {
        let track = snapshot
            .tracks
            .as_deref()
            .ok_or_else(|| LiveError::type_error("Cannot read properties of undefined (reading 'find')"))?
            .iter()
            .find(|t| Some(t.ref_.as_str()) == reference.as_str())
            .filter(|t| t.object_identity.as_ref().is_some_and(|s| is_non_empty_string(&json!(s), 256)))
            .ok_or_else(|| LiveError::error("track reference is not authoritative"))?;
        if track.kind == TrackKind::Group {
            return Err(LiveError::error(format!("track \"{}\" is a group: {what} its tracks instead", track.name)));
        }
        if matches!(track.kind, TrackKind::Return | TrackKind::Main) || track_media(track) != Some(kind) {
            return Err(LiveError::error(format!(
                "track \"{}\" isn't {} track",
                track.name,
                if kind == TrackMedia::Audio { "an audio" } else { "a MIDI" }
            )));
        }
        Ok(track)
    }
    pub async fn live_render_offline_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !has_only(params, &["trackRef", "fromBeat", "toBeat", "expectedName"])
            || !is_non_empty_string(&params["trackRef"], 256)
            || !is_finite_at_least(&params["fromBeat"], 0.)
            || !is_finite_at_least(&params["toBeat"], 0.)
            || params["toBeat"].as_f64().unwrap() <= params["fromBeat"].as_f64().unwrap()
            || params.get("expectedName").is_some_and(|v| !is_non_empty_string(v, 256))
        {
            return Some(error(id, -32602, "trackRef, fromBeat and a later toBeat are required; expectedName is optional", None));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        let result=async{
            self.require_operation("render.offline").await?;
            let context=LiveOperationContext{signal:signal.cloned(),deadline_ms:Some(now_ms_f64()+(30_000.+1000.*(params["toBeat"].as_f64().unwrap()-params["fromBeat"].as_f64().unwrap())).min(600_000.)),..Default::default()};
            let read_context=LiveOperationContext{signal:signal.cloned(),deadline_ms:Some(self.deadline(reads::AUDITION_DEADLINE_MS)),..Default::default()};
            let snapshot=self.views.view_for(Some(&read_context),&[params["trackRef"].clone()],None,&[]).await?;
            let track=self.extension_track(&snapshot,&params["trackRef"],TrackMedia::Audio,"render")?;
            if params.get("expectedName").is_some_and(|n|n!=&track.name){return Err(LiveError::error(format!("the track there is \"{}\" now, not \"{}\"",track.name,params["expectedName"].as_str().unwrap())));}
            let rendered=self.async_adapter().invoke_async(&LiveInvocation::new("render.offline",json!({"trackRef":params["trackRef"],"fromBeat":params["fromBeat"],"toBeat":params["toBeat"],"expectedName":track.name})),Some(&context)).await?;
            let mut result=json!({});
            // JavaScript's object spread keeps object fields and indexed array/string properties.
            if let Some(row)=rendered.as_object(){result=json!(row);}else if let Some(items)=rendered.as_array(){for (i,item) in items.iter().enumerate(){result[i.to_string()]=item.clone();}}else if let Some(text)=rendered.as_str(){for (i,ch) in text.chars().enumerate(){result[i.to_string()]=json!(ch.to_string());}}
            result["trackRef"]=params["trackRef"].clone();result["name"]=json!(track.name);result["fromBeat"]=params["fromBeat"].clone();result["toBeat"]=params["toBeat"].clone();
            Ok(success_text(id,&result))
        }.await;
        Some(result.unwrap_or_else(|cause| {
            adapter_tool_error(
                id,
                &cause,
                "Nothing was rendered: render an audio track (not a group) between two beats where it has clips.",
            )
        }))
    }
}
