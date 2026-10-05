use super::{
    analyze::{analyze_file, Analysis, AnalyzeOptions},
    decode::AudioError,
};
/// Run the meter on a worker thread so it cannot hold up terminal input or screen updates.
pub async fn in_worker(path: String, options: AnalyzeOptions, name: String, format: Option<String>) -> Result<Analysis, AudioError> {
    tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| AudioError(e.to_string()))?;
        runtime.block_on(async move {
            let mut result = analyze_file(&path, options).await?;
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
