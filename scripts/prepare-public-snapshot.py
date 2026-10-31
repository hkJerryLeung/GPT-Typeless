#!/usr/bin/env python3
"""Create or check a deliberately limited, history-free public source snapshot."""

import argparse
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

# Exact file allowlist: additions require an explicit review and an edit here.
SOURCE_FILES = """\
.gitignore
LICENSE
package.json
package-lock.json
index.html
postcss.config.js
tailwind.config.js
tsconfig.json
tsconfig.node.json
vite.config.ts
public/favicon.png
public/setup-app-icon.png
scripts/release-macos.sh
scripts/prepare-public-snapshot.py
docs/PUBLIC-README.md
src/App.css
src/App.tsx
src/main.tsx
src/types.ts
src/vite-env.d.ts
src/components/AnswerText.tsx
src/components/HistoryPage.tsx
src/components/Icons.tsx
src/components/Modal.tsx
src/components/NativeControls.tsx
src/components/Settings.tsx
src/components/SetupDrag.tsx
src/components/SetupWizard.tsx
src/components/VocabularyPage.tsx
src/components/VoiceOverlay.tsx
src/components/dictionaryCsv.ts
src/components/features/RecordingControl.tsx
src/components/features/SettingsModal.tsx
src/components/features/StatusPanel.tsx
src/components/features/TimerDisplay.tsx
src/components/features/TranscriptionResult.tsx
src/components/features/index.ts
src/components/layout/Footer.tsx
src/components/layout/Header.tsx
src/components/layout/MainContent.tsx
src/components/layout/index.ts
src/components/ui/Button.tsx
src/components/ui/Input.tsx
src/components/ui/Modal.tsx
src/components/ui/ProgressStepper.tsx
src/components/ui/Select.tsx
src/components/ui/StatusBadge.tsx
src/components/ui/Toast.tsx
src/components/ui/index.ts
src/design-tokens/border-radius.ts
src/design-tokens/colors.ts
src/design-tokens/index.ts
src/design-tokens/shadows.ts
src/design-tokens/spacing.ts
src/design-tokens/transitions.ts
src/design-tokens/typography.ts
src/hooks/index.ts
src/hooks/useDictation.ts
src/hooks/useFocusTrap.ts
src/hooks/useMediaQuery.ts
src/hooks/useReducedMotion.ts
src/hooks/useToast.ts
src/hooks/useUpdater.ts
src/styles/accessibility.css
src/styles/animations.css
src/styles/components.css
src/styles/index.css
src/styles/responsive.css
src/styles/utilities.css
src/utils/accessibility.ts
src/utils/index.ts
src-tauri/Cargo.lock
src-tauri/Cargo.toml
src-tauri/.gitignore
src-tauri/Entitlements.plist
src-tauri/Info.plist
src-tauri/build.rs
src-tauri/tauri.conf.json
src-tauri/capabilities/default.json
src-tauri/icons/128x128.png
src-tauri/icons/128x128@2x.png
src-tauri/icons/32x32.png
src-tauri/icons/Square107x107Logo.png
src-tauri/icons/Square142x142Logo.png
src-tauri/icons/Square150x150Logo.png
src-tauri/icons/Square284x284Logo.png
src-tauri/icons/Square30x30Logo.png
src-tauri/icons/Square310x310Logo.png
src-tauri/icons/Square44x44Logo.png
src-tauri/icons/Square71x71Logo.png
src-tauri/icons/Square89x89Logo.png
src-tauri/icons/StoreLogo.png
src-tauri/icons/icon.icns
src-tauri/icons/icon.ico
src-tauri/icons/icon.png
src-tauri/src/api.rs
src-tauri/src/audio.rs
src-tauri/src/dictation.rs
src-tauri/src/fn_shortcut.rs
src-tauri/src/lib.rs
src-tauri/src/macos_input.rs
src-tauri/src/main.rs
src-tauri/src/microphone.rs
src-tauri/src/setup_drag.rs
src-tauri/src/storage.rs
src-tauri/src/text_ai.rs
src-tauri/src/updater.rs
test/dictionary-csv.mjs
test/frontend-regression.mjs
test/voice-language.mjs
""".splitlines()
SNAPSHOT_FILES = set(SOURCE_FILES) | {"README.md"}
EXECUTABLES = {"scripts/release-macos.sh", "scripts/prepare-public-snapshot.py"}
SNAPSHOT_IGNORE_EXTRAS = b"\n# Additional public-snapshot exclusions\noutput/\n*.pem\n*.p12\n*.pfx\n*.p8\n*.cer\n*.crt\n*.der\n*.mobileprovision\n"

# Rules report a category and an approved filename only, never matching text.
RULES = (
    ("private key", re.compile(rb"-----BEGIN (?:RSA |EC |OPENSSH |DSA |ENCRYPTED )?PRIVATE KEY-----")),
    ("JWT-shaped credential", re.compile(rb"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b")),
    ("provider credential", re.compile(rb"\b(?:sk-|gh[pousr]_|github_pat_|xox[baprs]-)[A-Za-z0-9_-]{20,}\b")),
    ("private home path", re.compile(rb"/(?:Users|home)/[A-Za-z0-9._-]+/")),
    ("credential assignment", re.compile(rb"(?i)[\"']?(?:api[_-]?key|access[_-]?token|chatgpt[_-]?token|client[_-]?secret|password)[\"']?\s*[:=]\s*[\"']([A-Za-z0-9_./+\-=]{16,})[\"']")),
)


