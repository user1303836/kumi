//! A quoted, single-use Mirelo request. Python owns credit reservations; this operation rechecks
//! that authority, checkpoints before dispatch, and can recover an identified job only by GET.

use super::artifacts::StagedArtifact;
use super::ctx::{Call, Ctx, Effect, Host, JsonObject, OpError, OpFuture, Operation};
use super::generate_ledger::{Authority, CreateClaim};
use super::generate_output;
use super::generate_source::{self, Pin};
use super::jobs::JobHandle;
use kumi_ai::audio::{mirelo, Task};
use kumi_common::abort::{self, Signal};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

mod input;
mod records;

const QUOTE_MS: u64 = 5 * 60 * 1000;
const UPLOAD_QUOTE_LIMIT: Duration = Duration::from_secs(180);
const PENDING: usize = 8;
const OUTPUT_BYTES: u64 = 50 * 1024 * 1024;

pub const GENERATE: Operation = Operation {
    name: "generate",
    description: "Quote one Mirelo sound effect, audio-to-MIDI transcription, video sound effect, audio extension or inpaint, then create only that quote. Each create spends credits. Estimates are not bill ceilings; obtain the producer's quote-specific approval before create and show each job's actual or unknown credits. Uploads send the whole approved source. seconds is the billed span; source_seconds is the measured source, not the returned edit's length. Files are retained artifacts, never inserted into a DAW here. Job recovery only GETs an existing identified job.",
    schema,
    effect,
    run,
};

pub fn schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["action"],
        "properties": {
            "action": {"enum": ["quote", "create", "job"]},
            "kind": {"type": "string", "enum": super::generate_ledger::POLICY_KEYS},
            "prompt": {"type": "string", "minLength": 1, "maxLength": 500},
            "seconds": {"type": "number", "exclusiveMinimum": 0,
                "description": "Billed generation span: text-to-SFX defaults to 5 seconds; video defaults to its measured whole duration. Never the whole returned edit's length."},
            "append_seconds": {"type": "number", "exclusiveMinimum": 0,
                "description": "New seconds appended to the complete audio source; extend only."},
            "region_start_seconds": {"type": "number", "minimum": 0},
            "region_end_seconds": {"type": "number", "exclusiveMinimum": 0},
            "source": super::change::source_schema(),
            "preview": {"type": "boolean"},
            "quote_id": {"type": "string", "pattern": "^q-[a-f0-9]{32}$"},
            "job_id": {"type": "string", "pattern": "^j[0-9]+-[0-9]+$", "maxLength": 64}
        },
        "oneOf": [
            {"properties": {"action": {"const": "quote"}, "kind": {"const": "sfx"}},
             "required": ["kind", "prompt"], "not": {"anyOf": [
                {"required": ["source"]}, {"required": ["preview"]}, {"required": ["quote_id"]}, {"required": ["job_id"]},
                {"required": ["append_seconds"]}, {"required": ["region_start_seconds"]}, {"required": ["region_end_seconds"]}]}},
            {"properties": {"action": {"const": "quote"}, "kind": {"const": "audio_to_midi"}},
             "required": ["kind", "source"], "not": {"anyOf": [
                {"required": ["prompt"]}, {"required": ["seconds"]}, {"required": ["quote_id"]}, {"required": ["job_id"]},
                {"required": ["append_seconds"]}, {"required": ["region_start_seconds"]}, {"required": ["region_end_seconds"]}]}},
            {"properties": {"action": {"const": "quote"}, "kind": {"const": "video_to_sfx"},
                 "source": {"not": {"anyOf": [{"required": ["sound_id"]}, {"required": ["map_revision"]}]}}},
             "required": ["kind", "source"], "not": {"anyOf": [
                {"required": ["quote_id"]}, {"required": ["job_id"]}, {"required": ["append_seconds"]},
                {"required": ["region_start_seconds"]}, {"required": ["region_end_seconds"]}]}},
            {"properties": {"action": {"const": "quote"}, "kind": {"const": "extend"}},
             "required": ["kind", "source"],
             "anyOf": [{"required": ["preview"], "properties": {"preview": {"const": true}}}, {"required": ["append_seconds"]}],
             "not": {"anyOf": [
                {"required": ["seconds"]}, {"required": ["quote_id"]}, {"required": ["job_id"]},
                {"required": ["region_start_seconds"]}, {"required": ["region_end_seconds"]}]}},
            {"properties": {"action": {"const": "quote"}, "kind": {"const": "inpaint"}},
             "required": ["kind", "source"],
             "anyOf": [{"required": ["preview"], "properties": {"preview": {"const": true}}},
                 {"required": ["region_start_seconds", "region_end_seconds"]}],
             "not": {"anyOf": [
                {"required": ["seconds"]}, {"required": ["append_seconds"]}, {"required": ["quote_id"]}, {"required": ["job_id"]}]}},
            {"properties": {"action": {"const": "create"}}, "required": ["quote_id"], "not": {"anyOf": [
                {"required": ["kind"]}, {"required": ["prompt"]}, {"required": ["seconds"]}, {"required": ["source"]},
                {"required": ["preview"]}, {"required": ["job_id"]}, {"required": ["append_seconds"]},
                {"required": ["region_start_seconds"]}, {"required": ["region_end_seconds"]}]}},
            {"properties": {"action": {"const": "job"}}, "required": ["job_id"], "not": {"anyOf": [
                {"required": ["kind"]}, {"required": ["prompt"]}, {"required": ["seconds"]}, {"required": ["source"]},
                {"required": ["preview"]}, {"required": ["quote_id"]}, {"required": ["append_seconds"]},
                {"required": ["region_start_seconds"]}, {"required": ["region_end_seconds"]}]}}
        ]
    })
}

