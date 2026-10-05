//! Evaluation-only bridge to the real local composition. No credentials enter this process.
use std::{path::PathBuf, time::Duration};

use clap::Parser;
use serde::Deserialize;
use serde_json::json;
use xgen_cli::{
    DriverProgressControl, InferenceLimits, LocalCommandResult, LocalRunRequest, RequestOptions,
    run_local_with_evaluation_profile,
};
use xgen_provider_openai::{EvaluationProposalStepLimit, ResponseFormat, ThinkingMode};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    workspace: PathBuf,
    #[arg(long)]
    proxy_url: String,
    #[arg(long)]
    condition: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrialConfig {
    goal: String,
    model: String,
    tokenizer: String,
    response_format: ResponseFormat,
    thinking: ThinkingMode,
    max_output_tokens: u32,
    request_timeout_seconds: u64,
    max_model_turns: u32,
    max_ticks: u32,
    allow_executables: Vec<String>,
    #[serde(default)]
    final_response_schema: Option<serde_json::Value>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let config: TrialConfig = serde_json::from_slice(&std::fs::read(&args.config)?)?;
    let limit = match args.condition.as_str() {
        "X0" => None,
        "X1" => Some(EvaluationProposalStepLimit::One),
        "XN" => Some(EvaluationProposalStepLimit::Four),
        _ => return Err("unknown condition".into()),
    };
    let mut request = LocalRunRequest::with_defaults(
        config.goal,
        args.workspace,
        args.proxy_url,
        config.model,
        config.tokenizer,
        Vec::new(),
    );
    request.inference_limits = InferenceLimits::new(
        Duration::from_secs(config.request_timeout_seconds),
        config.max_output_tokens,
    )?;
    request.request_options = RequestOptions {
        response_format: config.response_format,
        thinking: config.thinking,
    };
    request.allow_dirs = vec![".".into()];
    request.allow_executables = config.allow_executables;
    request.allow_remote_model_egress = true;
    request.allow_read = true;
    request.allow_write = true;
    request.allow_execute = !request.allow_executables.is_empty();
    request.max_model_turns = Some(config.max_model_turns);
    request.final_response_schema = config.final_response_schema;
    request.max_ticks = config.max_ticks;
    let result = run_local_with_evaluation_profile(
        request,
        limit,
        |run_id| println!("{}", json!({"event":"started", "run_id":run_id})),
        |_| DriverProgressControl::Continue,
    );
    let output = match result {
        Ok(LocalCommandResult::Completed { run_id, summary }) => {
            json!({"event":"result","outcome":"completed","run_id":run_id,"summary":summary})
        }
        Ok(other) => json!({"event":"result","outcome":"incomplete","reason":format!("{other:?}")}),
        Err(error) => json!({"event":"result","outcome":"error","reason":format!("{error:?}")}),
    };
    println!("{output}");
    Ok(())
}
