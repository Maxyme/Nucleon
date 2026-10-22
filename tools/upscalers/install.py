#!/usr/bin/env python3
"""Install verified provider dependencies. No prefix, registry or game execution."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import tempfile

import native
import ngx_bridge


def install(args):
    root = args.root.resolve()
    root.mkdir(parents=True, exist_ok=True)
    cache = root / "cache"
    cache.mkdir(exist_ok=True)
    if args.provider == "bridge":
        native.require(args.wine is not None, "Bridge preparation requires --wine")
        print(ngx_bridge.install(root, args.wine))
        return
    if args.provider == "native":
        native.require(args.wine is not None, "Native preparation requires --wine")
        wine = args.wine.resolve(strict=True)
        original = wine / "lib/external/D3DMetal.framework/D3DMetal"
        digest = native.sha256(original)
        key = hashlib.sha256(json.dumps([digest, native.LOCK["fsr4"]["commit"],
            native.sha256(Path(native.__file__)), native.sha256(Path(native.__file__).with_name("fsr4_depth_compat.h"))],
            separators=(",", ":")).encode()).hexdigest()
        destination = root / "providers/native" / key
        if (destination / "provider.json").exists():
            print(destination / "provider.json")
            return
        source = args.source or native.fetch_source(cache)
        native.verify_source(source)
        destination.mkdir(parents=True, exist_ok=True)
        runtime = destination / "fsr4"
        if not runtime.exists():
            shutil.copytree(source, runtime, symlinks=True,
                            ignore=shutil.ignore_patterns("dist", "build", "testcases", "__pycache__"))
        native.build_fsr(runtime, wine)
        files = {str(p.relative_to(destination)): native.sha256(p)
                 for folder in ("src", "hook", "third_party", "patches", "tools", "nucleon", "dist/crossover")
                 for p in (runtime / folder).rglob("*") if p.is_file() and not p.is_symlink()
                 and "__pycache__" not in p.parts}
        # Models and shader translators are loaded at run time, so retain their identities too.
        for p in (runtime / "tools/bin").glob("*"):
            if p.is_file():
                files[str(p.relative_to(destination))] = native.sha256(p)
        manifest = {"schema": 1, "provider": "native", "version": "4.1.1",
                    "runtime": str(wine), "framework_sha256": digest, "files": files,
                    "source": "fsr4", "experimental": True}
    else:
        key = "stock" if args.provider == "stock" else "optiscaler"
        spec = native.LOCK[key]
        archive = args.archive or native.download(spec["url"], cache / spec["url"].rsplit("/", 1)[1], spec["sha256"])
        native.require(native.sha256(archive) == spec["sha256"], "Provider archive checksum mismatch")
        destination = root / "providers" / args.provider / spec["sha256"]
        destination.parent.mkdir(parents=True, exist_ok=True)
        if destination.exists():
            native.require((destination / "provider.json").is_file(), "Incomplete provider installation; inspect it before retrying")
            print(destination / "provider.json")
            return
        with tempfile.TemporaryDirectory(dir=destination.parent) as tmp:
            staging = Path(tmp)
            listing = subprocess.check_output(["/usr/bin/tar", "-tf", str(archive)], text=True).splitlines()
            members = []
            for name in listing:
                p = PurePosixPath(name.replace("\\", "/"))
                native.require(not p.is_absolute() and ".." not in p.parts, "Unsafe archive path")
                # Extract only needed runtime libraries/config and notices, never install scripts.
                if p.suffix.lower() in (".dll", ".ini", ".txt", ".md") or p.name.lower().startswith(("license", "notice")):
                    members.append(name)
            subprocess.run(["/usr/bin/tar", "-xf", str(archive), "-C", str(staging), *members], check=True)
            for p in staging.rglob("*"):
                native.require(not p.is_symlink(), "Release contains a symlink")
            for name in ("OptiScaler.dll", "OptiScaler.ini"):
                native.require((staging / name).is_file(), f"Release missing {name}")
            manifest = {"schema": 1, "provider": args.provider, "version": spec["version"],
                        "files": {str(p.relative_to(staging)): native.sha256(p) for p in staging.rglob("*") if p.is_file()},
                        "experimental": True, "release": spec}
            native.save_json(staging / "provider.json", manifest)
            shutil.copytree(staging, destination)
    native.save_json(destination / "provider.json", manifest)
    print(destination / "provider.json")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("provider", choices=("stock", "metalfx", "native", "bridge"))
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--source", type=Path)
    parser.add_argument("--wine", type=Path)
    install(parser.parse_args())
