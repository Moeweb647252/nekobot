use anyhow::{Result, anyhow};
use boa_engine::context::time::JsInstant;
use boa_engine::job::{GenericJob, TimeoutJob};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{
    Context, JsResult, Script, Source,
    context::ContextBuilder,
    job::{Job, JobExecutor, NativeAsyncJob, PromiseJob},
};
use boa_engine::{Finalize, JsData, JsError, JsString, JsValue, NativeFunction, Trace};
use boa_runtime::fetch::Fetcher;
use boa_runtime::fetch::request::JsRequest;
use boa_runtime::fetch::response::JsResponse;
use futures_concurrency::future::FutureGroup;
use futures_lite::{StreamExt, future};
use nekobot_core::agent::middleware::MiddlewareEvent;
use serde_json::Value;
use std::collections::BTreeMap;
use std::ops::DerefMut;
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
use tokio::sync::{mpsc, oneshot};
use tokio::task;
use tracing::{debug, error};
use turso::Connection;

pub struct NekobotContext {
    pub event_sender: mpsc::Sender<MiddlewareEvent>,
    pub app_db: Connection,
    pub session_id: i64,
    pub agent_name: String,
}

/// An event queue using tokio to drive futures to completion.
struct Queue {
    async_jobs: RefCell<VecDeque<NativeAsyncJob>>,
    promise_jobs: RefCell<VecDeque<PromiseJob>>,
    timeout_jobs: RefCell<BTreeMap<JsInstant, TimeoutJob>>,
    generic_jobs: RefCell<VecDeque<GenericJob>>,
}

impl Queue {
    fn new() -> Self {
        Self {
            async_jobs: RefCell::default(),
            promise_jobs: RefCell::default(),
            timeout_jobs: RefCell::default(),
            generic_jobs: RefCell::default(),
        }
    }

    fn drain_timeout_jobs(&self, context: &mut Context) {
        let now = context.clock().now();

        let mut timeouts_borrow = self.timeout_jobs.borrow_mut();
        let mut jobs_to_keep = timeouts_borrow.split_off(&now);
        jobs_to_keep.retain(|_, job| !job.is_cancelled());
        let jobs_to_run = std::mem::replace(timeouts_borrow.deref_mut(), jobs_to_keep);
        drop(timeouts_borrow);

        for job in jobs_to_run.into_values() {
            if let Err(e) = job.call(context) {
                error!(target:"js runtime", "Uncaught {e}");
            }
        }
    }

    fn drain_jobs(&self, context: &mut Context) {
        // Run the timeout jobs first.
        self.drain_timeout_jobs(context);

        let job = self.generic_jobs.borrow_mut().pop_front();
        if let Some(generic) = job
            && let Err(err) = generic.call(context)
        {
            eprintln!("Uncaught {err}");
        }

        let jobs = std::mem::take(&mut *self.promise_jobs.borrow_mut());
        for job in jobs {
            if let Err(e) = job.call(context) {
                eprintln!("Uncaught {e}");
            }
        }
        context.clear_kept_objects();
    }
}

impl JobExecutor for Queue {
    fn enqueue_job(self: Rc<Self>, job: Job, context: &mut Context) {
        match job {
            Job::PromiseJob(job) => self.promise_jobs.borrow_mut().push_back(job),
            Job::AsyncJob(job) => self.async_jobs.borrow_mut().push_back(job),
            Job::TimeoutJob(t) => {
                let now = context.clock().now();
                self.timeout_jobs.borrow_mut().insert(now + t.timeout(), t);
            }
            Job::GenericJob(g) => self.generic_jobs.borrow_mut().push_back(g),
            _ => panic!("unsupported job type"),
        }
    }

    // While the sync flavor of `run_jobs` will block the current thread until all the jobs have finished...
    fn run_jobs(self: Rc<Self>, context: &mut Context) -> JsResult<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();

