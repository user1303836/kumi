//! Keep the Remote Script authoritative for shared operations.
//! In Live 11 mode, extension is unavailable — all operations route through Remote Script.
use crate::live::*;
use async_trait::async_trait;
use serde_json::json;
use std::rc::Rc;

// Extension-only operations that are unavailable in Live 11
pub const EXTENSION_ONLY_OPERATIONS: &[&str] = &[
    "willington.device.read",
    "willington.editing.read",
    "native_editing.device.read",
    "native_editing.focus.read",
];

/// Check if an operation is available in Live 11 (not extension-only)
pub fn is_live11_compatible(operation: &str) -> bool {
    !EXTENSION_ONLY_OPERATIONS.contains(&operation)
}

/// Simple adapter that wraps Remote Script without Extension channel
#[derive(Clone)]
pub struct RemoteOnlyAdapter {
    remote: Rc<dyn AsyncLiveAdapter>,
}

impl RemoteOnlyAdapter {
    pub fn new(remote: Rc<dyn AsyncLiveAdapter>) -> Self {
        Self { remote }
    }
}

impl LiveAdapter for RemoteOnlyAdapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        let status = self.remote.status()?;
        // Mark as Live 11 compatible
        let mut status = status;
        status.extra.insert("liveVersion".into(), json!("11"));
        status.extra.insert("extensionAvailable".into(), json!(false));
        Ok(status)
    }
    
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        self.remote.snapshot()
    }
    
    fn get(&self, reference: &LiveRef) -> Result<Option<serde_json::Value>, LiveError> {
        self.remote.get(reference)
    }
    
    fn invoke(&self, invocation: &LiveInvocation) -> Result<serde_json::Value, LiveError> {
        // Check if operation is Live 11 compatible
        if !is_live11_compatible(&invocation.operation) {
            return Err(LiveError::error(format!(
                "Operation '{}' is unavailable in Ableton Live 11 (requires Live 12 Extension)",
                invocation.operation
            )));
        }
        self.remote.invoke(invocation)
    }
    
    fn subscribe(&self, listener: LiveListener) -> Result<Unsubscribe, LiveError> {
        self.remote.subscribe(listener)
    }
    
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.remote.reconnect()
    }
}

#[async_trait(?Send)]
impl AsyncLiveAdapter for RemoteOnlyAdapter {
    async fn snapshot_async(
        &self,
        context: Option<&LiveOperationContext>,
        request: Option<&LiveSnapshotRequest>,
    ) -> Result<LiveSnapshot, LiveError> {
        self.remote.snapshot_async(context, request).await
    }
    
    async fn discover_async(
        &self,
        request: &LiveDiscoveryRequest,
        context: Option<&LiveOperationContext>,
    ) -> Result<LiveDiscoveryResult, LiveError> {
        self.remote.discover_async(request, context).await
    }
    
    async fn get_async(&self, reference: &LiveRef, context: Option<&LiveOperationContext>) -> Result<Option<serde_json::Value>, LiveError> {
        self.remote.get_async(reference, context).await
    }
    
    async fn invoke_async(
        &self,
        invocation: &LiveInvocation,
        context: Option<&LiveOperationContext>,
    ) -> Result<serde_json::Value, LiveError> {
        // Check if operation is Live 11 compatible
        if !is_live11_compatible(&invocation.operation) {
            return Err(LiveError::error(format!(
                "Operation '{}' is unavailable in Ableton Live 11 (requires Live 12 Extension)",
                invocation.operation
            )));
        }
        self.remote.invoke_async(invocation, context).await
    }
    
    async fn reconnect_async(&self, context: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.remote.reconnect_async(context).await
    }
    
    async fn close(&self) -> Result<(), LiveError> {
        self.remote.close().await
    }
    
    fn has_refresh_status_async(&self) -> bool {
        self.remote.has_refresh_status_async()
    }
    
    async fn refresh_status_async(&self, context: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.remote.refresh_status_async(context).await
    }
    
    fn has_subscribe_status(&self) -> bool {
        self.remote.has_subscribe_status()
    }
    
    fn subscribe_status(&self, listener: StatusListener) -> Unsubscribe {
        self.remote.subscribe_status(listener)
    }
    
    fn has_retire_transaction_async(&self) -> bool {
        self.remote.has_retire_transaction_async()
    }
    
    async fn retire_transaction_async(
        &self,
        transaction: &str,
        context: Option<&LiveOperationContext>,
        terminal: bool,
    ) -> Result<serde_json::Value, LiveError> {
        self.remote.retire_transaction_async(transaction, context, terminal).await
    }
    
    fn retires_on_its_own(&self) -> bool {
        self.remote.retires_on_its_own()
    }
    
    fn has_expect_state_digest(&self) -> bool {
        self.remote.has_expect_state_digest()
    }
    
    fn expect_state_digest(&self, transaction: &str, invocation: &LiveInvocation) {
        self.remote.expect_state_digest(transaction, invocation)
    }
}
