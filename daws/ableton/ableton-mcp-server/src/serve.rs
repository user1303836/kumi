//! The production NDJSON host, including bounded request cancellation and adapter cleanup.
use crate::{
    host::{helpers::error, McpHost, McpHostOptions},
    live::{AsyncLiveAdapter, LiveError, UnavailableLiveAdapter},
    stdio::{self, Notify, StdioOptions},
};
use futures::FutureExt;
use kumi_common::js::json::stringify;
use serde_json::Value;
use std::{cell::RefCell, panic::AssertUnwindSafe, rc::Rc};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

pub async fn serve<I: AsyncRead + 'static, O: AsyncWrite + 'static, D: AsyncWrite + 'static>(
    input: I,
    output: O,
    diagnostics: D,
    adapter: Option<Rc<dyn AsyncLiveAdapter>>,
    mut options: McpHostOptions,
) -> Result<(), LiveError> {
    if options.tool_policy.is_none() {
        options.tool_policy = Some(
            serde_json::to_value(
                crate::tool_catalog::tool_policy_from_env(&kumi_common::env::vars()).map_err(|e| LiveError::error(e.to_string()))?,
            )
            .unwrap(),
        );
    }
    let adapter = adapter.unwrap_or_else(|| Rc::new(UnavailableLiveAdapter));
    let host = Rc::new(McpHost::new(adapter.clone(), options)?);
    let diagnostics = Rc::new(tokio::sync::Mutex::new(Box::pin(diagnostics)));
    let notify: Rc<RefCell<Option<Notify>>> = Rc::new(RefCell::new(None));
    let result = async {
        let emitted = notify.clone();
        host.set_event_emitter(Rc::new(move |line| {
            let emit = emitted.borrow().clone();
            async move {
                if let Some(emit) = emit {
                    emit(line).await.map_err(|e| LiveError::error(e.to_string()))?;
                }
                Ok(())
            }
            .boxed_local()
        }))?;
        let owner = host.clone();
        let stopping = host.clone();
        let handler = Rc::new(move |line: String, context: Option<stdio::RecordContext>| {
            let host = owner.clone();
            let diagnostics = diagnostics.clone();
            async move {
                let value: Value = match serde_json::from_str(&line) {
                    Ok(value) => value,
                    Err(_) => {
                        diagnostics.lock().await.write_all(b"mcp-host: malformed input\n").await.map_err(|e| e.to_string())?;
                        return Ok(Some(stringify(&error(&Value::Null, -32700, "Parse error", None))));
                    }
                };
                match AssertUnwindSafe(host.handle_async(&value, context.as_ref().map(|c| &c.signal))).catch_unwind().await {
                    Ok(Ok(value)) => Ok(value.map(|v| stringify(&v))),
                    _ => {
                        diagnostics.lock().await.write_all(b"mcp-host: internal fault\n").await.map_err(|e| e.to_string())?;
                        // The stdio layer answers it under the request's own id ("Internal error"): Kumi waits for
                        // that id, and would sit out its timeout on an answer to null.
                        Err("internal fault".into())
                    }
                }
            }
            .boxed_local()
        });
        stdio::serve_stdio(
            input,
            output,
            handler,
            StdioOptions {
                notifier: Some(Box::new(move |emit| *notify.borrow_mut() = Some(emit))),
                should_stop: Some(Box::new(move || stopping.is_shutting_down())),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| LiveError::error(e.to_string()))
    }
    .await;
    host.close_open_undo_step().await;
    adapter.close().await?;
    result
}
