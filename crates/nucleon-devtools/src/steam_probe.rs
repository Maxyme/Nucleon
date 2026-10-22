//! Steam integration validation consumes an existing runtime; it never builds one.
use crate::common::{
    CANCELLED, clean_environment, finish_probe, marker_run, p, repo, require_native, run, set, sha,
};
use anyhow::{Context, Result, ensure};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
};
#[derive(clap::Args)]
pub struct Args {
    #[arg(long)]
    runtime: PathBuf,
    #[arg(long, default_value = "target/fex-steam-probe")]
    work: PathBuf,
    #[arg(long, default_value = "x86_64-w64-mingw32-gcc")]
    compiler: PathBuf,
}
struct Probe {
    runtime: PathBuf,
    work: PathBuf,
    logs: PathBuf,
    compiler: PathBuf,
}
pub fn probe(args: &Args) -> Result<()> {
    require_native()?;
    let work = std::path::absolute(&args.work)?;
    let logs = work.join("logs");
    fs::create_dir_all(&logs)?;
    let _lease = crate::common::FileLock::acquire(&work.join(".probe.lock"))?;
    Probe {
        runtime: args.runtime.canonicalize()?,
        work,
        logs,
        compiler: args.compiler.clone(),
    }
    .run()
}
impl Probe {
    fn run(&self) -> Result<()> {
        use nucleon_core::{
            detector::{GraphicsApi, TargetEngine},
            launch::{GraphicsComponents, LaunchPlan},
            prefix_session::PrefixSession,
            runtime::ResolvedRuntime,
        };
        let probes = self.work.join("probes");
        fs::create_dir_all(&probes)?;
        let program = probes.join("steam-route-x64.exe");
        run(
            &self.compiler,
            &[
                "-O2".into(),
                "-Wall".into(),
                "-Wextra".into(),
                "-Werror".into(),
                p(repo().join("tools/runtime-probes/steam-route.c")),
                "-o".into(),
                p(&program),
            ],
            None,
            None,
            Some(&self.logs.join("steam-route-build.log")),
            60,
            0,
        )?;
        let root = self.runtime.clone();
        let runtime = ResolvedRuntime::inspect(&root)?;
        let compat = self.work.join("probe-compatdata/steam-route");
        let prefix = nucleon_core::paths::game_prefix(&compat, &runtime);
        let _lease = PrefixSession::acquire(&compat, &runtime, std::time::Duration::ZERO, || {
            CANCELLED.load(std::sync::atomic::Ordering::SeqCst)
        })?;
        let components = GraphicsComponents::discover(&runtime, None, None)?;
        let mut plan = LaunchPlan::new(
            runtime,
            prefix.clone(),
            TargetEngine::WineStaging,
            components,
            true,
            Some(true),
            &GraphicsApi::Unknown,
        )?;
        plan.bridge = std::env::var_os("NUCLEON_STEAM_BRIDGE")
            .map(PathBuf::from)
            .unwrap_or_else(nucleon_core::paths::bridge_dir);
        let mut env = clean_environment();
        for (k, v) in plan.execution_env(false, false) {
            set(&mut env, &k, v);
        }
        for name in ["SteamAppId", "SteamGameId"] {
            set(&mut env, name, "601150");
        }
        let log = self.logs.join("steam-route.log");
        let mut receipt = json!({"schema":2,"status":"TESTING"});
        let result = (|| {
            plan.prepare_prefix()?;
            let steam = prefix.join("drive_c/Program Files (x86)/Steam");
            let original = sha(&steam.join("steamclient64.dll"))?;
            let api = std::env::var_os("NUCLEON_STEAM_API")
                .map(PathBuf::from)
                .context("Set NUCLEON_STEAM_API to an installed game's x64 Steam API DLL")?;
            ensure!(api.is_file(), "Steam API DLL missing");
            set(&mut env, "NUCLEON_STEAM_API", windows_path(&api)?);
            run(
                &plan.runtime.loader,
                &[
                    "reg".into(),
                    "add".into(),
                    "HKCU\\Software\\Valve\\Steam\\ActiveProcess".into(),
                    "/v".into(),
                    "pid".into(),
                    "/t".into(),
                    "REG_DWORD".into(),
                    "/d".into(),
                    "0".into(),
                    "/f".into(),
                ],
                Some(&env),
                None,
                Some(&log),
                30,
                0,
            )?;
            marker_run(
                &plan.runtime.loader,
                &[p(&program)],
                &env,
                None,
                &self.logs.join("steam-presence-negative.log"),
                60,
                15,
                "NUCLEON_STEAM_NOT_RUNNING",
            )?;
            let positive = self.logs.join("steam-api-probe.log");
            if positive.exists() {
                fs::remove_file(&positive)?;
            }
            set(&mut env, "NUCLEON_STEAM_RESULT", windows_path(&positive)?);
            run(
                &plan.runtime.loader,
                &[p(steam.join("steam.exe")), windows_path(&program)?],
                Some(&env),
                None,
                Some(&log),
                60,
                0,
            )?;
            ensure!(
                fs::read_to_string(positive)?.contains("NUCLEON_STEAM_ROUTE_OK"),
                "No successful Steam route marker"
            );
            let mut negative = env.clone();
            set(&mut negative, "NUCLEON_STEAMCLIENT_REDIRECT", "0");
            marker_run(
                &plan.runtime.loader,
                &[p(&program)],
                &negative,
                None,
                &self.logs.join("steam-route-negative.log"),
                30,
                10,
                "NUCLEON_STEAM_ROUTE_LOAD_FAILED",
            )?;
            ensure!(
                sha(&steam.join("steamclient64.dll"))? == original,
                "Valve image changed on disk"
            );
            receipt["valve_sha256"] = original.into();
            receipt["unix_sha256"] = sha(&steam.join("lsteamclient.so"))?.into();
            receipt["steam_api_sha256"] = sha(&api)?.into();
            receipt["presence_negative_control"] = "EXPECTED_NOT_RUNNING_WITHOUT_HELPER".into();
            receipt["negative_control"] = "EXPECTED_LOAD_FAILURE_WITHOUT_REDIRECT".into();
            Ok(())
        })();
        let cleanup = crate::common::cleanup_server(&plan.runtime.server, &env, &log);
        finish_probe(
            &mut receipt,
            &probes.join("steam-route-result.json"),
            result,
            cleanup,
            "NATIVE_STEAM_ROUTE_PASSED",
        )
    }
}
pub fn windows_path(path: &Path) -> Result<String> {
    Ok(format!(
        "Z:{}",
        p(std::path::absolute(path)?).replace('/', "\\")
    ))
}
