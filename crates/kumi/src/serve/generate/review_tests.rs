use super::*;

async fn ordinary_jobs(fixture: &Fixture) {
    for _ in 0..101 {
        fixture.ctx.start_job("scan", |_, _| async { Ok(json!({"sounds": 0})) }).unwrap();
        fixture.ctx.jobs().all_settled().await;
    }
    fixture.ctx.snapshots().flush();
}

#[tokio::test]
async fn an_unfollowed_overrun_blocks_the_next_post_until_settled_and_explicitly_unfrozen() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = Server::new().await;
            server.actual_credits.store(16, Ordering::SeqCst);
            let mut fixture = Fixture::new(&server);
            let first = fixture.call(sfx(), "quote-one", None).await.unwrap();
            fixture.reserve(&first, "r-one", "create-one");
            let started =
                fixture.call(json!({"action":"create", "quote_id":first["quote_id"]}), "create-one", Some(("r-one", 15))).await.unwrap();
            let id = started["job_id"].as_str().unwrap();
            assert_eq!(fixture.done(id).await["result"]["credits"]["actual"], 16);
            // No host follow/reconcile occurred. Restart must still see the native receipt.
            fixture = fixture.restart(&server);
            let second = fixture.call(sfx(), "quote-two", None).await.unwrap();
            fixture.reserve(&second, "r-two", "create-two");
            let denied = fixture
                .call(json!({"action":"create", "quote_id":second["quote_id"]}), "create-two", Some(("r-two", 15)))
                .await
                .unwrap_err();
            assert_eq!(denied.code(), "not_submitted");
            assert_eq!(server.creates(), 1);
            // Model-side flags cannot clear this. These changes stand in for the ledger's
            // receipt reconciliation and the producer's separately approved unfreeze.
            let first_row = &mut fixture.ledger["reservations"]["r-one"];
            first_row["job_id"] = json!(id);
            first_row["provider_job_id"] = json!("sfx-job");
            first_row["submission"] = json!("submitted");
            first_row["settlement"] = json!("charged");
            first_row["actual_credits"] = json!(16);
            fixture.ledger["reservations"]["r-two"]["submission"] = json!("not_submitted");
            fixture.ledger["reservations"]["r-two"]["settlement"] = json!("not_submitted");
            fixture.ledger["frozen"] = json!(true);
            fixture.ledger["freeze_reason"] = json!("actual_exceeded_estimate");
            fixture.save();
            let third = fixture.call(sfx(), "quote-three", None).await.unwrap();
            fixture.reserve(&third, "r-three", "create-three");
            let denied = fixture
                .call(json!({"action":"create", "quote_id":third["quote_id"]}), "create-three", Some(("r-three", 15)))
                .await
                .unwrap_err();
            assert_eq!(denied.code(), "not_submitted");
            assert_eq!(server.creates(), 1);
            fixture.ledger["reservations"]["r-three"]["submission"] = json!("not_submitted");
            fixture.ledger["reservations"]["r-three"]["settlement"] = json!("not_submitted");
            fixture.ledger["frozen"] = json!(false);
            fixture.ledger["freeze_reason"] = Value::Null;
            fixture.save();
            server.actual_credits.store(12, Ordering::SeqCst);
            let fourth = fixture.call(sfx(), "quote-four", None).await.unwrap();
            fixture.reserve(&fourth, "r-four", "create-four");
            let started =
                fixture.call(json!({"action":"create", "quote_id":fourth["quote_id"]}), "create-four", Some(("r-four", 15))).await.unwrap();
            assert_eq!(fixture.done(started["job_id"].as_str().unwrap()).await["state"], "done");
            assert_eq!(server.creates(), 2);
        })
        .await;
}

