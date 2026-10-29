use ai_assistant::{
    bootstrap, roles, router::EVAL_SAMPLES, AutoApprove, PermissionPrompt, PermissionRequest,
};
use std::io::{self, Write};
use tokio::sync::oneshot;

struct CliPrompt;

impl PermissionPrompt for CliPrompt {
    fn request_write(&self, request: PermissionRequest, responder: oneshot::Sender<bool>) {
        println!(
            "\n[权限确认] {} {}：{}\n允许写入？[y/N] ",
            request.level, request.path, request.preview
        );
        let _ = io::stdout().flush();

        std::thread::spawn(move || {
            let mut answer = String::new();
            let approved = io::stdin().read_line(&mut answer).is_ok()
                && matches!(answer.trim().to_lowercase().as_str(), "y" | "yes");
            let _ = responder.send(approved);
        });
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let runtime = bootstrap()?;

    match args.first().map(String::as_str) {
        None | Some("status") => {
            let status = runtime.status().await;
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        Some("chat") => {
            let mut role = roles::DEFAULT_ROLE.to_string();
            let mut message = String::new();
            let mut yes = false;
            let mut index = 1;

            while index < args.len() {
                if args[index] == "--role" && index + 1 < args.len() {
                    role = args[index + 1].clone();
                    index += 2;
                    continue;
                }
                if args[index] == "--yes" {
                    yes = true;
                    index += 1;
                    continue;
                }
                if !message.is_empty() {
                    message.push(' ');
                }
                message.push_str(&args[index]);
                index += 1;
            }

            if message.trim().is_empty() {
                anyhow::bail!(
                    "用法：cargo run -- chat [--role dev|general|health-coach|mind-companion] [--yes] 你的消息"
                );
            }

            let prompt: Box<dyn PermissionPrompt> = if yes {
                Box::new(AutoApprove)
            } else {
                Box::new(CliPrompt)
            };
            let reply = runtime.chat(&role, &message, Some(prompt.as_ref())).await?;
            println!(
                "[{} / {} / {}ms]\n{}",
                reply.role, reply.model, reply.elapsed_ms, reply.content
            );
            for trace in &reply.tool_calls {
                println!(
                    "[tool] {} path={} ok={} {}ms",
                    trace.name,
                    trace.path.as_deref().unwrap_or("-"),
                    trace.ok,
                    trace.elapsed_ms
                );
            }
        }
        Some("route") => {
            let message = args.iter().skip(1).cloned().collect::<Vec<_>>().join(" ");
            if message.trim().is_empty() {
                anyhow::bail!("用法：cargo run -- route 你的消息");
            }
            let decision = runtime.route(&message).await;
            println!("{}", serde_json::to_string_pretty(&decision)?);
        }
        Some("router-eval") => {
            println!("预热路由模型 {} ...", runtime.config.router.model);
            runtime.warmup_router().await;

            let mut correct = 0usize;
            for (index, (input, expected)) in EVAL_SAMPLES.iter().enumerate() {
                let decision = runtime.route(input).await;
                let ok = decision.role == *expected;
                if ok {
                    correct += 1;
                }
                let fallback_text = if decision.fallback { "是" } else { "否" };
                println!(
                    "#{:02} 期望={:<8} 实际={:<8} 置信度={:.2} 耗时={}ms 兜底={} 意图={} | {}",
                    index + 1,
                    expected,
                    decision.role,
                    decision.confidence,
                    decision.elapsed_ms,
                    fallback_text,
                    decision.intent,
                    input
                );
            }

            let total = EVAL_SAMPLES.len();
            let pass = correct * 10 >= total * 8;
            println!(
                "\n准确率 {correct}/{total}（{:.0}%），{}",
                correct as f64 / total as f64 * 100.0,
                if pass {
                    "通过（阈值 ≥8/10）"
                } else {
                    "未通过（阈值 ≥8/10）"
                }
            );
            if !pass {
                std::process::exit(1);
            }
        }
        Some("warmup") => {
            println!("预热路由模型 {} ...", runtime.config.router.model);
            runtime.warmup_router().await;
            println!("预热协调模型 {} ...", runtime.config.coordinator.model);
            runtime.warmup_coordinator().await;
        }
        Some("init") => {
            println!("data directory ready: {}", runtime.data_root.display());
        }
        Some(other) => {
            anyhow::bail!(
                "未知命令：{other}（可用：status / chat / route / router-eval / warmup / init）"
            );
        }
    }

    Ok(())
}
