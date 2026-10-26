#!/usr/bin/env python3
"""Adapt the pinned upstream FSR4 source builder; publication is owned by Rust."""
import argparse
from pathlib import Path
import shutil

import native


def build(args):
    source = args.source or native.fetch_source(args.cache)
    native.verify_source(source)
    shutil.copytree(source, args.output, symlinks=True,
                    ignore=shutil.ignore_patterns("dist", "build", "testcases", "__pycache__"))
    native.build_fsr(args.output, args.wine)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--wine", type=Path, required=True)
    parser.add_argument("--source", type=Path)
    build(parser.parse_args())
