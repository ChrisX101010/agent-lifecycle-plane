from pathlib import Path
import re
import sys

BAD = []
PATTERNS = [
    ("private-key", re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----")),
    ("aws-access-key", re.compile(r"\bAKIA[0-9A-Z]{16}\b")),
    ("github-token", re.compile(r"\bgh[pousr]_[A-Za-z0-9_]{30,}\b")),
    ("openai-key", re.compile(r"\bsk-[A-Za-z0-9_-]{24,}\b")),
    ("slack-token", re.compile(r"\bxox[baprs]-[A-Za-z0-9-]{20,}\b")),
    ("stripe-live-key", re.compile(r"\b(?:sk|rk)_live_[A-Za-z0-9]{16,}\b")),
    (
        "generic-secret",
        re.compile(
            r"(?i)(?:api[_-]?key|secret[_-]?key|access[_-]?token|password)"
            r"\s*[:=]\s*[\"'](?!\$\{|<|changeme|change_me|example|ci-only-placeholder)"
            r"[A-Za-z0-9_./+=:-]{20,}[\"']"
        ),
    ),
]

SKIP_DIRS = {".git", "target", ".alp-backup", "node_modules", "_build", "deps"}
SKIP_FILES = {"Cargo.lock"}

for path in Path(".").rglob("*"):
    if not path.is_file() or path.name in SKIP_FILES or any(part in SKIP_DIRS for part in path.parts):
        continue
    text = path.read_text(errors="ignore")
    for name, regex in PATTERNS:
        if regex.search(text):
            BAD.append((str(path), name))

if BAD:
    print("SECRET_SCAN_FINDINGS=", BAD)
    sys.exit(1)

print("BUILTIN_SECRET_SCAN=PASS")
