use anyhow::{Context, Result};
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum Action {
    /// Inspect backend-owned caches from the latest launch, without modifying them
    Status {
        #[arg(long)]
        json: bool,
    },
}
pub fn run(action: Action) -> Result<()> {
    let Action::Status { json } = action;
    let directory = nucleon_core::paths::latest_run_dir()?
        .context("No launch receipt found; launch an application first")?;
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join("receipt.json"))?)?;
    let mut report = receipt
        .get("backend_caches")
        .filter(|v| !v.is_null())
        .cloned()
        .map(serde_json::from_value::<nucleon_core::backend_cache::D3dMetal>)
        .transpose()?;
    if let Some(report) = &mut report {
        report.refresh();
    }
    let output = serde_json::json!({
        "run_id": receipt["run_id"], "launch_engine": receipt["engine"],
        "graphics_backend": receipt["graphics_backend"], "snapshot": "current-filesystem",
        "backend_caches": report, "nucleon_managed_caches": receipt["shader_caches"],
        "note": "File presence and size do not measure shader cache hits or validate compatibility across updates."
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if let Some(report) = report {
        println!("Graphics: D3DMetal (configured); cache owner: backend/application");
        if let Some(cache) = report.cache {
            println!(
                "{}: {} files, {:.1} MiB ({})",
                cache.root.display(),
                cache.files.len(),
                cache.bytes as f64 / 1048576.0,
                cache.state
            );
            for error in cache.errors {
                println!("  {error}");
            }
        }
        if let Some(error) = report.discovery_error {
            println!("Cache discovery unavailable: {error}");
        }
        for cache in report.unreal {
            println!(
                "Unreal {}: {} pipeline-cache files, {} bytes ({})",
                cache.root.display(),
                cache.files.len(),
                cache.bytes,
                cache.state
            );
        }
        println!(
            "OS {} ({}); {:?}; runtime {}",
            report.os_version.as_deref().unwrap_or("unknown"),
            report.os_build.as_deref().unwrap_or("unknown"),
            report.process_architecture,
            report.runtime.display()
        );
        println!("{}", output["note"].as_str().unwrap());
    } else {
        println!(
            "No backend-owned cache report in the latest receipt. New D3DMetal launches record it automatically."
        );
        if !receipt["shader_caches"].is_null() {
            println!(
                "Managed caches: {}",
                serde_json::to_string_pretty(&receipt["shader_caches"])?
            );
        }
    }
    Ok(())
}
