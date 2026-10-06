//! The same stdio server attaches to the open app or owns a headless session.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
#[cfg(unix)]
use rmcp::model::ContentBlock;
use rmcp::{ServiceExt, model::CallToolResult};
use serde_json::Value;

use crate::{
    Server, catalog,
    tools::{self, Backend},
};

pub(crate) enum Target {
    Local(Arc<Backend>),
    #[cfg(unix)]
    App(Arc<crate::ipc::Remote>),
}

impl Target {
    pub async fn call(self: Arc<Self>, name: String, args: Value) -> CallToolResult {
        match self.as_ref() {
            Self::Local(backend) => tools::call(backend.clone(), name, args).await,
            #[cfg(unix)]
            Self::App(remote) => {
                let remote = remote.clone();
                let result = tokio::task::spawn_blocking(move || remote.call(name, args)).await;
                match result {
                    Ok(Ok(result)) => result,
                    Ok(Err(e)) => CallToolResult::error(vec![ContentBlock::text(format!("{e:#}"))]),
                    Err(e) => {
                        CallToolResult::error(vec![ContentBlock::text(format!("IPC_FAILED: {e}"))])
                    }
                }
            }
        }
    }
}

pub fn run(project: &Path, allow_write: bool, cache: PathBuf) -> Result<()> {
    #[cfg(unix)]
    let target = match crate::ipc::Remote::connect(project, allow_write)? {
        Some(remote) => Target::App(Arc::new(remote)),
        None => Target::Local(Arc::new(Backend::open(project, allow_write, cache)?)),
    };
    #[cfg(not(unix))]
    let target = Target::Local(Arc::new(Backend::open(project, allow_write, cache)?));
    let target = Arc::new(target);
    let server = Server {
        target: target.clone(),
        tools: Arc::new(catalog()?),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("MCP_FAILED: starting runtime")?;
    let result = runtime.block_on(async {
        let service = server
            .serve(rmcp::transport::stdio())
            .await
            .context("MCP_FAILED: initialization")?;
        service.waiting().await.context("MCP_FAILED: transport")?;
        Ok(())
    });
    match target.as_ref() {
        Target::Local(backend) => {
            let finish = backend.disconnect();
            backend.host.jobs.shutdown();
            result.and(finish)
        }
        #[cfg(unix)]
        Target::App(_) => result,
    }
}

pub fn run_args(args: &[String], mut cache: PathBuf) -> Result<()> {
    let mut project = None;
    let mut allow_write = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--project" => {
                project = Some(PathBuf::from(
                    args.next()
                        .context("INVALID_ARGUMENTS: --project requires a path")?,
                ))
            }
            "--cache" => {
                cache = PathBuf::from(
                    args.next()
                        .context("INVALID_ARGUMENTS: --cache requires a directory")?,
                )
            }
            "--allow-write" => allow_write = true,
            _ => bail!("INVALID_ARGUMENTS: unknown MCP argument: {arg}"),
        }
    }
    run(
        &project.context("INVALID_ARGUMENTS: mcp requires --project <path>")?,
        allow_write,
        cache,
    )
}
