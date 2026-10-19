#![forbid(unsafe_code)]
mod build;
mod common;
mod diagnostic;
mod frontend;
mod graphics;
mod release;

use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::{
    num::{NonZeroU64, NonZeroUsize},
    path::PathBuf,
    sync::atomic::Ordering,
};

#[derive(Parser)]
#[command(
    about = "Developer builds, probes and source-inclusive Wine/FEX packaging",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    /// Build or probe the supported native Tolkara Wine/FEX runtime
    Fex(FexArgs),
    /// Build/package the core runtime; both guest execution gates are mandatory
    Release(ReleaseArgs),
    /// Diagnostic game launch through the normal runner, using a private profile
    Launch(DiagnosticArgs),
    /// Build, assemble or probe the isolated Vulkan/Metal integration
    Dx12(GraphicsArgs),
    /// Build and validate separately paired experimental graphics frontends
    Frontend(FrontendArgs),
}
#[derive(Args)]
struct Workspace {
    #[arg(long, default_value = "target/arm64-fex")]
    work: PathBuf,
    #[arg(long, default_value = "6")]
    jobs: NonZeroUsize,
}
#[derive(Clone, Copy, ValueEnum)]
enum FexAction {
    Doctor,
    Fetch,
    BuildFex,
    BuildWine,
    Probe,
    ProbeDxmt,
    CheckReference,
    ProbeUnixlib,
}
#[derive(Args)]
struct FexArgs {
    #[arg(value_enum)]
    action: FexAction,
    #[command(flatten)]
    workspace: Workspace,
    #[arg(long)]
    reference: bool,
    #[arg(long)]
    window: bool,
    #[arg(long)]
    library: Option<PathBuf>,
}
#[derive(Clone, Copy, ValueEnum)]
enum ReleaseAction {
    Build,
    Package,
    /// Relocate and execute the current built core, without creating release assets
    Validate,
}
#[derive(Args)]
struct ReleaseArgs {
    #[arg(value_enum)]
    action: ReleaseAction,
    #[command(flatten)]
    workspace: Workspace,
    #[arg(long)]
    version: String,
    #[arg(long)]
    output: PathBuf,
}
#[derive(Args)]
pub struct DiagnosticArgs {
    #[arg(value_parser=["dirt","dmc5"])]
    game: String,
    #[arg(long, default_value = "target/arm64-fex")]
    work: PathBuf,
    #[arg(long)]
    runtime: Option<PathBuf>,
    #[arg(long)]
    game_dir: Option<PathBuf>,
    #[arg(long)]
    bridge: Option<PathBuf>,
    #[arg(long)]
    runner: Option<PathBuf>,
    #[arg(long,default_value="dx12",value_parser=["dx11","dx12"])]
    api: String,
    #[arg(long)]
    timeout: Option<NonZeroU64>,
    #[arg(long)]
    debug: bool,
}
#[derive(Args)]
struct GraphicsArgs {
    #[arg(value_enum)]
    action: GraphicsAction,
    #[command(flatten)]
    workspace: Workspace,
    #[arg(long)]
    sdk: Option<PathBuf>,
    #[arg(long)]
    debug: bool,
}
#[derive(Clone, Copy, ValueEnum)]
enum GraphicsAction {
    Fetch,
    BuildWine,
    BuildDriver,
    StageDxmt,
    PrepareRuntime,
    ProbeDriver,
    ProbeVulkan,
    ProbeD3d12,
    ProbeSteam,
}
#[derive(Args)]
struct FrontendArgs {
    #[arg(value_enum)]
    action: FrontendAction,
    #[command(flatten)]
    workspace: Workspace,
    #[arg(long)]
    sdk: Option<PathBuf>,
    #[arg(long)]
    debug: bool,
    #[arg(long,default_value="vkd3d",value_parser=["dxvk","vkd3d","dxmt"])]
    component: String,
    #[arg(long,default_value="arm64ec",value_parser=["arm64ec","x64"])]
    architecture: String,
    #[arg(long,default_value="dx12",value_parser=["dx11","dx11-vulkan","dx12"])]
    api: String,
    #[arg(long)]
    native_llvm: Option<PathBuf>,
    #[arg(long)]
    base_runtime: Option<PathBuf>,
}
#[derive(Clone, Copy, ValueEnum)]
enum FrontendAction {
    Fetch,
    Doctor,
    Build,
    PrepareRuntime,
    Probe,
    ProbeTls,
}
fn graphics_context(
    workspace: &Workspace,
    sdk: Option<&PathBuf>,
    debug: bool,
) -> Result<graphics::Graphics> {
    let sdk = sdk
        .cloned()
        .unwrap_or_else(|| nucleon_core::paths::home_dir().join("VulkanSDK/1.4.363.0/macOS"));
    graphics::Graphics::new(&workspace.work, &sdk, workspace.jobs.get(), debug)
}
fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();
    ctrlc::set_handler(|| common::CANCELLED.store(true, Ordering::SeqCst))?;
    match cli.command {
        Commands::Fex(args) => {
            let build = build::Build::new(&args.workspace.work, args.workspace.jobs.get())?;
            let _lease =
                common::FileLock::acquire(&build.workspace.join(".nucleon-devtools.lock"))?;
            match args.action {
                FexAction::Doctor => {
                    build.doctor()?;
                    Ok(())
                }
                FexAction::Fetch => build.fetch(args.reference),
                FexAction::BuildFex => {
                    common::require_native()?;
                    build.build_fex()
                }
                FexAction::BuildWine => {
                    common::require_native()?;
                    build.build_wine(&[], None)
                }
                FexAction::Probe => {
                    common::require_native()?;
                    build.probe(args.window)
                }
                FexAction::ProbeDxmt => {
                    common::require_native()?;
                    let graphics = graphics_context(&args.workspace, None, false)?;
                    graphics.stage_dxmt()?;
                    graphics.probe("dxmt")
                }
                FexAction::CheckReference => build.check_reference(),
                FexAction::ProbeUnixlib => {
                    common::require_native()?;
                    build.probe_unixlib(args.library.as_deref())
                }
            }
        }
        Commands::Release(args) => {
            common::require_native()?;
            let build = build::Build::new(&args.workspace.work, args.workspace.jobs.get())?;
            let _lease =
                common::FileLock::acquire(&build.workspace.join(".nucleon-devtools.lock"))?;
            let output = std::path::absolute(args.output)?;
            match args.action {
                ReleaseAction::Build => release::build_release(&build, &args.version, &output),
                ReleaseAction::Package => release::package(&build, &args.version, &output),
                ReleaseAction::Validate => release::validate_built(&build, &output),
            }
        }
        Commands::Launch(args) => diagnostic::game(&args),
        Commands::Dx12(args) => {
            let graphics = graphics_context(&args.workspace, args.sdk.as_ref(), args.debug)?;
            let _lease =
                common::FileLock::acquire(&graphics.base.workspace.join(".nucleon-devtools.lock"))?;
            match args.action {
                GraphicsAction::Fetch => graphics.fetch(),
                GraphicsAction::BuildWine => {
                    common::require_native()?;
                    graphics.build_wine()
                }
                GraphicsAction::BuildDriver => {
                    common::require_native()?;
                    graphics.build_driver()
                }
                GraphicsAction::StageDxmt => {
                    common::require_native()?;
                    println!("{}", graphics.stage_dxmt()?.display());
                    Ok(())
                }
                GraphicsAction::PrepareRuntime => {
                    println!("{}", graphics.prepare_runtime()?.display());
                    Ok(())
                }
                GraphicsAction::ProbeDriver => {
                    common::require_native()?;
                    graphics.probe_driver()
                }
                GraphicsAction::ProbeVulkan => {
                    common::require_native()?;
                    graphics.probe("vulkan")
                }
                GraphicsAction::ProbeD3d12 => {
                    common::require_native()?;
                    graphics.probe("d3d12")
                }
                GraphicsAction::ProbeSteam => {
                    common::require_native()?;
                    graphics.probe_steam()
                }
            }
        }
        Commands::Frontend(args) => {
            let frontend = frontend::Frontend::new(
                graphics_context(&args.workspace, args.sdk.as_ref(), args.debug)?,
                args.native_llvm,
            )?;
            let _lease = common::FileLock::acquire(
                &frontend
                    .graphics
                    .base
                    .workspace
                    .join(".nucleon-devtools.lock"),
            )?;
            let _legacy_lease = common::FileLock::configuration(&frontend.work.join("experiment"))?;
            match args.action {
                FrontendAction::Fetch => frontend.fetch(),
                FrontendAction::Doctor => {
                    common::require_native()?;
                    println!("{}", frontend.doctor()?);
                    Ok(())
                }
                FrontendAction::Build => {
                    common::require_native()?;
                    frontend.build(&args.component, &args.architecture)
                }
                FrontendAction::PrepareRuntime => {
                    let base = match args.base_runtime {
                        Some(path) => path,
                        None => frontend.graphics.prepare_runtime()?,
                    };
                    println!("{}", frontend.assemble(&base)?.display());
                    Ok(())
                }
                FrontendAction::Probe => {
                    common::require_native()?;
                    frontend.probe(&args.architecture, &args.api)
                }
                FrontendAction::ProbeTls => {
                    common::require_native()?;
                    frontend.probe_tls()
                }
            }
        }
    }
}