fn effect(args: &JsonObject) -> Effect {
    match args.get("action").and_then(Value::as_str) {
        Some("create") => Effect::Paid,
        Some("quote") if args.contains_key("source") && args.get("preview") != Some(&Value::Bool(true)) => Effect::Upload,
        _ => Effect::Read,
    }
}

#[derive(Default)]
pub struct Generation {
    pending: RefCell<Pending>,
    records: RefCell<records::Index>,
}
#[derive(Default)]
struct Pending {
    previews: HashMap<CallKey, (u64, Task, Rc<Pin>)>,
    quotes: HashMap<String, Quote>,
}
#[derive(Clone, PartialEq, Eq, Hash)]
struct CallKey {
    profile: Option<String>,
    session: Option<String>,
    call: String,
}
struct Quote {
    public: Value,
    prepared: mirelo::Prepared,
    credential: [u8; 32],
    source: Option<(Rc<Pin>, StagedArtifact)>,
}

impl Generation {
    fn prune(&self) {
        let now = now();
        let mut pending = self.pending.borrow_mut();
        pending.previews.retain(|_, (expires, _, _)| *expires > now);
        pending.quotes.retain(|_, quote| quote.public["expires_at"].as_u64().is_some_and(|expires| expires > now));
    }
}

fn run(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(async move {
        ctx.generation().prune();
        check(&call.signal)?;
        match call.args.get("action").and_then(Value::as_str) {
            Some("quote") => quote(ctx, call).await,
            Some("create") => create(ctx, call).await,
            Some("job") => follow(ctx, call).await,
            _ => Err(refused("invalid_request", "Choose quote, create, or job.")),
        }
    })
}

async fn ensure_records(ctx: &Ctx) -> Result<(), OpError> {
    if ctx.generation().records.borrow().loaded()? {
        return Ok(());
    }
    let state = ctx.paths().state_dir.clone();
    let loaded = tokio::task::spawn_blocking(move || records::load(&state))
        .await
        .map_err(|_| refused("ledger_unavailable", "The retained generation receipts could not be inspected."))??;
    // GET-only recovery may have learned a charge while this read ran. A second scan may also
    // finish after another create; neither may erase live evidence or hide a conflicting receipt.
    ctx.generation().records.borrow_mut().merge(loaded)
}

fn remember(ctx: &Ctx, id: &str, receipt: &Value) -> Result<(), OpError> {
    ctx.generation().records.borrow_mut().remember(id, receipt)
}

