#!/usr/bin/env python3
"""Isolated, source-built ARM64 Wine/FEX experiment; requires Python >= 3.12.

Only fetch performs network access. Build and probe never change Nucleon's
runtime selection or existing prefixes. Sources and products live under target.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import struct
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib
import zipfile

REPO = Path(__file__).resolve().parents[1]
RECIPE = REPO / "assets/wine/recipes/arm64-fex.toml"


def sha(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, delete=False) as stream:
        temporary = Path(stream.name)
        try:
            json.dump(value, stream, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        except BaseException:
            temporary.unlink(missing_ok=True)
            raise
    try:
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def atomic_copy(source, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=destination.parent, delete=False) as stream:
        temporary = Path(stream.name)
    try:
        shutil.copy2(source, temporary)
        with temporary.open('rb') as stream:
            os.fsync(stream.fileno())
        temporary.replace(destination)
    finally:
        temporary.unlink(missing_ok=True)


def environment(toolchain):
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("WINE", "DYLD_", "FEX_", "MACRUNNER_"))
           and k not in ("CC", "CXX", "CFLAGS", "CXXFLAGS", "CPPFLAGS", "LDFLAGS",
                         "CPATH", "LIBRARY_PATH", "SDKROOT", "PKG_CONFIG_PATH")}
    env["PATH"] = os.pathsep.join([str(toolchain / "bin"),
        "/opt/homebrew/opt/bison/bin", "/opt/homebrew/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"])
    env.update(CC="/usr/bin/clang", CXX="/usr/bin/clang++", LC_ALL="C", TZ="UTC")
    return env


def run(argv, *, cwd=None, env=None, log=None, timeout=None, expected=0):
    if not log:
        print(" ".join(map(str, argv[:3])), flush=True)
    stream = log.open("ab") if log else None
    try:
        process = subprocess.Popen(list(map(str, argv)), cwd=cwd, env=env,
            stdout=stream, stderr=subprocess.STDOUT if stream else None,
            start_new_session=True)
        try:
            code = process.wait(timeout=timeout)
        except BaseException:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=0.5)
            except subprocess.TimeoutExpired:
                pass
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except PermissionError:
                # An exited group can become unsignalable on macOS. Keep the
                # original failure if the owned leader has already exited.
                if process.poll() is None:
                    raise
            process.wait()
            raise
        if code != expected:
            raise RuntimeError(f"{argv[0]} exited {code}" + (f"; see {log}" if log else ""))
    finally:
        if stream:
            stream.close()


def output(argv, **kwargs):
    return subprocess.check_output(list(map(str, argv)), text=True, **kwargs).strip()


def source_diff(path):
    return hashlib.sha256(subprocess.check_output(
        ["git", "-C", str(path), "diff", "--binary", "HEAD"])).hexdigest()


def apply_wine_patches(source, names):
    for name in names:
        command = ["patch", "--batch", "-p1", "--input", str(REPO / name)]
        applicable = subprocess.run(command + ["--dry-run", "--forward"], cwd=source,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        run(command + (["--forward"] if applicable.returncode == 0 else ["--dry-run", "--reverse"]), cwd=source)
    if names and (source / ".git").exists():
        write_json(source / ".nucleon-overlay.json", dict(diff_sha256=source_diff(source),
                   patches={name: sha(REPO / name) for name in names}))


def pe_machine(path):
    with path.open("rb") as stream:
        if stream.read(2) != b"MZ":
            raise ValueError(f"Not a PE image: {path}")
        stream.seek(0x3c)
        offset = struct.unpack("<I", stream.read(4))[0]
        stream.seek(offset)
        if stream.read(4) != b"PE\0\0":
            raise ValueError(f"Invalid PE signature: {path}")
        return struct.unpack("<H", stream.read(2))[0]


def checkout(path, pin):
    if not re.fullmatch(r"[0-9a-f]{40}", pin["revision"]):
        raise ValueError("Source must pin a full Git revision")
    if not (path / ".git").exists():
        path.mkdir(parents=True, exist_ok=True)
        run(["git", "init", "-q", path])
        run(["git", "-C", path, "remote", "add", "origin", pin["url"]])
    if output(["git", "-C", path, "remote", "get-url", "origin"]) != pin["url"]:
        raise ValueError(f"Unexpected Git remote: {path}")
    if output(["git", "-C", path, "status", "--porcelain"]):
        marker = path / ".nucleon-overlay.json"
        untracked = set(output(["git", "-C", path, "ls-files", "--others", "--exclude-standard"]).splitlines())
        if not (marker.exists() and untracked <= {marker.name}
                and output(["git", "-C", path, "rev-parse", "HEAD"]) == pin["revision"]
                and source_diff(path) == json.loads(marker.read_text())["diff_sha256"]):
            raise ValueError(f"Preserve modified source checkout: {path}")
        return
    found = subprocess.run(["git", "-C", str(path), "cat-file", "-e", pin["revision"]],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0
    if not found:
        run(["git", "-C", path, "fetch", "--depth", "1", "origin", pin["revision"]])
    run(["git", "-C", path, "checkout", "--detach", pin["revision"]])


class Experiment:
    def __init__(self, work, jobs, variant="macrunner"):
        self.variant, self.jobs = variant, jobs
        self.workspace = work
        self.work = work if variant == "macrunner" else work / variant
        self.work.mkdir(parents=True, exist_ok=True)
        self.recipe = tomllib.loads(RECIPE.read_text())
        if self.recipe["schema"] != 1:
            raise ValueError("Unsupported recipe schema")
        self.sources = work / "sources"
        self.hb = self.sources / "hyperbridge"
        self.wine = self.sources / "wine"
        self.fex = work / "fex/src"
        self.compiler = work / "toolchain" / self.recipe["toolchain"]["directory"]
        self.source_pins = self.recipe["sources"]
        if variant == "tolkara":
            self.wine = self.sources / "tolkara-wine"
            self.fex = self.sources / "tolkara-fex"
            self.source_pins = {f"tolkara-{name}": pin
                               for name, pin in self.recipe["alternatives"][variant].items()}
            self.source_pins["wine"] = self.recipe["sources"]["wine"]
        self.env = environment(self.compiler)
        self.logs = self.work / "logs"
        self.logs.mkdir(parents=True, exist_ok=True)

    def doctor(self):
        checks = {name: shutil.which(name, path=self.env["PATH"])
                  for name in ["git", "clang", "make", "cmake", "ninja", "bison", "flex",
                               "arm64ec-w64-mingw32-gcc", "x86_64-w64-mingw32-gcc"]}
        checks["clang"] = "/usr/bin/clang" if Path("/usr/bin/clang").exists() else None
        print(json.dumps(dict(host=platform.machine(), system=platform.system(), tools=checks,
                              work=str(self.work)), indent=2))
        if platform.system() != "Darwin" or platform.machine() != "arm64":
            raise ValueError("Run natively on Apple Silicon macOS")
        return checks

    def fetch(self):
        for name, pin in self.source_pins.items():
            checkout(self.sources / name, pin)
        if self.variant == "tolkara":
            for sub in ["External/rpmalloc", "External/fmt", "External/xxhash",
                        "External/range-v3", "External/unordered_dense", "Source/Common/cpp-optparse"]:
                run(["git", "-C", self.fex, "submodule", "update", "--init", "--depth", "1", "--", sub])
        elif not (self.fex / ".nucleon-patches.json").exists():
            checkout(self.fex, self.recipe["fex"])
            manifest = json.loads((self.hb / "fex/MANIFEST.json").read_text())
            if manifest["upstream"]["commit"] != self.recipe["fex"]["revision"]:
                raise ValueError("FEX base and HyperBridge manifest differ")
            for sub, revision in manifest["upstream"]["submodules"].items():
                run(["git", "-C", self.fex, "submodule", "update", "--init", "--depth", "1", "--", sub])
                if output(["git", "-C", self.fex / sub, "rev-parse", "HEAD"]) != revision:
                    raise ValueError(f"FEX submodule revision differs: {sub}")
        self.fetch_toolchain()

    def fetch_toolchain(self):
        archive = self.fetch_archive(self.recipe["toolchain"])
        if not self.compiler.exists():
            with tarfile.open(archive) as package:
                package.extractall(self.workspace / "toolchain", filter="data")

    def fetch_archive(self, pin):
        archive = self.workspace / pin["archive"]
        if not archive.exists():
            with tempfile.NamedTemporaryFile(dir=self.workspace, delete=False) as stream:
                temporary = Path(stream.name)
            try:
                run(["curl", "--fail", "--location", "--retry", "2", "--output", temporary, pin["url"]])
                if sha(temporary) != pin["sha256"]:
                    raise ValueError("Archive checksum mismatch: " + pin["archive"])
                temporary.replace(archive)
            finally:
                temporary.unlink(missing_ok=True)
        if sha(archive) != pin["sha256"]:
            raise ValueError("Archive checksum mismatch: " + pin["archive"])
        return archive

    def prepare_fex(self):
        order = self.hb / self.recipe["fex"]["patch_order"]
        patches = [self.hb / "fex/patches" / name for name in order.read_text().splitlines()
                   if name and not name.startswith("#")]
        receipt = {"base": self.recipe["fex"]["revision"],
                   "patches": {p.name: sha(p) for p in patches}}
        marker = self.fex / ".nucleon-patches.json"
        if marker.exists():
            previous = json.loads(marker.read_text())
            if {k: previous[k] for k in receipt} != receipt:
                raise ValueError("FEX patch recipe changed; choose a fresh work directory")
            diff = subprocess.check_output(["git", "-C", str(self.fex), "diff", "--binary", "HEAD"])
            if hashlib.sha256(diff).hexdigest() != previous["diff_sha256"]:
                raise ValueError("Patched FEX source changed")
            return
        if output(["git", "-C", self.fex, "rev-parse", "HEAD"]) != receipt["base"]:
            raise ValueError("FEX checkout does not match the pinned base")
        if output(["git", "-C", self.fex, "status", "--porcelain"]):
            raise ValueError("FEX source is modified; preserve it and use a fresh work directory")
        for patch in patches:
            run(["git", "-C", self.fex, "apply", "--index", patch], log=self.logs / "fex-patches.log")
        diff = subprocess.check_output(["git", "-C", str(self.fex), "diff", "--binary", "HEAD"])
        receipt["diff_sha256"] = hashlib.sha256(diff).hexdigest()
        write_json(marker, receipt)

    def build_fex(self):
        if self.variant == "tolkara":
            return self.build_tolkara_fex()
        self.prepare_fex()
        build = self.work / "fex/arm64ec"
        argv = ["cmake", "-S", self.fex, "-B", build, "-G", "Ninja",
            "-DCMAKE_BUILD_TYPE=Release", f"-DCMAKE_TOOLCHAIN_FILE={self.fex}/Data/CMake/toolchain_mingw.cmake",
            "-DMINGW_TRIPLE=arm64ec-w64-mingw32", "-DENABLE_CCACHE=OFF", "-DTUNE_CPU=none",
            "-DENABLE_LTO=OFF", "-DENABLE_JEMALLOC_GLIBC_ALLOC=OFF", "-DRANGES_NATIVE=OFF",
            "-DBUILD_TESTING=OFF", "-DFEX_WINE_DARWIN=ON", "-DCMAKE_INSTALL_PREFIX=/nucleon/fex",
            "-DCMAKE_INSTALL_LIBDIR=aarch64-windows", "-DCMAKE_SHARED_LINKER_FLAGS=-static -static-libgcc -static-libstdc++"]
        run(argv, env=self.env, log=self.logs / "fex-configure.log")
        run(["cmake", "--build", build, "--target", "arm64ecfex", "--parallel", self.jobs],
            env=self.env, log=self.logs / "fex-build.log")
        unix = self.work / "fex/unixlib"
        run(["cmake", "-S", self.fex / "Source/Windows/UnixLib", "-B", unix, "-G", "Ninja",
             "-DCMAKE_BUILD_TYPE=Release", "-DCMAKE_OSX_ARCHITECTURES=arm64"],
            env=self.env, log=self.logs / "fex-unix-configure.log")
        run(["cmake", "--build", unix, "--parallel", self.jobs], env=self.env,
            log=self.logs / "fex-unix-build.log")
        # ARM64EC images may carry an AMD64 COFF header for x64 loader compatibility.
        # LLVM also checks the CHPE metadata and code maps, unlike a machine-only check.
        image = build / "Bin/xtajit64.dll"
        headers = output([self.compiler / "bin/llvm-readobj", "--file-headers", image])
        if pe_machine(image) not in (0xa641, 0x8664) or "Format: COFF-ARM64EC" not in headers:
            raise ValueError("FEX emulator must be ARM64EC PE")
        if output(["lipo", "-archs", unix / "xtajit64.so"]) != "arm64":
            raise ValueError("FEX Unix companion must be native ARM64")
        overlay = self.work / "fex/runtime"
        products = {build / "Bin/xtajit64.dll": "aarch64-windows/xtajit64.dll",
            unix / "xtajit64.so": "aarch64-unix/libarm64ecfex.so",
            unix / "libmacrunner-hwtso.dylib": "aarch64-unix/libmacrunner-hwtso.dylib"}
        files = {}
        for source, relative in products.items():
            target = overlay / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)
            files[relative] = sha(target)
        write_json(overlay / "build.json", dict(status="BUILT_NOT_EXECUTION_VALIDATED", files=files,
            recipe_sha256=sha(RECIPE), hyperbridge=self.recipe["sources"]["hyperbridge"]["revision"],
            fex=self.recipe["fex"]["revision"], toolchain_sha256=self.recipe["toolchain"]["sha256"]))

    def build_wine(self):
        if self.variant == "tolkara":
            return self.build_tolkara_wine()
        build = self.work / "wine-build"
        if not (build / "source-wine").exists():
            run([sys.executable, self.wine / "build-recipes/stage-source.py", build])
        apply_wine_patches(build / "source-wine", self.recipe["wine"].get("patches", []))
        env = self.env.copy()
        env.update(CFLAGS="-O2 -arch arm64", CXXFLAGS="-O2 -arch arm64", LDFLAGS="-arch arm64",
                   PKG_CONFIG="/usr/bin/false", PKG_CONFIG_LIBDIR="/nonexistent")
        for arch, triple in [("aarch64", "aarch64"), ("arm64ec", "arm64ec"),
                             ("x86_64", "x86_64"), ("i386", "i686")]:
            env[f"{arch}_CC"] = str(self.compiler / "bin" / f"{triple}-w64-mingw32-gcc")
            env[f"{arch}_CXX"] = str(self.compiler / "bin" / f"{triple}-w64-mingw32-g++")
        run(["make", "-f", self.wine / "third_party/hyperbridge/Makefile",
             f"SRC_ROOT={build}/hyperbridge", f"BUILD_ROOT={build}/hyperbridge", f"-j{self.jobs}"],
            env=env, log=self.logs / "wine-hyperbridge.log")
        if not (build / "Makefile").exists():
            run([build / "source-wine/configure", "--prefix=/", "--bindir=/bin", "--libdir=/lib",
                 "--enable-archs=" + ",".join(self.recipe["wine"]["architectures"]),
                 *self.recipe["wine"]["configure"]], cwd=build, env=env,
                log=self.logs / "wine-configure.log")
        run(["make", f"-j{self.jobs}"], cwd=build, env=env, log=self.logs / "wine-build.log")
        run(["make", "install", f"DESTDIR={self.work}/runtime"], cwd=build, env=env,
            log=self.logs / "wine-install.log")
        self.record_wine_build(self.recipe["wine"].get("patches", []))

    def build_tolkara_fex(self):
        for triple, target, directory in [("arm64ec", "arm64ecfex", "arm64ec"),
                                          ("aarch64", "wow64fex", "wow64")]:
            build = self.work / "fex" / directory
            run(["cmake", "-S", self.fex, "-B", build, "-G", "Ninja",
                 f"-DCMAKE_TOOLCHAIN_FILE={self.fex}/Data/CMake/toolchain_mingw.cmake",
                 f"-DMINGW_TRIPLE={triple}-w64-mingw32", "-DCMAKE_BUILD_TYPE=Release",
                 "-DENABLE_LTO=OFF", "-DENABLE_JEMALLOC_GLIBC_ALLOC=OFF", "-DBUILD_TESTING=OFF",
                 "-DENABLE_CCACHE=OFF", "-DTUNE_CPU=none", "-DFEX_TEB_TSD_OFFSET=0x17f8",
                 "-DFEX_GUEST_ADDRESS_WINDOW=ON", "-DCMAKE_SHARED_LINKER_FLAGS=-Wl,--section-alignment=0x10000"],
                env=self.env, log=self.logs / f"fex-{directory}-configure.log")
            run(["cmake", "--build", build, "--target", target, "--parallel", self.jobs],
                env=self.env, log=self.logs / f"fex-{directory}-build.log")
        self.build_tolkara_unixlib()

    def build_tolkara_unixlib(self):
        pin = self.recipe['alternatives']['tolkara']['fex']
        checkout(self.fex, pin)
        apply_wine_patches(self.fex, pin['unix_patches'])
        source = self.fex / 'Source/Windows/UnixLib'
        build = self.work / 'fex/unixlib'
        build.mkdir(parents=True, exist_ok=True)
        identity = dict(revision=pin['revision'], source=sha(source / 'FEXUnixLib.cpp'),
                        header=sha(source / 'FEXUnixLib.h'),
                        compiler=output(['/usr/bin/clang++', '--version']),
                        patches={name:sha(REPO / name) for name in pin['unix_patches']})
        receipt = build / 'build.json'
        previous = json.loads(receipt.read_text()) if receipt.is_file() else {}
        names = ['libarm64ecfex.so', 'libwow64fex.so']
        if previous.get('inputs') == identity and all((build / name).is_file()
                and sha(build / name) == previous.get('files', {}).get(name) for name in names):
            return build
        for name in names:
            run(['/usr/bin/clang++', '-std=c++17', '-O2', '-arch', 'arm64', '-dynamiclib',
                 '-Wall', '-Wextra', '-Werror', '-Wno-unused-parameter',
                 '-Wl,-install_name,@rpath/' + name, source / 'FEXUnixLib.cpp', '-o', build / name],
                log=self.logs / 'fex-unix-build.log')
            run(['/usr/bin/codesign', '--force', '--sign', '-', build / name],
                log=self.logs / 'fex-unix-build.log')
        write_json(receipt, dict(inputs=identity,files={name:sha(build / name) for name in names}))
        return build

    def build_tolkara_wine(self):
        settings = self.recipe["wine"]["tolkara"]
        relative = settings["shared_data_helper"]
        helper = self.sources / "wine" / relative
        if sha(helper) != settings["shared_data_helper_sha256"]:
            raise ValueError("Pinned shared-data helper checksum mismatch")
        shutil.copy2(helper, self.wine / relative)
        run(["git", "-C", self.wine, "add", "--intent-to-add", relative])
        apply_wine_patches(self.wine, self.recipe["wine"]["tolkara"]["patches"])
        build = self.work / "wine-build"
        build.mkdir(exist_ok=True)
        env = self.env.copy()
        env.update(CFLAGS="-O2 -arch arm64", CXXFLAGS="-O2 -arch arm64", LDFLAGS="-arch arm64",
                   PKG_CONFIG="/usr/bin/false", PKG_CONFIG_LIBDIR="/nonexistent")
        if not (build / "Makefile").exists():
            run([self.wine / "configure", "--prefix=/", "--bindir=/bin", "--libdir=/lib",
                 "--enable-archs=arm64ec,aarch64,i386", f"--with-mingw={self.compiler}/bin/clang",
                 *self.recipe["wine"]["configure"], "enable_amd_ags_x64=no"],
                cwd=build, env=env, log=self.logs / "wine-configure.log")
        run(["make", f"-j{self.jobs}"], cwd=build, env=env, log=self.logs / "wine-build.log")
        run(["make", "install", f"DESTDIR={self.work}/runtime"], cwd=build, env=env,
            log=self.logs / "wine-install.log")
        self.record_wine_build(settings["patches"])

    def record_wine_build(self, patches):
        runtime = self.work / "runtime"
        provenance = runtime / "share/nucleon-build"
        provenance.mkdir(parents=True, exist_ok=True)
        shutil.copy2(RECIPE, provenance / RECIPE.name)
        for name in patches:
            shutil.copy2(REPO / name, provenance / Path(name).name)
        licenses = runtime / "share/licenses/wine"
        licenses.mkdir(parents=True, exist_ok=True)
        for name in ["COPYING.LIB", "LICENSE", "AUTHORS"]:
            if (self.wine / name).exists():
                shutil.copy2(self.wine / name, licenses / name)
        if self.variant == "tolkara":
            relative = self.recipe["wine"]["tolkara"]["shared_data_helper"]
            shutil.copy2(self.wine / relative, provenance / Path(relative).name)
        else:
            shutil.copy2(self.wine / "third_party/hyperbridge/LICENSE", licenses / "HyperBridge-LICENSE")
        for path in self.fex.rglob("*"):
            if (path.is_file() and not path.is_symlink() and ".git" not in path.parts
                    and re.match(r"(?i)^(license|copying|copyright|notice)([._-]|$)", path.name)):
                target = runtime / "share/licenses/fex" / path.relative_to(self.fex)
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(path, target)
        names = ["bin/wine", "bin/wineserver", "lib/wine/aarch64-unix/ntdll.so",
                 "lib/wine/aarch64-unix/winemac.so"]
        write_json(runtime / "build.json", dict(status="BUILT_NOT_EXECUTION_VALIDATED",
            variant=self.variant, recipe_sha256=sha(RECIPE), sources=self.source_pins,
            patches={name: sha(REPO / name) for name in patches},
            toolchain_sha256=self.recipe["toolchain"]["sha256"],
            sdk=output(["xcrun", "--show-sdk-version"]),
            native_compiler=output(["/usr/bin/clang", "--version"]).splitlines()[0],
            host_files={name: sha(runtime / name) for name in names},
            features="SOURCE_BUILT_WINE_SEPARATE_CPU_AND_GRAPHICS_TEST_RECEIPTS"))

    def runtime_environment(self, prefix):
        runtime = self.work / "runtime"
        loader = runtime / "bin/wineloader"
        if not loader.exists():
            loader = runtime / "bin/wine"
        if not loader.exists():
            raise ValueError("Wine has not finished building")
        if output(["lipo", "-archs", loader]) != "arm64":
            raise ValueError("Wine loader must be native ARM64")
        env = self.env.copy()
        overlay = self.work / "fex/runtime"
        (self.work / "prefixes").mkdir(exist_ok=True)
        env.update(WINEPREFIX=str(self.work / "prefixes" / prefix),
            WINELOADER=str(loader), WINESERVER=str(runtime / "bin/wineserver"),
            WINEDLLPATH=f"{overlay}:{runtime}/lib/wine/aarch64-windows:{runtime}/lib/wine/x86_64-windows",
            DYLD_LIBRARY_PATH=f"{runtime}/lib/wine/aarch64-unix",
            MACRUNNER_CPU_BACKEND="fex", MACRUNNER_HB_BACKEND="fex",
            MACRUNNER_FEX_OVERLAY=str(overlay), MACRUNNER_FEX_HW_TSO="0", FEX_TSOENABLED="1",
            MACRUNNER_HB_X64_LOADER="0", MACRUNNER_HB_MAPSCAN_SKIP="1",
            WINEDLLOVERRIDES="winedbg.exe=d;winemenubuilder.exe=d;mscoree=d;mshtml=d", WINEDEBUG="+loaddll")
        if self.variant == "tolkara":
            unix = self.build_tolkara_unixlib()
            for name in ['libarm64ecfex.so', 'libwow64fex.so']:
                atomic_copy(unix / name, runtime / 'lib/wine/aarch64-unix' / name)
            for directory, name in [("arm64ec", "libarm64ecfex.dll"), ("wow64", "libwow64fex.dll")]:
                source = self.work / "fex" / directory / "Bin" / name
                target = runtime / "lib/wine/aarch64-windows" / name
                if not target.exists() or sha(source) != sha(target):
                    atomic_copy(source, target)
            env = {k: v for k, v in env.items() if not k.startswith("MACRUNNER_")}
            env["WINEDLLPATH"] = str(runtime / "lib/wine/aarch64-windows")
            env["NUCLEON_EMULATE_KUSD"] = "1"
        return loader, env

    def stop_server(self, env):
        server = self.work / "runtime/bin/wineserver"
        if server.exists():
            # Prefix-scoped only, including after a timeout.
            try:
                run([server, "-k"], env=env, log=self.logs / "smoke-cleanup.log", timeout=10)
            except (RuntimeError, subprocess.SubprocessError):
                pass

    def probe(self, window=False):
        probes = self.work / "probes"
        probes.mkdir(exist_ok=True)
        run(["/usr/bin/clang", "-O2", "-arch", "arm64", REPO / "tools/runtime-probes/host.c",
             "-o", probes / "host"], env=self.env)
        host = json.loads(output([probes / "host"], env=self.env))
        print(json.dumps(host, indent=2))
        run([self.compiler / "bin/x86_64-w64-mingw32-gcc", "-O2", "-municode", "-fms-extensions",
             REPO / "tools/runtime-probes/windows.c", "-o", probes / "smoke-x64.exe"], env=self.env)
        if pe_machine(probes / "smoke-x64.exe") != 0x8664:
            raise ValueError("Smoke workload must contain AMD64 Windows code")
        receipt = dict(host=host, workload_sha256=sha(probes / "smoke-x64.exe"),
                       status="NOT_EXECUTED", variant=self.variant, steam_launched=False)
        report = probes / "result.json"
        write_json(report, receipt)
        loader, env = self.runtime_environment("cpu-smoke")
        for name in ["wine-version.log", "cpu-smoke.log"]:
            (self.logs / name).write_bytes(b"")
        started = time.monotonic()
        try:
            run([loader, "--version"], env=env, log=self.logs / "wine-version.log", timeout=20)
            arguments = ["--shared-data"] if self.variant == "tolkara" else []
            run([loader, probes / "smoke-x64.exe", *arguments], env=env, log=self.logs / "cpu-smoke.log", timeout=120)
            if "NUCLEON_FEX_SMOKE_OK" not in (self.logs / "cpu-smoke.log").read_text(errors="replace"):
                raise ValueError("Wine exited without a successful CPU workload result")
            receipt["status"] = "X64_CPU_SMOKE_PASSED"
            if self.variant == "tolkara":
                control = dict(env, NUCLEON_EMULATE_KUSD="0")
                control_log = self.logs / "shared-data-negative.log"
                control_log.write_bytes(b"")
                run([loader, probes / "smoke-x64.exe", "--shared-data"], env=control,
                    log=control_log, timeout=30, expected=1)
                if "NUCLEON_FEX_FAIL stage=shared-data-read" not in control_log.read_text(errors="replace"):
                    raise ValueError("Shared-data negative control failed for an unexpected reason")
                receipt["shared_data_negative_control"] = "EXPECTED_READ_FAILURE"
            if window:
                run([self.compiler / "bin/x86_64-w64-mingw32-gcc", "-O2", "-municode",
                     REPO / "tools/runtime-probes/window.c", "-o", probes / "window-x64.exe",
                     "-luser32", "-lgdi32"], env=self.env)
                (self.logs / "window-smoke.log").write_bytes(b"")
                run([loader, probes / "window-x64.exe"], env=env,
                    log=self.logs / "window-smoke.log", timeout=30)
                if "NUCLEON_FEX_WINDOW_OK" not in (self.logs / "window-smoke.log").read_text(errors="replace"):
                    raise ValueError("Window test exited without a successful result")
                receipt["status"] = "X64_CPU_AND_WINDOW_SMOKE_PASSED"
        except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
            receipt.update(status="EXECUTION_FAILED", error=str(error))
            raise
        finally:
            receipt["elapsed_seconds"] = round(time.monotonic() - started, 3)
            log = self.logs / "cpu-smoke.log"
            if log.exists():
                receipt["passed_checks"] = re.findall(r"^NUCLEON_FEX_PASS ([^\r\n]+)",
                    log.read_text(errors="replace"), re.MULTILINE)
            write_json(report, receipt)
            self.stop_server(env)

    def probe_dxmt(self):
        if self.variant != "tolkara":
            raise ValueError("The graphics experiment currently requires --variant tolkara")
        pin = self.recipe["reference"]
        archive = self.workspace / pin["archive"]
        if sha(archive) != pin["sha256"]:
            raise ValueError("Reference archive checksum mismatch")
        stage = self.work / "dxmt"
        original = {}
        with zipfile.ZipFile(archive) as package:
            root = "MacRunner.app/Contents/Resources/engine/"
            inventory = json.loads(package.read(root + "ENGINE.json"))["files"]
            names = [f"x86_64-windows/{name}.dll" for name in ["d3d11", "dxgi", "d3d10core", "winemetal"]]
            for name in names + ["aarch64-unix/winemetal.so"]:
                data = package.read(root + "graphics/" + name)
                if hashlib.sha256(data).hexdigest() != inventory["graphics/" + name]:
                    raise ValueError("DXMT component checksum mismatch: " + name)
                target = stage / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(data)
                original[name] = sha(target)
        native = stage / "aarch64-unix/winemetal.so"
        if output(["lipo", "-archs", native]) != "arm64":
            raise ValueError("DXMT companion must be ARM64")
        # This private, unprivileged graphics library needs a valid local code
        # signature. The reference bundle and its provisioned loader are untouched.
        run(["codesign", "--force", "--sign", "-", "--timestamp=none", native])
        run(["codesign", "--verify", "--strict", native])
        probes = self.work / "probes"
        probes.mkdir(exist_ok=True)
        run([self.compiler / "bin/x86_64-w64-mingw32-gcc", "-O2", "-municode",
             REPO / "tools/runtime-probes/d3d11.c", "-o", probes / "d3d11-x64.exe",
             "-ld3d11", "-ldxgi", "-ld3dcompiler"], env=self.env)
        loader, env = self.runtime_environment("dxmt-smoke")
        receipt = dict(status="NOT_EXECUTED", steam_launched=False, source_archive_sha256=pin["sha256"],
                       dxmt_source_files=original, native_staged_sha256=sha(native),
                       graphics="REFERENCE_X64_PE_FRONTEND_ARM64_UNIX_COMPANION")
        log = self.logs / "d3d11-smoke.log"
        log.write_bytes(b"")
        try:
            # Complete any Wine-version update before installing native overrides.
            # --init also avoids processing user startup/run entries.
            run([loader, "wineboot", "-i"], env=env, log=self.logs / "dxmt-prefix.log", timeout=120)
            system = Path(env["WINEPREFIX"]) / "drive_c/windows/system32"
            for name in names:
                with tempfile.NamedTemporaryFile(dir=system, delete=False) as stream:
                    temporary = Path(stream.name)
                try:
                    shutil.copy2(stage / name, temporary)
                    temporary.replace(system / Path(name).name)
                finally:
                    temporary.unlink(missing_ok=True)
            env["WINEDLLPATH"] = str(stage) + ":" + env["WINEDLLPATH"]
            env["DYLD_LIBRARY_PATH"] = str(native.parent) + ":" + env["DYLD_LIBRARY_PATH"]
            env["WINEDLLOVERRIDES"] += ";d3d11,dxgi,d3d10core,winemetal=n,b"
            env["WINEDEBUG"] = "+loaddll,+module,+seh"
            (probes / "triangle.ppm").unlink(missing_ok=True)
            run([loader, probes / "d3d11-x64.exe"], env=env, cwd=probes, log=log, timeout=60)
            if "NUCLEON_D3D11_TRIANGLE_OK" not in log.read_text(errors="replace"):
                raise ValueError("No successful triangle readback result")
            control_log = self.logs / "d3d11-negative.log"
            control_log.write_bytes(b"")
            run([loader, probes / "d3d11-x64.exe", "--no-draw"], env=env, cwd=probes,
                log=control_log, timeout=60, expected=21)
            if "NUCLEON_D3D11_FAIL triangle-stage=21" not in control_log.read_text(errors="replace"):
                raise ValueError("GPU negative control failed for an unexpected reason")
            receipt.update(negative_control="EXPECTED_PIXEL_MISMATCH_WITHOUT_DRAW",
                           triangle_sha256=sha(probes / "triangle.ppm"))
            receipt["status"] = "D3D11_GPU_COPY_AND_TRIANGLE_PASSED"
        except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
            receipt.update(status="FAILED", error=str(error))
            raise
        finally:
            write_json(probes / "d3d11-result.json", receipt)
            self.stop_server(env)

    def check_reference(self):
        pin = self.recipe["reference"]
        archive = self.work / pin["archive"]
        if sha(archive) != pin["sha256"]:
            raise ValueError("Reference archive checksum mismatch")
        loader = self.work / "reference/MacRunner.app/Contents/Resources/wine-loader/MacRunner Wine Loader.app"
        result = subprocess.run(["codesign", "--verify", "--strict", "--verbose=2", str(loader)],
                                text=True, capture_output=True)
        receipt = dict(archive_sha256=sha(archive), signature_valid=result.returncode == 0,
                       diagnostics=result.stderr, executed=False)
        write_json(self.work / "probes/reference-signature.json", receipt)
        print(json.dumps(receipt, indent=2))
        if result.returncode:
            raise ValueError("Reference launcher signature verification failed; execution skipped")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["doctor", "fetch", "build-wine", "build-fex", "probe", "probe-dxmt", "check-reference"])
    parser.add_argument("--work", type=Path, default=REPO / "target/arm64-fex")
    parser.add_argument("--jobs", type=int, default=min(os.cpu_count() or 2, 6))
    parser.add_argument("--variant", choices=["macrunner", "tolkara"], default="macrunner")
    parser.add_argument("--window", action="store_true", help="Also create and close a one-second test window")
    parser.add_argument("--reference", action="store_true", help="Also fetch the pinned MacRunner reference archive")
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error("--jobs must be positive")
    experiment = Experiment(args.work.resolve(), args.jobs, args.variant)
    if args.action == "doctor":
        experiment.doctor()
    elif args.action == "fetch":
        experiment.fetch()
        if args.reference:
            experiment.fetch_archive(experiment.recipe["reference"])
    elif args.action == "check-reference":
        experiment.check_reference()
    else:
        missing = [k for k, v in experiment.doctor().items() if not v]
        if missing:
            raise ValueError("Missing tools: " + ", ".join(missing))
        for name, pin in experiment.source_pins.items():
            if output(["git", "-C", experiment.sources / name, "rev-parse", "HEAD"]) != pin["revision"]:
                raise ValueError(f"Source revision differs: {name}; run fetch")
        if args.action == "build-wine":
            experiment.build_wine()
        elif args.action == "build-fex":
            experiment.build_fex()
        elif args.action == "probe-dxmt":
            experiment.probe_dxmt()
        else:
            experiment.probe(window=args.window)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"ARM64/FEX experiment: {error}", file=sys.stderr)
        sys.exit(1)