class Refused(Exception):
    pass


def no_symlinks(path):
    """Check each existing absolute component without resolving symlinks."""
    if not path.is_absolute() or ".." in path.parts:
        raise Refused("Use an absolute path without '..'.")
    current = Path(path.anchor)
    for part in path.parts[1:]:
        current = current / part
        try:
            mode = os.lstat(current).st_mode
        except FileNotFoundError:
            break
        if stat.S_ISLNK(mode):
            raise Refused("A symlink in the path was refused.")


def overlapping(a, b):
    return a == b or a in b.parents or b in a.parents


def git_env():
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
               GIT_NO_REPLACE_OBJECTS="1", GIT_TERMINAL_PROMPT="0",
               GIT_NO_LAZY_FETCH="1")
    return env


def known_index_credential(source):
    """Read only the known index entry; keep its one credential in memory."""
    gitdir = source / ".git"
    if gitdir.is_symlink():
        raise Refused("Symlinked source Git metadata refused.")
    if not gitdir.is_dir() or not shutil.which("git"):
        return None
    try:
        result = subprocess.run(
            ["git", "-C", str(source), "show", "--no-ext-diff", "--no-textconv",
             ":src-tauri/gptservice-config.json"],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=git_env(), check=True,
        )
        value = json.loads(result.stdout).get("chatgpt_token", "")
        return value.encode("utf-8") if isinstance(value, str) and len(value) >= 16 else None
    except (OSError, ValueError, AttributeError, subprocess.CalledProcessError):
        return None


def open_source(source, rel):
    """Open every allowlisted component with O_NOFOLLOW, including directories."""
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    directory_flags = flags | getattr(os, "O_DIRECTORY", 0)
    descriptors = []
    try:
        fd = os.open(source, directory_flags)
        descriptors.append(fd)
        parts = Path(rel).parts
        for part in parts[:-1]:
            fd = os.open(part, directory_flags, dir_fd=fd)
            descriptors.append(fd)
        fd = os.open(parts[-1], flags, dir_fd=fd)
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode):
            os.close(fd)
            raise Refused("An allowlisted input is not a regular file: " + rel)
        return os.fdopen(fd, "rb"), info
    except OSError:
        raise Refused("Missing, unreadable, or symlinked allowlisted input: " + rel)
    finally:
        for fd in reversed(descriptors):
            os.close(fd)


def fingerprint(st):
    return (st.st_dev, st.st_ino, st.st_size, st.st_mtime_ns, st.st_ctime_ns)


def scan_data(data, name, known, location, findings):
    for label, pattern in RULES:
        for match in pattern.finditer(data):
            # Obvious synthetic values in test code are not credentials. The
            # exact-token and high-confidence rules still check every byte.
            if label == "credential assignment":
                value = match.group(1).lower()
                test_code = name.startswith("test/") or (
                    name.startswith("src-tauri/src/") and b"#[cfg(test)]" in data[:match.start()]
                )
                fixture = any(marker in value for marker in (b"fixture", b"synthetic", b"test"))
                fixture |= b"not" in value and b"secret" in value
                if test_code and fixture:
                    continue
            findings.add((location, name, label))
            break
    if known and known in data:
        findings.add((location, name, "exact known indexed credential"))


def check_tree(root, known):
    findings, files = set(), set()
    for base, dirs, names in os.walk(root, topdown=True, followlinks=False):
        base = Path(base)
        for name in list(dirs):
            path = base / name
            if stat.S_ISLNK(os.lstat(path).st_mode):
                raise Refused("Snapshot contains a symlink.")
            if base == root and name == ".git":
                dirs.remove(name)
        for name in names:
            path = base / name
            rel = path.relative_to(root).as_posix()
            info = os.lstat(path)
            if not stat.S_ISREG(info.st_mode):
                raise Refused("Snapshot contains a symlink or non-regular file.")
            if rel not in SNAPSHOT_FILES:
                findings.add(("worktree", "<unexpected path>", "not allowlisted"))
                continue
            files.add(rel)
            with path.open("rb") as handle:
                scan_data(handle.read(), rel, known, "worktree", findings)
    for name in SNAPSHOT_FILES - files:
        findings.add(("worktree", name, "missing allowlisted file"))
    return findings, files