#[tokio::test]
async fn a_freeze_between_create_and_dispatch_keeps_a_not_submitted_receipt() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = Server::new().await;
            let mut fixture = Fixture::new(&server);
            let quote = fixture.call(sfx(), "quote-call", None).await.unwrap();
            fixture.reserve(&quote, "r-one", "create-call");
            let started =
                fixture.call(json!({"action":"create", "quote_id":quote["quote_id"]}), "create-call", Some(("r-one", 15))).await.unwrap();
            // start_job does not synchronously poll its worker; no yield before the policy change.
            fixture.ledger["frozen"] = json!(true);
            fixture.ledger["freeze_reason"] = json!("actual_exceeded_estimate");
            fixture.save();
            let job = fixture.done(started["job_id"].as_str().unwrap()).await;
            assert_eq!(job["state"], "failed");
            assert_eq!(job["error"]["code"], "not_submitted");
            assert_eq!(job["result"]["submission"], "not_submitted");
            assert_eq!(job["result"]["credits"]["actual"], Value::Null);
            assert_eq!(server.creates(), 0);
        })
        .await;
}

#[tokio::test]
async fn an_unreadable_ledger_does_not_consume_the_opaque_prepared_request() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = Server::new().await;
            let mut fixture = Fixture::new(&server);
            let quote = fixture.call(sfx(), "quote-call", None).await.unwrap();
            fixture.reserve(&quote, "r-one", "create-call");
            std::fs::write(fixture.data.join("ledger.json"), b"broken ledger").unwrap();
            let args = json!({"action":"create", "quote_id":quote["quote_id"]});
            assert_eq!(fixture.call(args.clone(), "create-call", Some(("r-one", 15))).await.unwrap_err().code(), "ledger_unavailable");
            assert_eq!(server.creates(), 0);
            fixture.save();
            let started = fixture.call(args, "create-call", Some(("r-one", 15))).await.unwrap();
            assert_eq!(fixture.done(started["job_id"].as_str().unwrap()).await["state"], "done");
            assert_eq!(server.creates(), 1);
        })
        .await;
}

#[tokio::test]
async fn corrupt_unrelated_history_does_not_block_preview_or_get_recovery_but_blocks_creates() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = Server::new().await;
            server.download_fails.store(true, Ordering::SeqCst);
            let mut fixture = Fixture::new(&server);
            let quote = fixture.call(sfx(), "quote-one", None).await.unwrap();
            fixture.reserve(&quote, "r-one", "create-one");
            let started =
                fixture.call(json!({"action":"create", "quote_id":quote["quote_id"]}), "create-one", Some(("r-one", 15))).await.unwrap();
            let id = started["job_id"].as_str().unwrap();
            let failed = fixture.done(id).await;
            assert_eq!(failed["state"], "failed");
            assert_eq!(failed["result"]["credits"]["actual"], 12);
            fixture.ledger["reservations"]["r-one"]["job_id"] = json!(id);
            fixture.ledger["reservations"]["r-one"]["provider_job_id"] = json!("sfx-job");
            fixture.ledger["reservations"]["r-one"]["submission"] = json!("submitted");
            fixture.save();
            let unsent = fixture.call(sfx(), "quote-unsent", None).await.unwrap();
            fixture.reserve(&unsent, "r-unsent", "create-unsent");
            let corrupt = fixture.ctx.paths().state_dir.join("jobs/j1-1.json");
            std::fs::write(&corrupt, b"broken unrelated job").unwrap();
            fixture = fixture.restart(&server);
            assert!(corrupt.is_file());
            let requests = server.requests.lock().unwrap().len();
            let artifacts = std::fs::read_dir(fixture.ctx.paths().state_dir.join("artifacts")).unwrap().count();
            let source = fixture.source.join("melody.wav");
            std::fs::write(&source, wav()).unwrap();
            let preview = fixture
                .call(
                    json!({"action":"quote", "kind":"audio_to_midi", "preview":true,
                        "source":{"file":source, "provenance":{"host_tool":"fixture"}}}),
                    "preview-call",
                    None,
                )
                .await
                .unwrap();
            assert_eq!(preview["upload"]["bytes"], wav().len());
            assert_eq!(server.requests.lock().unwrap().len(), requests);
            assert_eq!(std::fs::read_dir(fixture.ctx.paths().state_dir.join("artifacts")).unwrap().count(), artifacts);
            assert!(!fixture.ctx.generation().records.borrow().loaded().unwrap());

            server.download_fails.store(false, Ordering::SeqCst);
            fixture.call(json!({"action":"job", "job_id":id}), "recover-call", None).await.unwrap();
            let recovered = fixture.done(id).await;
            assert_eq!(recovered["state"], "done");
            assert_eq!(recovered["result"]["credits"]["actual"], 12);
            assert!(!fixture.ctx.generation().records.borrow().loaded().unwrap());
            assert!(server.requests.lock().unwrap()[requests..].iter().all(|request| request.method == "GET"));
            assert_eq!(server.creates(), 1);

            // Incomplete history cannot prove that a missing prepared quote was never submitted.
            let missing = fixture
                .call(json!({"action":"create", "quote_id":unsent["quote_id"]}), "create-unsent", Some(("r-unsent", 15)))
                .await
                .unwrap_err();
            assert_eq!(missing.code(), "ledger_unavailable");
            let next = fixture.call(sfx(), "quote-next", None).await.unwrap();
            fixture.reserve(&next, "r-next", "create-next");
            let denied = fixture
                .call(json!({"action":"create", "quote_id":next["quote_id"]}), "create-next", Some(("r-next", 15)))
                .await
                .unwrap_err();
            assert_eq!(denied.code(), "ledger_unavailable");
            assert_eq!(server.creates(), 1);
            assert!(!fixture.ctx.generation().records.borrow().loaded().unwrap());
        })
        .await;
}

