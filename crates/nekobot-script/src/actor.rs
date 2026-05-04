//! Context actor — keeps a Boa `Context` alive on a dedicated blocking thread
//! so variables, imports, and function definitions persist across `eval_ts` calls.

use boa_engine::{Context, Source};
use boa_runtime::extensions::{ConsoleExtension, FetchExtension};
use boa_runtime::fetch::BlockingReqwestFetcher;
use std::sync::mpsc;
use tokio::sync::oneshot;

type EvalResult = Result<serde_json::Value, String>;

enum ActorCommand {
    Eval {
        code: String,
        reply: oneshot::Sender<EvalResult>,
    },
    Reset {
        reply: oneshot::Sender<()>,
    },
}

/// Runs on a dedicated thread. Receives eval/reset commands via `mpsc`.
fn actor_loop(rx: mpsc::Receiver<ActorCommand>) {
    let mut ctx: Option<Context> = None;

    while let Ok(cmd) = rx.recv() {
        match cmd {
            ActorCommand::Eval { code, reply } => {
                let result = eval_inner(&mut ctx, &code);
                let _ = reply.send(result);
            }
            ActorCommand::Reset { reply } => {
                ctx = None;
                let _ = reply.send(());
            }
        }
    }
}

fn eval_inner(ctx: &mut Option<Context>, code: &str) -> Result<serde_json::Value, String> {
    let context = ctx.get_or_insert_with(|| {
        let mut c = Context::default();
        boa_runtime::register(
            (
                ConsoleExtension::default(),
                FetchExtension(BlockingReqwestFetcher::default()),
            ),
            None,
            &mut c,
        )
        .inspect_err(|e| tracing::warn!("Failed to register Web API runtime: {e}"))
        .ok();
        c
    });

    let result = context
        .eval(Source::from_bytes(code))
        .map_err(|e| format!("JS execution error: {e}"))?;

    context
        .run_jobs()
        .map_err(|e| format!("JS job error: {e}"))?;

    result
        .to_json(context)
        .map_err(|e| format!("failed to convert to JSON: {e}"))
        .map(|v| v.unwrap_or(serde_json::Value::Null))
}

/// Handle for communicating with the background actor thread.
/// Cheaply cloneable (wraps an `mpsc::Sender`).
#[derive(Clone)]
pub struct ActorHandle {
    tx: mpsc::Sender<ActorCommand>,
}

impl ActorHandle {
    /// Spawn the actor thread and return a handle.
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || actor_loop(rx));
        Self { tx }
    }

    /// Evaluate JS code in the persistent context.
    pub async fn eval(&self, code: String) -> EvalResult {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(ActorCommand::Eval { code, reply })
            .map_err(|e| format!("actor disconnected: {e}"))?;
        rx.await.map_err(|e| format!("actor reply dropped: {e}"))?
    }

    /// Reset the context (clear all state).
    pub async fn reset(&self) {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(ActorCommand::Reset { reply }).is_ok() {
            let _ = rx.await;
        }
    }
}
