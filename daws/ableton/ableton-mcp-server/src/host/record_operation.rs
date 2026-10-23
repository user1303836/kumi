//! A retained transaction's apply/stop promise remains alive until its Live work finishes.
use super::*;
use futures::{
    future::{LocalBoxFuture, Shared},
    FutureExt,
};
use retention::TransactionRecord;
use std::{future::Future, rc::Weak};
pub(super) type RecordPromise = Shared<LocalBoxFuture<'static, Result<Value, LiveError>>>;
pub(super) struct RecordOperation {
    record: Weak<RefCell<Value>>,
    promise: RecordPromise,
}
impl McpHost {
    pub(super) fn record_operation(&self, record: &TransactionRecord) -> Option<RecordPromise> {
        self.record_operations
            .borrow()
            .iter()
            .find(|operation| operation.record.upgrade().is_some_and(|held| Rc::ptr_eq(&held, record)))
            .map(|operation| operation.promise.clone())
    }
    pub(super) fn start_record_operation<F>(&self, record: &TransactionRecord, execute: F) -> RecordPromise
    where
        F: Future<Output = Result<Value, LiveError>> + 'static,
    {
        let (send, receive) = tokio::sync::oneshot::channel();
        let promise = async move { receive.await.unwrap_or_else(|_| Err(LiveError::error("transaction execution ended unexpectedly"))) }
            .boxed_local()
            .shared();
        // An async source method executes through its first suspension before its promise is retained.
        let mut execute = execute.boxed_local();
        let immediate = execute.as_mut().now_or_never();
        tokio::task::spawn_local(async move {
            let result = match immediate {
                Some(result) => result,
                None => execute.await,
            };
            let _ = send.send(result);
        });
        let mut operations = self.record_operations.borrow_mut();
        operations.retain(|operation| operation.record.upgrade().is_some_and(|held| !Rc::ptr_eq(&held, record)));
        operations.push(RecordOperation { record: Rc::downgrade(record), promise: promise.clone() });
        record.borrow_mut()["inflight"] = json!({});
        promise
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn retained_operation_runs_before_return_and_survives_a_dropped_waiter() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let host = McpHost::default();
                let record = Rc::new(RefCell::new(json!({"state":"applying"})));
                let seen = Rc::new(Cell::new(0));
                let weak = Rc::downgrade(&record);
                let (send, receive) = tokio::sync::oneshot::channel();
                let held = record.clone();
                let observed = seen.clone();
                let first = host.start_record_operation(&record, async move {
                    observed.set(1);
                    receive.await.unwrap();
                    held.borrow_mut()["state"] = json!("applied");
                    observed.set(2);
                    Ok(json!({"complete":true}))
                });
                assert_eq!(seen.get(), 1);
                assert_eq!(record.borrow()["inflight"], json!({}));
                let joined = host.record_operation(&record).unwrap();
                drop(first);
                send.send(()).unwrap();
                assert_eq!(joined.await.unwrap(), json!({"complete":true}));
                assert_eq!(record.borrow()["state"], "applied");
                assert_eq!(seen.get(), 2);
                drop(record);
                assert!(weak.upgrade().is_none());
                let another = Rc::new(RefCell::new(json!({})));
                host.start_record_operation(&another, async { Ok(Value::Null) }).await.unwrap();
                assert_eq!(host.record_operations.borrow().len(), 1);
            })
            .await;
    }
    #[tokio::test]
    async fn retained_operation_shares_failures_and_replaces_the_next_phase() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let host = McpHost::default();
                let record = Rc::new(RefCell::new(json!({"state":"applying"})));
                let first = host.start_record_operation(&record, async { Err(LiveError::error("lost acknowledgement")) });
                let joined = host.record_operation(&record).unwrap();
                assert_eq!(first.await.unwrap_err().message(), "lost acknowledgement");
                assert_eq!(joined.await.unwrap_err().message(), "lost acknowledgement");
                let next = host.start_record_operation(&record, async { Ok(json!({"stopped":true})) });
                assert_eq!(next.await.unwrap(), json!({"stopped":true}));
                assert_eq!(host.record_operations.borrow().len(), 1);
                assert_eq!(host.record_operation(&record).unwrap().await.unwrap(), json!({"stopped":true}));
            })
            .await;
    }
}
