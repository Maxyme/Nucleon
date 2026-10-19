//! Supported Tolkara Wine/FEX build route. All products stay in the developer workspace.
use crate::common::{
    ArchivePin, Environment, Pin, build_environment, checkout, copy, fetch_archive, git,
    hash_files, json, output, p, patches, repo, require_native, run, set, sha, write_json,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub const RECIPE: &str = "assets/wine/recipes/arm64-fex.toml";
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WineSettings {
    pub patches: Vec<String>,
    pub shared_data_helper: String,
    pub shared_data_helper_sha256: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WineRecipe {
    pub configure: Vec<String>,
    pub tolkara: WineSettings,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Recipe {
    pub schema: u32,
    pub sources: BTreeMap<String, Pin>,
    pub alternatives: BTreeMap<String, BTreeMap<String, Pin>>,
    pub toolchain: ArchivePin,
    pub reference: ArchivePin,
    pub wine: WineRecipe,
}
pub struct Build {
    pub workspace: PathBuf,
    pub work: PathBuf,
    pub sources: PathBuf,
    pub wine: PathBuf,
    pub fex: PathBuf,
    pub compiler: PathBuf,
    pub logs: PathBuf,
    pub env: Environment,
    pub recipe: Recipe,
    pub jobs: usize,
}
impl Build {
    pub fn new(workspace: &Path, jobs: usize) -> Result<Self> {
        ensure!(jobs > 0, "--jobs must be positive");
        let workspace = std::path::absolute(workspace)?;
        let recipe: Recipe = toml::from_str(&fs::read_to_string(repo().join(RECIPE))?)?;
        ensure!(recipe.schema == 1, "Unsupported build recipe schema");
        let compiler = workspace.join("toolchain").join(
            recipe
                .toolchain
                .directory
                .as_ref()
                .context("Missing toolchain directory")?,
        );
        let work = workspace.join("tolkara");
        let logs = work.join("logs");
        fs::create_dir_all(&logs)?;
        let sources = workspace.join("sources");
        Ok(Self {
            wine: sources.join("tolkara-wine"),
            fex: sources.join("tolkara-fex"),
            sources,
            env: build_environment(&compiler),
            compiler,
            logs,
            work,
            workspace,
            recipe,
            jobs,
        })
    }
    pub fn source_pins(&self) -> BTreeMap<String, Pin> {
        let mut pins: BTreeMap<_, _> = self.recipe.alternatives["tolkara"]
            .iter()
            .map(|(k, v)| (format!("tolkara-{k}"), v.clone()))
            .collect();
        pins.insert("wine".into(), self.recipe.sources["wine"].clone());
        pins
    }
    pub fn fetch(&self, reference: bool) -> Result<()> {
        for (name, pin) in self.source_pins() {
            checkout(&self.sources.join(name), &pin)?;
        }
        for sub in [
            "External/rpmalloc",
            "External/fmt",
            "External/xxhash",
            "External/range-v3",
            "External/unordered_dense",
            "Source/Common/cpp-optparse",
        ] {
            git(
                &self.fex,
                &["submodule", "update", "--init", "--depth", "1", "--", sub],
            )?;
        }
        let archive = fetch_archive(&self.workspace, &self.recipe.toolchain)?;
        if !self.compiler.exists() {
            nucleon_core::artifacts::unpack(
                &archive,
                &self.workspace.join("toolchain"),
                8 * 1024 * 1024 * 1024,
            )?;
        }
        if reference {
            fetch_archive(&self.workspace, &self.recipe.reference)?;
        }
        Ok(())
    }
    pub fn verify_sources(&self) -> Result<()> {
        for (name, pin) in self.source_pins() {
            ensure!(
                git(&self.sources.join(&name), &["rev-parse", "HEAD"])? == pin.revision,
                "Source revision differs: {name}; run fetch"
            );
        }
        Ok(())
    }
    pub fn doctor(&self) -> Result<Value> {
        let mut tools = BTreeMap::new();
        for name in [
            "git",
            "clang",
            "make",
            "cmake",
            "ninja",
            "bison",
            "flex",
            "arm64ec-w64-mingw32-gcc",
            "x86_64-w64-mingw32-gcc",
        ] {
            let found = self
                .env
                .get(std::ffi::OsStr::new("PATH"))
                .into_iter()
                .flat_map(std::env::split_paths)
                .map(|p| p.join(name))
                .find(|p| p.is_file());
            tools.insert(name, found.map(p));
        }
        let result = json!({"host":std::env::consts::ARCH,"system":std::env::consts::OS,"tools":tools,"work":self.work});
        println!("{}", serde_json::to_string_pretty(&result)?);
        require_native()?;
        ensure!(
            tools.values().all(Option::is_some),
            "Missing build tools; see doctor output"
        );
        Ok(result)
    }
    pub fn run(
        &self,
        program: impl AsRef<std::ffi::OsStr>,
        args: &[String],
        cwd: Option<&Path>,
        log: &str,
    ) -> Result<()> {
        run(
            program,
            args,
            Some(&self.env),
            cwd,
            Some(&self.logs.join(log)),
            4 * 60 * 60,
            0,
        )
    }
    pub fn build_fex(&self) -> Result<()> {
        self.verify_sources()?;
        for (triple, target, dir) in [
            ("arm64ec", "arm64ecfex", "arm64ec"),
            ("aarch64", "wow64fex", "wow64"),
        ] {
            let build = self.work.join("fex").join(dir);
            let mut args = vec![
                "-S".into(),
                p(&self.fex),
                "-B".into(),
                p(&build),
                "-G".into(),
                "Ninja".into(),
                format!(
                    "-DCMAKE_TOOLCHAIN_FILE={}/Data/CMake/toolchain_mingw.cmake",
                    self.fex.display()
                ),
                format!("-DMINGW_TRIPLE={triple}-w64-mingw32"),
            ];
            args.extend(
                [
                    "-DCMAKE_BUILD_TYPE=Release",
                    "-DENABLE_LTO=OFF",
                    "-DENABLE_JEMALLOC_GLIBC_ALLOC=OFF",
                    "-DBUILD_TESTING=OFF",
                    "-DENABLE_CCACHE=OFF",
                    "-DTUNE_CPU=none",
                    "-DFEX_TEB_TSD_OFFSET=0x17f8",
                    "-DFEX_GUEST_ADDRESS_WINDOW=ON",
                    "-DCMAKE_SHARED_LINKER_FLAGS=-Wl,--section-alignment=0x10000",
                ]
                .map(str::to_owned),
            );
            self.run("cmake", &args, None, &format!("fex-{dir}-configure.log"))?;
            self.run(
                "cmake",
                &[
                    "--build".into(),
                    p(&build),
                    "--target".into(),
                    target.into(),
                    "--parallel".into(),
                    self.jobs.to_string(),
                ],
                None,
                &format!("fex-{dir}-build.log"),
            )?;
        }
        self.build_unixlib()?;
        Ok(())
    }
    pub fn build_unixlib(&self) -> Result<PathBuf> {
        let pin = &self.recipe.alternatives["tolkara"]["fex"];
        ensure!(
            git(&self.fex, &["rev-parse", "HEAD"])? == pin.revision,
            "Unexpected FEX revision"
        );
        patches(&self.fex, &pin.unix_patches)?;
        let source = self.fex.join("Source/Windows/UnixLib");
        let build = self.work.join("fex/unixlib");
        fs::create_dir_all(&build)?;
        let inputs = json!({"revision":pin.revision,"source":sha(&source.join("FEXUnixLib.cpp"))?,"header":sha(&source.join("FEXUnixLib.h"))?,"compiler":output("/usr/bin/clang++",&["--version".into()],None,None)?,"patches":hash_files(&repo(),&pin.unix_patches)?});
        let names = ["libarm64ecfex.so", "libwow64fex.so"].map(str::to_owned);
        let receipt = build.join("build.json");
        if let Ok(previous) = json(&receipt)
            && previous["inputs"] == inputs
            && names
                .iter()
                .all(|n| sha(&build.join(n)).is_ok_and(|s| previous["files"][n] == s))
        {
            return Ok(build);
        }
        for name in &names {
            let mut args = [
                "-std=c++17",
                "-O2",
                "-arch",
                "arm64",
                "-dynamiclib",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-Wno-unused-parameter",
            ]
            .map(str::to_owned)
            .to_vec();
            args.extend([
                format!("-Wl,-install_name,@rpath/{name}"),
                p(source.join("FEXUnixLib.cpp")),
                "-o".into(),
                p(build.join(name)),
            ]);
            self.run("/usr/bin/clang++", &args, None, "fex-unix-build.log")?;
            self.run(
                "codesign",
                &[
                    "--force".into(),
                    "--sign".into(),
                    "-".into(),
                    p(build.join(name)),
                ],
                None,
                "fex-unix-build.log",
            )?;
        }
        write_json(
            &receipt,
            &json!({"schema":2,"inputs":inputs,"files":hash_files(&build,&names)?}),
        )?;
        Ok(build)
    }
    pub fn prepare_wine(&self, extra: &[String]) -> Result<Vec<String>> {
        self.verify_sources()?;
        let settings = &self.recipe.wine.tolkara;
        let helper = self.sources.join("wine").join(&settings.shared_data_helper);
        ensure!(
            sha(&helper)? == settings.shared_data_helper_sha256,
            "Pinned shared-data helper checksum mismatch"
        );
        copy(&helper, &self.wine.join(&settings.shared_data_helper))?;
        git(
            &self.wine,
            &["add", "--intent-to-add", &settings.shared_data_helper],
        )?;
        let names = [settings.patches.clone(), extra.to_vec()].concat();
        patches(&self.wine, &names)?;
        Ok(names)
    }
    pub fn build_wine(&self, extra: &[String], sdk: Option<&Path>) -> Result<()> {
        let names = self.prepare_wine(extra)?;
        let build = self.work.join("wine-build");
        fs::create_dir_all(&build)?;
        let mut env = self.env.clone();
        for (k, v) in [
            ("CFLAGS", "-O2 -arch arm64"),
            ("CXXFLAGS", "-O2 -arch arm64"),
            ("LDFLAGS", "-arch arm64"),
            ("PKG_CONFIG", "/usr/bin/false"),
            ("PKG_CONFIG_LIBDIR", "/nonexistent"),
        ] {
            set(&mut env, k, v);
        }
        if let Some(sdk) = sdk {
            set(&mut env, "CPPFLAGS", format!("-I{}/include", sdk.display()));
            set(
                &mut env,
                "LDFLAGS",
                format!("-arch arm64 -L{}/lib", sdk.display()),
            );
            set(&mut env, "DYLD_LIBRARY_PATH", sdk.join("lib"));
        }
        if !build.join("Makefile").exists() {
            let mut args = vec![
                "--prefix=/".into(),
                "--bindir=/bin".into(),
                "--libdir=/lib".into(),
                "--enable-archs=arm64ec,aarch64,i386".into(),
                format!("--with-mingw={}/bin/clang", self.compiler.display()),
            ];
            args.extend(
                self.recipe
                    .wine
                    .configure
                    .iter()
                    .filter(|v| sdk.is_none() || *v != "--without-vulkan")
                    .cloned(),
            );
            if sdk.is_some() {
                args.push("--with-vulkan".into());
            }
            args.push("enable_amd_ags_x64=no".into());
            run(
                self.wine.join("configure"),
                &args,
                Some(&env),
                Some(&build),
                Some(&self.logs.join("wine-configure.log")),
                4 * 60 * 60,
                0,
            )?;
        }
        if sdk.is_some() {
            ensure!(
                fs::read_to_string(build.join("include/config.h"))?
                    .lines()
                    .any(|l| l.starts_with("#define SONAME_LIBVULKAN \"libvulkan")),
                "Wine must use the Khronos Vulkan loader"
            );
        }
        run(
            "make",
            &[format!("-j{}", self.jobs)],
            Some(&env),
            Some(&build),
            Some(&self.logs.join("wine-build.log")),
            4 * 60 * 60,
            0,
        )?;
        run(
            "make",
            &[
                "install".into(),
                format!("DESTDIR={}/runtime", self.work.display()),
            ],
            Some(&env),
            Some(&build),
            Some(&self.logs.join("wine-install.log")),
            4 * 60 * 60,
            0,
        )?;
        self.record_wine(&names)
    }
    pub fn record_wine(&self, names: &[String]) -> Result<()> {
        let runtime = self.work.join("runtime");
        let provenance = runtime.join("share/nucleon-build");
        copy(&repo().join(RECIPE), &provenance.join("arm64-fex.toml"))?;
        for name in names {
            copy(
                &repo().join(name),
                &provenance.join(Path::new(name).file_name().unwrap()),
            )?;
        }
        for name in ["COPYING.LIB", "LICENSE", "AUTHORS"] {
            if self.wine.join(name).is_file() {
                copy(
                    &self.wine.join(name),
                    &runtime.join("share/licenses/wine").join(name),
                )?;
            }
        }
        let helper = &self.recipe.wine.tolkara.shared_data_helper;
        copy(
            &self.wine.join(helper),
            &provenance.join(Path::new(helper).file_name().unwrap()),
        )?;
        for entry in walkdir::WalkDir::new(&self.fex)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| e.file_name() != ".git")
        {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if entry.file_type().is_file()
                && ["license", "copying", "copyright", "notice"]
                    .iter()
                    .any(|prefix| {
                        name == *prefix
                            || name
                                .strip_prefix(prefix)
                                .is_some_and(|s| s.starts_with(['.', '_', '-']))
                    })
            {
                copy(
                    entry.path(),
                    &runtime
                        .join("share/licenses/fex")
                        .join(entry.path().strip_prefix(&self.fex)?),
                )?;
            }
        }
        let host_files = [
            "bin/wine",
            "bin/wineserver",
            "lib/wine/aarch64-unix/ntdll.so",
            "lib/wine/aarch64-unix/winemac.so",
        ]
        .map(str::to_owned);
        write_json(
            &runtime.join("build.json"),
            &json!({"schema":2,"status":"BUILT_NOT_EXECUTION_VALIDATED","variant":"tolkara","recipe_sha256":sha(&repo().join(RECIPE))?,"sources":self.source_pins(),"patches":hash_files(&repo(),names)?,"toolchain_sha256":self.recipe.toolchain.sha256,"sdk":output("xcrun",&["--show-sdk-version".into()],None,None)?,"native_compiler":output("/usr/bin/clang",&["--version".into()],None,None)?.lines().next(),"host_files":hash_files(&runtime,&host_files)?,"features":"SOURCE_BUILT_WINE_SEPARATE_CPU_AND_GRAPHICS_TEST_RECEIPTS"}),
        )
    }
    pub fn runtime_environment(&self, prefix: &str) -> Result<(PathBuf, Environment)> {
        let runtime = self.work.join("runtime");
        let loader = runtime.join("bin/wine");
        ensure!(
            output("lipo", &["-archs".into(), p(&loader)], None, None)? == "arm64",
            "Wine loader must be native ARM64"
        );
        let unix = self.build_unixlib()?;
        for name in ["libarm64ecfex.so", "libwow64fex.so"] {
            copy(
                &unix.join(name),
                &runtime.join("lib/wine/aarch64-unix").join(name),
            )?;
        }
        for (dir, name) in [
            ("arm64ec", "libarm64ecfex.dll"),
            ("wow64", "libwow64fex.dll"),
        ] {
            copy(
                &self.work.join("fex").join(dir).join("Bin").join(name),
                &runtime.join("lib/wine/aarch64-windows").join(name),
            )?;
        }
        let mut env = self.env.clone();
        for (k, v) in [
            ("WINEPREFIX", p(self.work.join("prefixes").join(prefix))),
            ("WINELOADER", p(&loader)),
            ("WINESERVER", p(runtime.join("bin/wineserver"))),
            ("WINEDLLPATH", p(runtime.join("lib/wine/aarch64-windows"))),
            (
                "DYLD_LIBRARY_PATH",
                p(runtime.join("lib/wine/aarch64-unix")),
            ),
            ("FEX_TSOENABLED", "1".into()),
            ("NUCLEON_EMULATE_KUSD", "1".into()),
            (
                "WINEDLLOVERRIDES",
                "winedbg.exe=d;winemenubuilder.exe=d;mscoree=d;mshtml=d".into(),
            ),
            ("WINEDEBUG", "+loaddll".into()),
        ] {
            set(&mut env, k, v);
        }
        Ok((loader, env))
    }
    pub fn stop_server(&self, env: &Environment) -> Result<()> {
        crate::common::cleanup_server(
            &self.work.join("runtime/bin/wineserver"),
            env,
            &self.logs.join("smoke-cleanup.log"),
        )
    }
    pub fn check_reference(&self) -> Result<()> {
        let archive = self.workspace.join(&self.recipe.reference.archive);
        ensure!(
            sha(&archive)? == self.recipe.reference.sha256,
            "Reference archive checksum mismatch"
        );
        let log = self.logs.join("reference-signature.log");
        fs::write(&log, [])?;
        let loader = self.workspace.join(
            "reference/MacRunner.app/Contents/Resources/wine-loader/MacRunner Wine Loader.app",
        );
        let result = run(
            "codesign",
            &[
                "--verify".into(),
                "--strict".into(),
                "--verbose=2".into(),
                p(loader),
            ],
            None,
            None,
            Some(&log),
            60,
            0,
        );
        write_json(
            &self.work.join("probes/reference-signature.json"),
            &json!({"schema":2,"archive_sha256":sha(&archive)?,"signature_valid":result.is_ok(),"diagnostics":fs::read_to_string(log)?,"executed":false}),
        )?;
        result
    }
    pub fn probe_unixlib(&self, library: Option<&Path>) -> Result<()> {
        let library = library
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.work.join("fex/unixlib/libarm64ecfex.so"));
        ensure!(
            library.is_file(),
            "Build the native companion first or provide --library"
        );
        let temp = tempfile::tempdir()?;
        let program = temp.path().join("probe");
        self.run(
            "/usr/bin/clang",
            &[
                "-O2".into(),
                "-std=c11".into(),
                "-Wall".into(),
                "-Wextra".into(),
                "-Werror".into(),
                p(repo().join("tools/runtime-probes/fex-unixlib.c")),
                "-o".into(),
                p(&program),
            ],
            None,
            "unixlib-probe-build.log",
        )?;
        let log = self.logs.join("unixlib-probe.log");
        let mut report = json!({"schema":2,"library_sha256":sha(&library)?,"wine_executed":false,"steam_launched":false,"status":"TESTING"});
        let result = marker_run(
            &program,
            &[p(library)],
            &self.env,
            None,
            &log,
            30,
            0,
            "NUCLEON_FEX_UNIXLIB_OK",
        );
        finish_probe(
            &mut report,
            &self.work.join("probes/unixlib-result.json"),
            result,
            Ok(()),
            "UNIXLIB_ABI_PASSED",
        )
    }
    pub fn probe(&self, window: bool) -> Result<()> {
        let probes = self.work.join("probes");
        fs::create_dir_all(&probes)?;
        self.run(
            "/usr/bin/clang",
            &[
                "-O2".into(),
                "-arch".into(),
                "arm64".into(),
                p(repo().join("tools/runtime-probes/host.c")),
                "-o".into(),
                p(probes.join("host")),
            ],
            None,
            "host-build.log",
        )?;
        let host: Value =
            serde_json::from_str(&output(probes.join("host"), &[], Some(&self.env), None)?)?;
        let exe = probes.join("smoke-x64.exe");
        self.run(
            self.compiler.join("bin/x86_64-w64-mingw32-gcc"),
            &[
                "-O2".into(),
                "-municode".into(),
                "-fms-extensions".into(),
                p(repo().join("tools/runtime-probes/windows.c")),
                "-o".into(),
                p(&exe),
            ],
            None,
            "cpu-build.log",
        )?;
        let (loader, env) = self.runtime_environment("cpu-smoke")?;
        let mut report = json!({"schema":2,"host":host,"workload_sha256":sha(&exe)?,"status":"NOT_EXECUTED","variant":"tolkara","steam_launched":false});
        let result = (|| {
            marker_run(
                &loader,
                &[p(&exe), "--shared-data".into()],
                &env,
                None,
                &self.logs.join("cpu-smoke.log"),
                120,
                0,
                "NUCLEON_FEX_SMOKE_OK",
            )?;
            let mut negative = env.clone();
            set(&mut negative, "NUCLEON_EMULATE_KUSD", "0");
            marker_run(
                &loader,
                &[p(&exe), "--shared-data".into()],
                &negative,
                None,
                &self.logs.join("shared-data-negative.log"),
                30,
                1,
                "NUCLEON_FEX_FAIL stage=shared-data-read",
            )?;
            report["shared_data_negative_control"] = "EXPECTED_READ_FAILURE".into();
            if window {
                let exe = probes.join("window-x64.exe");
                self.run(
                    self.compiler.join("bin/x86_64-w64-mingw32-gcc"),
                    &[
                        "-O2".into(),
                        "-municode".into(),
                        p(repo().join("tools/runtime-probes/window.c")),
                        "-o".into(),
                        p(&exe),
                        "-luser32".into(),
                        "-lgdi32".into(),
                    ],
                    None,
                    "window-build.log",
                )?;
                marker_run(
                    &loader,
                    &[p(exe)],
                    &env,
                    None,
                    &self.logs.join("window-smoke.log"),
                    30,
                    0,
                    "NUCLEON_FEX_WINDOW_OK",
                )?;
            }
            Ok(())
        })();
        finish_probe(
            &mut report,
            &self.work.join("probes/result.json"),
            result,
            self.stop_server(&env),
            if window {
                "X64_CPU_AND_WINDOW_SMOKE_PASSED"
            } else {
                "X64_CPU_SMOKE_PASSED"
            },
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub fn marker_run(
    program: &Path,
    args: &[String],
    env: &Environment,
    cwd: Option<&Path>,
    log: &Path,
    seconds: u64,
    expected: i32,
    marker: &str,
) -> Result<()> {
    fs::create_dir_all(log.parent().unwrap())?;
    fs::write(log, [])?;
    run(program, args, Some(env), cwd, Some(log), seconds, expected)?;
    ensure!(
        fs::read_to_string(log)?.contains(marker),
        "Missing execution marker {marker}; see {}",
        log.display()
    );
    Ok(())
}
pub fn finish_probe(
    report: &mut Value,
    path: &Path,
    result: Result<()>,
    cleanup: Result<()>,
    success: &str,
) -> Result<()> {
    if let Err(error) = &cleanup {
        report["cleanup_error"] = format!("{error:#}").into();
    }
    let result = result.and(cleanup);
    match &result {
        Ok(()) => report["status"] = success.into(),
        Err(error) => {
            report["status"] = "FAILED".into();
            report["error"] = format!("{error:#}").into();
        }
    }
    result.and(write_json(path, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cleanup_failure_does_not_replace_primary_probe_failure() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("receipt.json");
        let mut report = json!({});
        let result = finish_probe(
            &mut report,
            &file,
            Err(anyhow::anyhow!("primary failure")),
            Err(anyhow::anyhow!("cleanup failure")),
            "PASSED",
        );
        assert_eq!(result.unwrap_err().to_string(), "primary failure");
        let report = json(&file).unwrap();
        assert_eq!(report["status"], "FAILED");
        assert_eq!(report["cleanup_error"], "cleanup failure");
    }
}
