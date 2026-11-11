//! Live check against a running Xunlei NAS client.
//!
//! Usage: `local_check [base_url]` (default `http://127.0.0.1:2345`).
//!
//! Prints the token source, device info summary, the daily task quota, and the
//! current task list — the same calls the official web UI makes.

use thunder_task_rs::LocalNas;
use url::Url;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "http://127.0.0.1:2345".to_string());
    let base = Url::parse(&base)?;

    println!("connecting to {base} ...");
    let nas = LocalNas::connect(base).await?;
    println!("token acquired from the client's own page");

    let info = nas.device_info().await?;
    println!("\n== device ==");
    println!("{}", serde_json::to_string_pretty(&info)?);

    println!("\n== daily task quota ==");
    match nas.quota().await {
        Ok(q) => match q.remaining() {
            Some(left) => println!("{} of {} used today; {left} left", q.usage, q.limit),
            None => println!("unlimited"),
        },
        Err(e) => println!("quota unavailable: {e}"),
    }

    println!("\n== tasks ==");
    let tasks = nas.tasks(Some(10)).await?;
    println!("{} task(s)", tasks.len());
    for t in tasks.iter().take(5) {
        println!("  {} [{}] {}", t.id, t.phase.as_str(), t.name);
    }

    Ok(())
}