fn no_unsettled_overrun(ctx: &Ctx, authority: &Authority) -> Result<(), OpError> {
    let held = ctx.generation().records.borrow();
    let records = held.complete()?;
    for reservation in authority.unsettled() {
        let Some(record) = records.get(reservation.quote_id) else { continue };
        if record.kind != reservation.kind
            || record.reserved != reservation.reserved_credits
            || reservation.job_id.is_some_and(|id| id != record.job_id)
        {
            return Err(stale());
        }
        let held_credits = if reservation.settlement == "manual_release" { 0 } else { reservation.reserved_credits };
        if record.actual.is_some_and(|actual| actual > held_credits) {
            return Err(refused("ledger_frozen", &format!(
                "ledger_frozen: job {} exceeded its held credits. Inspect that job to settle its charge before the producer clears the freeze.",
                record.job_id
            )));
        }
    }
    Ok(())
}

async fn authority(ctx: &Ctx) -> Result<Authority, OpError> {
    let state = ctx.paths().state_dir.clone();
    tokio::task::spawn_blocking(move || Authority::read(&state))
        .await
        .map_err(|_| refused("ledger_unavailable", "The credit ledger could not be read; nothing new was authorized."))?
}

fn private_key(call: &Call, creating: bool) -> Result<&str, OpError> {
    let private = call.host.private.get("_mirelo");
    if creating
        && (private.and_then(|value| value["acknowledged"].as_bool()) != Some(true)
            || private.and_then(|value| value["enabled"].as_bool()) != Some(true))
    {
        return Err(refused("not_enabled", "Mirelo generation of this kind is off or its disclosure has not been acknowledged."));
    }
    let key = private
        .and_then(|value| value["api_key"].as_str())
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| refused("not_configured", "Set the Mirelo secret in Hermes before using this provider."))?;
    let escaped = Value::String(key.to_owned()).to_string();
    if call.args.iter().any(|(field, value)| field.contains(key) || has_secret(value, key, &escaped[1..escaped.len() - 1])) {
        return Err(refused("invalid_request", "The private provider key must not appear in public arguments."));
    }
    Ok(key)
}

fn call_key(host: &Host) -> Result<CallKey, OpError> {
    let call = host
        .tool_call_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| refused("not_approved", "Generation requires the host's tool-call identity."))?;
    Ok(CallKey { profile: host.profile_id.clone(), session: host.session_id.clone(), call: call.to_owned() })
}

async fn quote(ctx: &Ctx, mut call: Call) -> Result<Value, OpError> {
    if !call.args.contains_key("source") || call.args.get("preview") == Some(&Value::Bool(true)) {
        return quote_inner(ctx, call).await;
    }
    let deadline = Signal::new();
    call.signal = abort::any([call.signal.clone(), deadline.clone()]);
    let result = tokio::time::timeout(UPLOAD_QUOTE_LIMIT, quote_inner(ctx, call)).await;
    deadline.cancel();
    result.map_err(|_| {
        refused(
        "quote_timeout",
        "Preparing the approved upload exceeded 180 seconds. The upload may have reached Mirelo, but no quote or generation was created.",
    )
    })?
}

