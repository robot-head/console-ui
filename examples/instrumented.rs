//! Run with `cargo run --example instrumented`, then connect Console UI to localhost:6669.
use std::{sync::Arc, time::Duration};
use tokio::sync::{Mutex, Semaphore};
use tracing_subscriber::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let console = console_subscriber::ConsoleLayer::builder()
        .with_default_env()
        .spawn();
    tracing_subscriber::registry().with(console).init();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mutex = Arc::new(Mutex::new(0u64));
        let permits = Arc::new(Semaphore::new(2));
        for index in 0..8 {
            let mutex = Arc::clone(&mutex);
            let permits = Arc::clone(&permits);
            tokio::task::Builder::new()
                .name(&format!("worker-{index}"))
                .spawn(async move {
                    loop {
                        let permit = permits.acquire().await.unwrap();
                        {
                            // Deliberately hold a lock across a timer to make contention visible.
                            let mut counter = mutex.lock().await;
                            *counter += 1;
                            tokio::time::sleep(Duration::from_millis(80)).await;
                        }
                        drop(permit);
                        tokio::time::sleep(Duration::from_millis(100 + index * 15)).await;
                    }
                })?;
        }
        tokio::task::Builder::new()
            .name("self-waking-example")
            .spawn(async {
                loop {
                    for _ in 0..20 {
                        tokio::task::yield_now().await;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })?;
        println!(
            "Telemetry on http://127.0.0.1:6669 (or TOKIO_CONSOLE_BIND). Press Ctrl+C to stop."
        );
        tokio::signal::ctrl_c().await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })
}
