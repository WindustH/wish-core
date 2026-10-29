#!/usr/bin/env python3
"""Small source-boundary guard; behavior is verified by the external wish-test suites."""
from pathlib import Path
import re
import sys
root = Path(__file__).resolve().parents[1] / "src"
rules = {
    "extensions/shared": (r"crate::extensions::(?!shared::)[a-z_]+::",),
    "extensions": (r"crate::(?:application|server|session|executor|tool|extension_host)::",),
    "extension_host": (r"crate::extensions::(?!(?:shared|image_runtime|media_runtime|transcription_runtime|image_registry|media_registry|read_registry)::)[a-z_]+::",),
    "application": (r"crate::extensions::(?!(?:catalog|invocation|package|shared|image_runtime|media_runtime|transcription_runtime)::)[a-z_]+::",),
    "client": (r"crate::extensions::",),
    "protocol": (r"crate::extensions::",),
    "task": (r"crate::(?:executor|session|tool)::",),
    "tool": (r"crate::executor::", r"\bexecutor::\{"),
    "storage": (r"crate::(?:application|server|session|executor)::",),
}
errors = []
for module, forbidden in rules.items():
    for path in [root / (module + ".rs"), *(root / module).rglob("*.rs")]:
        if not path.is_file():
            continue
        text = "\n".join(line for line in path.read_text().splitlines() if not line.lstrip().startswith("//"))
        for pattern in forbidden:
            if re.search(pattern, text):
                errors.append(f"{path.relative_to(root)} crosses the {module} dependency boundary: {pattern}")
if errors:
    print("\n".join(errors), file=sys.stderr)
    sys.exit(1)
print("Architecture boundaries checked.")
