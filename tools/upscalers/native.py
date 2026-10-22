"""Shared pinned-source preparation and synthetic native Metal validation. Never runs Wine."""
from __future__ import annotations
import argparse
import concurrent.futures
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import tempfile
import urllib.request

LOCK = json.loads(Path(__file__).with_name("dependencies.json").read_text())

def require(condition, message):
    if not condition:
        raise RuntimeError(message)

def sha256(path):
    with Path(path).open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()

def atomic_write(path, data, mode=0o644):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".nucleon-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
            os.fchmod(f.fileno(), mode)
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)

def save_json(path, value):
    atomic_write(path, (json.dumps(value, indent=2) + "\n").encode())

def request(url):
    return urllib.request.urlopen(urllib.request.Request(
        url, headers={"User-Agent": "Nucleon-upscaler-setup"}), timeout=60)

def download(url, destination, expected):
    destination = Path(destination)
    if destination.is_file() and sha256(destination) == expected:
        return destination
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_suffix(destination.suffix + ".part")
    with request(url) as response, temporary.open("wb") as output:
        shutil.copyfileobj(response, output)
    require(sha256(temporary) == expected, f"Checksum mismatch: {destination.name}")
    os.replace(temporary, destination)
    return destination

def source_path_needed(name):
    # Keep the original model sources/licenses, but avoid unused shipped compilers.
    return (name.startswith(("src/", "hook/", "patches/", "third_party/"))
            or (name.startswith("tools/") and not name.startswith(
                ("tools/bin/", "tools/lib/", "tools/installer/")))
            or name in {"Makefile", "README.md", "LICENSE", "NOTICE.md", "BENCHMARK.md"})

def fetch_source(cache):
    spec = LOCK["fsr4"]
    root = cache / "sources" / ("fsr4-" + spec["commit"])
    root.mkdir(parents=True, exist_ok=True)
    tree_file = cache / "fsr4-pinned-tree.json"
    if tree_file.is_file():
        tree = json.loads(tree_file.read_text())
    else:
        with request(f"https://api.github.com/repos/{spec['repository']}/git/trees/{spec['commit']}?recursive=1") as r:
            tree = json.load(r)
        save_json(tree_file, tree)
    require(tree.get("sha") == spec["tree"] and not tree.get("truncated"), "Invalid source tree identity")
    files = [x for x in tree["tree"] if x["type"] == "blob" and source_path_needed(x["path"])]

    def get(entry):
        name = entry["path"]
        require(not PurePosixPath(name).is_absolute() and ".." not in PurePosixPath(name).parts, "Unsafe source path")
        path = root / name
        def matches(data):
            return hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest() == entry["sha"]
        if path.is_file() and matches(path.read_bytes()):
            return
        with request(f"https://raw.githubusercontent.com/{spec['repository']}/{spec['commit']}/{name}") as r:
            data = r.read()
        require(matches(data), f"Source hash mismatch: {name}")
        atomic_write(path, data)

    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        list(pool.map(get, files))
    save_json(root / ".nucleon-source.json", {"commit": spec["commit"], "files": {x["path"]: x["sha"] for x in files}})
    return root

def verify_source(root):
    record = json.loads((root / ".nucleon-source.json").read_text())
    require(record["commit"] == LOCK["fsr4"]["commit"], "FSR4 source revision differs from dependency lock")
    for name, expected in record["files"].items():
        path = root / name
        require(not Path(name).is_absolute() and ".." not in Path(name).parts, "Unsafe recorded source path")
        data = path.read_bytes()
        actual = hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()
        require(actual == expected, f"FSR4 source changed: {name}")

