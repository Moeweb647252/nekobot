//! JavaScript execution via Boa engine with Web API runtime.

use boa_engine::{Context, Source};
use boa_runtime::extensions::{ConsoleExtension, FetchExtension};
use boa_runtime::fetch::BlockingReqwestFetcher;

/// Execute JavaScript code in a fresh Boa context with Web APIs.
///
/// Runs on a blocking thread because Boa's `Context` is `!Send` and
/// `JsPromise` is private (preventing native async polling). The async
/// API enables timeout via `tokio::time::timeout`.
///
/// Provides `fetch()`, `console`, `URL`, `TextEncoder`, `TextDecoder`,
/// `setTimeout`, and `structuredClone`.
pub async fn execute(js_code: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let mut context = Context::default();

        boa_runtime::register(
            (
                ConsoleExtension::default(),
                FetchExtension(BlockingReqwestFetcher::default()),
            ),
            None,
            &mut context,
        )
        .inspect_err(|e| tracing::warn!("Failed to register Web API runtime: {e}"))
        .ok();

        let result = context
            .eval(Source::from_bytes(&js_code))
            .map_err(|e| format!("JS execution error: {e}"))?;

        context
            .run_jobs()
            .map_err(|e| format!("JS job error: {e}"))?;

        result
            .to_string(&mut context)
            .map(|s| s.to_std_string_escaped())
            .map_err(|e| format!("failed to stringify result: {e}"))
    })
    .await
    .map_err(|e| format!("eval_ts panicked: {e}"))?
}
