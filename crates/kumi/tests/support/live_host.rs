//! Kumi's Live integration over the bridge's own host in this process, which talks to the Remote Script in Live: for the
//! operator fixtures run by hand on real Live (never in CI).
use ableton_mcp_server::host::McpHost;
use async_trait::async_trait;
use kumi_agent::mcp::{
    client::{McpEndpoint, StderrStatus},
    types::{CallToolResult, Implementation, ListToolsResult},
};
use kumi_common::abort::Signal;
use kumi_types::{contracts::JsonObject, errors::RuntimeError};
use serde_json::{json, Value};
use std::rc::Rc;
pub struct HostEndpoint(pub Rc<McpHost>);
impl HostEndpoint {
    async fn request(&self, method: &str, params: Value) -> Result<Value, RuntimeError> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let reply = self
            .0
            .handle_async(
                &json!({"jsonrpc":"2.0","id":NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),"method":method,"params":params}),
                None,
            )
            .await
            .map_err(|e| RuntimeError::plain(e.to_string()))?
            .unwrap();
        if reply.get("error").is_some() {
            return Err(RuntimeError::plain(reply["error"].to_string()));
        }
        Ok(reply["result"].clone())
    }
}
#[async_trait(?Send)]
impl McpEndpoint for HostEndpoint {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name":"real-live-fixture","version":"1.0.87"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        serde_json::from_value(self.request("tools/list", json!({})).await?).map_err(|e| RuntimeError::plain(e.to_string()))
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        serde_json::from_value(self.request("tools/call", json!({"name":name,"arguments":args})).await?)
            .map_err(|e| RuntimeError::plain(e.to_string()))
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