async fn quote_inner(ctx: &Ctx, call: Call) -> Result<Value, OpError> {
    let kind = call.args.get("kind").and_then(Value::as_str).unwrap_or("");
    let task = task(kind)?;
    let key = private_key(&call, true)?;
    let host = call_key(&call.host)?;
    authority(ctx).await?.policy(kind)?;
    let client = client(ctx, key)?;
    if call.args.get("preview") != Some(&Value::Bool(true)) && ctx.generation().pending.borrow().quotes.len() >= PENDING {
        return Err(refused("busy", "Finish an existing generation quote or wait for it to expire."));
    }
    let source = if task != Task::TextToSfx {
        let source = call.args.get("source").ok_or_else(|| refused("invalid_request", "This generation requires one approved source."))?;
        if call.args.get("preview") == Some(&Value::Bool(true)) {
            let previous = ctx.generation().pending.borrow().previews.get(&host).map(|(_, kind, pin)| (*kind, pin.clone()));
            if let Some((saved_task, pin)) = previous {
                if saved_task != task || pin.source() != source {
                    return Err(stale());
                }
                return Ok(pin.preview());
            }
            if ctx.generation().pending.borrow().previews.len() >= PENDING {
                return Err(refused("busy", "Finish an existing source approval or wait for it to expire."));
            }
            let pin = Rc::new(generate_source::preview(ctx, source, task == Task::VideoToSfx, &call.signal).await?);
            let result = pin.preview();
            ctx.generation().pending.borrow_mut().previews.insert(host, (now() + QUOTE_MS, task, pin));
            return Ok(result);
        }
        let saved = ctx.generation().pending.borrow_mut().previews.remove(&host);
        let pin = saved
            .filter(|(expires, saved_task, pin)| *expires > now() && *saved_task == task && pin.source() == source)
            .map(|(_, _, pin)| pin)
            .ok_or_else(|| refused("stale", "The source or task no longer matches an unexpired upload preview for this host call."))?;
        let stage = pin.stage(ctx, &call.signal).await?;
        Some((pin, stage))
    } else {
        None
    };
    let input::Input { request, mut details } = input::prepare(ctx, &call, task, source.as_ref()).await?;
    if let Some((pin, stage)) = &source {
        // Recheck after measurement and immediately before the SDK's preparation can upload.
        // A private true flag or an environment key is never stored producer consent.
        authority(ctx).await?.policy(kind)?;
        stage.verify()?;
        pin.current(ctx, &call.signal).await?;
    }
    call.progress.report(
        0.0,
        None,
        Some(if source.is_some() { "Preparing the approved Mirelo upload and estimate" } else { "Reading Mirelo's estimate" }),
    );
    let mut uploads = mirelo::Uploads::default();
    if let Some((_, stage)) = &source {
        let upload = mirelo::Upload {
            file: stage.opened().try_clone().map_err(|_| refused("invalid_source", "The approved copy could not be opened for upload."))?,
            extension: stage.path().extension().and_then(|extension| extension.to_str()).unwrap_or(""),
        };
        if task == Task::VideoToSfx {
            uploads.video = Some(upload);
        } else {
            uploads.audio = Some(upload);
        }
    }
    let prepared = client.prepare(task, &request, uploads, &call.signal).await.map_err(provider)?;
    check(&call.signal)?;
    let public = details.as_object_mut().ok_or_else(stale)?;
    public.extend([
        ("state".into(), json!("quoted")),
        ("quote_id".into(), json!(format!("q-{:032x}", rand::random::<u128>()))),
        ("estimate".into(), json!({"credits": prepared.credits(), "max_credits": prepared.credits()})),
        ("credit_unit".into(), json!("mirelo_credits")),
        ("upload".into(), json!(source.is_some())),
        ("expires_at".into(), json!(now() + QUOTE_MS)),
        ("request_sha256".into(), json!(prepared.request_sha256())),
    ]);
    let public = details;
    if ctx.generation().pending.borrow().quotes.len() >= PENDING {
        return Err(refused("busy", "Other quotes filled this profile's pending slots; no billed generation was submitted."));
    }
    ctx.generation().pending.borrow_mut().quotes.insert(
        public["quote_id"].as_str().unwrap().to_owned(),
        Quote { public: public.clone(), prepared, credential: Sha256::digest(key.as_bytes()).into(), source },
    );
    Ok(public)
}

struct Reservation {
    id: String,
    amount: u64,
    call: String,
}

async fn approve(ctx: &Ctx, quote: &Value, reservation: &Reservation, job_id: Option<&str>) -> Result<(), OpError> {
    let authority = authority(ctx).await?;
    let claim = CreateClaim {
        quote_id: quote["quote_id"].as_str().unwrap_or(""),
        request_sha256: quote["request_sha256"].as_str().unwrap_or(""),
        kind: quote["kind"].as_str().unwrap_or(""),
        reservation_id: &reservation.id,
        reserved_credits: reservation.amount,
        host_call_id: &reservation.call,
        now_ms: now(),
        expected_job_id: job_id,
    };
    // Only genuine ownership can turn a later refusal into release proof. This also proves
    // quote() hits an active row, so it performs no archival I/O on the runtime thread.
    authority.prove_own(&claim)?;
    if authority.quote(claim.quote_id)?.as_ref() != Some(quote) {
        return Err(not_submitted(stale()));
    }
    authority.approve(&claim)?;
    no_unsettled_overrun(ctx, &authority).map_err(not_submitted)?;
    Ok(())
}

