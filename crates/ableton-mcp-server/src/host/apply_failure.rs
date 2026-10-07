//! What a failed apply says about trying again.
use super::*;

/// The host's adapter: it passes every call through and notes the last one that failed. An apply that stopped
/// before sending anything can then tell a Live read that failed, which a retry may pass, from its own refusal of
/// what it read (a staged file changed, a drum pad without identity), which every retry meets again.
pub(super) struct FailureNotingAdapter {
    pub(super) adapter: Rc<dyn AsyncLiveAdapter>,
    pub(super) last_failure: Rc<RefCell<Option<String>>>,
}
impl FailureNotingAdapter {
    fn note<T>(&self, result: Result<T, LiveError>) -> Result<T, LiveError> {
        if let Err(error) = &result {
            *self.last_failure.borrow_mut() = Some(error.message().to_owned());
        }
        result
    }
}
impl LiveAdapter for FailureNotingAdapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        self.note(self.adapter.status())
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        self.note(self.adapter.snapshot())
    }
    fn get(&self, r: &LiveRef) -> Result<Option<Value>, LiveError> {
        self.note(self.adapter.get(r))
    }
    fn invoke(&self, i: &LiveInvocation) -> Result<Value, LiveError> {
        self.note(self.adapter.invoke(i))
    }
    fn subscribe(&self, l: LiveListener) -> Result<Unsubscribe, LiveError> {
        self.adapter.subscribe(l)
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.adapter.reconnect()
    }
}
#[async_trait::async_trait(?Send)]
impl AsyncLiveAdapter for FailureNotingAdapter {
    async fn snapshot_async(&self, c: Option<&LiveOperationContext>, r: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        self.note(self.adapter.snapshot_async(c, r).await)
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        self.note(self.adapter.discover_async(r, c).await)
    }
    async fn get_async(&self, r: &LiveRef, c: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.note(self.adapter.get_async(r, c).await)
    }
    async fn invoke_async(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.note(self.adapter.invoke_async(i, c).await)
    }
    async fn reconnect_async(&self, c: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.adapter.reconnect_async(c).await
    }
    async fn close(&self) -> Result<(), LiveError> {
        self.adapter.close().await
    }
    fn has_refresh_status_async(&self) -> bool {
        self.adapter.has_refresh_status_async()
    }
    async fn refresh_status_async(&self, c: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.note(self.adapter.refresh_status_async(c).await)
    }
    fn has_subscribe_status(&self) -> bool {
        self.adapter.has_subscribe_status()
    }
    fn subscribe_status(&self, l: StatusListener) -> Unsubscribe {
        self.adapter.subscribe_status(l)
    }
    fn has_retire_transaction_async(&self) -> bool {
        self.adapter.has_retire_transaction_async()
    }
    async fn retire_transaction_async(&self, id: &str, c: Option<&LiveOperationContext>, terminal: bool) -> Result<Value, LiveError> {
        self.adapter.retire_transaction_async(id, c, terminal).await
    }
    fn retires_on_its_own(&self) -> bool {
        self.adapter.retires_on_its_own()
    }
    fn has_expect_state_digest(&self) -> bool {
        self.adapter.has_expect_state_digest()
    }
    fn expect_state_digest(&self, id: &str, i: &LiveInvocation) {
        self.adapter.expect_state_digest(id, i)
    }
}
impl McpHost {
    /// What a failed apply answers. Its transaction turns `uncertain` only from `applying`, the state each apply sets
    /// right before its first send. A failure before that changed nothing in Live. Only one that came from a Live call
    /// (a read that failed) may pass on a retry, so only that one is told the change can be applied again.
    /// `uncertain` is the remediation once something may have changed.
    pub(super) fn apply_failed(&self, id: &Value, record: &RefCell<Value>, cause: &LiveError, uncertain: &str) -> Value {
        let state = record.borrow()["state"].clone();
        if state == "applying" {
            record.borrow_mut()["state"] = json!("uncertain");
        }
        let read_failed = self.last_live_failure.borrow_mut().take().is_some_and(|failure| failure == cause.message());
        let remediation = if state != "previewed" {
            uncertain
        } else if read_failed {
            "Nothing changed in Live; this change can be applied again."
        } else {
            "Nothing changed in Live."
        };
        adapter_tool_error(id, cause, remediation)
    }
}
