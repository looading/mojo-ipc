"""Explicit Chromium source identity shared by native producer commands."""
import argparse
import re

OFFICIAL_COMMIT = "bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd"
OFFICIAL_URL = "https://chromium.googlesource.com/chromium/src.git"


def revision(value: str) -> str:
    if not re.fullmatch(r"[0-9a-f]{40}", value):
        raise argparse.ArgumentTypeError("Chromium revision must be 40 lowercase hexadecimal characters")
    return value


def add_revision_argument(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--chromium-revision", type=revision, default=OFFICIAL_COMMIT,
                        help="exact Chromium checkout commit (default: official SDK pin)")