async fn missing_quote(ctx: &Ctx, id: &str, reservation: &Reservation) -> Result<Value, OpError> {
    if ctx.generation().records.borrow().complete()?.contains_key(id) {
        return Err(refused("quote_consumed", "A job already owns this quote; recover that job instead of resubmitting it."));
    }
    let authority = authority(ctx).await?;
    // Look only at active reservations: a previously settled/archived quote never authorizes
    // another create. Its absence in pending memory is not by itself release evidence.
    if !authority.unsettled().any(|held| held.quote_id == id && held.settlement == "pending") {
        return Err(refused("quote_consumed", "This quote is unavailable or already used; never resubmit it."));
    }
    let quote = authority.quote(id)?.ok_or_else(stale)?;
    authority.prove_own(&CreateClaim {
        quote_id: id,
        request_sha256: quote["request_sha256"].as_str().unwrap_or(""),
        kind: quote["kind"].as_str().unwrap_or(""),
        reservation_id: &reservation.id,
        reserved_credits: reservation.amount,
        host_call_id: &reservation.call,
        now_ms: now(),
        expected_job_id: None,
    })?;
    // The authority read yielded; do not turn a concurrently started job into release proof.
    if ctx.generation().records.borrow().complete()?.contains_key(id) {
        return Err(refused("quote_consumed", "A job already owns this quote; recover that job instead of resubmitting it."));
    }
    // Every POST requires a durable quote_id checkpoint. Retention never prunes an unsettled
    // generation. No such checkpoint, plus an unbound owned reservation, proves no submission.
    Err(not_submitted(refused("quote_unavailable", "This runtime no longer holds the prepared quote; request a new quote.")))
}

async fn create(ctx: &Ctx, call: Call) -> Result<Value, OpError> {
    let host = call_key(&call.host)?;
    let private = call.host.private.get("_mirelo").ok_or_else(|| refused("not_approved", "A durable credit reservation is required."))?;
    let reservation = Reservation {
        id: private["reservation_id"]
            .as_str()
            .ok_or_else(|| refused("not_approved", "A durable credit reservation is required."))?
            .to_owned(),
        amount: private["reserved_credits"].as_u64().ok_or_else(|| refused("not_approved", "A bounded credit reservation is required."))?,
        call: host.call,
    };
    // Historical corruption blocks paid creates, not upload previews or GET-only job recovery.
    ensure_records(ctx).await?;
    let id = call.args.get("quote_id").and_then(Value::as_str).unwrap_or("");
    let public = ctx.generation().pending.borrow().quotes.get(id).map(|quote| quote.public.clone());
    let Some(public) = public else { return missing_quote(ctx, id, &reservation).await };
    // Do not consume Prepared while a ledger lock/read can fail. A successful ownership check
    // precedes every definitive non-submission refusal, including changed private consent.
    if let Err(error) = approve(ctx, &public, &reservation, None).await {
        if error.code() == "not_submitted" {
            ctx.generation().pending.borrow_mut().quotes.remove(id);
        }
        return Err(error);
    }
    let quote = ctx
        .generation()
        .pending
        .borrow_mut()
        .quotes
        .remove(id)
        .ok_or_else(|| refused("quote_consumed", "Another call already owns this quote; never resubmit it."))?;
    let checked = async {
        let key = private_key(&call, true)?.to_owned();
        if quote.credential != <[u8; 32]>::from(Sha256::digest(key.as_bytes())) {
            return Err(stale());
        }
        check(&call.signal)?;
        Ok((client(ctx, &key)?, key))
    }
    .await;
    let (client, key) = checked.map_err(not_submitted)?;
    let native = ctx
        .start_job("generate", move |ctx, job| async move {
            let mut receipt = generate_output::receipt(&quote.public, reservation.amount, None, "not_submitted", None)?;
            checkpoint(&ctx, &job, &receipt).await?;
            job.report(0.0, None, Some("Checking the approved Mirelo generation"));
            let ready = async {
                if let Some((pin, stage)) = &quote.source {
                    pin.current(&ctx, &job.signal).await?;
                    stage.verify()?;
                }
                check(&job.signal)?;
                receipt["submission"] = json!("unknown");
                checkpoint(&ctx, &job, &receipt).await?;
                // This is the last authority read before dispatch, after durable I/O and source checks.
                approve(&ctx, &quote.public, &reservation, Some(job.id())).await?;
                check(&job.signal)
            }
            .await;
            if let Err(error) = ready {
                receipt["submission"] = json!("not_submitted");
                checkpoint(&ctx, &job, &receipt).await?;
                return Err(not_submitted(error));
            }
            let vendor = match client.submit(quote.prepared, &job.signal).await {
                Ok(job) => job,
                Err(error) => {
                    if error.not_submitted() {
                        receipt["submission"] = json!("not_submitted");
                        checkpoint(&ctx, &job, &receipt).await?;
                    }
                    return Err(refused(if error.not_submitted() { "not_submitted" } else { "submission_unknown" }, &error.to_string()));
                }
            };
            // Keep the private source alive through submission, but never publish its input copy.
            drop(quote.source);
            finish(&ctx, &job, &client, vendor, receipt, &key, false).await
        })
        .map_err(not_submitted)?;
    remember(
        ctx,
        &native,
        &generate_output::receipt(&public, private["reserved_credits"].as_u64().unwrap(), None, "not_submitted", None)?,
    )?;
    let mut response = json!({"state": "running", "job_id": native, "quote_id": public["quote_id"], "kind": public["kind"],
        "prompt": public["prompt"], "seconds": public["seconds"], "estimate": public["estimate"], "credit_unit": "mirelo_credits"});
    generate_output::request_metadata(&public, &mut response)?;
    Ok(response)
}