        task::LocalSet::default().block_on(&runtime, self.run_jobs_async(&RefCell::new(context)))
    }

    // ...the async flavor won't, which allows concurrent execution with external async tasks.
    async fn run_jobs_async(self: Rc<Self>, context: &RefCell<&mut Context>) -> JsResult<()> {
        let mut group = FutureGroup::new();
        loop {
            for job in std::mem::take(&mut *self.async_jobs.borrow_mut()) {
                group.insert(job.call(context));
            }

            if group.is_empty()
                && self.promise_jobs.borrow().is_empty()
                && self.timeout_jobs.borrow().is_empty()
                && self.generic_jobs.borrow().is_empty()
            {
                // All queues are empty. We can exit.
                return JsResult::Ok(());
            }

            // We have some jobs pending on the microtask queue. Try to poll the pending
            // tasks once to see if any of them finished, and run the pending microtasks
            // otherwise.
            if let Some(Err(err)) = future::poll_once(group.next()).await.flatten() {
                error!(target:"js runtime", "Uncaught {err}");
            };

            // Only one macrotask can be executed before the next drain of the microtask queue.
            self.drain_jobs(&mut context.borrow_mut());
            task::yield_now().await
        }
    }
}

pub struct EvalTask {
    pub code: String,
    pub result_sender: oneshot::Sender<Result<Value>>,
}

pub struct Runtime {
    context: RefCell<Context>,
    queue: Rc<Queue>,
    task_receiver: mpsc::UnboundedReceiver<EvalTask>,
}

impl Runtime {
    pub fn try_new(
        receiver: mpsc::UnboundedReceiver<EvalTask>,
        ctx: NekobotContext,
    ) -> Result<Self> {
        let queue = Rc::new(Queue::new());
        let context = ContextBuilder::new()
            .job_executor(queue.clone())
            .build()
            .map_err(|e| anyhow!("Failed to build boa context: {}", e))?;
        let mut runtime = Self {
            context: RefCell::new(context),
            queue,
            task_receiver: receiver,
        };
        runtime.add_runtime(ctx);
        Ok(runtime)
    }

    /// Adds the custom runtime to the context.
    fn add_runtime(&mut self, ctx: NekobotContext) {
        let mut context = self.context.borrow_mut();
        boa_runtime::register(
            (
                // A fetcher can be added if the `fetch` feature flag is enabled.
                // This fetcher uses the Reqwest blocking API to allow fetching using HTTP.
                boa_runtime::extensions::FetchExtension(ReqwestFetcher::default()),
                boa_runtime::extensions::TimeoutExtension,
            ),
            None,
            &mut context,
        )
        .unwrap();

        let session = ObjectInitializer::new(&mut context)
            .property(JsString::from("id"), ctx.session_id, Attribute::all())
            .property(
                JsString::from("agentName"),
                JsString::from(ctx.agent_name.as_str()),
                Attribute::all(),
            )
            .build();

        let tx = ctx.event_sender.clone();
        let notify_fn = unsafe {
            NativeFunction::from_closure(move |_this, args, _ctx| {
                let msg = args
                    .first()
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_std_string_escaped())
                    .unwrap_or_default();
                debug!(target: "actor", "notify called with message: {}", msg);
                tx.try_send(MiddlewareEvent::activate(msg))
                    .map(|_| JsValue::undefined())
                    .map_err(|e| JsError::from_rust(e))
            })
        };

        let nekobot = ObjectInitializer::new(&mut context)
            .property(JsString::from("session"), session, Attribute::all())
            .function(notify_fn, JsString::from("notify"), 1)
            .build();

