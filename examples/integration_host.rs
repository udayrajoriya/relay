//! Test-only host for driving the actual Rust engine and bridge from browser automation.
use anyhow::{Context, Result};
use relay::{bridge::Bridge, engine::Engine};
use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
};

#[tokio::main]
async fn main() -> Result<()> {
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .context("expected a test directory")?,
    );
    let engine = Engine::open(root.join("data"), root.join("out"))?;
    let (send, mut folders) = tokio::sync::mpsc::unbounded_channel();
    let bridge = Bridge::start(engine.clone(), "extension-test-token".into(), 17864, send).await?;
    let destination = root.join("out");
    tokio::spawn(async move {
        while let Some(request) = folders.recv().await {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let _ = request.reply.send(Some(destination.clone()));
        }
    });
    println!("ready");
    io::stdout().flush()?;
    let (send, mut commands) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else {
                break;
            };
            if send.send(line).is_err() {
                break;
            }
        }
    });
    while let Some(command) = commands.recv().await {
        match command.as_str() {
            "state" => {
                println!("{}", serde_json::to_string(&engine.snapshot())?);
                io::stdout().flush()?;
            }
            "close" => break,
            _ => (),
        }
    }
    drop(bridge);
    engine.close().await
}