def build_fsr(source, wine):
    """Upstream build + synthetic GPU tests only. No Wine or game command exists here."""
    verify_source(source)
    (source / "tools/bin").mkdir(parents=True, exist_ok=True)
    # The native graph still translates its PRE/POST HLSL with these tools.
    # Use installed compilers instead of downloading upstream's large binaries.
    for name in ("dxc", "spirv-cross", "spirv-val"):
        tool = shutil.which(name)
        require(tool is not None, f"Missing shader compiler on PATH: {name}")
        link = source / "tools/bin" / name
        if link.is_file() and not link.is_symlink():
            require(sha256(link) == sha256(tool), f"Unexpected existing compiler: {link}")
            link.unlink()
        if not link.exists():
            link.symlink_to(Path(tool).resolve())
    spec = importlib.util.spec_from_file_location("nucleon_fsr4_builder", source / "tools/crossover/crossover_fsr4.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    overlay = source / "nucleon"
    overlay.mkdir(exist_ok=True)
    hook = (source / "hook/fsr4_metalfx.m").read_text()
    replacements = {
        '#import "fsr4_live.h"': '#import "fsr4_live.h"\n#import "fsr4_depth_compat.h"',
        '    f->depthReversed = s.depthReversed ? 1 : 0;':
            '    const int capturedDepth = nucleon_depth_reversed(scaler);\n'
            '    if (capturedDepth < 0) { fsr4_live_set_inactive("depth direction unavailable"); return 0; }\n'
            '    f->depthReversed = capturedDepth;',
        '    if (!cls || orig_encode(cls)) return;':
            '    if (!cls || orig_encode(cls)) return;\n    nucleon_capture_depth_setter(cls);',
    }
    for before, after in replacements.items():
        require(hook.count(before) == 1, "Pinned FSR4 hook no longer matches the compatibility adapter")
        hook = hook.replace(before, after)
    atomic_write(overlay / "fsr4_metalfx.m", hook.encode())
    for name in ("fsr4_depth_compat.h", "test_depth_compat.m"):
        atomic_write(overlay / name, Path(__file__).with_name(name).read_bytes())
    module.HOOK_SRC = overlay / "fsr4_metalfx.m"
    # Keep each game's settings/status private even when immutable shaders and
    # the patched framework are shared. Launch a separate panel instance per game.
    panel = (source / "src/fsr4_panel.mm").read_text()
    old = '    const char *root = getenv("FSR4_ROOT");'
    require(panel.count(old) == 1, "Pinned panel publisher changed")
    panel = panel.replace(old, '    const char *state = getenv("FSR4_STATE_DIR");\n'
                          '    if (state && state[0]) return std::string(state);\n' + old)
    old = '    const std::string app = directory + "/FSR4 Panel.app";'
    require(panel.count(old) == 1, "Pinned panel launch path changed")
    panel = panel.replace(old, '    const std::string app = std::string(getenv("FSR4_ROOT")) + "/dist/crossover/FSR4 Panel.app";')
    panel = panel.replace('{"/usr/bin/open", "-g", "-a",', '{"/usr/bin/open", "-g", "-n", "-a",')
    atomic_write(overlay / "fsr4_panel.mm", panel.encode())
    module.PANEL_SRC = overlay / "fsr4_panel.mm"
    app = (source / "tools/crossover/panel_app.mm").read_text()
    old = 'static std::string dist_path(const char *name) { return g_root + "/dist/crossover/" + name; }'
    require(app.count(old) == 1, "Pinned panel app path changed")
    app = app.replace(old, 'static std::string dist_path(const char *name) {\n'
                      '    const char *state = getenv("FSR4_STATE_DIR");\n'
                      '    return state && state[0] ? std::string(state) + "/" + name : g_root + "/dist/crossover/" + name;\n}')
    atomic_write(overlay / "panel_app.mm", app.encode())
    module.PANEL_APP_SRC = overlay / "panel_app.mm"
    # Test the missing getter, captured true/false values, inherited setters and
    # normal public getter before linking this compatibility code into the hook.
    test = overlay / "test-depth-compat"
    subprocess.run(["clang", "-fobjc-arc", "-arch", "x86_64", "-framework", "Foundation",
                    str(overlay / "test_depth_compat.m"), "-o", str(test)], check=True)
    subprocess.run(["/usr/bin/arch", "-x86_64", str(test)], check=True)
    framework = wine / "lib/external/D3DMetal.framework"
    require(framework.is_dir(), "Selected Wine has no bundled D3DMetal framework")
    original_hash = sha256(framework / "D3DMetal")
    # Adapt only framework discovery. --no-bottle retains the upstream build/tests
    # without touching CrossOver configuration, the selected runtime or registry.
    module.find_framework = lambda _: framework
    module.cmd_install(argparse.Namespace(no_bottle=True, profile="lossy", fsr_version="4.1.1",
                                         crossover="", bottle="", panel=True))
    # The full-quality choice is a distinct profile; validate it as well as Lite.
    env = os.environ.copy()
    env.update(FSR4_ROOT=str(source), FSR4_REPLACE="1", FSR4_VERSION="4.1.1",
               FSR4_PROFILE="exact-fast", FSR4_SHARPNESS="0", FSR4_SETTINGS="none", FSR4_PANEL="0",
               FSR4_HOOK_LOG=str(source / "dist/crossover/hook-full.log"))
    reference = module.build_reference(env)
    subprocess.run(["/usr/bin/arch", "-x86_64", str(source / "dist/crossover/live_test"),
                    str(source / "testcases/testcase_quality_720to1080"), str(reference)],
                   env=env, check=True)
    require(sha256(framework / "D3DMetal") == original_hash, "Original GPTK framework changed during build")
    report = {"schema": 1, "model": "FSR 4.1.1 Lite", "profile": "lossy",
              "synthetic_reference_and_live_hook": "passed", "profiles_validated": ["lossy", "exact-fast"], "game_tested": False,
              "original_framework_sha256": original_hash,
              "compilers": {name: {"path": str((source / "tools/bin" / name).resolve()),
                                    "sha256": sha256(source / "tools/bin" / name)}
                            for name in ("dxc", "spirv-cross", "spirv-val")}}
    save_json(source / "dist/crossover/nucleon-validation.json", report)
