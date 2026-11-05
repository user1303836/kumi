use super::*;

fn enable(fixture: &mut Fixture, kind: &str) {
    let enabled = fixture.ledger["policy"]["enabled"].as_object_mut().unwrap();
    for key in ["video_to_sfx", "extend", "inpaint"] {
        enabled.entry(key).or_insert(json!(false));
    }
    fixture.ledger["policy"]["enabled"][kind] = json!(true);
    fixture.save();
}

fn source(path: &Path) -> Value {
    json!({"file": path, "provenance": {"host_tool": "fixture"}})
}

fn audio(seconds: u32) -> Vec<u8> {
    let data = seconds * 16000;
    let mut bytes = wav();
    bytes[4..8].copy_from_slice(&(data + 36).to_le_bytes());
    bytes[40..44].copy_from_slice(&data.to_le_bytes());
    bytes.resize(data as usize + 44, 0);
    // A nonzero last sample detects a truncated upload, not just matching silence.
    *bytes.last_mut().unwrap() = 7;
    bytes
}

fn video() -> Vec<u8> {
    b"\0\0\0\x18ftypisom\0\0\0\0isommp42\0\0\0\x10mdatfixture!".to_vec()
}

fn request(fixture: &Fixture, kind: &str) -> (Value, Vec<u8>) {
    let (name, bytes) = if kind == "video_to_sfx" { ("scene.mp4", video()) } else { ("source.wav", audio(4)) };
    let path = fixture.source.join(name);
    std::fs::write(&path, &bytes).unwrap();
    let mut args = json!({"action": "quote", "kind": kind, "source": source(&path), "prompt": "a soft metallic shimmer"});
    match kind {
        "video_to_sfx" => args["seconds"] = json!(1.2346),
        "extend" => args["append_seconds"] = json!(1.2346),
        "inpaint" => {
            args["region_start_seconds"] = json!(0.1114);
            args["region_end_seconds"] = json!(1.3456);
        }
        _ => panic!("unexpected fixture kind"),
    }
    (args, bytes)
}

fn kinds() -> Vec<&'static str> {
    #[cfg(unix)]
    {
        vec!["video_to_sfx", "extend", "inpaint"]
    }
    #[cfg(not(unix))]
    {
        vec!["extend", "inpaint"]
    }
}

// A selected program, never a PATH search or installed ffmpeg. The tiny ISO header above is
// inspected by the real local_video helper; this stand-in only reports a deterministic probe.
#[cfg(unix)]
fn select_probe(fixture: &mut Fixture, server: &Server) {
    use std::os::unix::fs::PermissionsExt;
    let program = fixture.data.join("selected-video-probe");
    std::fs::write(&program, b"#!/bin/sh\nprintf 'probe\\n' >> \"$0.calls\"\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = '-i' ]; then shift; input=$1; break; fi\n  shift\ndone\nprintf \"Input #0, mov,mp4,m4a,3gp,3g2,mj2, from '%s':\\n  Duration: 00:00:04.00, start: 0.000000, bitrate: 1 kb/s\\n\" \"$input\" >&2\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    fixture.ctx = Fixture::context_with_tools(&fixture.data, &fixture.source, server, BTreeMap::from([("ffmpeg".into(), program)]));
}

fn fixture_for(server: &Server, kind: &str) -> Fixture {
    let mut fixture = Fixture::new(server);
    enable(&mut fixture, kind);
    #[cfg(unix)]
    if kind == "video_to_sfx" {
        select_probe(&mut fixture, server);
    }
    fixture
}

async fn preview(fixture: &Fixture, args: &Value, call: &str) -> Value {
    let mut args = args.clone();
    args["preview"] = json!(true);
    fixture.call(args, call, None).await.unwrap()
}

fn route(kind: &str) -> &'static str {
    match kind {
        "video_to_sfx" => "/v3/video-to-sfx/generations",
        "extend" => "/v3/extend/edits",
        "inpaint" => "/v3/inpaint/edits",
        _ => panic!("unexpected fixture kind"),
    }
}

