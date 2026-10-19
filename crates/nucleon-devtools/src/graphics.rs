use crate::{
    build::{Build, finish_probe, marker_run},
    common::{
        ArchivePin, CANCELLED, Environment, Pin, checkout, clean_environment, copy, copy_tree,
        fetch_archive, git, hash_files, json, key, output, p, repo, run, set, sha, source_diff,
        write_json,
    },
};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub const RECIPE: &str = "assets/wine/recipes/arm64-fex-dx12.toml";
#[derive(Deserialize)]
pub struct GraphicsRecipe {
    pub schema: u32,
    pub steamac: Pin,
    pub mesa: Pin,
    pub python: BTreeMap<String, String>,
    pub wine: ExtraPatches,
    pub archives: BTreeMap<String, ArchivePin>,
}
#[derive(Deserialize)]
pub struct ExtraPatches {
    pub patches: Vec<String>,
}
pub struct Graphics {
    pub base: Build,
    pub work: PathBuf,
    pub sources: PathBuf,
    pub logs: PathBuf,
    pub sdk: PathBuf,
    pub env: Environment,
    pub pin: GraphicsRecipe,
    pub debug: bool,
}
impl Graphics {
    pub fn new(workspace: &Path, sdk: &Path, jobs: usize, debug: bool) -> Result<Self> {
        let base = Build::new(workspace, jobs)?;
        let work = base.workspace.join("dx12");
        let logs = work.join("logs");
        fs::create_dir_all(&logs)?;
        let pin: GraphicsRecipe = toml::from_str(&fs::read_to_string(repo().join(RECIPE))?)?;
        ensure!(pin.schema == 1, "Unsupported graphics recipe");
        let mut env = base.env.clone();
        let path = env
            .get(std::ffi::OsStr::new("PATH"))
            .unwrap()
            .to_string_lossy()
            .into_owned();
        set(
            &mut env,
            "PATH",
            format!(
                "{}/venv/bin:/opt/homebrew/opt/llvm/bin:{path}",
                work.display()
            ),
        );
        Ok(Self {
            sources: work.join("sources"),
            base,
            work,
            logs,
            sdk: std::path::absolute(sdk)?,
            env,
            pin,
            debug,
        })
    }
    pub fn wine_build(&self) -> Result<Build> {
        let mut build = Build::new(&self.base.workspace, self.base.jobs)?;
        build.work = self.work.clone();
        build.logs = self.logs.clone();
        build.wine = self.sources.join("wine");
        build.env = self.env.clone();
        let link = self.work.join("fex");
        if !link.exists() {
            std::os::unix::fs::symlink(self.base.work.join("fex"), link)?;
        }
        Ok(build)
    }
    pub fn fetch(&self) -> Result<()> {
        for (name, pin) in [("steamac", &self.pin.steamac), ("mesa", &self.pin.mesa)] {
            checkout(&self.sources.join(name), pin)?;
        }
        for pin in self.pin.archives.values() {
            fetch_archive(&self.work.join("archives"), pin)?;
        }
        let wine = self.sources.join("wine");
        if !wine.join(".git").exists() {
            fs::create_dir_all(&self.sources)?;
            run(
                "git",
                &[
                    "clone".into(),
                    "--shared".into(),
                    "--no-checkout".into(),
                    p(&self.base.wine),
                    p(&wine),
                ],
                None,
                None,
                None,
                600,
                0,
            )?;
            git(
                &wine,
                &[
                    "checkout",
                    "--detach",
                    &self.base.recipe.alternatives["tolkara"]["wine"].revision,
                ],
            )?;
        }
        let venv = self.work.join("venv");
        if !venv.join("bin/python3").exists() {
            run(
                "python3",
                &["-m".into(), "venv".into(), p(&venv)],
                None,
                None,
                None,
                300,
                0,
            )?;
        }
        let mut args = vec!["-m".into(), "pip".into(), "install".into()];
        args.extend(self.pin.python.iter().map(|(k, v)| format!("{k}=={v}")));
        run(
            venv.join("bin/python3"),
            &args,
            None,
            None,
            Some(&self.logs.join("python-dependencies.log")),
            600,
            0,
        )
    }
    pub fn build_wine(&self) -> Result<()> {
        let build = self.wine_build()?;
        build.build_wine(&self.pin.wine.patches, Some(&self.sdk))?;
        let runtime = self.work.join("runtime");
        let path = runtime.join("build.json");
        let mut receipt = json(&path)?;
        let names = [
            "lib/wine/aarch64-windows/ntdll.dll",
            "lib/wine/aarch64-windows/winevulkan.dll",
            "lib/wine/aarch64-windows/vulkan-1.dll",
            "lib/wine/aarch64-windows/user32.dll",
            "lib/wine/aarch64-unix/winevulkan.so",
            "lib/wine/aarch64-unix/win32u.so",
        ]
        .map(str::to_owned);
        receipt["graphics_boundary_files"] = serde_json::to_value(hash_files(&runtime, &names)?)?;
        receipt["dx12_recipe_sha256"] = sha(&repo().join(RECIPE))?.into();
        receipt["vulkan_sdk"] = p(&self.sdk).into();
        receipt["vulkan_loader_sha256"] = sha(&self.sdk.join("lib/libvulkan.1.dylib"))?.into();
        copy(
            &repo().join(RECIPE),
            &runtime.join("share/nucleon-build/arm64-fex-dx12.toml"),
        )?;
        write_json(&path, &receipt)
    }
    pub fn prepare_mesa(&self) -> Result<()> {
        for (name, pin) in [("steamac", &self.pin.steamac), ("mesa", &self.pin.mesa)] {
            ensure!(
                git(&self.sources.join(name), &["rev-parse", "HEAD"])? == pin.revision,
                "Unexpected {name} revision"
            );
        }
        let source = self.sources.join("mesa");
        let marker = source.join(".nucleon-overlay.json");
        let mut patches: Vec<_> =
            fs::read_dir(self.sources.join("steamac/host/kosmickrisp/patches"))?
                .map(|e| e.map(|e| e.path()))
                .collect::<std::io::Result<_>>()?;
        patches.retain(|p| p.extension().is_some_and(|s| s == "patch"));
        patches.sort();
        ensure!(!patches.is_empty(), "Steamac patches missing; run fetch");
        let hashes: BTreeMap<_, _> = patches
            .iter()
            .map(|path| Ok((p(path.file_name().unwrap()), sha(path)?)))
            .collect::<Result<_>>()?;
        if marker.exists() {
            let old = json(&marker)?;
            ensure!(
                old["diff_sha256"] == source_diff(&source)?
                    && old["patches"] == serde_json::to_value(hashes)?,
                "Preserve modified Mesa checkout"
            );
            return Ok(());
        }
        ensure!(
            git(&source, &["status", "--porcelain"])?.is_empty(),
            "Preserve modified Mesa checkout"
        );
        for patch in patches {
            run(
                "git",
                &["apply".into(), "--whitespace=nowarn".into(), p(patch)],
                None,
                Some(&source),
                Some(&self.logs.join("mesa-patches.log")),
                60,
                0,
            )?;
        }
        let untracked = git(&source, &["ls-files", "--others", "--exclude-standard"])?;
        if !untracked.is_empty() {
            let mut args = vec!["add", "--intent-to-add", "--"];
            args.extend(untracked.lines());
            git(&source, &args)?;
        }
        write_json(
            &marker,
            &json!({"diff_sha256":source_diff(&source)?,"patches":hashes}),
        )
    }
    pub fn build_driver(&self) -> Result<()> {
        self.prepare_mesa()?;
        let installed: Vec<Value> = serde_json::from_str(&output(
            self.work.join("venv/bin/python3"),
            &[
                "-m".into(),
                "pip".into(),
                "list".into(),
                "--format=json".into(),
            ],
            None,
            None,
        )?)?;
        ensure!(
            self.pin
                .python
                .iter()
                .all(|(name, version)| installed.iter().any(|v| v["name"]
                    .as_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
                    && v["version"] == *version)),
            "Pinned Meson dependencies missing; run dx12 fetch"
        );
        let source = self.sources.join("mesa");
        let clc = self.work.join("clc");
        let common = [
            "--buildtype=release",
            "-Db_ndebug=true",
            "-Dgallium-drivers=",
            "-Dopengl=false",
            "-Dglx=disabled",
            "-Degl=disabled",
            "-Dgbm=disabled",
            "-Dzstd=disabled",
        ]
        .map(str::to_owned);
        for driver in [false, true] {
            let build = self.work.join(if driver {
                "mesa-build"
            } else {
                "mesa-clc-build"
            });
            let mut env = self.env.clone();
            if driver {
                let path = env
                    .get(std::ffi::OsStr::new("PATH"))
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                set(&mut env, "PATH", format!("{}/bin:{path}", clc.display()));
                set(&mut env, "MACOSX_DEPLOYMENT_TARGET", "26.0");
            }
            if !build.join("build.ninja").exists() {
                let mut args = vec!["setup".into(), p(&build), p(&source)];
                args.extend_from_slice(&common);
                args.extend(if driver {
                    vec![
                        "-Dplatforms=macos",
                        "-Dvulkan-drivers=kosmickrisp",
                        "-Dexpat=disabled",
                        "-Dllvm=disabled",
                        "-Dspirv-tools=disabled",
                        "-Dmesa-clc=system",
                        "-Dprecomp-compiler=enabled",
                        "--prefer-static",
                    ]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
                } else {
                    vec![
                        format!("--prefix={}", clc.display()),
                        "-Dplatforms=".into(),
                        "-Dvulkan-drivers=".into(),
                        "-Dllvm=enabled".into(),
                        "-Dshared-llvm=enabled".into(),
                        "-Dmesa-clc=enabled".into(),
                        "-Dinstall-mesa-clc=true".into(),
                        "-Dprecomp-compiler=enabled".into(),
                        "-Dinstall-precomp-compiler=true".into(),
                    ]
                });
                run(
                    "meson",
                    &args,
                    Some(&env),
                    None,
                    Some(&self.logs.join(if driver {
                        "driver-configure.log"
                    } else {
                        "clc-configure.log"
                    })),
                    3600,
                    0,
                )?;
            }
            let mut args = vec!["-C".into(), p(&build), format!("-j{}", self.base.jobs)];
            if !driver {
                args.push("install".into());
            }
            run(
                "ninja",
                &args,
                Some(&env),
                None,
                Some(&self.logs.join(if driver {
                    "driver-build.log"
                } else {
                    "clc-build.log"
                })),
                4 * 3600,
                0,
            )?;
        }
        let stage = self.work.join("driver");
        let native = stage.join("libvulkan_kosmickrisp.dylib");
        copy(
            &self
                .work
                .join("mesa-build/src/kosmickrisp/vulkan/libvulkan_kosmickrisp.dylib"),
            &native,
        )?;
        run(
            "install_name_tool",
            &[
                "-id".into(),
                "@rpath/libvulkan_kosmickrisp.dylib".into(),
                p(&native),
            ],
            None,
            None,
            None,
            30,
            0,
        )?;
        sign(&native)?;
        ensure!(
            output("lipo", &["-archs".into(), p(&native)], None, None)? == "arm64",
            "Driver must be native ARM64"
        );
        let imports = output("otool", &["-L".into(), p(&native)], None, None)?;
        ensure!(
            imports.lines().skip(2).all(|l| ["/usr/lib/", "/System/"]
                .iter()
                .any(|prefix| l.trim().starts_with(prefix))),
            "Driver has non-system dependencies"
        );
        write_json(
            &stage.join("kosmickrisp_icd.json"),
            &json!({"file_format_version":"1.0.1","ICD":{"library_path":"./libvulkan_kosmickrisp.dylib","api_version":"1.4.0"}}),
        )?;
        write_json(
            &stage.join("build.json"),
            &json!({"schema":2,"status":"BUILT_NOT_EXECUTION_VALIDATED","recipe_sha256":sha(&repo().join(RECIPE))?,"sources":{"steamac":self.pin.steamac,"mesa":self.pin.mesa},"overlay":json(&source.join(".nucleon-overlay.json"))?,"driver_sha256":sha(&native)?,"llvm":output("llvm-config",&["--version".into()],Some(&self.env),None)?}),
        )?;
        copy(
            &source.join("docs/license.rst"),
            &stage.join("MESA-LICENSE.rst"),
        )?;
        fs::write(
            stage.join("STEAMAC-LICENSE"),
            git(
                &self.sources.join("steamac"),
                &["show", &format!("{}:LICENSE", self.pin.steamac.revision)],
            )?,
        )?;
        for entry in fs::read_dir(self.sources.join("steamac/host/kosmickrisp/patches"))? {
            let path = entry?.path();
            if path.extension().is_some_and(|s| s == "patch") {
                copy(
                    &path,
                    &stage.join("source-patches").join(path.file_name().unwrap()),
                )?;
            }
        }
        Ok(())
    }
    pub fn graphics(&self) -> Result<PathBuf> {
        let stage = self.work.join("graphics");
        fs::create_dir_all(&stage)?;
        let mut hashes = BTreeMap::new();
        for (name, members) in [
            ("vkd3d", vec!["x64/d3d12.dll", "x64/d3d12core.dll"]),
            ("dxvk", vec!["x64/dxgi.dll"]),
        ] {
            let pin = &self.pin.archives[name];
            let archive = self.work.join("archives").join(&pin.archive);
            ensure!(
                sha(&archive)? == pin.sha256,
                "Graphics archive checksum mismatch"
            );
            let temp = tempfile::tempdir()?;
            nucleon_core::artifacts::unpack(&archive, temp.path(), 2 * 1024 * 1024 * 1024)?;
            let entries = crate::release::files(temp.path())?;
            for member in members {
                let candidates: Vec<_> = entries.iter().filter(|p| p.ends_with(member)).collect();
                ensure!(
                    candidates.len() == 1,
                    "Ambiguous/missing archive component: {member}"
                );
                let file = Path::new(member).file_name().unwrap();
                copy(&temp.path().join(candidates[0]), &stage.join(file))?;
                hashes.insert(p(file), sha(&stage.join(file))?);
            }
            for entry in entries {
                let file = entry.file_name().unwrap().to_string_lossy().into_owned();
                if ["license", "copying", "copying.lib"].contains(&file.to_lowercase().as_str()) {
                    copy(
                        &temp.path().join(&entry),
                        &stage.join(format!("{name}-{file}")),
                    )?;
                }
            }
        }
        write_json(
            &stage.join("build.json"),
            &json!({"schema":2,"archives":self.pin.archives,"files":hashes}),
        )?;
        Ok(stage)
    }
    pub fn stage_dxmt(&self) -> Result<PathBuf> {
        let pin = &self.base.recipe.reference;
        let archive = self.base.workspace.join(&pin.archive);
        ensure!(
            sha(&archive)? == pin.sha256,
            "Reference archive checksum mismatch"
        );
        let mut zip = zip::ZipArchive::new(fs::File::open(&archive)?)?;
        let root = "MacRunner.app/Contents/Resources/engine/";
        let inventory: Value = serde_json::from_slice(&nucleon_core::artifacts::read_limited(
            zip.by_name(&format!("{root}ENGINE.json"))?,
            16 * 1024 * 1024,
        )?)?;
        let stage = self.base.work.join("dxmt");
        for name in [
            "x86_64-windows/d3d11.dll",
            "x86_64-windows/dxgi.dll",
            "x86_64-windows/d3d10core.dll",
            "x86_64-windows/winemetal.dll",
            "aarch64-unix/winemetal.so",
        ] {
            let bytes = nucleon_core::artifacts::read_limited(
                zip.by_name(&format!("{root}graphics/{name}"))?,
                128 * 1024 * 1024,
            )?;
            use sha2::Digest;
            ensure!(
                inventory["files"][format!("graphics/{name}")]
                    == format!("{:x}", sha2::Sha256::digest(&bytes)),
                "DXMT component checksum mismatch"
            );
            let target = stage.join(name);
            fs::create_dir_all(target.parent().unwrap())?;
            fs::write(target, bytes)?;
        }
        sign(&stage.join("aarch64-unix/winemetal.so"))?;
        Ok(stage)
    }
    pub fn prepare_runtime(&self) -> Result<PathBuf> {
        let mut source = self.work.join("runtime");
        if !source.join("bin/wine").is_file() {
            source = self.base.work.join("runtime");
        }
        let unix = self.base.build_unixlib()?;
        let receipt = json(&source.join("build.json"))?;
        verify_hashes(&source, &receipt["host_files"])?;
        let dxmt = self.base.work.join("dxmt");
        let dxmt_files = crate::release::files(&dxmt)?;
        ensure!(
            !dxmt_files.is_empty(),
            "Stage validated DXMT first: dx12 stage-dxmt"
        );
        let vulkan = source.join("lib/wine/aarch64-unix/winevulkan.so").is_file();
        let graphics = if vulkan { Some(self.graphics()?) } else { None };
        let mut inputs = json!({"schema":2,"wine":receipt,"dxmt":hash_files(&dxmt,&dxmt_files.iter().map(p).collect::<Vec<_>>())?,"recipe_sha256":sha(&repo().join(RECIPE))?,"unixlib":json(&unix.join("build.json"))?,"vulkan":vulkan,"reference_sha256":self.base.recipe.reference.sha256});
        let mut emulators = BTreeMap::new();
        for (dir, name) in [
            ("arm64ec", "libarm64ecfex.dll"),
            ("wow64", "libwow64fex.dll"),
        ] {
            emulators.insert(
                name,
                sha(&self.base.work.join("fex").join(dir).join("Bin").join(name))?,
            );
        }
        inputs["emulators"] = serde_json::to_value(emulators)?;
        let driver = self.work.join("driver");
        let loader = self.sdk.join("lib/libvulkan.1.dylib");
        if let Some(graphics) = &graphics {
            inputs["graphics"] = json(&graphics.join("build.json"))?;
            inputs["driver"] = json(&driver.join("build.json"))?;
            ensure!(
                inputs["driver"]["driver_sha256"]
                    == sha(&driver.join("libvulkan_kosmickrisp.dylib"))?,
                "Driver differs from build receipt"
            );
            inputs["loader_sha256"] = sha(&loader)?.into();
        }
        let parent = self.base.workspace.join("integrated");
        fs::create_dir_all(&parent)?;
        let target = parent.join(format!("rust-{}", &key(&inputs)?[..20]));
        if target.exists() {
            nucleon_core::runtime::ResolvedRuntime::inspect(&target)?;
            ensure!(
                json(&target.join("share/nucleon/integration-inputs.json"))? == inputs,
                "Existing runtime generation changed"
            );
            return Ok(target);
        }
        let temp = tempfile::tempdir_in(&parent)?;
        let stage = temp.path().join("runtime");
        copy_tree(&source, &stage)?;
        let shared = stage.join("share/nucleon");
        fs::create_dir_all(&shared)?;
        copy_tree(&dxmt, &shared.join("dxmt"))?;
        for (dir, name) in [
            ("arm64ec", "libarm64ecfex.dll"),
            ("wow64", "libwow64fex.dll"),
        ] {
            copy(
                &self.base.work.join("fex").join(dir).join("Bin").join(name),
                &stage.join("lib/wine/aarch64-windows").join(name),
            )?;
        }
        for name in ["libarm64ecfex.so", "libwow64fex.so"] {
            copy(
                &unix.join(name),
                &stage.join("lib/wine/aarch64-unix").join(name),
            )?;
        }
        let mut descriptor: toml::Value = toml::from_str(&crate::release::core_descriptor())?;
        descriptor["backends"] = toml::Value::try_from(if vulkan {
            vec!["auto", "dxmt", "dxvk", "kosmickrisp"]
        } else {
            vec!["auto", "dxmt"]
        })?;
        descriptor["steam_protocol"] =
            toml::Value::String(if vulkan { "valve-redirect" } else { "bridge" }.into());
        let mut paths = BTreeMap::from([("dxmt", "share/nucleon/dxmt")]);
        if let Some(graphics) = graphics {
            copy_tree(&driver, &stage.join("lib/vulkan"))?;
            copy(&loader, &stage.join("lib/vulkan/libvulkan.1.dylib"))?;
            copy_tree(&graphics, &shared.join("graphics"))?;
            paths.extend([
                ("dxvk", "share/nucleon/graphics"),
                ("vkd3d", "share/nucleon/graphics"),
                ("vulkan_icd", "lib/vulkan/kosmickrisp_icd.json"),
                ("vulkan_loader", "lib/vulkan/libvulkan.1.dylib"),
            ]);
        }
        descriptor
            .as_table_mut()
            .unwrap()
            .insert("graphics".into(), toml::Value::try_from(paths)?);
        let reference = self
            .base
            .workspace
            .join(&self.base.recipe.reference.archive);
        ensure!(
            sha(&reference)? == self.base.recipe.reference.sha256,
            "Reference notice archive checksum mismatch"
        );
        let mut zip = zip::ZipArchive::new(fs::File::open(reference)?)?;
        for name in ["dxmt-LICENSE", "dxmt-COPYING.LIB"] {
            let bytes = nucleon_core::artifacts::read_limited(
                zip.by_name(&format!(
                    "MacRunner.app/Contents/Resources/engine/licenses/{name}"
                ))?,
                16 * 1024 * 1024,
            )?;
            let path = stage.join("share/licenses/dxmt").join(name);
            fs::create_dir_all(path.parent().unwrap())?;
            fs::write(path, bytes)?;
        }
        for (source, name) in [
            (
                self.sdk.join("share/vulkan/LICENSE.txt"),
                "VULKAN-LOADER-LICENSE",
            ),
            (
                self.sdk.parent().unwrap().join("Licenses/LICENSE.txt"),
                "LUNARG-SDK-LICENSES.txt",
            ),
        ] {
            if vulkan && source.is_file() {
                copy(&source, &shared.join(name))?;
            }
        }
        write_json(
            &shared.join("local-import-provenance.json"),
            &json!({"usage":"LOCAL_IMPORT_NOT_A_REDISTRIBUTION_PACKAGE","dxmt_reference":self.base.recipe.reference,"vulkan_sdk":self.sdk,"vulkan_loader_license":"Supply component notices before redistribution"}),
        )?;
        write_json(&shared.join("integration-inputs.json"), &inputs)?;
        fs::write(
            stage.join("nucleon-runtime.toml"),
            toml::to_string(&descriptor)?,
        )?;
        nucleon_core::runtime::ResolvedRuntime::inspect(&stage)?;
        nucleon_core::artifacts::publish_new(&stage, &target)?;
        Ok(target)
    }
    pub fn execution_environment(&self, prefix: &str) -> Result<(Build, PathBuf, Environment)> {
        let build = self.wine_build()?;
        let (loader, mut env) = build.runtime_environment(prefix)?;
        set(
            &mut env,
            "DYLD_LIBRARY_PATH",
            format!(
                "{}:{}:{}",
                self.work.join("driver").display(),
                self.work.join("runtime/lib/wine/aarch64-unix").display(),
                self.sdk.join("lib").display()
            ),
        );
        for name in ["VK_DRIVER_FILES", "VK_ICD_FILENAMES"] {
            set(
                &mut env,
                name,
                self.work.join("driver/kosmickrisp_icd.json"),
            );
        }
        let overrides = env[std::ffi::OsStr::new("WINEDLLOVERRIDES")]
            .to_string_lossy()
            .to_string();
        set(
            &mut env,
            "WINEDLLOVERRIDES",
            overrides + ";vulkan-1,winevulkan=b;d3d12,d3d12core,dxgi=n,b",
        );
        let cache = self.work.join("caches").join(prefix);
        fs::create_dir_all(&cache)?;
        set(&mut env, "VKD3D_SHADER_CACHE_PATH", &cache);
        set(&mut env, "MESA_SHADER_CACHE_DIR", cache.join("mesa"));
        for (name, value) in [
            ("VKD3D_DEBUG", if self.debug { "info" } else { "warn" }),
            ("DXVK_LOG_LEVEL", if self.debug { "info" } else { "warn" }),
            (
                "WINEDEBUG",
                if self.debug {
                    "-all,warn+all,err+all,+loaddll"
                } else {
                    "-all,warn+all,err+all"
                },
            ),
        ] {
            set(&mut env, name, value);
        }
        Ok((build, loader, env))
    }
    pub fn probe(&self, api: &str) -> Result<()> {
        let probes = self.work.join("probes");
        fs::create_dir_all(&probes)?;
        let dx12 = api == "d3d12";
        let dx11 = api == "dxmt";
        let vulkan = api == "vulkan";
        ensure!(dx12 || dx11 || vulkan, "Unknown graphics probe");
        let program = probes.join(format!("{api}-x64.exe"));
        let mut args = vec![
            "-O2".into(),
            "-Wall".into(),
            "-Wextra".into(),
            "-Werror".into(),
            p(repo().join(format!(
                "tools/runtime-probes/{}",
                if dx12 {
                    "d3d12.cpp"
                } else if dx11 {
                    "d3d11.c"
                } else {
                    "vulkan.c"
                }
            ))),
            "-o".into(),
            p(&program),
        ];
        args.extend(if dx12 {
            vec![
                "-static-libgcc".into(),
                "-static-libstdc++".into(),
                "-ld3d12".into(),
                "-ldxgi".into(),
                "-ld3dcompiler".into(),
                "-ldxguid".into(),
            ]
        } else if dx11 {
            vec![
                "-municode".into(),
                "-ld3d11".into(),
                "-ldxgi".into(),
                "-ld3dcompiler".into(),
            ]
        } else {
            vec![format!("-I{}/include", self.sdk.display())]
        });
        self.base.run(
            self.base.compiler.join(if dx12 {
                "bin/x86_64-w64-mingw32-g++"
            } else {
                "bin/x86_64-w64-mingw32-gcc"
            }),
            &args,
            None,
            "graphics-probe-build.log",
        )?;
        let (build, loader, mut env) = if dx11 {
            let (loader, env) = self.base.runtime_environment("dxmt-smoke")?;
            (
                Build::new(&self.base.workspace, self.base.jobs)?,
                loader,
                env,
            )
        } else {
            self.execution_environment(&format!("{api}-smoke"))?
        };
        let mut report = json!({"schema":2,"status":"TESTING","probe_sha256":sha(&program)?,"steam_launched":false});
        let result = (|| {
            run(
                &loader,
                &["wineboot".into(), "-i".into()],
                Some(&env),
                None,
                Some(&self.logs.join(format!("{api}-prefix.log"))),
                120,
                0,
            )?;
            let system = PathBuf::from(&env[std::ffi::OsStr::new("WINEPREFIX")])
                .join("drive_c/windows/system32");
            if dx12 {
                let graphics = self.graphics()?;
                for name in ["d3d12.dll", "d3d12core.dll", "dxgi.dll"] {
                    copy(&graphics.join(name), &system.join(name))?;
                }
                let compiler = shader_compiler()?;
                copy(&compiler, &probes.join("d3dcompiler_47.dll"))?;
                report["shader_compiler_sha256"] = sha(&compiler)?.into();
                let overrides = env[std::ffi::OsStr::new("WINEDLLOVERRIDES")]
                    .to_string_lossy()
                    .to_string();
                set(
                    &mut env,
                    "WINEDLLOVERRIDES",
                    overrides + ";d3dcompiler_47=n",
                );
            }
            if dx11 {
                let stage = self.base.work.join("dxmt");
                for name in ["d3d11.dll", "dxgi.dll", "d3d10core.dll", "winemetal.dll"] {
                    copy(&stage.join("x86_64-windows").join(name), &system.join(name))?;
                }
                set(
                    &mut env,
                    "WINEDLLPATH",
                    format!(
                        "{}:{}",
                        stage.display(),
                        self.base
                            .work
                            .join("runtime/lib/wine/aarch64-windows")
                            .display()
                    ),
                );
                set(
                    &mut env,
                    "DYLD_LIBRARY_PATH",
                    format!(
                        "{}:{}",
                        stage.join("aarch64-unix").display(),
                        self.base
                            .work
                            .join("runtime/lib/wine/aarch64-unix")
                            .display()
                    ),
                );
                let overrides = env[std::ffi::OsStr::new("WINEDLLOVERRIDES")]
                    .to_string_lossy()
                    .to_string();
                set(
                    &mut env,
                    "WINEDLLOVERRIDES",
                    overrides + ";d3d11,dxgi,d3d10core,winemetal=n,b",
                );
            }
            marker_run(
                &loader,
                &[p(&program)],
                &env,
                Some(&probes),
                &self.logs.join(format!("{api}.log")),
                180,
                0,
                if dx12 {
                    "NUCLEON_D3D12_OK"
                } else if dx11 {
                    "NUCLEON_D3D11_TRIANGLE_OK"
                } else {
                    "NUCLEON_VULKAN_OK device-and-win32-surface"
                },
            )?;
            if !vulkan {
                marker_run(
                    &loader,
                    &[p(&program), "--no-draw".into()],
                    &env,
                    Some(&probes),
                    &self.logs.join(format!("{api}-negative.log")),
                    90,
                    21,
                    if dx12 {
                        "NUCLEON_D3D12_FAIL triangle-pixels"
                    } else {
                        "NUCLEON_D3D11_FAIL triangle-stage=21"
                    },
                )?;
                report["negative_control"] = "EXPECTED_PIXEL_MISMATCH_WITHOUT_DRAW".into();
            }
            Ok(())
        })();
        finish_probe(
            &mut report,
            &probes.join(format!("{api}-result.json")),
            result,
            build.stop_server(&env),
            if vulkan {
                "X64_VULKAN_DEVICE_AND_SURFACE_PASSED"
            } else {
                "GPU_PROBES_PASSED_NOT_GAME_VALIDATED"
            },
        )
    }
}
pub fn verify_hashes(root: &Path, files: &Value) -> Result<()> {
    for (name, digest) in files.as_object().context("Missing file hashes")? {
        ensure!(
            sha(&root.join(name))? == digest.as_str().context("Invalid file digest")?,
            "Modified component: {name}"
        );
    }
    Ok(())
}
impl Graphics {
    fn compile_native(&self, source: &Path, target: &Path) -> Result<()> {
        run(
            "/usr/bin/clang",
            &[
                "-arch".into(),
                "arm64".into(),
                "-std=c11".into(),
                "-Wall".into(),
                "-Werror".into(),
                "-O1".into(),
                format!("-I{}/include", self.sdk.display()),
                p(source),
                format!("-L{}/lib", self.sdk.display()),
                "-lvulkan".into(),
                format!("-Wl,-rpath,{}/lib", self.sdk.display()),
                "-o".into(),
                p(target),
            ],
            Some(&self.env),
            None,
            Some(&self.logs.join("driver-tests-build.log")),
            300,
            0,
        )
    }
    pub fn probe_driver(&self) -> Result<()> {
        let driver = self.work.join("driver/libvulkan_kosmickrisp.dylib");
        run(
            "codesign",
            &["--verify".into(), "--strict".into(), p(&driver)],
            None,
            None,
            None,
            60,
            0,
        )?;
        let probes = self.work.join("probes");
        fs::create_dir_all(&probes)?;
        let here = self.sources.join("steamac/host/moltenvk");
        let mut env = self.env.clone();
        for (name, value) in [
            (
                "VK_DRIVER_FILES",
                p(self.work.join("driver/kosmickrisp_icd.json")),
            ),
            ("REPRO_DRIVER", "kosmickrisp".into()),
            ("MTL_DEBUG_LAYER", "1".into()),
            ("MTL_DEBUG_LAYER_ERROR_MODE", "assert".into()),
            (
                "DYLD_LIBRARY_PATH",
                format!(
                    "{}:{}",
                    self.work.join("driver").display(),
                    self.sdk.join("lib").display()
                ),
            ),
        ] {
            set(&mut env, name, value);
        }
        env.remove(std::ffi::OsStr::new("VK_ICD_FILENAMES"));
        let program = probes.join("driver-caps");
        self.compile_native(&here.join("probe/probe.c"), &program)?;
        run(
            program.as_os_str(),
            &[],
            Some(&env),
            None,
            Some(&self.logs.join("driver-caps.log")),
            60,
            0,
        )?;
        let shaders = probes.join("shaders");
        fs::create_dir_all(&shaders)?;
        for entry in fs::read_dir(here.join("repro/shaders"))? {
            let path = entry?.path();
            let extension = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            let output_path = shaders.join(format!(
                "{}.spv",
                path.file_name().unwrap().to_string_lossy()
            ));
            if ["vert", "geom", "frag", "comp"].contains(&extension) {
                let mut args = vec!["-V".into(), "--quiet".into()];
                if extension == "comp" {
                    args.extend(["--target-env".into(), "vulkan1.3".into()]);
                }
                args.extend([p(&path), "-o".into(), p(output_path)]);
                run(
                    "glslangValidator",
                    &args,
                    Some(&env),
                    None,
                    Some(&self.logs.join("driver-shaders.log")),
                    300,
                    0,
                )?;
            } else if extension == "spvasm" {
                run(
                    "spirv-as",
                    &[
                        "--target-env".into(),
                        "vulkan1.0".into(),
                        p(&path),
                        "-o".into(),
                        p(shaders.join(format!(
                            "{}.spv",
                            path.file_stem().unwrap().to_string_lossy()
                        ))),
                    ],
                    Some(&env),
                    None,
                    Some(&self.logs.join("driver-shaders.log")),
                    300,
                    0,
                )?;
            }
        }
        let mut receipt = json!({"schema":2,"driver_sha256":sha(&driver)?,"status":"TESTING","passed":["capabilities"],"failures":{}});
        let report = probes.join("driver-result.json");
        let result = (|| {
            for name in [
                "geometry",
                "robust_access",
                "device_address",
                "texel_buffer",
                "queries",
                "sparse",
            ] {
                let program = probes.join(name);
                self.compile_native(&here.join(format!("repro/{name}.c")), &program)?;
                let result = run(
                    &program,
                    &[p(&shaders)],
                    Some(&env),
                    None,
                    Some(&self.logs.join(format!("{name}.log"))),
                    180,
                    0,
                );
                match result {
                    Ok(()) => receipt["passed"].as_array_mut().unwrap().push(name.into()),
                    Err(error) => receipt["failures"][name] = format!("{error:#}").into(),
                };
                write_json(&report, &receipt)?;
            }
            ensure!(
                receipt["failures"].as_object().unwrap().is_empty(),
                "Native driver checks failed: {}",
                receipt["failures"]
            );
            Ok(())
        })();
        finish_probe(
            &mut receipt,
            &report,
            result,
            Ok(()),
            "NATIVE_GPU_CHECKS_PASSED",
        )
    }
    pub fn probe_steam(&self) -> Result<()> {
        use nucleon_core::{
            detector::{GraphicsApi, TargetEngine},
            launch::{GraphicsComponents, LaunchPlan},
            prefix_session::PrefixSession,
            runtime::ResolvedRuntime,
        };
        let probes = self.work.join("probes");
        fs::create_dir_all(&probes)?;
        let program = probes.join("steam-route-x64.exe");
        self.base.run(
            self.base.compiler.join("bin/x86_64-w64-mingw32-gcc"),
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
            "steam-route-build.log",
        )?;
        let root = self.prepare_runtime()?;
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
            set(
                &mut env,
                "NUCLEON_STEAM_API",
                crate::frontend::windows_path(&api)?,
            );
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
            set(
                &mut env,
                "NUCLEON_STEAM_RESULT",
                crate::frontend::windows_path(&positive)?,
            );
            run(
                &plan.runtime.loader,
                &[
                    p(steam.join("steam.exe")),
                    crate::frontend::windows_path(&program)?,
                ],
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
pub fn sign(path: &Path) -> Result<()> {
    run(
        "codesign",
        &[
            "--force".into(),
            "--sign".into(),
            "-".into(),
            "--timestamp=none".into(),
            p(path),
        ],
        None,
        None,
        None,
        60,
        0,
    )?;
    run(
        "codesign",
        &["--verify".into(), "--strict".into(), p(path)],
        None,
        None,
        None,
        60,
        0,
    )
}
pub fn shader_compiler() -> Result<PathBuf> {
    let path=std::env::var_os("NUCLEON_PROBE_D3DCOMPILER").map(PathBuf::from).unwrap_or_else(||nucleon_core::paths::home_dir().join("Library/Application Support/Steam/steamapps/common/DiRT Rally 2.0/d3dcompiler_47.dll"));
    ensure!(
        path.is_file(),
        "Set NUCLEON_PROBE_D3DCOMPILER to a licensed x64 d3dcompiler_47.dll"
    );
    Ok(path)
}