async fn finish(
    ctx: &Ctx,
    job: &JobHandle,
    client: &mirelo::Client,
    vendor: mirelo::Job,
    mut receipt: Value,
    key: &str,
    retained: bool,
) -> Result<Value, OpError> {
    let id = vendor
        .id()
        .ok_or_else(|| refused("submission_unknown", "Mirelo did not identify the submitted job; its credits remain reserved."))?;
    if receipt["provider_job_id"].as_str().is_some_and(|prior| prior != id) {
        return Err(stale());
    }
    receipt["provider_job_id"] = json!(id);
    receipt["submission"] = json!("submitted");
    if terminal(&vendor) {
        if let Some(actual) = vendor.actual_credits() {
            receipt["credits"]["actual"] = json!(actual);
        }
    }
    checkpoint(ctx, job, &receipt).await?;
    job.report(0.0, None, Some("Mirelo is generating"));
    let vendor = if terminal(&vendor) { vendor } else { client.wait(vendor, &job.signal).await.map_err(provider)? };
    if let Some(actual) = vendor.actual_credits() {
        receipt["credits"]["actual"] = json!(actual);
    }
    // Known terminal billing is durable before any download, decoder, or artifact publication.
    checkpoint(ctx, job, &receipt).await?;
    if !vendor.succeeded() {
        return Err(refused("provider_failed", "Mirelo did not complete the job successfully; its reported charge is retained."));
    }
    if retained {
        restore_media(ctx, &mut receipt)?;
        return Ok(receipt);
    }
    let scratch = ctx
        .scratch("generate")
        .map_err(|_| refused("output_failed", "Private generation scratch could not be created; the recorded charge is unchanged."))?;
    job.report(0.0, None, Some("Downloading Mirelo's completed outputs"));
    let delivery = client.deliver(vendor, scratch.path(), OUTPUT_BYTES, &job.signal).await.map_err(provider)?;
    if delivery.notes.as_ref().is_some_and(|notes| contains_secret(notes, key)) {
        return Err(refused("output_failed", "The provider returned private credential material in its notes; nothing was published."));
    }
    job.report(0.0, None, Some("Retaining the generated files"));
    // Preserve a previously retained prefix until replacement files are ready. If this attempt
    // publishes nothing, keep the prior artifact references rather than losing the partial result.
    let previous = ["outputs", "artifact_id", "file", "output_type", "media_tag", "notes", "note_count"]
        .map(|field| (field, receipt.get_mut(field).map(Value::take)));
    receipt["outputs"] = json!([]);
    let result = generate_output::retain(ctx, &scratch, delivery, &mut receipt, &job.signal).await;
    if result.is_err() && receipt["outputs"].as_array().is_some_and(Vec::is_empty) {
        for (field, value) in previous {
            if let Some(value) = value {
                receipt[field] = value;
            }
        }
    }
    checkpoint(ctx, job, &receipt).await?;
    result?;
    if contains_secret(&receipt, key) {
        return Err(refused("output_failed", "The generation result cannot contain private credentials."));
    }
    job.report(1.0, Some(1.0), Some("Mirelo output retained"));
    Ok(receipt)
}

