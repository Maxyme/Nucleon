use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use nucleon_core::{
    application::Settings,
    upscaler::{self, Backend, Input, Installation},
};
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum Action {
    /// Fetch/verify a stock, metalfx, native, bridge or patcher provider; never starts Wine
    Install {
        provider: String,
        #[arg(long)]
        archive: Option<PathBuf>,
        /// Complete matched Intel GPTK runtime; required for native Metal preparation
        #[arg(long)]
        wine: Option<String>,
        /// Reuse verified native source instead of downloading it
        #[arg(long)]
        source: Option<PathBuf>,
    },
    /// Resolve and stage game-local DLLs, without running Wine or the game
    Prepare {
        executable: PathBuf,
        #[arg(long)]
        backend: Backend,
        #[arg(long, default_value = "auto")]
        input: Input,
        /// Rendering EXE, relative to the launch EXE's directory (or absolute within it)
        #[arg(long)]
        renderer: Option<PathBuf>,
        #[arg(long)]
        wine: Option<String>,
        #[arg(long)]
        emulator: Option<nucleon_core::runtime_selection::Emulator>,
        #[arg(long, default_value = "auto")]
        engine: String,
        #[arg(long)]
        graphics_api: Option<String>,
        /// Only inspect this existing prefix for active processes; never initialize it
        #[arg(long)]
        prefix: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Inspect ownership, file integrity and requested vs observed activation
    Status { executable: PathBuf },
    /// Restore Nucleon-owned game files; preserve edited settings in the preparation record
    Restore { executable: PathBuf },
    /// Transfer the existing Talos journal without losing its original backups or launching it
    MigrateTalos,
}
pub fn run(action: Action) -> Result<()> {
    let root = upscaler::root();
    match action {
        Action::Install {
            provider,
            archive,
            wine,
            source,
        } => {
            if provider == "patcher" {
                return upscaler::install_patcher(&root, archive.as_deref());
            }
            let runtime = wine
                .as_deref()
                .map(nucleon_core::runtime_selection::named)
                .transpose()?;
            if matches!(provider.as_str(), "native" | "bridge") {
                ensure!(
                    runtime
                        .as_ref()
                        .is_some_and(|r| r.mode == nucleon_core::runtime::ExecutionMode::Intel),
                    "Native FSR4 preparation needs --wine pointing to a complete Intel GPTK runtime"
                );
            }
            upscaler::install(
                &root,
                &provider,
                archive.as_deref(),
                runtime.as_ref().map(|r| r.root.as_path()),
                source.as_deref(),
            )?;
        }
        Action::Prepare {
            executable,
            backend,
            input,
            renderer,
            wine,
            emulator,
            engine,
            graphics_api,
            prefix,
            dry_run,
        } => {
            let executable = executable
                .canonicalize()
                .context("Cannot resolve launch executable")?;
            let settings = Settings {
                wine,
                emulator,
                engine: Some(engine),
                graphics_api: graphics_api
                    .map(|value| {
                        serde_json::from_value::<nucleon_core::profile::GraphicsPreference>(
                            value.into(),
                        )
                    })
                    .transpose()?,
                upscaler: Some(upscaler::Settings {
                    backend,
                    input,
                    renderer: renderer.clone(),
                }),
                ..Default::default()
            };
            let plan = nucleon_core::application::resolve_launch(
                &executable,
                &root,
                prefix.as_deref().or(Some(&root.join("unused-prefix"))),
                &settings,
            )?;
            let resolved = upscaler::resolve(
                &root,
                &plan,
                &executable,
                &upscaler::Settings {
                    backend,
                    input,
                    renderer,
                },
            )?;
            upscaler::preview(&root, &resolved)?;
            if dry_run {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"plan":resolved,"executed":false,"game_started":false})
                    )?
                );
            } else {
                let installation = Installation::open(&root, &resolved)?;
                installation.assert_idle(prefix.as_deref())?;
                let binding = upscaler::prepare(&root, &installation, &resolved)?;
                println!("{}", serde_json::to_string_pretty(&binding)?);
                println!("Prepared experimental upscaling. No game was started.");
            }
        }
        Action::Status { executable } => println!(
            "{}",
            serde_json::to_string_pretty(&upscaler::status(&root, &executable)?)?
        ),
        Action::Restore { executable } => {
            if let Some(installation) = Installation::existing(&root, &executable)? {
                installation.assert_idle(None)?;
                upscaler::restore(&installation)?;
            }
            println!("Original game files restored. No game was started.");
        }
        Action::MigrateTalos => {
            upscaler::migrate_talos(&root)?;
            println!("Talos preparation ownership transferred. No game was started.");
        }
    }
    Ok(())
}
