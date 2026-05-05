//! Context actor — keeps a Boa `Context` alive on a dedicated blocking thread
//! so variables, imports, and function definitions persist across `eval_ts` calls.
//!
//! Registers a `nekobot` global object with `session`, `notify`, etc.

use boa_engine::property::Attribute;
use boa_engine::string::JsString;
use boa_engine::{Context, NativeFunction, Source};
use boa_runtime::extensions::{ConsoleExtension, FetchExtension};
use boa_runtime::fetch::BlockingReqwestFetcher;
use nekobot_core::agent::{Context as SessionContext, middleware::MiddlewareEvent};
use std::sync::{Arc, mpsc};
use tokio::sync::oneshot;
use tracing::debug;

type EvalResult = Result<serde_json::Value, String>;

/// State of a background task spawned by `spawn_ts`.
#[derive(Debug, Clone)]
pub enum ExecTaskState {
    Running,
    Done { output: String },
}

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
fn actor_loop(rx: mpsc::Receiver<ActorCommand>, session_ctx: SessionContext) {
    let mut ctx: Option<Context> = None;

    while let Ok(cmd) = rx.recv() {
        match cmd {
            ActorCommand::Eval { code, reply } => {
                let result = eval_inner(&mut ctx, &code, &session_ctx);
                let _ = reply.send(result);
            }
            ActorCommand::Reset { reply } => {
                ctx = None;
                let _ = reply.send(());
            }
        }
    }
}

fn create_context(session_ctx: &SessionContext) -> Result<Context, String> {
    let mut c = Context::default();
    boa_runtime::register(
        (
            ConsoleExtension::default(),
            FetchExtension(BlockingReqwestFetcher::default()),
        ),
        None,
        &mut c,
    )
    .map_err(|e| format!("failed to register Web API runtime: {e}"))?;

    register_nekobot(&mut c, session_ctx)?;

    Ok(c)
}

fn eval_inner(
    boa_ctx: &mut Option<Context>,
    code: &str,
    session_ctx: &SessionContext,
) -> Result<serde_json::Value, String> {
    let context = boa_ctx.get_or_insert_with(|| {
        create_context(session_ctx).unwrap_or_else(|e| {
            panic!("failed to create Boa context: {e}");
        })
    });
    eval_sync(context, code)
}

fn register_nekobot(c: &mut Context, session_ctx: &SessionContext) -> Result<(), String> {
    use boa_engine::object::ObjectInitializer;
    use boa_engine::value::JsValue;

    let session = ObjectInitializer::new(c)
        .property(
            JsString::from("id"),
            session_ctx.session_id,
            Attribute::all(),
        )
        .property(
            JsString::from("agentName"),
            JsString::from(session_ctx.agent_name.as_str()),
            Attribute::all(),
        )
        .build();

    let tx = session_ctx.event_sender.clone();
    // SAFETY: `NativeFunction::from_closure` requires the closure to not capture
    // or manipulate GC-managed JS values across yield points.
    // This closure only reads args (ephemeral) and sends via `mpsc::Sender`,
    // which owns no JS state. It never calls back into the JS engine.
    let notify_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            let msg = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            debug!(target: "actor", "notify called with message: {}", msg);
            let _ = tx.blocking_send(MiddlewareEvent::activate(msg));
            Ok(JsValue::undefined())
        })
    };

    let nekobot = ObjectInitializer::new(c)
        .property(JsString::from("session"), session, Attribute::all())
        .function(notify_fn, JsString::from("notify"), 1)
        .build();

    c.register_global_property(JsString::from("nekobot"), nekobot, Attribute::all())
        .map_err(|e| format!("failed to register nekobot: {e}"))?;

    Ok(())
}

/// Run a script in a standalone Boa context (used by spawn_background).
fn run_background(code: &str, session_ctx: &SessionContext) -> Result<serde_json::Value, String> {
    let mut c = create_context(session_ctx)?;
    eval_sync(&mut c, code)
}

fn eval_sync(context: &mut Context, code: &str) -> Result<serde_json::Value, String> {
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
///
/// `Send` + `Sync`: safe because the inner `mpsc::Sender` owns no JS state.
/// The actor thread is the sole owner of the Boa `Context`.
/// Cheaply cloneable (wraps an `mpsc::Sender`).
#[derive(Clone)]
pub struct ActorHandle {
    tx: mpsc::Sender<ActorCommand>,
}

impl ActorHandle {
    /// Spawn the actor thread with the given session context and return a handle.
    pub fn spawn(ctx: SessionContext) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || actor_loop(rx, ctx));
        Self { tx }
    }

    /// Spawn a one-shot background task with its own Boa context.
    /// The script runs in a new thread with an optional timeout.
    /// Writes the result to `registry` and exits when done.
    /// Returns a UUID taskId.
    pub fn spawn_background(
        ctx: SessionContext,
        code: String,
        timeout_ms: u64,
        registry: Arc<std::sync::RwLock<std::collections::HashMap<String, ExecTaskState>>>,
    ) -> String {
        let task_id = uuid::Uuid::new_v4().to_string();
        let tid = task_id.clone();
        registry
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(task_id.clone(), ExecTaskState::Running);

        let tid2 = tid.clone();
        let registry2 = Arc::clone(&registry);
        std::thread::spawn(move || {
            // Spawn a work thread and wait with optional timeout.
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let result = run_background(&code, &ctx);
                let _ = tx.send(result);
            });

            let result = if timeout_ms > 0 {
                match rx.recv_timeout(std::time::Duration::from_millis(timeout_ms)) {
                    Ok(r) => r,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        Err(format!("background task timed out after {timeout_ms}ms"))
                    }
                    Err(e) => Err(format!("background task error: {e}")),
                }
            } else {
                rx.recv()
                    .unwrap_or_else(|e| Err(format!("background task error: {e}")))
            };

            let state = match result {
                Ok(val) => ExecTaskState::Done {
                    output: val.to_string(),
                },
                Err(e) => ExecTaskState::Done { output: e },
            };
            registry2
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(tid2, state);
        });

        task_id
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
        match self.tx.send(ActorCommand::Reset { reply }) {
            Ok(()) => {
                let _ = rx.await;
            }
            Err(e) => {
                tracing::debug!("reset_ts: actor disconnected, ignoring: {e}");
            }
        }
    }
}
