use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use nucleon_core::{
    application::Settings,
    detector::TargetEngine,
    launch::{GraphicsComponents, LaunchPlan},
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
                engine: Some(engine.clone()),
                ..Default::default()
            };
            let runtime = settings.resolve_runtime()?;
            let guest = nucleon_core::detector::executable_guest(&executable)?;
            let target = renderer
                .as_ref()
                .map(|p| executable.parent().unwrap().join(p))
                .unwrap_or_else(|| executable.clone());
            let api = match graphics_api {
                Some(value) => serde_json::from_value::<nucleon_core::profile::GraphicsPreference>(
                    value.into(),
                )?
                .api()
                .unwrap_or_else(|| nucleon_core::detector::detect_target_engine(&target).api),
                None => nucleon_core::detector::detect_target_engine(&target).api,
            };
            let selected_engine = nucleon_core::configuration::engine("--engine", &engine)?;
            let mut graphics = if matches!(
                selected_engine,
                TargetEngine::WineStaging | TargetEngine::Gptk
            ) {
                GraphicsComponents::default()
            } else {
                GraphicsComponents::discover(&runtime, None, None)?
            };
            graphics.gptk = GraphicsComponents::bundled_gptk(&runtime);
            let engine = match selected_engine {
                TargetEngine::Auto => graphics
                    .available(
                        &runtime,
                        &api,
                        guest == nucleon_core::runtime::GuestAbi::X64,
                    )
                    .select(&api)?,
                selected => selected,
            };
            let plan = LaunchPlan::new(
                runtime,
                prefix.clone().unwrap_or_else(|| root.join("unused-prefix")),
                engine,
                graphics,
                guest == nucleon_core::runtime::GuestAbi::X64,
                Some(false),
                &api,
            )?
            .without_steam();
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
