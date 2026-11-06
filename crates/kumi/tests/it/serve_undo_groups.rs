use super::*;

/// A typed pad refusal behind the actual Ableton integration and shared History, so grouped undo's ordering and
/// partial state are exercised rather than supplied by the test's Integration implementation.
struct GroupedPadBridge {
    refuse: String,
    remaining: std::cell::RefCell<Vec<String>>,
    asked: std::cell::RefCell<Vec<String>>,
}
#[async_trait(?Send)]
impl McpEndpoint for GroupedPadBridge {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        None
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        let tools = ["live_status", "live_undo", "live_undo_step_begin", "live_undo_step_end", "live_transaction_release"]
            .map(|name| json!({"name":name,"inputSchema":{"type":"object"}}));
        Ok(serde_json::from_value(json!({"tools":tools})).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        let body = match name {
            "live_status" => json!({"connected":true,"adapter":"real-live","epoch":7}),
            "live_undo_step_begin" => json!({"stepId":"pad-group-undo"}),
            "live_undo_step_end" => json!({"closed":true}),
            "live_transaction_release" => json!({"released":0}),
            "live_undo" => {
                let transaction = args["transactionId"].as_str().unwrap().to_owned();
                self.asked.borrow_mut().push(transaction.clone());
                if transaction == self.refuse {
                    let body = json!({"code":"changed_since","reason":"pad sample changed in Live; undo refused"});
                    return Ok(serde_json::from_value(json!({"isError":true,"content":[{"type":"text","text":body.to_string()}]})).unwrap());
                }
                let mut remaining = self.remaining.borrow_mut();
                let index = remaining.iter().position(|item| item == &transaction).expect("an applied pad transaction");
                remaining.remove(index);
                json!({"state":"undone"})
            }
            other => panic!("unexpected grouped pad bridge call {other}"),
        };
        Ok(serde_json::from_value(json!({"content":[{"type":"text","text":body.to_string()}],"structuredContent":body})).unwrap())
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn stderr_status(&self) -> StderrStatus {
        StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}

struct GroupedPadLive {
    live: Rc<kumi_daw_ableton::integration::Ableton>,
    on_connection: kumi_agent::core::contracts::ConnectionListener,
}
#[async_trait(?Send)]
impl kumi_agent::core::contracts::Integration for GroupedPadLive {
    async fn start(&self, signal: Signal) -> Result<(), RuntimeError> {
        self.live.connection.start(signal.clone()).await?;
        self.live.connection.tools().unwrap().refresh(signal).await?;
        self.live.connection.available.set(true);
        self.live.connection.epoch.set(Some(7.));
        (self.on_connection)(kumi_agent::core::contracts::ConnectionState::Connected, None);
        Ok(())
    }
    async fn observe(
        &self,
        _: Signal,
        _: Option<kumi_agent::core::contracts::ObserveHints>,
    ) -> Result<kumi_agent::core::contracts::Observation, RuntimeError> {
        Ok(kumi_agent::core::contracts::Observation {
            native_epoch: None,
            key: "live:pad-group".into(),
            revision: None,
            label: "Live — Pad group".into(),
            context: "Drums".into(),
            instructions: String::new(),
            tools: vec![],
            project: None,
            set: Some("set-pad-group".into()),
            tracks: None,
            saved_at: None,
            identity: None,
        })
    }
    fn has_undo(&self) -> bool {
        true
    }
    async fn guarded_undo(
        &self,
        id: &str,
        signal: Signal,
    ) -> Result<Result<kumi_types::contracts::ChangeRecord, kumi_agent::core::contracts::UndoRefusal>, RuntimeError> {
        kumi_agent::core::contracts::Integration::guarded_undo(self.live.as_ref(), id, signal).await
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.live.connection.close().await
    }
}

case!(serve_preserves_grouped_pad_refusals_and_reports_only_completed_undos, async {
    use kumi_types::contracts::ChangeState;
    for (whole_change, count, refused, previously_unsure) in [
        (false, 1, "pad-1", false),
        (false, 3, "pad-3", false),
        (false, 3, "pad-2", false),
        (true, 1, "pad-1", false),
        (true, 3, "pad-3", false),
        (true, 3, "pad-2", false),
        (true, 3, "pad-2", true),
    ] {
        let home = tempfile::tempdir().unwrap();
        let transport = home.path().join("kumi-home");
        let config = transport.join("bridge/state/bridge-config.json");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, "{}").unwrap();
        let scripts = home.path().join("Remote Scripts");
        std::fs::create_dir_all(scripts.join("AbletonMcpBridge")).unwrap();
        std::fs::write(scripts.join("AbletonMcpBridge/bridge-reference.json"), json!({"config":config}).to_string()).unwrap();
        let env = Env::from([
            ("HOME".into(), home.path().display().to_string()),
            ("KUMI_HOME".into(), transport.display().to_string()),
            ("KUMI_REMOTE_SCRIPTS_DIR".into(), scripts.display().to_string()),
            ("KUMI_PROCESSES".into(), "none".into()),
        ]);
        let ids: Vec<String> = (1..=count).map(|index| format!("pad-{index}")).collect();
        let mut remaining = ids.clone();
        if whole_change {
            remaining.insert(0, "older-row".into());
        }
        let bridge = Rc::new(GroupedPadBridge {
            refuse: refused.into(),
            remaining: std::cell::RefCell::new(remaining.clone()),
            asked: Default::default(),
        });
        let integration = Rc::new(std::cell::RefCell::new(None));
        let factory = {
            let bridge = bridge.clone();
            let integration = integration.clone();
            AbletonFactory(Rc::new(move |mut options| {
                let endpoint: Rc<dyn McpEndpoint> = bridge.clone();
                options.connect = Some(Rc::new(move |_| {
                    let endpoint = endpoint.clone();
                    async move { Ok(endpoint) }.boxed_local()
                }));
                let on_connection = options.on_connection.clone();
                let live = kumi_daw_ableton::integration::Ableton::new(options);
                *integration.borrow_mut() = Some(live.clone());
                Rc::new(GroupedPadLive { live, on_connection }) as Rc<dyn kumi_agent::core::contracts::Integration>
            }))
        };
        let ctx = Ctx::new(
            Args {
                state_dir: home.path().join("state"),
                kumi_home: transport,
                hermes_home: None,
                hermes_root: None,
                daw: DawId::Ableton,
                library_roots: vec![],
                media_roots: vec![],
                tools: BTreeMap::new(),
            },
            env,
            kumi_daw::io::Answers::default().with(factory),
        )
        .unwrap();
        let mut wire = Wire::start(ctx.clone()).await;
        let (connected, error) = wire.call("connect", json!({}), None).await;
        assert!(!error, "{connected}");
        wire.call("observe", json!({"scope":"focus"}), None).await;
        let live = integration.borrow().as_ref().unwrap().clone();
        let older = whole_change.then(|| {
            let record =
                serde_json::from_value(json!({"id":"older-row","family":"device","title":"Load older sample","state":"applied","at":0}))
                    .unwrap();
            live.history.remember(record, "older-row".into(), None);
            live.history.entries.borrow()["older-row"].borrow_mut().tool = Some("load_sample_to_pad".into());
            ctx.history().rows(20).into_iter().find(|row| row.label == "older-row").unwrap()
        });
        let mut made = vec![];
        live.history
            .quietly(Some(&mut made), async {
                for id in &ids {
                    let record = serde_json::from_value(
                        json!({"id":id,"family":"device","title":format!("Load sample {id}"),"state":"applied","at":1}),
                    )
                    .unwrap();
                    live.history.remember(record, id.clone(), None);
                    live.history.entries.borrow()[id].borrow_mut().tool = Some("load_sample_to_pad".into());
                }
                if previously_unsure {
                    let entries = live.history.entries.borrow();
                    let mut entry = entries[refused].borrow_mut();
                    entry.record.state = ChangeState::Unsure;
                    entry.record.note = Some("An earlier pad undo wasn't confirmed.".into());
                }
            })
            .await;
        let group = live.history.grouped("Load drum samples", &made, &[]).unwrap();
        let row = ctx.history().rows(20).into_iter().find(|row| row.label == group).unwrap();
        let mut args = json!({"history_id":row.id});
        if let Some(older) = &older {
            ctx.history().one_change(&[older.id.clone(), row.id.clone()], ctx.snapshots());
        }
        // DAW-only history exposes whether this undo is counted as another write: a refusal must leave this latest
        // step "next", while a partial or unconfirmed undo buries it. It isn't part of the change being undone.
        let history = ctx.history();
        history.begin_call();
        history.daw_undo_call("Live's own Undo takes this separate change back.".into());
        let marker =
            serde_json::from_value(json!({"id":"latest-manual","family":"mixer","title":"Latest manual change","state":"applied","at":2}))
                .unwrap();
        let marker = history.record(&marker, &row.project_key, ctx.snapshots());
        let identity =
            kumi::serve::history::CallIdentity { profile_id: None, session_id: None, tool_call_id: None, digest: "latest-manual" };
        history.end_call(identity, ctx.snapshots());
        assert_eq!(history.daw_undo_place(&marker), Some("next"));
        if let Some(older) = &older {
            let (preview, error) = wire.call("undo", json!({"history_id":row.id,"whole_change":true,"preview":true}), None).await;
            assert!(!error, "{preview}");
            assert_eq!(preview["rows"][0]["history_id"], row.id, "{preview}");
            assert_eq!(preview["rows"][1]["history_id"], older.id, "{preview}");
            args["whole_change"] = json!(true);
            args["expect_rows"] = preview["rows_digest"].clone();
        }
        let (rows_before, epoch_before) = (ctx.history().rows(20).len(), ctx.link().epoch());
        let before_history = wire.call("history", json!({}), None).await.0;
        let before_records: Vec<_> = ids.iter().map(|id| live.history.entries.borrow()[id].borrow().record.clone()).collect();
        let group_before = live.history.entries.borrow()[&group].borrow().record.clone();
        let older_before = older.as_ref().map(|older| serde_json::to_value(history.row(&older.id).unwrap()).unwrap());
        let older_core_before = older.as_ref().map(|_| live.history.entries.borrow()["older-row"].borrow().record.clone());
        let (answer, error) = wire.call("undo", args, Some("tc-pad-group-undo")).await;
        assert!(!error, "{answer}");
        let expected_asked = if refused == "pad-2" { vec!["pad-3", "pad-2"] } else { vec![refused] };
        assert_eq!(bridge.asked.borrow().iter().map(String::as_str).collect::<Vec<_>>(), expected_asked);
        assert_eq!(ctx.history().rows(20).len(), rows_before + usize::from(previously_unsure), "{answer}");
        let partial = refused == "pad-2";
        let entries = live.history.entries.borrow();
        for (id, before) in ids.iter().zip(&before_records) {
            assert_eq!(
                entries[id].borrow().record.state,
                if partial && id == "pad-3" { ChangeState::Undone } else { before.state },
                "{id}: {answer}"
            );
            if !partial || id != "pad-3" {
                assert_eq!(&entries[id].borrow().record, before, "untouched child {id}: {answer}");
            }
        }
        if partial {
            let state = if previously_unsure { ChangeState::Unsure } else { ChangeState::Kept };
            assert_eq!(
                (answer["state"].clone(), entries[&group].borrow().record.state),
                (json!(if previously_unsure { "unsure" } else { "kept" }), state)
            );
            let reason = answer["reason"].as_str().unwrap();
            assert!(reason.contains("took back 1 of its 3 changes") && reason.contains("pad sample changed"), "{reason}");
            assert!(!reason.to_ascii_lowercase().contains("nothing changed"), "{reason}");
            assert_eq!(entries[&group].borrow().record.note.as_deref(), Some(reason));
            remaining.retain(|id| id != "pad-3");
            assert_eq!(*bridge.remaining.borrow(), remaining, "the completed undo stays completed");
            assert_eq!(ctx.link().epoch(), epoch_before + 1);
            assert_eq!(history.daw_undo_place(&marker), Some("buried"));
            if previously_unsure {
                assert!(reason.contains("an earlier undo wasn't confirmed"), "{reason}");
                assert_eq!(answer["history_ids"].as_array().unwrap().len(), 1, "{answer}");
                let audit = history.row(answer["history_ids"][0].as_str().unwrap()).unwrap();
                assert_eq!(audit.note.as_deref(), Some(reason));
                assert_eq!(audit.tool_call_id.as_deref(), Some("tc-pad-group-undo"));
                assert_eq!(audit.state, "unsure");
                assert!(!audit.undo_available);
            } else {
                assert!(answer.get("history_ids").is_none(), "a confirmed partial undo needs no unsure audit: {answer}");
            }
        } else {
            assert_eq!(answer["state"], "refused", "{answer}");
            let refused_row = if whole_change { &answer["rows"][0] } else { &answer };
            assert_eq!(refused_row["code"], "changed_since", "{answer}");
            assert_eq!(entries[&group].borrow().record, group_before);
            assert_eq!(*bridge.remaining.borrow(), remaining, "the refusal removed nothing");
            assert_eq!(ctx.link().epoch(), epoch_before, "a no-write refusal leaves the epoch alone");
            assert_eq!(history.daw_undo_place(&marker), Some("next"), "a no-write refusal isn't a later write");
            assert!(answer.get("history_ids").is_none());
        }
        if let Some(older) = &older {
            assert_eq!(answer["rows"][0]["history_id"], row.id, "{answer}");
            assert_eq!(answer["rows"][0]["state"], answer["state"], "{answer}");
            assert_eq!(answer["rows"][0]["reason"], answer["reason"], "{answer}");
            assert_eq!(answer["rows"][1], json!({"history_id":older.id,"state":"not_tried"}), "{answer}");
            assert_eq!(Some(serde_json::to_value(history.row(&older.id).unwrap()).unwrap()), older_before);
            assert_eq!(Some(entries["older-row"].borrow().record.clone()), older_core_before);
        }
        drop(entries);
        if !partial {
            assert_eq!(wire.call("history", json!({}), None).await.0, before_history, "no records or audit changed");
        }
        wire.end().await;
    }
});
