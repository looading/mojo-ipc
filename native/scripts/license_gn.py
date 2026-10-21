#!/usr/bin/env python3
"""Keep Chromium's standalone SDK graph when the license tool calls gn desc.

The pinned license tool's LICENSES_GN_PATH hook runs this wrapper with Python.
Its temporary output directory contains args.gn, but loses the original build's
command-line graph root. Supply that root again without patching Chromium.
"""

import os
import subprocess
import sys

from build_native import LABEL


def command(arguments: list[str], executable: str) -> list[str]:
    return [executable, *arguments, "--root-target=" + LABEL, "--root-pattern=" + LABEL]


if __name__ == "__main__":
    raise SystemExit(subprocess.call(command(sys.argv[1:], os.environ["MOJO_LICENSE_GN"])))