#[tokio::test]
async fn an_incomplete_live_index_never_proves_absence_or_no_unsettled_overrun() {
    let server = Server::new().await;
    let mut fixture = Fixture::new(&server);
    let quote = fixture.call(sfx(), "quote-call", None).await.unwrap();
    fixture.reserve(&quote, "r-one", "create-call");
    let held = Authority::read(&fixture.ctx.paths().state_dir).unwrap();
    assert_eq!(no_unsettled_overrun(&fixture.ctx, &held).unwrap_err().code(), "ledger_unavailable");
    let reservation = Reservation { id: "r-one".into(), amount: 15, call: "create-call".into() };
    let id = quote["quote_id"].as_str().unwrap();
    assert_eq!(missing_quote(&fixture.ctx, id, &reservation).await.unwrap_err().code(), "ledger_unavailable");
    // Remembering one unrelated live receipt still does not prove a full historical scan.
    let mut receipt = generate_output::receipt(&quote, 15, None, "not_submitted", None).unwrap();
    receipt["quote_id"] = json!("q-00000000000000000000000000000001");
    remember(&fixture.ctx, "j1-1", &receipt).unwrap();
    assert_eq!(no_unsettled_overrun(&fixture.ctx, &held).unwrap_err().code(), "ledger_unavailable");
    assert_eq!(missing_quote(&fixture.ctx, id, &reservation).await.unwrap_err().code(), "ledger_unavailable");
    assert_eq!(server.creates(), 0);
}

#[tokio::test]
async fn restart_proves_an_owned_unsent_quote_but_never_releases_a_lost_post() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for sent in [false, true] {
                let server = Server::new().await;
                let mut fixture = Fixture::new(&server);
                let quote = fixture.call(sfx(), "quote-call", None).await.unwrap();
                fixture.reserve(&quote, "r-one", "create-call");
                let args = json!({"action":"create", "quote_id":quote["quote_id"]});
                if sent {
                    server.lose_create.store(true, Ordering::SeqCst);
                    let started = fixture.call(args.clone(), "create-call", Some(("r-one", 15))).await.unwrap();
                    assert_eq!(fixture.done(started["job_id"].as_str().unwrap()).await["result"]["submission"], "unknown");
                }
                fixture = fixture.restart(&server);
                let error = fixture.call(args, "create-call", Some(("r-one", 15))).await.unwrap_err();
                assert_eq!(error.code(), if sent { "quote_consumed" } else { "not_submitted" });
                assert_eq!(server.creates(), usize::from(sent));
            }
        })
        .await;
}

#[tokio::test]
async fn private_consent_cannot_be_replaced_by_the_stored_enabled_policy() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for (ack, enabled) in [(false, true), (true, false)] {
                let server = Server::new().await;
                let mut fixture = Fixture::new(&server);
                assert_eq!(fixture.call_private(sfx(), "denied-quote", None, ack, enabled).await.unwrap_err().code(), "not_enabled");
                assert!(server.requests.lock().unwrap().is_empty());
                let quote = fixture.call(sfx(), "quote-call", None).await.unwrap();
                fixture.reserve(&quote, "r-one", "create-call");
                assert_eq!(
                    fixture
                        .call_private(
                            json!({"action":"create", "quote_id":quote["quote_id"]}),
                            "create-call",
                            Some(("r-one", 15)),
                            ack,
                            enabled
                        )
                        .await
                        .unwrap_err()
                        .code(),
                    "not_submitted"
                );
                assert_eq!(server.creates(), 0);
            }
        })
        .await;
}

