use crate::{
    build::{finish_probe, marker_run},
    common::{
        Pin, checkout, copy, copy_tree, git, hash_files, json, key, output, p, patches, repo, run,
        set, sha, source_diff, write_json,
    },
    graphics::{Graphics, shader_compiler, verify_hashes},
};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

const RECIPE: &str = "assets/wine/recipes/arm64ec-graphics.toml";
const SHARED: &str = "share/nucleon/arm64ec";
#[derive(Deserialize)]
struct Recipe {
    schema: u32,
    sources: BTreeMap<String, Pin>,
    darwin: Darwin,
    dxmt: Dxmt,
}
#[derive(Deserialize)]
struct Darwin {
    teb_tsd_offset: String,
}
#[derive(Deserialize)]
struct Dxmt {
    native_llvm_version: String,
    patches: Vec<String>,
}
pub struct Frontend {
    pub graphics: Graphics,
    pub work: PathBuf,
    logs: PathBuf,
    pin: Recipe,
    native_llvm: PathBuf,
}
pub fn quote_args(args: &[String]) -> Result<String> {
    args.iter()
        .map(|s| {
            shlex::try_quote(s)
                .map(|s| s.into_owned())
                .map_err(anyhow::Error::from)
        })
        .collect::<Result<Vec<_>>>()
        .map(|v| v.join(" "))
}
pub fn cross_file(compiler: &Path, architecture: &str) -> Result<String> {
    let triple = match architecture {
        "arm64ec" => "arm64ec",
        "x64" => "x86_64",
        _ => anyhow::bail!("Unsupported frontend architecture"),
    };
    let mut out = "[binaries]\n".to_string();
    for (name, suffix) in [
        ("c", "gcc"),
        ("cpp", "g++"),
        ("ar", "ar"),
        ("strip", "strip"),
        ("windres", "windres"),
        ("widl", "widl"),
    ] {
        let path = compiler.join(format!("bin/{triple}-w64-mingw32-{suffix}"));
        ensure!(path.is_file(), "Missing compiler tool {}", path.display());
        out.push_str(&format!("{name} = {}\n", serde_json::to_string(&p(path))?));
    }
    out.push_str(&format!("pkg-config = '/usr/bin/false'\n[properties]\nneeds_exe_wrapper = true\n[host_machine]\nsystem = 'windows'\ncpu_family = '{}'\ncpu = '{}'\nendian = 'little'\n",if architecture=="arm64ec"{"aarch64"}else{"x86_64"},if architecture=="arm64ec"{"aarch64"}else{"x86_64"}));
    Ok(out)
}
pub fn experimental_descriptor(original: &str) -> Result<String> {
    let mut d: toml::Value = toml::from_str(original)?;
    ensure!(
        d["mode"].as_str() == Some("arm64-fex")
            && d.get("graphics")
                .and_then(|g| g.get("frontend_abi"))
                .and_then(|a| a.as_str())
                .unwrap_or("x64")
                == "x64",
        "ARM64EC assembly requires baseline ARM64/FEX x64 frontends"
    );
    d["guests"] = toml::Value::try_from(vec!["x64"])?;
    let windows: Vec<_> = d["windows_dirs"]
        .as_array()
        .context("Missing Windows paths")?
        .iter()
        .filter(|v| {
            v.as_str().is_some_and(|s| {
                Path::new(s)
                    .file_name()
                    .is_some_and(|s| s == "aarch64-windows")
            })
        })
        .cloned()
        .collect();
    ensure!(windows.len() == 1, "Expected one paired Windows directory");
    d["windows_dirs"] = windows.into();
    let emulators: Vec<_> = d["emulators"]
        .as_array()
        .context("Missing emulators")?
        .iter()
        .filter(|v| v.get("guest").and_then(|s| s.as_str()) == Some("x64"))
        .cloned()
        .collect();
    ensure!(
        emulators.len() == 1 && emulators[0].get("unix_library").is_some(),
        "Expected complete x64 FEX emulator pair"
    );
    d["emulators"] = emulators.into();
    let family = format!(
        "{}-arm64ec",
        d["prefix"]["family"]
            .as_str()
            .context("Missing prefix family")?
    );
    ensure!(
        family.len() <= 64,
        "Experimental prefix family exceeds 64 characters"
    );
    d["prefix"]["family"] = family.into();
    d["backends"] = toml::Value::try_from(vec!["auto", "dxvk", "kosmickrisp"])?;
    let graphics = d["graphics"]
        .as_table_mut()
        .context("Missing baseline graphics")?;
    ensure!(
        graphics.contains_key("vulkan_icd") && graphics.contains_key("vulkan_loader"),
        "ARM64EC Vulkan assembly requires paired driver/loader"
    );
    graphics.remove("dxmt");
    for (key, value) in [
        ("frontend_abi", "arm64ec"),
        ("frontend_manifest", "share/nucleon/arm64ec/frontend.json"),
        ("dxvk", "share/nucleon/arm64ec/dxvk"),
        ("vkd3d", "share/nucleon/arm64ec/vkd3d"),
    ] {
        graphics.insert(key.into(), value.into());
    }
    let env = d
        .as_table_mut()
        .unwrap()
        .entry("environment")
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .context("Invalid environment")?;
    for name in ["FEX_TSOENABLED", "NUCLEON_EMULATE_KUSD"] {
        env.insert(name.into(), "1".into());
    }
    Ok(toml::to_string(&d)?)
}
impl Frontend {
    pub fn new(graphics: Graphics, native_llvm: Option<PathBuf>) -> Result<Self> {
        let work = graphics.base.workspace.join("arm64ec");
        let logs = work.join("logs");
        fs::create_dir_all(&logs)?;
        let pin: Recipe = toml::from_str(&fs::read_to_string(repo().join(RECIPE))?)?;
        ensure!(pin.schema == 1, "Unsupported frontend recipe");
        Ok(Self {
            graphics,
            work,
            logs,
            pin,
            native_llvm: native_llvm.unwrap_or_else(|| "/opt/homebrew/opt/llvm@15".into()),
        })
    }
    fn source(&self, name: &str) -> PathBuf {
        self.work.join("sources").join(name)
    }
    pub fn fetch(&self) -> Result<()> {
        for (name, pin) in &self.pin.sources {
            checkout(&self.source(name), pin)?;
            git(
                &self.source(name),
                &[
                    "submodule",
                    "update",
                    "--init",
                    "--recursive",
                    "--depth",
                    "1",
                ],
            )?;
        }
        Ok(())
    }
    fn verify_source(&self, name: &str) -> Result<String> {
        let source = self.source(name);
        ensure!(
            git(&source, &["rev-parse", "HEAD"])? == self.pin.sources[name].revision,
            "Frontend source revision mismatch: {name}"
        );
        if !git(&source, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
            let marker = json(&source.join(".nucleon-overlay.json"))?;
            ensure!(
                name == "dxmt"
                    && marker["diff_sha256"] == source_diff(&source)?
                    && marker["patches"]
                        == serde_json::to_value(hash_files(&repo(), &self.pin.dxmt.patches)?)?,
                "Modified frontend source: {name}"
            );
        }
        let modules = git(&source, &["submodule", "status", "--recursive"])?;
        ensure!(
            !modules.lines().any(|l| l.starts_with(['-', '+', 'U'])),
            "Missing/changed frontend submodules; run fetch"
        );
        Ok(modules)
    }
    pub fn doctor(&self) -> Result<Value> {
        let llvm = output(
            self.native_llvm.join("bin/llvm-config"),
            &["--version".into()],
            Some(&self.graphics.env),
            None,
        )
        .context("Install LLVM 15.0.7 or pass --native-llvm")?;
        ensure!(
            llvm == self.pin.dxmt.native_llvm_version,
            "DXMT requires LLVM {}",
            self.pin.dxmt.native_llvm_version
        );
        let metal = output(
            "/usr/bin/xcrun",
            &[
                "-sdk".into(),
                "macosx".into(),
                "metal".into(),
                "--version".into(),
            ],
            Some(&self.graphics.env),
            None,
        )
        .context("Install Xcode's Metal toolchain")?;
        Ok(json!({"native_llvm":llvm,"metal":metal}))
    }
    fn teb_flags(&self) -> Vec<String> {
        vec![
            "-femulated-tls".into(),
            "-include".into(),
            p(repo().join("tools/runtime-probes/darwin-teb.h")),
            format!(
                "-DNUCLEON_TEB_TSD_OFFSET={}",
                self.pin.darwin.teb_tsd_offset
            ),
        ]
    }
    pub fn darwin_startup(&self) -> Result<PathBuf> {
        self.verify_source("mingw")?;
        let offset = &self.pin.darwin.teb_tsd_offset;
        ensure!(
            fs::read_to_string(self.graphics.work.join("wine-build/Makefile"))?
                .contains(&format!("-D__WINE_TEB_TSD_OFFSET={offset} "))
                && fs::read_to_string(self.graphics.base.work.join("fex/arm64ec/CMakeCache.txt"))?
                    .contains(&format!("FEX_TEB_TSD_OFFSET:STRING={offset}\n")),
            "Darwin thread-pointer ABI differs from paired Wine/FEX"
        );
        let startup = self.work.join("crt");
        fs::create_dir_all(&startup)?;
        let source = self.source("mingw").join("mingw-w64-crt");
        let compiler = self
            .graphics
            .base
            .compiler
            .join("bin/arm64ec-w64-mingw32-gcc");
        for (input, target, mut flags) in [
            (
                source.join("crt/crtdll.c"),
                "dllcrt2.o",
                vec![
                    "-D_SYSCRT".into(),
                    "-DCRTDLL".into(),
                    "-D_UCRT".into(),
                    format!("-I{}/include", source.display()),
                ],
            ),
            (
                repo().join("tools/runtime-probes/darwin-emutls.c"),
                "emutls.o",
                vec!["-Wall".into(), "-Wextra".into(), "-Werror".into()],
            ),
        ] {
            let mut args = vec![
                "-O2".into(),
                "-c".into(),
                p(input),
                "-o".into(),
                p(startup.join(target)),
            ];
            args.append(&mut flags);
            args.extend(self.teb_flags());
            run(
                &compiler,
                &args,
                Some(&self.graphics.env),
                None,
                Some(&self.logs.join("crt-build.log")),
                300,
                0,
            )?;
        }
        Ok(startup)
    }
    pub fn build(&self, name: &str, architecture: &str) -> Result<()> {
        ensure!(
            ["dxvk", "vkd3d", "dxmt"].contains(&name),
            "Unknown frontend component"
        );
        ensure!(
            name != "dxmt" || architecture == "arm64ec",
            "DXMT requires a matching ARM64 Unix companion; x64 control remains separately paired"
        );
        let prerequisites = if name == "dxmt" {
            Some(self.doctor()?)
        } else {
            None
        };
        let modules = self.verify_source(name)?;
        if name == "dxmt" {
            patches(&self.source(name), &self.pin.dxmt.patches)?;
            self.verify_source(name)?;
        }
        let cross = self.work.join(format!("{architecture}.ini"));
        fs::write(
            &cross,
            cross_file(&self.graphics.base.compiler, architecture)?,
        )?;
        let build = self.work.join("build").join(architecture).join(name);
        fs::create_dir_all(build.parent().unwrap())?;
        let log = self.logs.join(format!("build-{name}-{architecture}.log"));
        let mut options = vec![
            "-Dbuildtype=release".into(),
            "--wrap-mode=nodownload".into(),
        ];
        let mut provenance = json!({});
        if architecture == "arm64ec" {
            let startup = self.darwin_startup()?;
            let flags = quote_args(&self.teb_flags())?;
            let link = quote_args(&[
                format!("-B{}", startup.display()),
                p(startup.join("emutls.o")),
            ])?;
            options.extend([
                format!("-Dc_args={flags}"),
                format!("-Dcpp_args={flags}"),
                format!("-Dc_link_args={link}"),
                format!("-Dcpp_link_args={link}"),
            ]);
            provenance = json!({"crt_sha256":sha(&startup.join("dllcrt2.o"))?,"emutls_sha256":sha(&startup.join("emutls.o"))?,"teb_header_sha256":sha(&repo().join("tools/runtime-probes/darwin-teb.h"))?,"mingw_source":self.pin.sources["mingw"]});
        }
        options.extend(match name {
            "dxvk" => vec![
                "-Denable_d3d8=false".into(),
                "-Denable_d3d9=false".into(),
                "-Denable_d3d10=false".into(),
            ],
            "vkd3d" => vec![
                "-Denable_tests=false".into(),
                "-Denable_extras=false".into(),
            ],
            _ => vec![
                "-Denable_tests=false".into(),
                "-Dwine_builtin_dll=false".into(),
                format!(
                    "-Dwine_build_path={}/wine-build",
                    self.graphics.work.display()
                ),
                format!("-Dnative_llvm_path={}", self.native_llvm.display()),
            ],
        });
        let mut receipt = json!({"schema":2,"status":"BUILDING","architecture":architecture,"source":self.pin.sources[name],"submodules":modules,"recipe_sha256":sha(&repo().join(RECIPE))?,"cross_sha256":sha(&cross)?,"toolchain":self.graphics.base.recipe.toolchain,"log":log,"options":options,"prerequisites":prerequisites});
        receipt
            .as_object_mut()
            .unwrap()
            .extend(provenance.as_object().unwrap().clone());
        if name == "dxmt" {
            receipt["overlay"] = json(&self.source(name).join(".nucleon-overlay.json"))?;
        }
        let report = self.work.join(format!("build-{name}-{architecture}.json"));
        write_json(&report, &receipt)?;
        let result = (|| {
            let mut args = vec!["setup".into()];
            if build.join("build.ninja").exists() {
                args.push("--reconfigure".into());
            }
            args.extend([
                p(&build),
                p(self.source(name)),
                "--cross-file".into(),
                p(&cross),
            ]);
            args.extend(options);
            run(
                self.graphics.work.join("venv/bin/meson"),
                &args,
                Some(&self.graphics.env),
                None,
                Some(&log),
                3600,
                0,
            )?;
            run(
                "ninja",
                &[
                    "-C".into(),
                    p(&build),
                    "-j".into(),
                    self.graphics.base.jobs.to_string(),
                ],
                Some(&self.graphics.env),
                None,
                Some(&log),
                4 * 3600,
                0,
            )?;
            let names = match name {
                "dxvk" => vec!["dxgi.dll", "d3d11.dll"],
                "vkd3d" => vec!["d3d12.dll", "d3d12core.dll"],
                _ => vec!["d3d11.dll", "dxgi.dll", "winemetal.dll", "winemetal.so"],
            };
            let stage = self.work.join("bundles").join(architecture).join(name);
            fs::create_dir_all(stage.parent().unwrap())?;
            let temp = tempfile::tempdir_in(stage.parent().unwrap())?;
            let outputs = crate::release::files(&build)?;
            for name in &names {
                let candidates: Vec<_> = outputs
                    .iter()
                    .filter(|p| p.file_name().is_some_and(|s| s == *name))
                    .collect();
                ensure!(
                    candidates.len() == 1,
                    "Ambiguous/missing build output: {name}"
                );
                let source = build.join(candidates[0]);
                if name.ends_with(".dll") {
                    let headers = output(
                        self.graphics.base.compiler.join("bin/llvm-readobj"),
                        &["--file-headers".into(), p(&source)],
                        None,
                        None,
                    )?;
                    ensure!(
                        headers.contains(if architecture == "arm64ec" {
                            "COFF-ARM64EC"
                        } else {
                            "COFF-x86-64"
                        }),
                        "Unexpected frontend ABI: {name}"
                    );
                }
                copy(&source, &temp.path().join(name))?;
            }
            receipt["status"] = "BUILT_NOT_EXECUTION_VALIDATED".into();
            receipt["files"] = serde_json::to_value(hash_files(
                temp.path(),
                &names.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
            )?)?;
            write_json(&temp.path().join("build.json"), &receipt)?;
            if stage.exists() {
                fs::rename(
                    &stage,
                    stage.with_extension(format!(
                        "previous-{}",
                        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
                    )),
                )?;
            }
            nucleon_core::artifacts::publish_new(temp.path(), &stage)?;
            Ok(())
        })();
        if let Err(error) = &result {
            receipt["status"] = "BUILD_FAILED".into();
            receipt["error"] = format!("{error:#}").into();
        }
        write_json(&report, &receipt)?;
        result
    }
    pub fn assemble(&self, baseline: &Path) -> Result<PathBuf> {
        let baseline = baseline.canonicalize()?;
        let original = fs::read_to_string(baseline.join("nucleon-runtime.toml"))?;
        let descriptor = experimental_descriptor(&original)?;
        let d: toml::Value = toml::from_str(&descriptor)?;
        let base = nucleon_core::runtime::ResolvedRuntime::inspect(&baseline)?;
        let mut copies: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
        let mut builds = BTreeMap::new();
        for (component, names) in [
            ("dxvk", vec!["d3d11.dll", "dxgi.dll"]),
            ("vkd3d", vec!["d3d12.dll", "d3d12core.dll"]),
        ] {
            let bundle = self.work.join("bundles/arm64ec").join(component);
            let report = json(&bundle.join("build.json"))?;
            ensure!(
                report["architecture"] == "arm64ec"
                    && report["status"] == "BUILT_NOT_EXECUTION_VALIDATED",
                "Incomplete ARM64EC frontend build"
            );
            verify_hashes(&bundle, &report["files"])?;
            for name in names {
                copies.insert(
                    Path::new(SHARED).join(component).join(name),
                    bundle.join(name),
                );
            }
            copies.insert(
                Path::new(SHARED).join(component).join("build.json"),
                bundle.join("build.json"),
            );
            for (name, field, source) in [
                ("dllcrt2.o", "crt_sha256", self.work.join("crt/dllcrt2.o")),
                ("emutls.o", "emutls_sha256", self.work.join("crt/emutls.o")),
                (
                    "darwin-teb.h",
                    "teb_header_sha256",
                    repo().join("tools/runtime-probes/darwin-teb.h"),
                ),
            ] {
                ensure!(report[field] == sha(&source)?, "ARM64EC CRT/TLS mismatch");
                copies.insert(Path::new(SHARED).join("crt").join(name), source);
            }
            builds.insert(component, report);
        }
        copies.insert(
            Path::new(SHARED).join("arm64ec-graphics.toml"),
            repo().join(RECIPE),
        );
        copies.insert(
            Path::new(SHARED).join("crt/darwin-emutls.c"),
            repo().join("tools/runtime-probes/darwin-emutls.c"),
        );
        let windows = &base.windows_dirs[0];
        let icd = base
            .graphics
            .vulkan_icd
            .as_ref()
            .context("Missing baseline Vulkan driver")?;
        let driver = nucleon_core::artifacts::vulkan_library(icd)?;
        for source in [
            base.loader.clone(),
            base.server.clone(),
            base.unix_dir.join("ntdll.so"),
            base.unix_dir.join("winevulkan.so"),
            base.unix_dir.join("libarm64ecfex.so"),
            windows.join("ntdll.dll"),
            windows.join("winevulkan.dll"),
            windows.join("libarm64ecfex.dll"),
            icd.clone(),
            driver,
            base.graphics
                .vulkan_loader
                .clone()
                .context("Missing Vulkan loader")?,
        ] {
            let canonical = source.canonicalize()?;
            ensure!(
                canonical.starts_with(&baseline),
                "Runtime component escapes baseline"
            );
            copies.insert(canonical.strip_prefix(&baseline)?.to_path_buf(), canonical);
        }
        let hashes: BTreeMap<_, _> = copies
            .iter()
            .map(|(relative, source)| Ok((p(relative), sha(source)?)))
            .collect::<Result<_>>()?;
        let contract = json!({"schema":1,"frontend_abi":"arm64ec","builds":builds,"files":hashes});
        let inputs = json!({"schema":2,"baseline":baseline,"descriptor":descriptor,"pairing":contract,"validation":"NOT_EXECUTION_VALIDATED","steam_validation":"NOT_RUN","gameplay_validation":"NOT_RUN"});
        let parent = self.work.parent().unwrap().join("arm64ec-integrated");
        fs::create_dir_all(&parent)?;
        let target = parent.join(format!("rust-{}", &key(&inputs)?[..20]));
        if target.exists() {
            nucleon_core::runtime::ResolvedRuntime::inspect(&target)?;
            ensure!(
                json(&target.join(SHARED).join("frontend.json"))? == contract,
                "Existing ARM64EC generation changed"
            );
            return Ok(target);
        }
        let temp = tempfile::tempdir_in(&parent)?;
        let stage = temp.path().join("runtime");
        copy_tree(&baseline, &stage)?;
        let receipt = stage.join(nucleon_core::managed_runtime::RECEIPT);
        if receipt.exists() {
            fs::remove_file(receipt)?;
        }
        for path in [
            &base.graphics.dxmt,
            &base.graphics.dxvk,
            &base.graphics.vkd3d,
        ]
        .into_iter()
        .flatten()
        {
            let obsolete = stage.join(path.strip_prefix(&baseline)?);
            if obsolete.is_dir() {
                fs::remove_dir_all(obsolete)?;
            }
        }
        for (relative, source) in &copies {
            copy(source, &stage.join(relative))?;
        }
        verify_hashes(&stage, &contract["files"])?;
        write_json(&stage.join(SHARED).join("frontend.json"), &contract)?;
        write_json(&stage.join(SHARED).join("assembly.json"), &inputs)?;
        fs::write(stage.join("nucleon-runtime.toml"), toml::to_string(&d)?)?;
        // Reuse the production contract validator instead of duplicating provenance/ABI checks.
        nucleon_core::runtime::ResolvedRuntime::inspect(&stage)?;
        nucleon_core::artifacts::publish_new(&stage, &target)?;
        Ok(target)
    }
    pub fn probe(&self, architecture: &str, api: &str) -> Result<()> {
        let names = match api {
            "dx12" => vec!["dxvk", "vkd3d"],
            "dx11" => vec!["dxmt"],
            "dx11-vulkan" => vec!["dxvk"],
            _ => anyhow::bail!("Unknown graphics API"),
        };
        let label = format!("frontend-{architecture}-{api}");
        let directory = self.work.join("runs").join(&label).join(
            chrono::Utc::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
                .to_string(),
        );
        fs::create_dir_all(&directory)?;
        let log = directory.join("setup.log");
        let report = directory.join("receipt.json");
        let program = self.work.join(format!("probe-{api}-x64.exe"));
        let dx12 = api == "dx12";
        let mut args = vec![
            "-O2".into(),
            "-Wall".into(),
            "-Wextra".into(),
            "-Werror".into(),
            p(repo().join(if dx12 {
                "tools/runtime-probes/d3d12.cpp"
            } else {
                "tools/runtime-probes/d3d11.c"
            })),
            "-o".into(),
            p(&program),
            "-static".into(),
        ];
        args.extend(
            if dx12 {
                vec!["-ld3d12", "-ldxgi", "-ld3dcompiler", "-ldxguid", "-luser32"]
            } else {
                vec![
                    "-ld3d11",
                    "-ldxgi",
                    "-ld3dcompiler",
                    "-ldxguid",
                    "-municode",
                ]
            }
            .into_iter()
            .map(str::to_owned),
        );
        run(
            self.graphics.base.compiler.join(if dx12 {
                "bin/x86_64-w64-mingw32-g++"
            } else {
                "bin/x86_64-w64-mingw32-gcc"
            }),
            &args,
            Some(&self.graphics.env),
            None,
            Some(&log),
            300,
            0,
        )?;
        let compiler = shader_compiler()?;
        copy(&compiler, &self.work.join("d3dcompiler_47.dll"))?;
        let (build, loader, mut env) = self.graphics.execution_environment(&label)?;
        let mut receipt = json!({"schema":2,"status":"TESTING","architecture":architecture,"api":api,"bundles":{},"probe_sha256":sha(&program)?,"shader_compiler_sha256":sha(&compiler)?,"gameplay_validation":"NOT_RUN","steam_validation":"NOT_RUN","passed":[]});
        let result = (|| {
            run(
                &loader,
                &["wineboot".into(), "-i".into()],
                Some(&env),
                None,
                Some(&log),
                120,
                0,
            )?;
            let system = PathBuf::from(&env[std::ffi::OsStr::new("WINEPREFIX")])
                .join("drive_c/windows/system32");
            for name in names {
                let bundle = self.work.join("bundles").join(architecture).join(name);
                let input = json(&bundle.join("build.json"))?;
                ensure!(
                    input["architecture"] == architecture
                        && input["status"] == "BUILT_NOT_EXECUTION_VALIDATED",
                    "Wrong/incomplete frontend build"
                );
                verify_hashes(&bundle, &input["files"])?;
                for entry in fs::read_dir(&bundle)? {
                    let path = entry?.path();
                    if path.extension().is_some_and(|s| s == "dll") {
                        let name = path.file_name().unwrap();
                        copy(&path, &system.join(name))?;
                        set(
                            &mut env,
                            &format!(
                                "NUCLEON_IMAGE_{}",
                                name.to_string_lossy().to_uppercase().replace('.', "_")
                            ),
                            nucleon_core::artifacts::pe_identity(&path)?,
                        );
                    }
                }
                receipt["bundles"][name] = input;
                if api == "dx11" {
                    copy(&bundle.join("winemetal.so"), &system.join("winemetal.so"))?;
                }
            }
            let overrides = if api == "dx11" {
                "d3d11,dxgi,winemetal=n,b;d3d12,d3d12core=b;d3dcompiler_47=n"
            } else if api == "dx11-vulkan" {
                "vulkan-1,winevulkan=b;d3d11,dxgi=n,b;d3dcompiler_47=n"
            } else {
                "vulkan-1,winevulkan=b;d3d12,d3d12core,dxgi,d3d11=n,b;d3dcompiler_47=n"
            };
            set(&mut env, "WINEDLLOVERRIDES", overrides);
            set(&mut env, "NUCLEON_EXPECT_GRAPHICS_ABI", architecture);
            if api == "dx11" {
                set(
                    &mut env,
                    "WINEDLLPATH",
                    format!(
                        "{}:{}",
                        system.display(),
                        self.graphics
                            .work
                            .join("runtime/lib/wine/aarch64-windows")
                            .display()
                    ),
                );
                for name in ["DYLD_LIBRARY_PATH", "DYLD_FALLBACK_LIBRARY_PATH"] {
                    set(
                        &mut env,
                        name,
                        format!(
                            "{}:{}",
                            system.display(),
                            self.graphics
                                .work
                                .join("runtime/lib/wine/aarch64-unix")
                                .display()
                        ),
                    );
                }
                for name in ["VK_DRIVER_FILES", "VK_ICD_FILENAMES", "VK_LOADER_DEBUG"] {
                    env.remove(std::ffi::OsStr::new(name));
                }
                set(&mut env, "NUCLEON_EXPECT_METAL", "1");
            }
            for (phase, negative) in [("draw", false), ("control", true)] {
                let mut args = vec![windows_path(&program)?];
                if negative {
                    args.push("--no-draw".into());
                }
                let marker = match (dx12, negative) {
                    (true, false) => "NUCLEON_D3D12_OK",
                    (false, false) => "NUCLEON_D3D11_TRIANGLE_OK",
                    (true, true) => "NUCLEON_D3D12_FAIL triangle-pixels",
                    (false, true) => "NUCLEON_D3D11_FAIL triangle-stage=21",
                };
                marker_run(
                    &loader,
                    &args,
                    &env,
                    Some(&self.work),
                    &directory.join(format!("setup-{phase}.log")),
                    90,
                    if negative { 21 } else { 0 },
                    marker,
                )?;
                receipt["passed"].as_array_mut().unwrap().push(phase.into());
                write_json(&report, &receipt)?;
            }
            if api == "dx11" {
                let mut wrong = env.clone();
                set(
                    &mut wrong,
                    "NUCLEON_IMAGE_D3D11_DLL",
                    "00000000:00000000:00000000",
                );
                marker_run(
                    &loader,
                    &[windows_path(&program)?],
                    &wrong,
                    Some(&self.work),
                    &directory.join("wrong-component.log"),
                    90,
                    6,
                    "NUCLEON_FRONTEND_FAIL wrong component d3d11.dll",
                )?;
                receipt["component_negative_control"] = "EXPECTED_COMPONENT_MISMATCH_EXIT_6".into();
            }
            Ok(())
        })();
        let result = finish_probe(
            &mut receipt,
            &report,
            result,
            build.stop_server(&env),
            "GPU_PROBES_PASSED_NOT_GAME_VALIDATED",
        );
        write_json(&self.work.join(format!("{label}.json")), &receipt)?;
        result
    }
    pub fn probe_tls(&self) -> Result<()> {
        let startup = self.darwin_startup()?;
        let dir = self.work.join("tls-probe");
        fs::create_dir_all(&dir)?;
        let log = self.logs.join("tls-probe.log");
        let mut flags = vec![
            "-O2".into(),
            "-shared".into(),
            "-Wall".into(),
            "-Wextra".into(),
            "-Werror".into(),
            "-DNUCLEON_TLS_TEST".into(),
        ];
        flags.extend(self.teb_flags());
        flags.extend([
            format!("-B{}", startup.display()),
            p(repo().join("tools/runtime-probes/frontend-tls.c")),
            p(repo().join("tools/runtime-probes/darwin-emutls.c")),
            "-static".into(),
            "-o".into(),
            p(dir.join("frontend-tls.dll")),
        ]);
        run(
            self.graphics
                .base
                .compiler
                .join("bin/arm64ec-w64-mingw32-gcc"),
            &flags,
            Some(&self.graphics.env),
            None,
            Some(&log),
            300,
            0,
        )?;
        run(
            self.graphics
                .base
                .compiler
                .join("bin/x86_64-w64-mingw32-gcc"),
            &[
                "-O2".into(),
                "-Wall".into(),
                "-Wextra".into(),
                "-Werror".into(),
                p(repo().join("tools/runtime-probes/frontend-tls-caller.c")),
                "-static".into(),
                "-o".into(),
                p(dir.join("caller.exe")),
            ],
            Some(&self.graphics.env),
            None,
            Some(&log),
            300,
            0,
        )?;
        let (build, loader, mut env) = self.graphics.execution_environment("frontend-tls")?;
        let mut receipt = json!({"schema":2,"status":"TESTING","dll_sha256":sha(&dir.join("frontend-tls.dll"))?,"caller_sha256":sha(&dir.join("caller.exe"))?,"log":log});
        let result = (|| {
            run(
                &loader,
                &["wineboot".into(), "-i".into()],
                Some(&env),
                None,
                Some(&log),
                120,
                0,
            )?;
            set(&mut env, "WINEDLLOVERRIDES", "frontend-tls=n");
            marker_run(
                &loader,
                &[windows_path(&dir.join("caller.exe"))?],
                &env,
                Some(&dir),
                &log,
                60,
                0,
                "NUCLEON_FRONTEND_TLS_PASS",
            )
        })();
        finish_probe(
            &mut receipt,
            &self.work.join("tls-result.json"),
            result,
            build.stop_server(&env),
            "TLS_PROBE_PASSED",
        )
    }
}
pub fn windows_path(path: &Path) -> Result<String> {
    Ok(format!(
        "Z:{}",
        p(std::path::absolute(path)?).replace('/', "\\")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cross_files_and_shell_arguments_keep_paths_as_data() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("bin")).unwrap();
        for name in ["gcc", "g++", "ar", "strip", "windres", "widl"] {
            fs::write(
                dir.path().join(format!("bin/arm64ec-w64-mingw32-{name}")),
                [],
            )
            .unwrap();
        }
        let text = cross_file(dir.path(), "arm64ec").unwrap();
        assert!(text.contains("cpu_family = 'aarch64'"));
        assert!(cross_file(dir.path(), "x86").is_err());
        let values = vec![
            "-femulated-tls".into(),
            "path with spaces and 'quotes'".into(),
        ];
        assert_eq!(shlex::split(&quote_args(&values).unwrap()).unwrap(), values);
    }
    #[test]
    fn descriptor_transformation_scopes_guests_and_preserves_execution_contract() {
        let mut source: toml::Value = toml::from_str(include_str!(
            "../../../assets/wine/runtime-arm64-fex.example.toml"
        ))
        .unwrap();
        let result = experimental_descriptor(&toml::to_string(&source).unwrap()).unwrap();
        let native: toml::Value = toml::from_str(&result).unwrap();
        assert_eq!(native["guests"].as_array().unwrap().len(), 1);
        assert_eq!(native["graphics"]["frontend_abi"].as_str(), Some("arm64ec"));
        assert!(native["graphics"].get("dxmt").is_none());
        assert_eq!(native["emulators"].as_array().unwrap().len(), 1);
        source["mode"] = "intel".into();
        assert!(experimental_descriptor(&toml::to_string(&source).unwrap()).is_err());
    }
}
