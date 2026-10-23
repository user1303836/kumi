use crate::tui::tty::TtyOutput;
use std::time::{Duration, Instant};
use tokio::{runtime::Runtime, task::LocalSet};

const GRACE: Duration = Duration::from_secs(2);

pub(super) fn finish(runtime: Runtime, local: LocalSet, mut code: i32, explicit_exit: bool, err: &dyn TtyOutput) -> i32 {
    if explicit_exit {
        // The source explicitly exits after /update's replacement process finishes.
        drop(local);
        runtime.shutdown_timeout(Duration::ZERO);
        return code;
    }
    // Returning from block_on does not finish detached local tools. Keep polling
    // them as Node keeps pending I/O alive, bounded by the source CLI watchdog.
    let began = Instant::now();
    let drained = runtime.block_on(async { tokio::time::timeout(GRACE, local).await.is_ok() });
    if !drained {
        err.write("Kumi shutdown left a live handle; terminating this Kumi process.\n");
        code = 1;
    }
    // Blocking file work shares the same grace, rather than adding another two seconds.
    runtime.shutdown_timeout(GRACE.saturating_sub(began.elapsed()));
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::{stream, StreamExt};
    use kumi_agent::{
        core::contracts::StopReason,
        kernel::agent::{create_agent_kernel, AgentKernelOptions},
    };
    use kumi_ai::{
        binding::{LanguageModel, ModelBinding},
        error::LanguageModelError,
        types::*,
    };
    use kumi_common::abort::Signal;
    use kumi_types::{
        contracts::{JsonObject, KernelTool, ToolResult},
        errors::RuntimeError,
    };
    use std::{
        cell::RefCell,
        io::Write,
        process::{Command, Stdio},
        rc::Rc,
        time::Duration,
    };

    const FIXTURE: &str = "KUMI_CLI_SHUTDOWN_FIXTURE";

    struct Model;
    #[async_trait(?Send)]
    impl LanguageModel for Model {
        async fn do_stream(&self, _: CallOptions) -> Result<StreamParts, LanguageModelError> {
            Ok(stream::iter([
                StreamPart::ToolCall(ToolCall {
                    tool_call_id: "c1".into(),
                    tool_name: "settle".into(),
                    input: "{}".into(),
                    provider_executed: None,
                    dynamic: None,
                    provider_metadata: None,
                }),
                StreamPart::Finish {
                    usage: Usage::default(),
                    finish_reason: FinishReason { unified: FinishReasonUnified::ToolCalls, raw: None },
                    provider_metadata: None,
                },
            ])
            .boxed_local())
        }
    }

    struct Tool {
        entered: RefCell<Option<tokio::sync::oneshot::Sender<()>>>,
        never: bool,
    }
    #[async_trait(?Send)]
    impl KernelTool for Tool {
        fn name(&self) -> &str {
            "settle"
        }
        fn description(&self) -> &str {
            "A dispatched operation settling after cancellation"
        }
        fn input_schema(&self) -> JsonObject {
            serde_json::from_value(serde_json::json!({"type":"object"})).unwrap()
        }
        async fn execute(&self, _: JsonObject, _: Signal) -> Result<ToolResult, RuntimeError> {
            println!("[shutdown] admitted");
            self.entered.borrow_mut().take().unwrap().send(()).unwrap();
            if self.never {
                loop {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            println!("[shutdown] settled");
            tokio::time::sleep(Duration::from_millis(25)).await;
            println!("[shutdown] cleanup");
            Ok(ToolResult::text("settled"))
        }
    }

    #[test]
    fn shutdown_process() {
        let Ok(case) = std::env::var(FIXTURE) else { return };
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let local = LocalSet::new();
        let code = local.block_on(&runtime, async {
            let (entered, admitted) = tokio::sync::oneshot::channel();
            let kernel = Rc::new(
                create_agent_kernel(AgentKernelOptions {
                    conversation: None,
                    instructions: "fixture".into(),
                    signal: Signal::new(),
                    checkpoint: None,
                    max_steps: None,
                    budget: None,
                    tools: vec![Rc::new(Tool { entered: RefCell::new(Some(entered)), never: case == "deadline" })],
                    binding: ModelBinding {
                        id: "test/fixture".into(),
                        model: Rc::new(Model),
                        budget: None,
                        prepare: Box::new(|request| CallOptions {
                            prompt: request.messages,
                            tools: Some(request.tools),
                            ..Default::default()
                        }),
                    },
                })
                .unwrap(),
            );
            let signal = Signal::new();
            let running = tokio::task::spawn_local({
                let kernel = kernel.clone();
                let signal = signal.clone();
                async move { kernel.run("go", signal, Rc::new(|_| Ok(()))).await.unwrap() }
            });
            admitted.await.unwrap();
            signal.cancel();
            assert_eq!(running.await.unwrap().stop_reason, StopReason::Cancelled);
            kernel.close().await;
            println!("[shutdown] cli-return");
            if case == "error" {
                7
            } else {
                0
            }
        });
        let code = finish(runtime, local, code, case == "update", &crate::tui::tty::Stdout::stderr());
        println!("[shutdown] exit:{code}");
        std::io::stdout().flush().unwrap();
        std::process::exit(code);
    }

    fn probe(case: &str) -> std::process::Output {
        let home = tempfile::tempdir().unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "cli::native::shutdown::tests::shutdown_process", "--nocapture"]).env(FIXTURE, case);
        for key in ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "KUMI_HOME"] {
            command.env(key, home.path());
        }
        command.env_remove("KUMI_REMOTE_SCRIPTS_DIR").env_remove("KUMI_LIVE_EXTENSIONS_DIR");
        let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("CLI shutdown did not respect its bounded grace");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        child.wait_with_output().unwrap()
    }

    fn trace(output: &std::process::Output) -> Vec<String> {
        String::from_utf8_lossy(&output.stdout).lines().filter_map(|line| line.strip_prefix("[shutdown] ").map(str::to_string)).collect()
    }

    #[test]
    fn normal_exit_allows_admitted_work_to_settle_and_clean_up() {
        let output = probe("normal");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(trace(&output), ["admitted", "cli-return", "settled", "cleanup", "exit:0"]);
    }

    #[test]
    fn normal_exit_enforces_the_source_two_second_watchdog() {
        let began = std::time::Instant::now();
        let output = probe("deadline");
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(trace(&output), ["admitted", "cli-return", "exit:1"]);
        assert!(String::from_utf8_lossy(&output.stderr).contains("Kumi shutdown left a live handle; terminating this Kumi process."));
        assert!(began.elapsed() >= Duration::from_secs(2));
        assert!(began.elapsed() < Duration::from_secs(10), "shutdown exceeded its bounded grace");
    }

    #[test]
    fn update_keeps_the_sources_explicit_exit() {
        let output = probe("update");
        assert!(output.status.success());
        assert_eq!(trace(&output), ["admitted", "cli-return", "exit:0"]);
    }

    #[test]
    fn completed_cleanup_preserves_the_commands_exit_status() {
        let output = probe("error");
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(trace(&output), ["admitted", "cli-return", "settled", "cleanup", "exit:7"]);
    }
}