#[tokio::test]
async fn no_key_returns_a_recoverable_saved_receipt_without_network() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = Server::new().await;
            server.download_fails.store(true, Ordering::SeqCst);
            let mut fixture = Fixture::new(&server);
            let quote = fixture.call(sfx(), "quote-call", None).await.unwrap();
            fixture.reserve(&quote, "r-one", "create-call");
            let started =
                fixture.call(json!({"action":"create", "quote_id":quote["quote_id"]}), "create-call", Some(("r-one", 15))).await.unwrap();
            let id = started["job_id"].as_str().unwrap();
            let saved = fixture.done(id).await;
            assert_eq!(saved["state"], "failed");
            let requests = server.requests.lock().unwrap().len();
            let read = run(
                &fixture.ctx,
                Call {
                    args: json!({"action":"job", "job_id":id}).as_object().unwrap().clone(),
                    host: Host::default(),
                    signal: Signal::new(),
                    progress: Progress::none(),
                },
            )
            .await
            .unwrap();
            assert_eq!(read, saved);
            assert_eq!(read["result"]["credits"]["actual"], 12);
            assert_eq!(server.requests.lock().unwrap().len(), requests);
        })
        .await;
}

#[tokio::test]
async fn manual_release_and_job_pruning_cannot_remove_get_only_recovery() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = Server::new().await;
            server.download_fails.store(true, Ordering::SeqCst);
            let mut fixture = Fixture::new(&server);
            let quote = fixture.call(sfx(), "quote-call", None).await.unwrap();
            fixture.reserve(&quote, "r-one", "create-call");
            let started =
                fixture.call(json!({"action":"create", "quote_id":quote["quote_id"]}), "create-call", Some(("r-one", 15))).await.unwrap();
            let id = started["job_id"].as_str().unwrap();
            assert_eq!(fixture.done(id).await["state"], "failed");
            fixture.ledger["reservations"]["r-one"]["job_id"] = json!(id);
            fixture.ledger["reservations"]["r-one"]["settlement"] = json!("manual_release");
            fixture.ledger["reservations"]["r-one"]["manually_released"] = json!(true);
            fixture.save();
            let path = fixture.ctx.paths().state_dir.join("jobs").join(format!("{id}.json"));
            File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1)))
                .unwrap();
            ordinary_jobs(&fixture).await;
            assert!(path.is_file());
            fixture = fixture.restart(&server);
            assert!(path.is_file());
            server.download_fails.store(false, Ordering::SeqCst);
            fixture.call(json!({"action":"job", "job_id":id}), "late-recovery", None).await.unwrap();
            let recovered = fixture.done(id).await;
            assert_eq!(recovered["state"], "done");
            assert_eq!(recovered["result"]["credits"]["actual"], 12);
            let artifact = fixture.ctx.artifacts().resolve(&fixture.ctx, recovered["result"]["artifact_id"].as_str().unwrap()).unwrap();
            assert_eq!(std::fs::read(artifact).unwrap(), wav());
            assert_eq!(server.creates(), 1);
            // No host reconciliation has booked this late charge yet. The released row holds
            // zero, so even 12 actual against its original 15 approval must block another POST.
            assert_eq!(fixture.ledger["reservations"]["r-one"]["reserved_credits"], 15);
            assert_eq!(fixture.ledger["reservations"]["r-one"]["actual_credits"], Value::Null);
            let next = fixture.call(sfx(), "quote-next", None).await.unwrap();
            fixture.reserve(&next, "r-next", "create-next");
            let denied = fixture
                .call(json!({"action":"create", "quote_id":next["quote_id"]}), "create-next", Some(("r-next", 15)))
                .await
                .unwrap_err();
            assert_eq!(denied.code(), "not_submitted");
            assert!(denied.reason().contains("ledger_frozen"));
            assert_eq!(server.creates(), 1);
        })
        .await;
}

