//! Newline-delimited JSON-RPC over stdin and stdout: each request handled as it arrives, answers written whole, in
//! the order they finish.
use crate::host::Host;
use crate::mcp;
use daw_bridge::frame::Framer;
use serde_json::Value;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

/// The longest message read from the client.
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// Serves until the input ends (or breaks), then lets the requests in flight answer.
pub async fn serve<I, O>(host: Arc<Host>, mut input: I, mut output: O) -> std::io::Result<()>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin + Send + 'static,
{
    let (answers, mut answered) = mpsc::unbounded_channel::<Vec<u8>>();
    let writer = tokio::spawn(async move {
        while let Some(line) = answered.recv().await {
            output.write_all(&line).await?;
            output.flush().await?;
        }
        Ok::<_, std::io::Error>(())
    });
    // The bridge's events, as notifications between the answers.
    let mut events = host.events();
    let notices = answers.clone();
    let forwarding = tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => {
                    let notification = serde_json::json!({"jsonrpc": "2.0", "method": "notifications/daw_event", "params": event});
                    if notices.send(encode(&notification)).is_err() {
                        break;
                    }
                }
                // Events dropped on the way: the client is told, so it never stays a track shift behind.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let lost = serde_json::json!({"jsonrpc": "2.0", "method": "notifications/daw_event", "params": {"name": "events_lost", "data": {}}});
                    if notices.send(encode(&lost)).is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    let mut framer = Framer::new(MAX_MESSAGE_BYTES);
    let mut chunk = vec![0u8; 64 * 1024];
    let mut tasks = tokio::task::JoinSet::new();
    'reading: loop {
        while let Some(line) = framer.next_line() {
            let Ok(line) = line else {
                // A message too long or not UTF-8: nothing after it can be framed reliably.
                let _ =
                    answers.send(encode(&serde_json::json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}})));
                break 'reading;
            };
            let host = host.clone();
            let answers = answers.clone();
            spawn_reaping(&mut tasks, async move {
                let answer = match serde_json::from_str::<Value>(&line) {
                    Ok(message) => mcp::handle(&host, &message).await,
                    Err(_) => Some(serde_json::json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}})),
                };
                if let Some(answer) = answer {
                    let _ = answers.send(encode(&answer));
                }
            });
        }
        match input.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(count) => framer.feed(&chunk[..count]),
        }
    }
    // The input is over: Kumi stops the host soon after it closes stdin, so what runs on the bridge is stopped now (a
    // render's settings put back), while the requests in flight answer, not after them.
    let joining = async { while tasks.join_next().await.is_some() {} };
    tokio::join!(host.cancel_running_jobs(), joining);
    forwarding.abort();
    drop(answers);
    writer.await.unwrap_or(Ok(()))?;
    host.close().await;
    Ok(())
}

/// Spawns a request's handling, reaping the ones that finished first: a finished task's cell (as big as the whole handler)
/// stays allocated until it's joined, and FOCUS asks for the focus several times a second all session.
fn spawn_reaping<F>(tasks: &mut tokio::task::JoinSet<()>, task: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    while tasks.try_join_next().is_some() {}
    tasks.spawn(task);
}

fn encode(value: &Value) -> Vec<u8> {
    let mut line = serde_json::to_vec(value).expect("JSON serializes");
    line.push(b'\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn finished_requests_are_let_go_as_new_ones_come() {
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..1000 {
            spawn_reaping(&mut tasks, async {});
            tokio::task::yield_now().await;
        }
        assert!(tasks.len() < 10, "{} requests' tasks held", tasks.len());
    }
}
