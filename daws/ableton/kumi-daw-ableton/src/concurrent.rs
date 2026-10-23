//! Concurrent dispatch with the source Promise.all failure ordering.
// Start every scheduled read even when another fails immediately. Unlike try_join_all,
// an early ready error must not prevent the later Live requests from being dispatched.
pub(crate) async fn eager_all<I, F, T, E>(work: I) -> Result<Vec<T>, E>
where
    I: IntoIterator<Item = F>,
    F: std::future::Future<Output = Result<T, E>>,
{
    use std::task::Poll;
    let mut work: Vec<_> = work.into_iter().map(|f| Some(Box::pin(f))).collect();
    let mut values: Vec<Option<T>> = std::iter::repeat_with(|| None).take(work.len()).collect();
    std::future::poll_fn(move |cx| {
        let mut error = None;
        let mut pending = false;
        for (index, future) in work.iter_mut().enumerate() {
            let Some(active) = future else { continue };
            match active.as_mut().poll(cx) {
                Poll::Pending => pending = true,
                Poll::Ready(value) => {
                    *future = None;
                    match value {
                        Ok(value) => values[index] = Some(value),
                        Err(e) => {
                            if error.is_none() {
                                error = Some(e);
                            }
                        }
                    }
                }
            }
        }
        if let Some(error) = error {
            Poll::Ready(Err(error))
        } else if pending {
            Poll::Pending
        } else {
            Poll::Ready(Ok(values.iter_mut().map(|v| v.take().unwrap()).collect()))
        }
    })
    .await
}
