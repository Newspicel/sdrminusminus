use std::{
    future::Future,
    sync::{Mutex, PoisonError},
};

use tokio::{
    runtime::{Builder, Handle, Runtime},
    sync::oneshot,
    task::JoinHandle,
};

use crate::error::CoreError;

const WORKER_THREADS: usize = 2;
const THREAD_NAME: &str = "sdrmm-core";

pub(crate) struct CoreRuntime {
    runtime: Mutex<Option<Runtime>>,
    handle: Handle,
}

impl CoreRuntime {
    pub(crate) fn start() -> Result<Self, CoreError> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(WORKER_THREADS)
            .thread_name(THREAD_NAME)
            .enable_io()
            .enable_time()
            .build()
            .map_err(|error| CoreError::internal(format!("Runtime failed: {error}")))?;
        Ok(Self {
            handle: runtime.handle().clone(),
            runtime: Mutex::new(Some(runtime)),
        })
    }

    pub(crate) async fn run<F, T>(&self, work: F) -> Result<T, CoreError>
    where
        F: Future<Output = Result<T, CoreError>> + Send + 'static,
        T: Send + 'static,
    {
        if !self.running() {
            return Err(CoreError::stopped());
        }
        let (done, result) = oneshot::channel();
        self.handle.spawn(async move {
            let _ = done.send(work.await);
        });
        result.await.map_err(|_| CoreError::stopped())?
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn spawn<F>(&self, task: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.handle.spawn(task)
    }

    pub(crate) fn shutdown(&self) {
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(runtime) = runtime {
            runtime.shutdown_background();
        }
    }

    fn running(&self) -> bool {
        self.runtime
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }
}

impl Drop for CoreRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn async_calls_complete_without_a_foreign_runtime() {
        let runtime = CoreRuntime::start().expect("runtime");
        let value = futures::executor::block_on(runtime.run(async {
            tokio::time::sleep(Duration::from_millis(1)).await;
            Ok(42)
        }));
        assert_eq!(value, Ok(42));
    }

    #[test]
    fn work_runs_on_the_core_threads() {
        let runtime = CoreRuntime::start().expect("runtime");
        let name = futures::executor::block_on(
            runtime.run(async { Ok(std::thread::current().name().map(str::to_owned)) }),
        );
        assert_eq!(name, Ok(Some(THREAD_NAME.to_owned())));
    }

    #[test]
    fn errors_come_back_unchanged() {
        let runtime = CoreRuntime::start().expect("runtime");
        let result: Result<(), CoreError> =
            futures::executor::block_on(runtime.run(async { Err(CoreError::NoMission) }));
        assert_eq!(result, Err(CoreError::NoMission));
    }

    #[test]
    fn calls_after_shutdown_are_refused() {
        let runtime = CoreRuntime::start().expect("runtime");
        runtime.shutdown();
        runtime.shutdown();
        let result = futures::executor::block_on(runtime.run(async { Ok(1) }));
        assert_eq!(result, Err(CoreError::stopped()));
    }

    #[test]
    fn shutdown_ends_a_pending_call() {
        let runtime = std::sync::Arc::new(CoreRuntime::start().expect("runtime"));
        let caller = std::thread::spawn({
            let runtime = runtime.clone();
            move || {
                futures::executor::block_on(runtime.run(async {
                    std::future::pending::<()>().await;
                    Ok(())
                }))
            }
        });
        std::thread::sleep(Duration::from_millis(20));
        runtime.shutdown();
        assert_eq!(caller.join().expect("joined"), Err(CoreError::stopped()));
    }

    #[test]
    fn shutdown_from_a_core_thread_does_not_panic() {
        let runtime = std::sync::Arc::new(CoreRuntime::start().expect("runtime"));
        let task = runtime.spawn({
            let runtime = runtime.clone();
            async move { runtime.shutdown() }
        });
        assert!(futures::executor::block_on(task).is_ok());
        assert!(!runtime.running());
    }
}