fn assert_window(value: &Value, kind: &str) {
    assert_eq!(value["kind"], kind);
    assert_eq!(value["source_seconds"], 4.0, "{value}");
    assert_eq!(value["seconds"], 1.235, "{value}");
    assert_eq!(value["model"], if kind == "video_to_sfx" { "sfx-2.0" } else { "sfx-1.6" });
    match kind {
        "extend" => assert_eq!(value["append_seconds"], 1.235),
        "inpaint" => {
            assert_eq!(value["region_start_seconds"], 0.111);
            assert_eq!(value["region_end_seconds"], 1.346);
        }
        _ => {}
    }
}

fn uploaded_file(body: &[u8]) -> &[u8] {
    let first_line = body.windows(2).position(|bytes| bytes == b"\r\n").unwrap();
    let boundary = &body[..first_line];
    let marker = b"name=\"file\"";
    let field = body.windows(marker.len()).position(|bytes| bytes == marker).unwrap();
    let start = field + body[field..].windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap() + 4;
    let mut ending = b"\r\n".to_vec();
    ending.extend_from_slice(boundary);
    let end = start + body[start..].windows(ending.len()).position(|bytes| bytes == ending).unwrap();
    &body[start..end]
}

#[tokio::test]
async fn new_tasks_quote_create_and_job_keep_normalized_windows_and_send_the_exact_prepared_body() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for kind in kinds() {
                let server = Server::new().await;
                let mut fixture = fixture_for(&server, kind);
                let (args, bytes) = request(&fixture, kind);
                let artifacts = fixture.ctx.paths().state_dir.join("artifacts");
                let before = std::fs::read_dir(&artifacts).unwrap().count();
                let approval = preview(&fixture, &args, "quote-call").await;
                assert_eq!(approval["upload"]["identity_sha256"], format!("{:x}", Sha256::digest(&bytes)));
                assert_eq!(approval["upload"]["bytes"], bytes.len());
                assert!(server.requests.lock().unwrap().is_empty());
                assert_eq!(std::fs::read_dir(&artifacts).unwrap().count(), before);
                assert!(!fixture.data.join("selected-video-probe.calls").exists());
                let quote = fixture.call(args.clone(), "quote-call", None).await.unwrap();
                if kind == "video_to_sfx" {
                    assert_eq!(std::fs::read(fixture.data.join("selected-video-probe.calls")).unwrap(), b"probe\n");
                }
                assert_window(&quote, kind);
                assert_eq!(quote["file"], if kind == "video_to_sfx" { "scene.mp4" } else { "source.wav" });
                assert!(!quote.to_string().contains(fixture.source.to_str().unwrap()));
                assert_eq!(server.creates(), 0);
                fixture.reserve(&quote, "r-edit", "create-call");
                let created = fixture
                    .call(json!({"action":"create", "quote_id":quote["quote_id"]}), "create-call", Some(("r-edit", 15)))
                    .await
                    .unwrap();
                assert_window(&created, kind);
                assert!(created.get("file").is_none());
                let id = created["job_id"].as_str().unwrap();
                let job = fixture.done(id).await;
                assert_eq!(job["state"], "done", "{job}");
                assert_window(&job["result"], kind);
                let output = fixture.ctx.artifacts().resolve(&fixture.ctx, job["result"]["artifact_id"].as_str().unwrap()).unwrap();
                assert_eq!(std::fs::read(&output).unwrap(), wav());
                assert_eq!(job["result"]["file"], output.file_name().unwrap().to_str().unwrap());
                assert_eq!(job["result"]["credits"], json!({"estimate":15, "reserved":15, "actual":12}));
                assert_eq!(fixture.call(json!({"action":"job", "job_id":id}), "job-call", None).await.unwrap(), job);
                assert_eq!(std::fs::read(args["source"]["file"].as_str().unwrap()).unwrap(), bytes);
                let requests = server.requests.lock().unwrap();
                let models = requests.iter().position(|r| r.path == "/v3/models").unwrap();
                let ticket = requests.iter().position(|r| r.path == "/v3/assets").unwrap();
                assert!(models < ticket);
                assert_eq!(
                    serde_json::from_slice::<Value>(&requests[ticket].body).unwrap(),
                    json!({"content_type": if kind == "video_to_sfx" { "video/mp4" } else { "audio/wav" }})
                );
                let uploads: Vec<_> = requests.iter().filter(|r| r.path == "/put").collect();
                assert_eq!(uploads.len(), 1);
                assert_eq!(uploads[0].method, "POST");
                assert_eq!(uploaded_file(&uploads[0].body), bytes);
                let preflight = requests.iter().find(|r| r.path == format!("{}/preflight", route(kind))).unwrap();
                let create = requests.iter().find(|r| r.path == format!("{}?wait=25", route(kind))).unwrap();
                assert_eq!(preflight.body, create.body);
                let body: Value = serde_json::from_slice(&create.body).unwrap();
                let mut expected = json!({"model": if kind == "video_to_sfx" { "sfx-2.0" } else { "sfx-1.6" },
                "input": {"prompt":"a soft metallic shimmer"}, "output":{"format":"wav"}});
                expected["input"][if kind == "video_to_sfx" { "video" } else { "audio" }] = json!({"type":"asset", "id":"approved-source"});
                match kind {
                    "video_to_sfx" => {
                        expected["duration_ms"] = json!(1235);
                        expected["controls"] = json!({"preserve_speech":false});
                    }
                    "extend" => expected["append_duration_ms"] = json!(1235),
                    "inpaint" => expected["region"] = json!({"start_ms":111, "end_ms":1346}),
                    _ => unreachable!(),
                }
                assert_eq!(body, expected);
                drop(requests);
                assert_eq!(server.creates(), 1);
                fixture.assert_private();
            }
        })
        .await;
}