async fn follow(ctx: &Ctx, call: Call) -> Result<Value, OpError> {
    let id = call.args.get("job_id").and_then(Value::as_str).unwrap_or("");
    let view = ctx
        .jobs()
        .get(id, &ctx.paths().state_dir)
        .filter(|view| view["job_id"] == id && view["kind"] == "generate")
        .ok_or_else(|| refused("unknown_job", "No retained generation job has that ID."))?;
    if let Some(saved) = view.get("result").filter(|saved| !saved.is_null()) {
        remember(ctx, id, saved)?;
    }
    if view["state"] == "running" {
        return Ok(view);
    }
    let saved = &view["result"];
    let Some(vendor_id) = saved["provider_job_id"].as_str() else {
        return Ok(view);
    };
    let complete = view["state"] == "done" && saved["artifact_id"].is_string();
    if complete && saved["credits"]["actual"].as_u64().is_some() {
        return Ok(view);
    }
    // With no current key, job is a local receipt read. Off gates only new uploads/creates: an
    // existing approved job can still be fetched and settled, never submitted a second time.
    if call.host.private.get("_mirelo").and_then(|value| value["api_key"].as_str()).is_none_or(|key| key.trim().is_empty()) {
        return Ok(view);
    }
    let key = private_key(&call, false)?.to_owned();
    let state = ctx.paths().state_dir.clone();
    let native_id = id.to_owned();
    let (recovery, quote) = tokio::task::spawn_blocking(move || {
        let authority = Authority::read(&state)?;
        let recovery = authority
            .for_job(&native_id)?
            .ok_or_else(|| refused("not_approved", "This job has no matching durable credit reservation."))?;
        let quote = authority.quote(&recovery.quote_id)?.ok_or_else(stale)?;
        Ok::<_, OpError>((recovery, quote))
    })
    .await
    .map_err(|_| refused("ledger_unavailable", "The saved generation authority could not be read."))??;
    if saved["quote_id"] != recovery.quote_id
        || saved["kind"] != recovery.kind
        || saved["provider"] != "mirelo"
        || saved["credits"]["reserved"].as_u64() != Some(recovery.reserved_credits)
        || recovery.provider_job_id.as_deref().is_some_and(|known| known != vendor_id)
        || recovery.settlement == "not_submitted"
    {
        return Err(stale());
    }
    if saved["credits"]["estimate"] != quote["estimate"]["credits"] {
        return Err(stale());
    }
    let task = task(&recovery.kind)?;
    let vendor_id = vendor_id.to_owned();
    let client = client(ctx, &key)?;
    // Billing and partial retained outputs survive every unsuccessful GET or delivery attempt.
    let mut receipt = saved.clone();
    ctx.resume_job(id, "generate", move |ctx, job| async move {
        let fetched = client.get(task, &vendor_id, &job.signal).await;
        let vendor = match fetched {
            Ok(vendor) => vendor,
            Err(error) => {
                checkpoint(&ctx, &job, &receipt).await?;
                return Err(provider(error));
            }
        };
        if let Some(known) = recovery.actual_credits {
            if terminal(&vendor) && vendor.actual_credits().is_some_and(|actual| actual != known) {
                checkpoint(&ctx, &job, &receipt).await?;
                return Err(refused(
                    "stale",
                    "Mirelo reported a different charge for an already settled job; the saved receipt is unchanged.",
                ));
            }
            receipt["credits"]["actual"] = json!(known);
        }
        finish(&ctx, &job, &client, vendor, receipt, &key, complete).await
    })?;
    ctx.jobs().get(id, &ctx.paths().state_dir).ok_or_else(|| refused("unknown_job", "The generation job is no longer retained."))
}

