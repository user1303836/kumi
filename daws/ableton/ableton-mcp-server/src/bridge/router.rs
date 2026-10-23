//! Keep the Remote Script authoritative for shared operations and route extension-only operations beside it.
use super::extension_channel::ExtensionChannel;
use super::listeners::Listeners;
use crate::live::*;
use async_trait::async_trait;
use serde_json::json;
use std::rc::Rc;
/// Operations the extension deliberately replaces. Empty until both channels' semantics agree.
pub const EXTENSION_FIRST: &[&str] = &[];
pub fn route_to_extension(operation: &str, remote: &LiveStatus, extension: Option<&LiveStatus>) -> bool {
    extension.is_some_and(|extension| extension.has_operation(operation))
        && (!remote.has_operation(operation) || EXTENSION_FIRST.contains(&operation))
}
pub fn merged_status(remote: &LiveStatus, extension: Option<&LiveStatus>, reason: &str) -> LiveStatus {
    let mut status = remote.clone();
    let Some(extension) = extension.filter(|_| remote.connected) else {
        status.extra.insert("channels".into(), json!({"extension":{"connected":false,"reason":reason}}));
        return status;
    };
    let own = remote.operations.as_deref().unwrap_or_default();
    let added: Vec<_> = extension
        .operations
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|op| op.as_str() != "status" && !own.contains(op))
        .cloned()
        .collect();
    status.operations = Some(own.iter().chain(&added).cloned().collect());
    let mut capabilities = vec![];
    for cap in remote.capabilities.iter().copied().chain(live_capabilities_for_operations(&added)) {
        if !capabilities.contains(&cap) {
            capabilities.push(cap);
        }
    }
    status.capabilities = capabilities;
    let mut channel = json!({"connected":true,"operations":added});
    if let Some(version) =
        extension.extra.get("extension").and_then(|v| v.get("version")).and_then(|v| v.as_str()).filter(|v| !v.is_empty())
    {
        channel["version"] = version.into();
    }
    status.extra.insert("channels".into(), json!({"extension":channel}));
    status
}
struct RoutedInner {
    remote: Rc<dyn AsyncLiveAdapter>,
    extension: ExtensionChannel,
    listeners: Listeners<dyn Fn(Option<&LiveStatus>)>,
    on_close: Rc<dyn Fn()>,
}
#[derive(Clone)]
pub struct RoutedLiveAdapter(Rc<RoutedInner>);
pub fn routed_adapter(remote: Rc<dyn AsyncLiveAdapter>, extension: ExtensionChannel, on_close: Option<Rc<dyn Fn()>>) -> RoutedLiveAdapter {
    let adapter = RoutedLiveAdapter(Rc::new(RoutedInner {
        remote,
        extension,
        listeners: Listeners::default(),
        on_close: on_close.unwrap_or_else(|| Rc::new(|| {})),
    }));
    let weak = Rc::downgrade(&adapter.0);
    let notify: StatusListener = Rc::new(move |_| {
        if let Some(inner) = weak.upgrade() {
            inner.listeners.each(|listener| listener(None));
        }
    });
    let _ = adapter.0.extension.subscribe_status(notify.clone());
    if adapter.0.remote.has_subscribe_status() {
        let _ = adapter.0.remote.subscribe_status(notify);
    }
    adapter
}
impl LiveAdapter for RoutedLiveAdapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        Ok(merged_status(&self.0.remote.status()?, self.0.extension.status().as_ref(), &self.0.extension.reason()))
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        self.0.remote.snapshot()
    }
    fn get(&self, reference: &LiveRef) -> Result<Option<serde_json::Value>, LiveError> {
        self.0.remote.get(reference)
    }
    fn invoke(&self, invocation: &LiveInvocation) -> Result<serde_json::Value, LiveError> {
        self.0.remote.invoke(invocation)
    }
    fn subscribe(&self, listener: LiveListener) -> Result<Unsubscribe, LiveError> {
        let remote_listener = listener.clone();
        let first = self.0.remote.subscribe(Rc::new(move |event| {
            let mut event = event.clone();
            event.channel = Some(LiveEventChannel::RemoteScript);
            remote_listener(&event);
        }))?;
        let second = self.0.extension.subscribe(Rc::new(move |event| {
            let mut event = event.clone();
            event.channel = Some(LiveEventChannel::Extension);
            listener(&event);
        }));
        Ok(Box::new(move || {
            first();
            second();
        }))
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.0.remote.reconnect()
    }
}
#[async_trait(?Send)]
impl AsyncLiveAdapter for RoutedLiveAdapter {
    async fn snapshot_async(
        &self,
        context: Option<&LiveOperationContext>,
        request: Option<&LiveSnapshotRequest>,
    ) -> Result<LiveSnapshot, LiveError> {
        self.0.remote.snapshot_async(context, request).await
    }
    async fn discover_async(
        &self,
        request: &LiveDiscoveryRequest,
        context: Option<&LiveOperationContext>,
    ) -> Result<LiveDiscoveryResult, LiveError> {
        self.0.remote.discover_async(request, context).await
    }
    async fn get_async(&self, reference: &LiveRef, context: Option<&LiveOperationContext>) -> Result<Option<serde_json::Value>, LiveError> {
        self.0.remote.get_async(reference, context).await
    }
    async fn invoke_async(
        &self,
        invocation: &LiveInvocation,
        context: Option<&LiveOperationContext>,
    ) -> Result<serde_json::Value, LiveError> {
        if route_to_extension(&invocation.operation, &self.0.remote.status()?, self.0.extension.status().as_ref()) {
            self.0.extension.invoke(&invocation.operation, &invocation.args, context).await
        } else {
            self.0.remote.invoke_async(invocation, context).await
        }
    }
    async fn reconnect_async(&self, context: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.0.remote.reconnect_async(context).await
    }
    async fn close(&self) -> Result<(), LiveError> {
        (self.0.on_close)();
        self.0.extension.close().await?;
        self.0.remote.close().await
    }
    fn has_refresh_status_async(&self) -> bool {
        true
    }
    async fn refresh_status_async(&self, context: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        if self.0.remote.has_refresh_status_async() {
            self.0.remote.refresh_status_async(context).await?;
        }
        let extension = self.0.extension.clone();
        tokio::task::spawn_local(async move {
            let _ = extension.connect().await;
        });
        self.status()
    }
    fn has_subscribe_status(&self) -> bool {
        true
    }
    fn subscribe_status(&self, listener: StatusListener) -> Unsubscribe {
        self.0.listeners.add(listener.clone());
        let weak = Rc::downgrade(&self.0);
        Box::new(move || {
            if let Some(inner) = weak.upgrade() {
                inner.listeners.remove(&listener);
            }
        })
    }
    fn has_retire_transaction_async(&self) -> bool {
        self.0.remote.has_retire_transaction_async()
    }
    async fn retire_transaction_async(
        &self,
        transaction: &str,
        context: Option<&LiveOperationContext>,
        terminal: bool,
    ) -> Result<serde_json::Value, LiveError> {
        self.0.remote.retire_transaction_async(transaction, context, terminal).await
    }
    fn retires_on_its_own(&self) -> bool {
        self.0.remote.retires_on_its_own()
    }
    fn has_expect_state_digest(&self) -> bool {
        self.0.remote.has_expect_state_digest()
    }
    fn expect_state_digest(&self, transaction: &str, invocation: &LiveInvocation) {
        self.0.remote.expect_state_digest(transaction, invocation);
    }
}