        context
            .register_global_property(JsString::from("nekobot"), nekobot, Attribute::all())
            .map_err(|e| format!("failed to register nekobot: {e}"))
            .unwrap();
    }

    pub async fn start(&mut self) {
        // Initialize the queue and the context
        let mut context = self.context.borrow_mut();
        let context = RefCell::new(context.deref_mut());

        let local_set = &mut task::LocalSet::default();
        let queue = self.queue.clone();
        let engine = local_set.run_until(async {
            let mut current_job = queue
                .async_jobs
                .borrow_mut()
                .pop_front()
                .map(|job| job.call(&context));

            loop {
                if self.task_receiver.is_closed()
                    && queue.promise_jobs.borrow().is_empty()
                    && queue.timeout_jobs.borrow().is_empty()
                    && queue.generic_jobs.borrow().is_empty()
                    && current_job.is_none()
                {
                    break;
                }
                if let Ok(task) = self.task_receiver.try_recv() {
                    let EvalTask {
                        code,
                        result_sender,
                    } = task;

                    match {
                        let context = &mut context.borrow_mut();
                        Script::parse(Source::from_bytes(&code), None, context)
                    } {
                        Ok(script) => match {
                            let context = &mut context.borrow_mut();
                            script.evaluate_async_with_budget(context, u32::MAX).await
                        } {
                            Ok(result) => {
                                let context = &mut context.borrow_mut();
                                match result.to_json(context) {
                                    Ok(json) => {
                                        result_sender.send(Ok(json.unwrap_or(Value::Null))).ok();
                                    }
                                    Err(e) => {
                                        result_sender
                                            .send(Err(anyhow!(
                                                "Failed to convert result to JSON: {e}"
                                            )))
                                            .ok();
                                    }
                                }
                            }
                            Err(e) => {
                                result_sender.send(Err(anyhow!("Runtime error: {e}"))).ok();
                            }
                        },
                        Err(e) => {
                            result_sender.send(Err(anyhow!("Parse error: {e}"))).ok();
                            continue;
                        }
                    }
                }
                if let Some(job) = &mut current_job {
                    if let Some(ret) = future::poll_once(job).await {
                        if let Err(err) = ret {
                            error!(target:"js runtime", "Uncaught {err}");
                        }
                        current_job = queue
                            .async_jobs
                            .borrow_mut()
                            .pop_front()
                            .map(|job| job.call(&context));
                    }
                }
                queue.drain_jobs(context.borrow_mut().deref_mut());
                task::yield_now().await;
            }
            #[allow(unreachable_code)]
            JsResult::Ok(())
        });
        if let Err(e) = engine.await {
            error!("Runtime error: {e}");
        }
    }
}
#[derive(Debug, Clone, Trace, Finalize, JsData)]
pub struct ReqwestFetcher {
    #[unsafe_ignore_trace]
    client: reqwest::Client,
}

impl Fetcher for ReqwestFetcher {
    async fn fetch(
        self: Rc<Self>,
        request: JsRequest,
        _context: &RefCell<&mut Context>,
    ) -> JsResult<JsResponse> {
        let request = request.into_inner();
        let url = request.uri().to_string();
        let req = reqwest::Client::new()
            .request(request.method().clone(), &url)
            .headers(request.headers().clone());

        let req = req
            .body(request.body().clone())
            .build()
            .map_err(JsError::from_rust)?;

        let resp = self.client.execute(req).await.map_err(JsError::from_rust)?;

        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.bytes().await.map_err(JsError::from_rust)?;
        let mut builder = http::Response::builder().status(status.as_u16());

        for k in headers.keys() {
            for v in headers.get_all(k) {
                builder = builder.header(k.as_str(), v);
            }
        }

        builder
            .body(bytes.to_vec())
            .map_err(JsError::from_rust)
            .map(|request| JsResponse::basic(JsString::from(url), request))
    }
}

impl Default for ReqwestFetcher {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeHandle(pub mpsc::UnboundedSender<EvalTask>);

impl RuntimeHandle {
    pub fn new() -> (Self, mpsc::UnboundedReceiver<EvalTask>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self(tx), rx)
    }

    pub async fn eval(&self, code: String) -> Result<Value> {
        let (result_tx, result_rx) = oneshot::channel();
        self.0
            .send(EvalTask {
                code,
                result_sender: result_tx,
            })
            .map_err(|e| anyhow!("Failed to send eval task: {e}"))?;
        result_rx
            .await
            .map_err(|e| anyhow!("Eval task cancelled: {e}"))?
    }
}
