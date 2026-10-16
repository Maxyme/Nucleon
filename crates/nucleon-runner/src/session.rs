//! One launch owns its logs and receipt; concurrent games never truncate each other.
use anyhow::{Context, Result};
use nucleon_core::{configuration, fs_util::atomic_write_file, paths};
use serde::Serialize;
use std::{
    cell::{Cell, RefCell},
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

thread_local! { static ACTIVE: RefCell<Option<(File,bool)>> = const {RefCell::new(None)}; }

pub fn log(message: &str) {
    let line = format!(
        "{} [nucleon-runner] {message}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    print!("{line}");
    ACTIVE.with(|active| {
        if let Some((file, _)) = active.borrow_mut().as_mut()
            && let Err(error) = file.write_all(line.as_bytes())
        {
            eprintln!("Cannot write runner log: {error}");
        }
    });
}
pub fn trace(message: &str) {
    if ACTIVE.with(|active| active.borrow().as_ref().is_some_and(|(_, debug)| *debug)) {
        log(message);
    }
}

#[derive(Clone)]
pub struct Clock {
    start: Instant,
    game: Rc<Cell<Option<Instant>>>,
    cleanup: Rc<Cell<Option<Instant>>>,
}
impl Clock {
    pub fn begin_cleanup(&self) {
        if self.cleanup.get().is_none() {
            self.cleanup.set(Some(Instant::now()));
        }
    }
    fn timings(&self) -> Timings {
        let end = Instant::now();
        let game = self.game.get();
        let cleanup = self.cleanup.get();
        let millis = |a: Instant, b: Instant| {
            b.saturating_duration_since(a)
                .as_millis()
                .min(u64::MAX as u128) as u64
        };
        Timings {
            preparation_ms: millis(self.start, game.or(cleanup).unwrap_or(end)),
            runtime_ms: game.map(|game| millis(game, cleanup.unwrap_or(end))),
            cleanup_ms: cleanup.map(|cleanup| millis(cleanup, end)),
            total_ms: millis(self.start, end),
        }
    }
}
#[derive(Serialize)]
struct Timings {
    preparation_ms: u64,
    runtime_ms: Option<u64>,
    cleanup_ms: Option<u64>,
    total_ms: u64,
}
#[derive(Serialize)]
struct Receipt {
    schema: u32,
    run_id: String,
    appid: Option<String>,
    executable: Option<PathBuf>,
    started_at: String,
    finished_at: Option<String>,
    status: String,
    exit_code: Option<i32>,
    primary_exit: Option<String>,
    error: Option<String>,
    debug: bool,
    runtime: Option<PathBuf>,
    execution_mode: Option<String>,
    frontend_abi: Option<nucleon_core::runtime::FrontendAbi>,
    frontend_manifest: Option<PathBuf>,
    engine: Option<String>,
    prefix: Option<PathBuf>,
    shader_caches: Option<serde_json::Value>,
    staged_files: Option<usize>,
    runner_log: PathBuf,
    wine_log: PathBuf,
    timings: Timings,
    gameplay_validation: &'static str,
}
pub struct RunSession {
    pub directory: PathBuf,
    pub appid: Option<String>,
    pub wine_log: PathBuf,
    pub debug: bool,
    pub clock: Clock,
    receipt: Receipt,
    mirror: Option<PathBuf>,
    previous: Option<(File, bool)>,
}
impl RunSession {
    pub fn create(args: &[String]) -> Result<Self> {
        Self::create_at(&paths::support_dir(), args)
    }
    fn create_at(support: &Path, args: &[String]) -> Result<Self> {
        let start = Instant::now();
        let appid = std::env::var("STEAM_COMPAT_APP_ID")
            .ok()
            .or_else(|| std::env::var("SteamAppId").ok())
            .or_else(|| {
                std::env::var_os("STEAM_COMPAT_DATA_PATH").and_then(|p| {
                    PathBuf::from(p)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                })
            })
            .filter(|id| {
                !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
            });
        let group = appid.as_deref().unwrap_or("manual").to_string();
        let parent = support.join("logs").join(&group);
        fs::create_dir_all(&parent)?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let run_id = format!(
            "{}-{}-{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ"),
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let directory = parent.join(&run_id);
        fs::create_dir(&directory)?;
        let runner_log = directory.join("runner.log");
        let file = File::create(&runner_log)?;
        let wine_log = directory.join("wine.log");
        let clock = Clock {
            start,
            game: Rc::new(Cell::new(None)),
            cleanup: Rc::new(Cell::new(None)),
        };
        let receipt = Receipt {
            schema: 1,
            run_id: run_id.clone(),
            appid: appid.clone(),
            executable: args.get(2).map(PathBuf::from),
            started_at: chrono::Utc::now().to_rfc3339(),
            finished_at: None,
            status: "preparing".into(),
            exit_code: None,
            primary_exit: None,
            error: None,
            debug: false,
            runtime: None,
            execution_mode: None,
            frontend_abi: None,
            frontend_manifest: None,
            engine: None,
            prefix: None,
            shader_caches: None,
            staged_files: None,
            runner_log,
            wine_log: wine_log.clone(),
            timings: clock.timings(),
            gameplay_validation: "not-validated",
        };
        let previous = ACTIVE.with(|active| active.replace(Some((file, false))));
        let session = Self {
            directory,
            appid,
            wine_log,
            debug: false,
            clock,
            receipt,
            mirror: None,
            previous,
        };
        session.save()?;
        atomic_write_file(
            support.join("logs/latest-run.json"),
            serde_json::to_vec(&serde_json::json!({"directory":format!("{group}/{run_id}")}))?,
        )?;
        Ok(session)
    }
    pub fn configure(&mut self) -> Result<()> {
        self.debug = configuration::environment("NUCLEON_DEBUG")?
            .map(|s| configuration::boolean("NUCLEON_DEBUG", &s))
            .transpose()?
            .unwrap_or(false);
        self.receipt.debug = self.debug;
        ACTIVE.with(|active| {
            if let Some((_, debug)) = active.borrow_mut().as_mut() {
                *debug = self.debug;
            }
        });
        if let Some(path) = configuration::environment("NUCLEON_WINE_LOG")? {
            anyhow::ensure!(
                !path.is_empty(),
                "NUCLEON_WINE_LOG must name a writable file"
            );
            self.wine_log = std::path::absolute(Path::new(&path))?;
            self.receipt.wine_log = self.wine_log.clone();
        }
        if let Some(path) = configuration::environment("NUCLEON_RUN_RECEIPT")? {
            anyhow::ensure!(
                !path.is_empty(),
                "NUCLEON_RUN_RECEIPT must name a writable file"
            );
            self.mirror = Some(std::path::absolute(Path::new(&path))?);
        }
        self.save()
    }
    pub fn resolved(
        &mut self,
        plan: &nucleon_core::launch::LaunchPlan,
        staged: Option<usize>,
        caches: Option<serde_json::Value>,
    ) -> Result<()> {
        self.receipt.runtime = Some(plan.runtime.root.clone());
        self.receipt.execution_mode = Some(format!("{:?}", plan.runtime.mode));
        self.receipt.frontend_abi = Some(plan.runtime.graphics.frontend_abi);
        self.receipt.frontend_manifest = plan.runtime.graphics.frontend_manifest.clone();
        self.receipt.engine = Some(plan.engine.display_name().into());
        self.receipt.prefix = Some(plan.prefix.clone());
        self.receipt.staged_files = staged;
        self.receipt.shader_caches = caches;
        self.save()
    }
    pub fn started(&mut self) -> Result<()> {
        self.clock.game.set(Some(Instant::now()));
        self.receipt.status = "running".into();
        self.receipt.timings = self.clock.timings();
        self.save()
    }
    pub fn primary_exited(&mut self, status: std::process::ExitStatus) -> Result<()> {
        self.receipt.primary_exit = Some(status.to_string());
        self.save()
    }
    pub fn finish(&mut self, status: &str, code: Option<i32>, error: Option<String>) -> Result<()> {
        self.receipt.status = status.into();
        self.receipt.exit_code = code;
        self.receipt.error = error;
        self.receipt.finished_at = Some(chrono::Utc::now().to_rfc3339());
        self.receipt.timings = self.clock.timings();
        self.save()
    }
    fn save(&self) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(&self.receipt)?;
        atomic_write_file(self.directory.join("receipt.json"), &bytes)
            .context("Write launch receipt")?;
        if let Some(path) = &self.mirror {
            atomic_write_file(path, &bytes).context("Write diagnostic launch receipt")?;
        }
        Ok(())
    }
}
impl Drop for RunSession {
    fn drop(&mut self) {
        let previous = self.previous.take();
        ACTIVE.with(|active| {
            active.replace(previous);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_run_files_are_distinct_and_cleanup_is_separate_from_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let args = vec!["runner".into(), "run".into(), "game.exe".into()];
        let mut a = RunSession::create_at(temp.path(), &args).unwrap();
        log("first run");
        let b = RunSession::create_at(temp.path(), &args).unwrap();
        log("second run");
        assert_ne!(a.directory, b.directory);
        drop(b);
        a.started().unwrap();
        a.clock.begin_cleanup();
        a.finish("bounded", Some(0), None).unwrap();
        let receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(a.directory.join("receipt.json")).unwrap()).unwrap();
        assert!(receipt["timings"]["runtime_ms"].is_number());
        assert!(receipt["timings"]["cleanup_ms"].is_number());
        let log = fs::read_to_string(a.directory.join("runner.log")).unwrap();
        assert!(log.contains("first run"));
        assert!(!log.contains("second run"));
        assert_eq!(receipt["gameplay_validation"], "not-validated");
    }
}
