use super::{
    analyze::{analyze_source, Analysis, AnalyzeOptions},
    decode::{open_audio, AudioError},
};
/// Run the meter on a worker thread so it cannot hold up terminal input or screen updates. `seconds` is the file's
/// length when `path` is a converted copy that stops short of it.
pub async fn in_worker(
    path: String,
    options: AnalyzeOptions,
    name: String,
    format: Option<String>,
    seconds: Option<f64>,
) -> Result<Analysis, AudioError> {
    tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| AudioError(e.to_string()))?;
        runtime.block_on(async move {
            let mut source = open_audio(&path, options.signal.clone()).await?;
            if let Some(seconds) = seconds {
                source.as_long_as(seconds);
            }
            let analyzed = analyze_source(&mut source, &path, options).await;
            source.close().await?;
            let mut result = analyzed?;
            result.file = name.rsplit(['\\', '/']).next().unwrap_or(&name).into();
            if let Some(format) = format {
                result.format = format;
            }
            Ok(result)
        })
    })
    .await
    .map_err(|e| AudioError(e.to_string()))?
}
