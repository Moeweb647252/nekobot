//! JavaScript execution via Boa engine.

use boa_engine::{Context, Source};

/// Execute JavaScript code in a fresh Boa context.
///
/// Creates a new [`Context`] per call to avoid `!Send` / `!Sync` threading
/// issues.  Boa contexts are lightweight enough for this to be practical.
pub fn execute(js_code: &str) -> Result<String, String> {
    let mut context = Context::default();

    match context.eval(Source::from_bytes(js_code)) {
        Ok(res) => res
            .to_string(&mut context)
            .map(|s| s.to_std_string_escaped())
            .map_err(|e| format!("failed to stringify result: {e}")),
        Err(e) => Err(format!("JS execution error: {e}")),
    }
}