def check_index(root, known, files, findings):
    gitdir = root / ".git"
    if not gitdir.exists():
        return "absent"
    if not gitdir.is_dir() or gitdir.is_symlink():
        raise Refused("Candidate Git metadata must be a real directory inside the snapshot.")
    # Git alternates could cause object reads outside this snapshot.
    if (gitdir / "objects/info/alternates").exists():
        raise Refused("Git object alternates are not allowed.")
    for base, dirs, names in os.walk(gitdir, followlinks=False):
        if any(stat.S_ISLNK(os.lstat(Path(base) / n).st_mode) for n in dirs + names):
            raise Refused("Symlink in candidate Git metadata refused.")
    config = gitdir / "config"
    if config.exists() and re.search(rb"(?im)^\s*\[\s*(?:include|includeif|extensions)\b", config.read_bytes()):
        raise Refused("Git includes and extensions are not allowed in a candidate index.")
    if not shutil.which("git"):
        raise Refused("Git is required to check the candidate index.")
    command = ["git", "--git-dir=" + str(gitdir), "--work-tree=" + str(root)]
    try:
        listing = subprocess.run(command + ["ls-files", "--stage", "-z"], env=git_env(),
                                 stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=True).stdout
        indexed = set()
        for entry in listing.split(b"\0"):
            if not entry:
                continue
            metadata, raw_path = entry.split(b"\t", 1)
            mode, object_id, stage = metadata.split()
            name = raw_path.decode("utf-8", "strict")
            if name not in SNAPSHOT_FILES:
                findings.add(("index", "<unexpected path>", "not allowlisted"))
                continue
            indexed.add(name)
            if mode not in (b"100644", b"100755") or stage != b"0":
                findings.add(("index", name, "non-regular or unmerged entry"))
                continue
            blob = subprocess.run(command + ["cat-file", "blob", object_id.decode("ascii")],
                                  env=git_env(), stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL, check=True).stdout
            scan_data(blob, name, known, "index", findings)
            if name not in files or blob != (root / name).read_bytes():
                findings.add(("index", name, "differs from worktree"))
        for name in files - indexed:
            findings.add(("index", name, "not staged"))
    except (OSError, ValueError, UnicodeError, subprocess.CalledProcessError):
        raise Refused("Unable to read the candidate index safely.")
    return "checked"


def report(root, known):
    findings, files = check_tree(root, known)
    index = check_index(root, known, files, findings)
    for location, name, kind in sorted(findings):
        print("FINDING {} {}: {}".format(location, name, kind))
    print("Findings: {} | candidate index: {} | known-index comparison: {}".format(
        len(findings), index, "performed" if known else "unavailable"))
    return not findings


def create(source, destination, known):
    no_symlinks(source)
    no_symlinks(destination)
    if overlapping(source, destination):
        raise Refused("Destination must be outside and separate from the source tree.")
    if not destination.parent.is_dir() or os.path.lexists(destination):
        raise Refused("Destination parent must exist and destination must not exist.")
    stage = Path(tempfile.mkdtemp(prefix=".gpt-public-", dir=destination.parent))
    created = False
    try:
        states = {}
        for rel in SOURCE_FILES:
            src, before = open_source(source, rel)
            target = stage / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            with src, target.open("xb") as out:
                shutil.copyfileobj(src, out)
                after = os.fstat(src.fileno())
            if fingerprint(before) != fingerprint(after):
                raise Refused("Source changed while copying: " + rel)
            states[rel] = fingerprint(after)
            target.chmod(0o755 if rel in EXECUTABLES else 0o644)
        # Strengthen the copied ignore rules only in the new snapshot.
        with (stage / ".gitignore").open("ab") as ignore:
            ignore.write(SNAPSHOT_IGNORE_EXTRAS)
        shutil.copyfile(stage / "docs/PUBLIC-README.md", stage / "README.md")
        for rel, expected in states.items():
            src, current = open_source(source, rel)
            src.close()
            if fingerprint(current) != expected:
                raise Refused("Source changed during snapshot creation: " + rel)
        if not report(stage, known):
            raise Refused("Snapshot scan failed; no destination was created.")
        # mkdir is exclusive: never replace an existing destination, even in a race.
        os.mkdir(destination, 0o700)
        created = True
        for item in stage.iterdir():
            os.rename(item, destination / item.name)
        os.chmod(destination, 0o755)
        created = False
        print("Created history-free snapshot:", destination)
    finally:
        if created and destination.is_dir() and not destination.is_symlink():
            shutil.rmtree(destination)
        shutil.rmtree(stage, ignore_errors=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path, help="absolute new snapshot path, or snapshot to check")
    parser.add_argument("--check", action="store_true", help="scan a snapshot and its index if present")
    args = parser.parse_args()
    source = Path(os.path.abspath(Path(__file__).parent.parent))
    no_symlinks(source)
    no_symlinks(args.destination)
    known = known_index_credential(source)
    if args.check:
        if overlapping(source, args.destination):
            raise Refused("Check only a separate snapshot outside the source tree.")
        if not args.destination.is_dir():
            raise Refused("Snapshot directory does not exist.")
        if not all((args.destination / p).is_file() for p in (
            "docs/PUBLIC-README.md", "scripts/prepare-public-snapshot.py", "src-tauri/Cargo.toml"
        )):
            raise Refused("The destination does not look like a public snapshot.")
        return 0 if report(args.destination, known) else 1
    create(source, args.destination, known)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (Refused, OSError) as error:
        print("Refused: {}".format(error if isinstance(error, Refused) else "filesystem operation failed"), file=sys.stderr)
        sys.exit(2)
