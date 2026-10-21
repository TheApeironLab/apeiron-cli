//! Rust CLI adapter. Domain rules remain behind Zygos's REST API.
//! Consumers can embed `parse` / `execute`, or call `run` with arguments excluding argv[0].
pub mod args;
mod auth;
mod baml;
pub mod client;
pub mod commands;
mod dataset_crud;
pub mod error;
mod evaluation_tasks;
mod field_groups;
mod guidance;
mod help;
pub mod io;
mod lifecycle;
mod presentation;

pub use args::{Invocation, Parsed, parse};
pub use client::Client;
pub use commands::execute;
pub use error::{CliError, Result};
pub use presentation::View;

pub async fn run(args: &[String]) -> i32 {
    run_with_command(args, &["zygos".into()]).await
}

/// Embed the CLI while preserving a runnable command prefix in generated guidance.
pub async fn run_with_command(args: &[String], command: &[String]) -> i32 {
    let mut output: io::Output = Box::new(tokio::io::stdout());
    let parsed = match parse(args) {
        Ok(parsed) => parsed,
        Err(error) => return emit_error(error, None, command).await,
    };
    let result = async {
        if let Some(help) = &parsed.help {
            let mut value = help::render(help, parsed.help_topic.as_deref(), parsed.view);
            prefix_help(&mut value, command);
            return io::emit(&mut output, &value, parsed.compact).await;
        }
        let invocation = parsed.invocation.as_ref().unwrap();
        if let Some(verb) = invocation.spec.name.strip_prefix("auth ") {
            let value = auth::execute(&parsed.base_url, verb).await?;
            let mut value = guidance::attach(value.clone(), &value, &parsed);
            if parsed.view != View::Full {
                prefix_guidance(&mut value, command);
            }
            return io::emit(&mut output, &value, parsed.compact).await;
        }
        let token = auth::access_token(&parsed.base_url).await?;
        let client = Client::new(&parsed.base_url, parsed.timeout_seconds)?.with_token(token);
        if let Some(value) = execute(
            &client,
            parsed.invocation.as_ref().unwrap(),
            &mut output,
            parsed.compact,
        )
        .await?
        {
            let displayed = presentation::render(
                &invocation.spec.name,
                value.clone(),
                parsed.view,
                &parsed.fields,
            )?;
            let mut value = guidance::attach(displayed, &value, &parsed);
            if parsed.view != View::Full {
                prefix_guidance(&mut value, command);
            }
            io::emit(&mut output, &value, parsed.compact).await?;
        }
        Ok(())
    }
    .await;
    match result {
        Ok(()) => 0,
        Err(error) => emit_error(error, Some(&parsed), command).await,
    }
}

async fn emit_error(error: CliError, parsed: Option<&Parsed>, command: &[String]) -> i32 {
    let mut stderr: io::Output = Box::new(tokio::io::stderr());
    let mut value = guidance::error(error.envelope, parsed);
    if !parsed.is_some_and(|p| p.view == View::Full) {
        prefix_guidance(&mut value, command);
    }
    let _ = io::emit(&mut stderr, &value, false).await;
    error.exit_code
}

fn prefix_guidance(value: &mut serde_json::Value, command: &[String]) {
    for key in ["full", "help"] {
        if let Some(argv) = value
            .get_mut("_cli")
            .and_then(|v| v.get_mut(key))
            .and_then(|v| v.as_array_mut())
        {
            if argv.first().and_then(|v| v.as_str()) == Some("zygos") {
                argv.splice(0..1, command.iter().cloned().map(serde_json::Value::String));
            }
        }
    }
}

fn prefix_help(value: &mut serde_json::Value, command: &[String]) {
    match value {
        serde_json::Value::String(text) => {
            *text = text.replace("zygos ", &format!("{} ", command.join(" ")))
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(|v| prefix_help(v, command)),
        serde_json::Value::Object(fields) => {
            fields.values_mut().for_each(|v| prefix_help(v, command))
        }
        _ => {}
    }
}