#[tokio::test]
async fn missing_expired_different_source_and_wrong_call_previews_are_stale_without_upload() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for scenario in ["missing", "expired", "different", "wrong-call"] {
                let server = Server::new().await;
                let fixture = Fixture::new(&server);
                let path = fixture.source.join("melody.wav");
                std::fs::write(&path, wav()).unwrap();
                let mut quote =
                    json!({"action":"quote", "kind":"audio_to_midi", "source":{"file":path, "provenance":{"host_tool":"fixture"}}});
                if scenario != "missing" {
                    let mut preview = quote.clone();
                    preview["preview"] = json!(true);
                    fixture.call(preview, "quote-call", None).await.unwrap();
                }
                if scenario == "expired" {
                    for (expires, _) in fixture.ctx.generation().pending.borrow_mut().previews.values_mut() {
                        *expires = 0;
                    }
                } else if scenario == "different" {
                    let other = fixture.source.join("different.wav");
                    std::fs::write(&other, wav()).unwrap();
                    quote["source"]["file"] = json!(other);
                }
                let call = if scenario == "wrong-call" { "another-call" } else { "quote-call" };
                assert_eq!(fixture.call(quote, call, None).await.unwrap_err().code(), "stale", "{scenario}");
                assert!(server.requests.lock().unwrap().is_empty(), "{scenario}");
            }
        })
        .await;
}

#[tokio::test]
async fn source_changes_after_the_quote_are_not_submitted() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = Server::new().await;
            let mut fixture = Fixture::new(&server);
            let source = fixture.source.join("melody.wav");
            std::fs::write(&source, wav()).unwrap();
            let request = json!({"action":"quote", "kind":"audio_to_midi", "source":{"file":source, "provenance":{"host_tool":"fixture"}}});
            let mut preview = request.clone();
            preview["preview"] = json!(true);
            fixture.call(preview, "quote-call", None).await.unwrap();
            let quote = fixture.call(request, "quote-call", None).await.unwrap();
            fixture.reserve(&quote, "r-midi", "create-call");
            let mut changed = wav();
            *changed.last_mut().unwrap() = 1;
            std::fs::write(&source, changed).unwrap();
            let started =
                fixture.call(json!({"action":"create", "quote_id":quote["quote_id"]}), "create-call", Some(("r-midi", 3))).await.unwrap();
            let job = fixture.done(started["job_id"].as_str().unwrap()).await;
            assert_eq!(job["state"], "failed");
            assert_eq!(job["error"]["code"], "not_submitted");
            assert_eq!(job["result"]["submission"], "not_submitted");
            assert_eq!(server.creates(), 0);
        })
        .await;
}

#[tokio::test]
async fn a_genuine_provider_midi_file_is_the_primary_registered_output() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = Server::new().await;
            server.midi_file.store(true, Ordering::SeqCst);
            let mut fixture = Fixture::new(&server);
            let source = fixture.source.join("melody.wav");
            std::fs::write(&source, wav()).unwrap();
            let request = json!({"action":"quote", "kind":"audio_to_midi", "source":{"file":source, "provenance":{"host_tool":"fixture"}}});
            let mut preview = request.clone();
            preview["preview"] = json!(true);
            fixture.call(preview, "quote-call", None).await.unwrap();
            let quote = fixture.call(request, "quote-call", None).await.unwrap();
            fixture.reserve(&quote, "r-midi", "create-call");
            let started =
                fixture.call(json!({"action":"create", "quote_id":quote["quote_id"]}), "create-call", Some(("r-midi", 3))).await.unwrap();
            let job = fixture.done(started["job_id"].as_str().unwrap()).await;
            assert_eq!(job["state"], "done", "{job}");
            assert_eq!(job["result"]["output_type"], "midi");
            assert_eq!(job["result"]["timing"], "seconds");
            assert_eq!(job["result"]["notes"][0]["end"], 0.5);
            let artifact = fixture.ctx.artifacts().resolve(&fixture.ctx, job["result"]["artifact_id"].as_str().unwrap()).unwrap();
            assert_eq!(std::fs::read(artifact).unwrap(), midi());
            assert_eq!(server.creates(), 1);
        })
        .await;
}