#[tokio::test]
async fn each_new_task_needs_its_own_stored_and_private_consent_before_even_previewing() {
    for kind in ["video_to_sfx", "extend", "inpaint"] {
        let server = Server::new().await;
        let mut fixture = Fixture::new(&server);
        let (mut args, _) = request(&fixture, kind);
        args["preview"] = json!(true);
        // Legacy task consent is already On, but absent new-task keys are Off.
        assert_eq!(fixture.call(args.clone(), "off-call", None).await.unwrap_err().code(), "not_enabled");
        enable(&mut fixture, kind);
        for (ack, enabled) in [(false, true), (true, false)] {
            assert_eq!(fixture.call_private(args.clone(), "private-off", None, ack, enabled).await.unwrap_err().code(), "not_enabled");
        }
        fixture.ledger["policy"]["acknowledged"] = json!(false);
        fixture.save();
        assert_eq!(fixture.call(args.clone(), "disclosure-off", None).await.unwrap_err().code(), "not_approved");
        fixture.ledger["policy"]["acknowledged"] = json!(true);
        fixture.ledger["policy"]["enabled"][kind] = json!(false);
        fixture.save();
        assert_eq!(fixture.call(args.clone(), "stored-off", None).await.unwrap_err().code(), "not_enabled");
        args.as_object_mut().unwrap().remove("preview");
        assert_eq!(fixture.call(args, "upload-off", None).await.unwrap_err().code(), "not_enabled");
        assert!(server.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn source_approval_is_bound_to_both_task_and_host_call_before_any_upload() {
    for kind in ["video_to_sfx", "extend", "inpaint"] {
        let server = Server::new().await;
        let mut fixture = Fixture::new(&server);
        for allowed in ["video_to_sfx", "extend", "inpaint"] {
            enable(&mut fixture, allowed);
        }
        let (args, _) = request(&fixture, kind);
        preview(&fixture, &args, "approved-call").await;
        assert_eq!(fixture.call(args.clone(), "other-call", None).await.unwrap_err().code(), "stale");
        let mut other = json!({"action":"quote", "kind": if kind == "inpaint" { "extend" } else { "inpaint" }, "source":args["source"]});
        if kind == "inpaint" {
            other["append_seconds"] = json!(1);
        } else {
            other["region_start_seconds"] = json!(0);
            other["region_end_seconds"] = json!(1);
        }
        let mut other_preview = other.clone();
        other_preview["preview"] = json!(true);
        assert_eq!(fixture.call(other_preview, "approved-call", None).await.unwrap_err().code(), "stale");
        assert_eq!(fixture.call(other, "approved-call", None).await.unwrap_err().code(), "stale");
        assert!(server.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn invalid_overrun_and_rounded_zero_windows_never_request_an_asset() {
    for kind in kinds() {
        let server = Server::new().await;
        let fixture = fixture_for(&server, kind);
        let (base, _) = request(&fixture, kind);
        let windows = match kind {
            "video_to_sfx" => vec![json!({"seconds":0.0004}), json!({"seconds":4.001}), json!({"seconds":-1})],
            "extend" => vec![json!({"append_seconds":0.0004}), json!({"append_seconds":-1}), json!({"append_seconds":31})],
            "inpaint" => vec![
                json!({"region_start_seconds":2, "region_end_seconds":1}),
                json!({"region_start_seconds":0.1111, "region_end_seconds":0.1114}),
                json!({"region_start_seconds":0, "region_end_seconds":4.001}),
                json!({"region_start_seconds":-1, "region_end_seconds":1}),
                json!({"region_start_seconds":0, "region_end_seconds":0.0004}),
            ],
            _ => unreachable!(),
        };
        for (index, window) in windows.into_iter().enumerate() {
            let id = format!("invalid-{index}");
            // The hash approval itself has no duration/decoder prerequisite.
            preview(&fixture, &base, &id).await;
            let mut args = base.clone();
            args.as_object_mut().unwrap().extend(window.as_object().unwrap().clone());
            assert!(fixture.call(args, &id, None).await.is_err());
        }
        assert!(server.requests.lock().unwrap().iter().all(|r| r.method == "GET" && r.path == "/v3/models"));
    }
}

#[tokio::test]
async fn extension_does_not_round_a_short_source_up_to_its_three_second_minimum() {
    let server = Server::new().await;
    let fixture = fixture_for(&server, "extend");
    let mut bytes = audio(3);
    bytes.truncate(bytes.len() - 2); // One 8 kHz frame short, but ceil(milliseconds) is 3000.
    let data = bytes.len() as u32 - 44;
    bytes[4..8].copy_from_slice(&(data + 36).to_le_bytes());
    bytes[40..44].copy_from_slice(&data.to_le_bytes());
    let path = fixture.source.join("short.wav");
    std::fs::write(&path, bytes).unwrap();
    let args = json!({"action":"quote", "kind":"extend", "source":source(&path), "append_seconds":0.5});
    preview(&fixture, &args, "short").await;
    assert_eq!(fixture.call(args, "short", None).await.unwrap_err().code(), "invalid_source");
    assert!(server.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn extending_past_sixty_combined_seconds_keeps_the_whole_source_and_full_append() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = Server::new().await;
            let mut fixture = fixture_for(&server, "extend");
            let bytes = audio(59);
            let path = fixture.source.join("long.wav");
            std::fs::write(&path, &bytes).unwrap();
            let args = json!({"action":"quote", "kind":"extend", "source":source(&path), "append_seconds":2.3456});
            preview(&fixture, &args, "quote-call").await;
            let quote = fixture.call(args, "quote-call", None).await.unwrap();
            assert_eq!(quote["source_seconds"], 59.0);
            assert_eq!(quote["seconds"], 2.346);
            assert_eq!(quote["append_seconds"], 2.346);
            fixture.reserve(&quote, "r-long", "create-call");
            let created =
                fixture.call(json!({"action":"create", "quote_id":quote["quote_id"]}), "create-call", Some(("r-long", 15))).await.unwrap();
            let job = fixture.done(created["job_id"].as_str().unwrap()).await;
            assert_eq!(job["state"], "done", "{job}");
            assert_eq!(job["result"]["source_seconds"], 59.0);
            assert_eq!(job["result"]["append_seconds"], 2.346);
            let requests = server.requests.lock().unwrap();
            assert_eq!(uploaded_file(&requests.iter().find(|r| r.path == "/put").unwrap().body), bytes);
            let create = requests.iter().find(|r| r.path == "/v3/extend/edits?wait=25").unwrap();
            assert_eq!(serde_json::from_slice::<Value>(&create.body).unwrap()["append_duration_ms"], 2346);
        })
        .await;
}

#[tokio::test]
async fn video_hash_preview_needs_no_ffmpeg_but_upload_does() {
    let server = Server::new().await;
    let mut fixture = Fixture::new(&server);
    enable(&mut fixture, "video_to_sfx");
    let (args, bytes) = request(&fixture, "video_to_sfx");
    let approval = preview(&fixture, &args, "quote-call").await;
    assert_eq!(approval["upload"]["identity_sha256"], format!("{:x}", Sha256::digest(&bytes)));
    assert_eq!(fixture.call(args.clone(), "quote-call", None).await.unwrap_err().code(), "invalid_source");
    fixture.ctx = Fixture::context_with_tools(
        &fixture.data,
        &fixture.source,
        &server,
        BTreeMap::from([("ffmpeg".into(), fixture.data.join("missing-selected-ffmpeg"))]),
    );
    preview(&fixture, &args, "missing-program").await;
    assert_eq!(fixture.call(args, "missing-program", None).await.unwrap_err().code(), "invalid_source");
    assert!(server.requests.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn selected_probe_cannot_turn_a_mismatched_video_header_into_an_upload() {
    let server = Server::new().await;
    let fixture = fixture_for(&server, "video_to_sfx");
    let (args, _) = request(&fixture, "video_to_sfx");
    let bytes = wav();
    std::fs::write(args["source"]["file"].as_str().unwrap(), &bytes).unwrap();
    let approval = preview(&fixture, &args, "quote-call").await;
    assert_eq!(approval["upload"]["identity_sha256"], format!("{:x}", Sha256::digest(&bytes)));
    assert_eq!(fixture.call(args, "quote-call", None).await.unwrap_err().code(), "invalid_source");
    assert!(server.requests.lock().unwrap().is_empty());
    assert!(!fixture.data.join("selected-video-probe.calls").exists());
}

#[tokio::test]
async fn edit_metadata_survives_restart_and_get_only_recovery_with_all_switches_off_and_unknown_billing() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for kind in kinds() {
                let server = Server::new().await;
                server.download_fails.store(true, Ordering::SeqCst);
                server.unknown_credits.store(true, Ordering::SeqCst);
                let mut fixture = fixture_for(&server, kind);
                let (args, _) = request(&fixture, kind);
                preview(&fixture, &args, "quote-call").await;
                let quote = fixture.call(args, "quote-call", None).await.unwrap();
                fixture.reserve(&quote, "r-recover", "create-call");
                let created = fixture
                    .call(json!({"action":"create", "quote_id":quote["quote_id"]}), "create-call", Some(("r-recover", 15)))
                    .await
                    .unwrap();
                let id = created["job_id"].as_str().unwrap();
                let failed = fixture.done(id).await;
                assert_eq!(failed["state"], "failed");
                assert_window(&failed["result"], kind);
                assert_eq!(failed["result"]["credits"]["actual"], Value::Null);
                fixture.ledger["reservations"]["r-recover"]["job_id"] = json!(id);
                fixture.ledger["reservations"]["r-recover"]["provider_job_id"] = json!("sfx-job");
                fixture.ledger["reservations"]["r-recover"]["submission"] = json!("submitted");
                fixture.ledger["policy"]["acknowledged"] = json!(false);
                for key in ["sfx", "audio_to_midi", "video_to_sfx", "extend", "inpaint"] {
                    fixture.ledger["policy"]["enabled"][key] = json!(false);
                }
                fixture.save();
                fixture = fixture.restart(&server);
                server.download_fails.store(false, Ordering::SeqCst);
                let before = server.requests.lock().unwrap().len();
                fixture.call_private(json!({"action":"job", "job_id":id}), "recover-call", None, false, false).await.unwrap();
                let recovered = fixture.done(id).await;
                assert_eq!(recovered["state"], "done", "{recovered}");
                assert_eq!(recovered["job_id"], id);
                assert_window(&recovered["result"], kind);
                assert_eq!(recovered["result"]["credits"], json!({"estimate":15, "reserved":15, "actual":null}));
                let output = fixture.ctx.artifacts().resolve(&fixture.ctx, recovered["result"]["artifact_id"].as_str().unwrap()).unwrap();
                assert_eq!(std::fs::read(&output).unwrap(), wav());
                assert_eq!(recovered["result"]["file"], output.file_name().unwrap().to_str().unwrap());
                let requests = server.requests.lock().unwrap();
                assert!(requests[before..].iter().all(|r| r.method == "GET"));
                assert!(requests[before..].iter().any(|r| r.path == format!("{}/sfx-job", route(kind))));
                drop(requests);
                assert_eq!(server.creates(), 1);
            }
        })
        .await;
}

#[tokio::test]
async fn shared_video_resolution_preserves_audio_only_import_and_current_root_regular_file_guards() {
    let server = Server::new().await;
    let mut fixture = Fixture::new(&server);
    enable(&mut fixture, "video_to_sfx");
    let (args, _) = request(&fixture, "video_to_sfx");
    preview(&fixture, &args, "valid-video").await;
    let error = crate::serve::import::find(&fixture.ctx, &args["source"], &Signal::new()).await.unwrap_err();
    assert_eq!(error.code(), "not_allowed");
    let outside = tempfile::tempdir().unwrap();
    for (kind, extension, bytes) in [("video_to_sfx", "mp4", video()), ("extend", "wav", audio(4))] {
        enable(&mut fixture, kind);
        let unapproved = outside.path().join(format!("outside.{extension}"));
        std::fs::write(&unapproved, &bytes).unwrap();
        let directory = fixture.source.join(format!("directory.{extension}"));
        std::fs::create_dir(&directory).unwrap();
        for (index, path) in [unapproved, directory].into_iter().enumerate() {
            let args = json!({"action":"quote", "kind":kind, "source":source(&path), "preview":true});
            assert!(fixture.call(args.clone(), &format!("{kind}-{index}"), None).await.is_err());
            assert!(crate::serve::import::find(&fixture.ctx, &args["source"], &Signal::new()).await.is_err());
        }
    }
    assert!(server.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn upload_quote_deadline_cancels_an_explicitly_held_upload_without_creating_a_quote_or_job() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = Server::new().await;
            server.hold_upload.store(true, Ordering::SeqCst);
            let fixture = Rc::new(fixture_for(&server, "extend"));
            let (args, _) = request(&fixture, "extend");
            preview(&fixture, &args, "quote-call").await;
            let worker = fixture.clone();
            let quote = tokio::task::spawn_local(async move { worker.call(args, "quote-call", None).await });
            tokio::time::timeout(Duration::from_secs(10), server.upload_held.notified()).await.unwrap();
            // Start manual time only after actual HTTP bytes reached the held transfer. This avoids
            // auto-advance racing source hashing, process startup, or socket readiness.
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(181)).await;
            let error = quote.await.unwrap().unwrap_err();
            tokio::time::resume();
            assert_eq!(error.code(), "quote_timeout");
            assert!(fixture.ctx.generation().pending.borrow().quotes.is_empty());
            assert_eq!(server.creates(), 0);
            assert!(!server.requests.lock().unwrap().iter().any(|r| r.path.ends_with("/preflight")));
        })
        .await;
}

#[tokio::test]
async fn switching_a_new_task_off_after_quote_refuses_create_without_any_further_post() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for kind in kinds() {
                for stored_off in [true, false] {
                    let server = Server::new().await;
                    let mut fixture = fixture_for(&server, kind);
                    let (args, _) = request(&fixture, kind);
                    preview(&fixture, &args, "quote-call").await;
                    let quote = fixture.call(args, "quote-call", None).await.unwrap();
                    fixture.reserve(&quote, "r-off", "create-call");
                    if stored_off {
                        fixture.ledger["policy"]["enabled"][kind] = json!(false);
                        fixture.save();
                    }
                    let before = server.requests.lock().unwrap().len();
                    let error = fixture
                        .call_private(
                            json!({"action":"create", "quote_id":quote["quote_id"]}),
                            "create-call",
                            Some(("r-off", 15)),
                            true,
                            stored_off,
                        )
                        .await
                        .unwrap_err();
                    assert_eq!(error.code(), "not_submitted");
                    assert_eq!(server.requests.lock().unwrap().len(), before);
                    assert_eq!(server.creates(), 0);
                }
            }
        })
        .await;
}
