"""Build an x64 export forwarder; preserve the vendor bridge's original name."""
import hashlib
import json
from pathlib import Path
import re
import shutil
import struct
import subprocess
import tempfile

import native


def exports(path):
    data = Path(path).read_bytes()
    native.require(data[:2] == b"MZ", "Bridge is not a PE image")
    pe = struct.unpack_from("<I", data, 60)[0]
    native.require(data[pe:pe + 4] == b"PE\0\0" and
                   struct.unpack_from("<H", data, pe + 4)[0] == 0x8664, "Bridge must be x64")
    count = struct.unpack_from("<H", data, pe + 6)[0]
    optional_size = struct.unpack_from("<H", data, pe + 20)[0]
    optional = pe + 24
    native.require(struct.unpack_from("<H", data, optional)[0] == 0x20b, "Bridge must be PE32+")
    sections = optional + optional_size

    def offset(rva):
        for i in range(count):
            header = sections + i * 40
            size, address, raw_size, raw = struct.unpack_from("<IIII", data, header + 8)
            if address <= rva < address + max(size, raw_size):
                result = raw + rva - address
                native.require(result < len(data), "Invalid PE export address")
                return result
        raise RuntimeError("PE export address is outside its sections")

    directory = offset(struct.unpack_from("<I", data, optional + 112)[0])
    number = struct.unpack_from("<I", data, directory + 24)[0]
    names = offset(struct.unpack_from("<I", data, directory + 32)[0])
    native.require(number < 65536, "Invalid PE export count")
    result = []
    for i in range(number):
        start = offset(struct.unpack_from("<I", data, names + i * 4)[0])
        end = data.index(b"\0", start, min(len(data), start + 512))
        name = data[start:end].decode("ascii")
        if name.startswith("NVSDK_NGX_"):
            native.require(re.fullmatch(r"[A-Za-z0-9_]+", name), "Invalid NGX export name")
            result.append(name)
    required = {"NVSDK_NGX_D3D12_" + n for n in (
        "Init_Ext", "Init_ProjectID", "AllocateParameters", "GetParameters", "GetCapabilityParameters",
        "CreateFeature", "EvaluateFeature", "DestroyParameters", "ReleaseFeature")}
    native.require(required <= set(result), "GPTK bridge lacks required DX12 exports")
    return sorted(set(result))


def install(root, wine):
    bridge = Path(wine).resolve() / "lib/wine/x86_64-windows/nvngx-on-metalfx.dll"
    names = exports(bridge)
    digest = native.sha256(bridge)
    key = hashlib.sha256(json.dumps([digest, native.sha256(__file__)], separators=(",", ":")).encode()).hexdigest()
    destination = Path(root) / "providers/bridge" / key
    if (destination / "provider.json").exists():
        return destination / "provider.json"
    compiler = shutil.which("x86_64-w64-mingw32-gcc")
    native.require(compiler, "NGX forwarding setup requires x86_64-w64-mingw32-gcc")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=destination.parent) as tmp:
        staging = Path(tmp)
        definition = staging / "nvngx.def"
        definition.write_text("LIBRARY nvngx\nEXPORTS\n" + "".join(
            f"{name}=nvngx-on-metalfx.{name}\n" for name in names))
        subprocess.run([compiler, "-shared", "-nostdlib", str(definition), "-Wl,--entry,0",
                        "-Wl,--no-insert-timestamp", "-o", str(staging / "nvngx.dll")], check=True)
        native.require(exports(staging / "nvngx.dll") == names, "Forwarder exports differ from vendor bridge")
        manifest = {"schema": 1, "provider": "bridge", "version": "ngx-forward-v1",
                    "bridge_sha256": digest, "files": {p.name: native.sha256(p) for p in staging.iterdir()},
                    "experimental": True}
        native.save_json(staging / "provider.json", manifest)
        native.require(not destination.exists(), "Incomplete bridge setup exists; inspect it before retrying")
        staging.rename(destination)
    return destination / "provider.json"