/// Keep the compact billing/output identity receipt, not large notes or absolute MEDIA paths.
/// A successful outcome carries full notes; a restored successful receipt re-resolves its artifact.
async fn checkpoint(ctx: &Ctx, job: &JobHandle, receipt: &Value) -> Result<(), OpError> {
    let compact = receipt
        .as_object()
        .ok_or_else(stale)?
        .iter()
        .map(|(field, value)| (field.clone(), if matches!(field.as_str(), "notes" | "media_tag") { Value::Null } else { value.clone() }))
        .collect::<serde_json::Map<String, Value>>();
    remember(ctx, job.id(), receipt)?;
    job.checkpoint(Value::Object(compact)).await.map_err(|_| {
        refused("checkpoint_failed", "The generation receipt could not be kept durably; no further provider action was taken.")
    })
}
fn restore_media(ctx: &Ctx, receipt: &mut Value) -> Result<(), OpError> {
    let id = receipt["artifact_id"].as_str().ok_or_else(stale)?;
    let path = ctx
        .artifacts()
        .resolve(ctx, id)
        .map_err(|_| refused("output_unavailable", "The retained generation artifact is no longer available."))?;
    let path = path.to_str().filter(|path| !path.chars().any(char::is_control)).ok_or_else(stale)?;
    receipt["media_tag"] = json!(format!("MEDIA:{path}"));
    Ok(())
}
fn terminal(job: &mirelo::Job) -> bool {
    matches!(job.status(), "succeeded" | "partially_succeeded" | "failed" | "canceled" | "cancelled" | "expired" | "errored")
}
fn task(kind: &str) -> Result<Task, OpError> {
    match kind {
        "sfx" => Ok(Task::TextToSfx),
        "audio_to_midi" => Ok(Task::AudioToMidi),
        "video_to_sfx" => Ok(Task::VideoToSfx),
        "extend" => Ok(Task::Extend),
        "inpaint" => Ok(Task::Inpaint),
        _ => Err(refused("invalid_request", "This generation kind is not supported.")),
    }
}
fn client(ctx: &Ctx, key: &str) -> Result<mirelo::Client, OpError> {
    #[cfg(test)]
    if let Some(base) = ctx.answers().get::<FixtureEndpoint>() {
        return mirelo::Client::with_base(&base.0, key).map_err(provider);
    }
    let _ = ctx;
    mirelo::Client::new(key).map_err(provider)
}
#[cfg(test)]
struct FixtureEndpoint(String);
fn contains_secret(value: &Value, key: &str) -> bool {
    let encoded = json!(key).to_string();
    has_secret(value, key, &encoded[1..encoded.len() - 1])
}
fn has_secret(value: &Value, key: &str, encoded: &str) -> bool {
    match value {
        Value::String(text) => text.contains(key) || text.contains(encoded),
        Value::Array(values) => values.iter().any(|value| has_secret(value, key, encoded)),
        Value::Object(values) => {
            values.iter().any(|(field, value)| field.contains(key) || field.contains(encoded) || has_secret(value, key, encoded))
        }
        _ => false,
    }
}
fn now() -> u64 {
    kumi_common::time::now_ms().max(0) as u64
}
fn check(signal: &Signal) -> Result<(), OpError> {
    if signal.is_cancelled() {
        Err(refused("cancelled", "Generation preparation was cancelled."))
    } else {
        Ok(())
    }
}
fn provider(error: mirelo::Error) -> OpError {
    refused("provider_error", &error.to_string())
}
fn not_submitted(error: OpError) -> OpError {
    refused("not_submitted", &format!("No generation was submitted. {}", error.reason()))
}
fn stale() -> OpError {
    refused("stale", "The generation no longer matches its approved quote, source, or receipt.")
}
fn refused(code: &'static str, reason: &str) -> OpError {
    OpError::Refused { code, reason: reason.into(), owner: None }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "generate/schema_tests.rs"]
mod schema_tests;
