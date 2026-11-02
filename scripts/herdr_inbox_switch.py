#!/usr/bin/env python3
"""Back up and switch local stable/inbox sessions. Dry run unless --apply.

Run from a separate terminal: shutdown ends every pane process and shell job.
Provider histories stay in their native locations and are shared by both builds.
"""

import argparse
import errno
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import socket
import stat
import struct
import subprocess
import sys
import time
import uuid
from dataclasses import dataclass

try:
    import fcntl
except ImportError:
    fcntl = None


DURABLE = (
    "session.json", "session-history.json", "agent-conversations.json",
    "agent-conversations.json.bak", "session-backups", "session-snapshots",
)
TREES = {"session-backups", "session-snapshots"}
ALPHABET = "123456789ABCDEFGHJKMNPQRSTVWXYZ0"
TIMEOUT = 30


class Refusal(RuntimeError):
    pass


def checked_path(path):
    """Refuse symlinks in the path, including dangling links and parent links."""
    path = Path(path).absolute()
    for part in [path] + list(path.parents):
        if part.is_symlink():
            raise Refusal("Symlink refused: " + str(part))
    return path


def fsync_dir(path):
    fd = os.open(str(path), os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def atomic_json(path, value):
    checked_path(path)
    temporary = path.with_name(path.name + ".tmp-" + uuid.uuid4().hex)
    with temporary.open("x", encoding="utf-8") as stream:
        os.chmod(temporary, 0o600)
        json.dump(value, stream, indent=2, sort_keys=True)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)
    fsync_dir(path.parent)


def read_json(path):
    checked_path(path)
    with path.open(encoding="utf-8") as stream:
        return json.load(stream)


def digest(path):
    checked_path(path)
    with path.open("rb") as stream:
        result = hashlib.sha256()
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(block)
        return result.hexdigest()


def inventory(directory, roots=DURABLE):
    """Only durable data; never sockets, locks, PID files, logs or temp files."""
    checked_path(directory)
    entries = {}

    def visit(path):
        checked_path(path)
        info = path.lstat()
        relative = str(path.relative_to(directory))
        mode = stat.S_IMODE(info.st_mode)
        if stat.S_ISREG(info.st_mode):
            if info.st_nlink != 1:
                raise Refusal("Hard-linked session data refused: " + str(path))
            entries[relative] = {"kind": "file", "sha256": digest(path), "mode": mode}
        elif stat.S_ISDIR(info.st_mode):
            entries[relative] = {"kind": "directory", "mode": mode}
            for child in sorted(path.iterdir()):
                if child.name.endswith((".tmp", ".lock", ".pid")) or ".tmp-" in child.name:
                    continue
                visit(child)
        else:
            raise Refusal("Non-regular durable data refused: " + str(path))

    for name in roots:
        path = directory / name
        checked_path(path)
        if path.exists():
            if name in DURABLE and (name in TREES) != path.is_dir():
                raise Refusal("Unexpected session data type: " + str(path))
            visit(path)
    return entries


def root_inventory(path, name):
    return {name + relative[len(path.name):]: item
            for relative, item in inventory(path.parent, (path.name,)).items()}


def content(entries):
    return {name: {key: value for key, value in item.items() if key != "mode"}
            for name, item in entries.items()}


def validate_entries(entries):
    if not isinstance(entries, dict):
        raise Refusal("Invalid manifest entries")
    for relative, item in entries.items():
        path = Path(relative)
        if path.is_absolute() or not path.parts or any(part in (".", "..") for part in path.parts) or str(path) != relative:
            raise Refusal("Unsafe manifest path: " + repr(relative))
        if path.parts[0] not in DURABLE or (len(path.parts) > 1 and path.parts[0] not in TREES):
            raise Refusal("Unexpected manifest path: " + relative)
        if item.get("kind") not in ("file", "directory") or not isinstance(item.get("mode"), int) or not 0 <= item["mode"] <= 0o777:
            raise Refusal("Invalid manifest permissions/type: " + relative)
        if item["kind"] == "file" and not re.fullmatch(r"[a-f0-9]{64}", item.get("sha256", "")):
            raise Refusal("Invalid manifest digest: " + relative)
        if len(path.parts) == 1 and (path.parts[0] in TREES) != (item["kind"] == "directory"):
            raise Refusal("Invalid manifest root type: " + relative)
        if len(path.parts) > 1 and entries.get(str(path.parent), {}).get("kind") != "directory":
            raise Refusal("Missing manifest parent directory: " + relative)


def copy_inventory(source, destination, entries, private=False):
    validate_entries(entries)
    destination.mkdir(parents=True, exist_ok=True, mode=0o700)
    checked_path(destination)
    for relative, item in sorted(entries.items(), key=lambda pair: (pair[0].count("/"), pair[0])):
        src, dst = source / relative, destination / relative
        checked_path(src)
        checked_path(dst)
        if item["kind"] == "directory":
            dst.mkdir(mode=0o700)
        else:
            with src.open("rb") as incoming, dst.open("xb") as outgoing:
                os.chmod(dst, 0o600)
                shutil.copyfileobj(incoming, outgoing)
                os.chmod(dst, 0o600 if private else item["mode"])
                outgoing.flush()
                os.fsync(outgoing.fileno())
    if not private:
        for relative, item in sorted(entries.items(), reverse=True):
            if item["kind"] == "directory":
                os.chmod(destination / relative, item["mode"])
    for parent, _, _ in os.walk(destination, topdown=False):
        fsync_dir(Path(parent))


def backup_session(session, destination):
    entries = inventory(session.directory)
    destination.mkdir(parents=True, mode=0o700)
    copy_inventory(session.directory, destination / "data", entries, private=True)
    if content(inventory(destination / "data")) != content(entries):
        raise Refusal("Backup content mismatch: " + str(destination))
    if inventory(session.directory) != entries:
        raise Refusal("Session changed while backing up: " + str(session.directory))
    atomic_json(destination / "manifest.json", {
        "source": str(session.directory), "entries": entries,
    })
    verify_backup(destination)
    return str(destination)


def verify_backup(directory):
    directory = checked_path(directory)
    manifest = read_json(directory / "manifest.json")
    validate_entries(manifest["entries"])
    if stat.S_IMODE(directory.stat().st_mode) & 0o077:
        raise Refusal("Backup directory is not private: " + str(directory))
    if content(inventory(directory / "data")) != content(manifest["entries"]):
        raise Refusal("Backup verification failed: " + str(directory))
    for relative in manifest["entries"]:
        if stat.S_IMODE((directory / "data" / relative).stat().st_mode) & 0o077:
            raise Refusal("Backup data is not private: " + str(directory / "data" / relative))
    return manifest


@dataclass(frozen=True)
class Context:
    label: str
    binary: Path
    config: Path
    state: Path

    def environment(self):
        env = {key: value for key, value in os.environ.items()
               if not key.startswith("HERDR_") and key != "CODEX_THREAD_ID"}
        env["XDG_CONFIG_HOME"] = str(self.config.parent)
        env["XDG_STATE_HOME"] = str(self.state.parent)
        env["PATH"] = str(self.binary.parent) + os.pathsep + env.get("PATH", "")
        if self.label == "inbox":
            env["HERDR_DISABLE_UPDATES"] = "1"
        return env

    def session(self, name):
        validate_name(name)
        return Session(self, name)

    def as_json(self):
        return {"label": self.label, "binary": str(self.binary),
                "config": str(self.config), "state": str(self.state)}

    @classmethod
    def from_json(cls, value):
        return cls(value["label"], *(Path(value[key]) for key in ("binary", "config", "state")))


@dataclass(frozen=True)
class Session:
    context: Context
    name: str

    @property
    def directory(self):
        return self.context.config if self.name == "default" else self.context.config / "sessions" / self.name

    @property
    def endpoint(self):
        return self.directory / "herdr.sock"

    def command(self, *args):
        return [str(self.context.binary), "--session", self.name] + list(args)

def validate_name(name):
    if name in (".", "..") or not re.fullmatch(r"[A-Za-z0-9_.-]{1,64}", name):
        raise Refusal("Unsupported session name: " + repr(name))


def api(session, method):
    checked_path(session.endpoint)
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(3)
        connection.connect(str(session.endpoint))
        connection.sendall((json.dumps({"id": "inbox-switch", "method": method, "params": {}}) + "\n").encode())
        with connection.makefile("rb") as stream:
            line = stream.readline(32 * 1024 * 1024)
    response = json.loads(line)
    if "error" in response:
        raise Refusal("API refused " + method + ": " + str(response["error"]))
    return response["result"]


def running(session):
    checked_path(session.endpoint)
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
            connection.settimeout(1)
            connection.connect(str(session.endpoint))
        return True
    except OSError as error:
        if error.errno in (errno.ENOENT, errno.ECONNREFUSED):
            return False
        raise Refusal("Cannot determine server state at %s: %s" % (session.endpoint, error)) from error


def discover(context):
    checked_path(context.config)
    sessions = []
    default = context.session("default")
    if inventory(default.directory) or running(default):
        sessions.append(default)
    directory = checked_path(context.config / "sessions")
    if directory.exists():
        for child in sorted(directory.iterdir()):
            checked_path(child)
            if not child.is_dir():
                raise Refusal("Unexpected entry in sessions directory: " + str(child))
            validate_name(child.name)
            if child.name == "default":
                raise Refusal("Named/default session collision: " + str(child))
            session = context.session(child.name)
            # Empty named directories are still discovered and backed up.
            inventory(session.directory)
            running(session)
            sessions.append(session)
    return sessions


def encoded(number):
    if not isinstance(number, int) or number < 0:
        raise Refusal("Invalid persisted public number")
    result = ""
    while number:
        number, digit = divmod(number - 1, len(ALPHABET))
        result = ALPHABET[digit] + result
    return result or "0"


def semantic_disk(document):
    """Compare only facts exposed by session.snapshot, preserving bytes separately."""
    if document.get("version") != 3:
        raise Refusal("Only persisted session format 3 is supported")
    workspaces, panes, tabs, layouts = [], {}, {}, {}
    for workspace in document["workspaces"]:
        wid = workspace["id"]
        if not isinstance(wid, str) or not wid or any(item["id"] == wid for item in workspaces):
            raise Refusal("Missing or duplicate persisted workspace ID")
        workspaces.append(wid)
        numbers = workspace["public_pane_numbers"]
        tab_ids = []
        if len(workspace["tabs"]) != len(workspace["public_tab_numbers"]):
            raise Refusal("Incomplete persisted public tab mapping")
        for index, tab in enumerate(workspace["tabs"]):
            tid = wid + ":t" + encoded(workspace["public_tab_numbers"][index])
            if tid in tabs:
                raise Refusal("Duplicate persisted public tab ID")
            tab_ids.append(tid)
            leaf_ids, splits = [], []

            def pane_id(raw):
                return wid + ":p" + encoded(numbers[str(raw)])

            def walk(node, path=""):
                if set(node) == {"Pane"}:
                    leaf_ids.append(pane_id(node["Pane"]))
                elif set(node) == {"Split"}:
                    split = node["Split"]
                    direction = {"Horizontal": "right", "Vertical": "down"}[split["direction"]]
                    splits.append(["split_%d_%s" % (len(splits), path or "root"), direction, round(float(split["ratio"]), 5)])
                    walk(split["first"], path + "0")
                    walk(split["second"], path + "1")
                else:
                    raise Refusal("Unknown persisted layout node")

            walk(tab["layout"])
            member_ids = []
            for raw, pane in tab["panes"].items():
                pid = pane_id(raw)
                if pid in panes:
                    raise Refusal("Duplicate persisted public pane ID")
                cwd = pane["cwd"]
                if not isinstance(cwd, str) or not os.path.isabs(cwd):
                    raise Refusal("Persisted pane cwd must be absolute")
                ref = pane.get("agent_session")
                if ref and (set(ref) != {"source", "agent", "kind", "value"} or
                            ref["kind"] not in ("id", "path") or not ref["value"] or
                            (ref["kind"] == "path" and not os.path.isabs(ref["value"]))):
                    raise Refusal("Invalid persisted native agent reference")
                panes[pid] = {"workspace": wid, "tab": tid, "cwd": cwd,
                              "label": pane.get("label"), "agent_session": ref}
                member_ids.append(pid)
            if len(set(leaf_ids)) != len(leaf_ids) or set(leaf_ids) != set(member_ids):
                raise Refusal("Persisted layout and pane map disagree")
            tabs[tid] = wid
            layouts[tid] = {"panes": leaf_ids, "focus": pane_id(tab["focused"]),
                            "zoomed": tab["zoomed"], "splits": splits}
        if not tab_ids or not 0 <= workspace["active_tab"] < len(tab_ids):
            raise Refusal("Invalid persisted active tab")
        # Workspace order and tab order are part of the saved topology.
        workspaces[-1] = {"id": wid, "tabs": tab_ids,
                          "active_tab": tab_ids[workspace["active_tab"]]}
    active = document["active"]
    if active is not None and not 0 <= active < len(workspaces):
        raise Refusal("Invalid persisted active workspace")
    return {"workspaces": workspaces, "panes": panes, "tabs": tabs, "layouts": layouts,
            "active": None if active is None else workspaces[active]["id"]}


def semantic_live(document):
    workspaces, panes, tabs, layouts = [], {}, {}, {}
    for workspace in document["workspaces"]:
        wid = workspace["workspace_id"]
        members = [tab for tab in document["tabs"] if tab["workspace_id"] == wid]
        ids = [tab["tab_id"] for tab in members]
        if workspace["tab_count"] != len(ids):
            raise Refusal("Incomplete live workspace topology")
        workspaces.append({"id": wid, "tabs": ids, "active_tab": workspace["active_tab_id"]})
    for tab in document["tabs"]:
        tid = tab["tab_id"]
        if tid in tabs:
            raise Refusal("Duplicate live tab")
        tabs[tid] = tab["workspace_id"]
        if tab["pane_count"] != sum(pane["tab_id"] == tid for pane in document["panes"]):
            raise Refusal("Incomplete live tab topology")
    for pane in document["panes"]:
        if pane.get("restore_error"):
            raise Refusal("Pane restore failed: " + str(pane["restore_error"]))
        pid = pane["pane_id"]
        if pid in panes or pane["tab_id"] not in tabs:
            raise Refusal("Invalid live pane membership")
        panes[pid] = {"workspace": pane["workspace_id"], "tab": pane["tab_id"],
                      "cwd": pane.get("cwd"), "label": pane.get("label"),
                      "agent_session": pane.get("agent_session")}
    for layout in document["layouts"]:
        tid = layout["tab_id"]
        if tid in layouts:
            raise Refusal("Duplicate live layout")
        layouts[tid] = {"panes": [pane["pane_id"] for pane in layout["panes"]],
                        "focus": layout["focused_pane_id"], "zoomed": layout["zoomed"],
                        "splits": [[split["id"], split["direction"], round(float(split["ratio"]), 5)]
                                   for split in layout["splits"]]}
    return {"workspaces": workspaces, "panes": panes, "tabs": tabs, "layouts": layouts,
            "active": document.get("focused_workspace_id")}


def disk_state(session):
    path = session.directory / "session.json"
    if not path.exists():
        raise Refusal("No saved session snapshot: " + str(path))
    try:
        return semantic_disk(read_json(path))
    except (KeyError, IndexError, TypeError, ValueError) as error:
        raise Refusal("Invalid persisted snapshot at %s: %s" % (path, error)) from error


def live_state(session):
    document = api(session, "session.snapshot")["snapshot"]
    try:
        return document, semantic_live(document)
    except (KeyError, IndexError, TypeError, ValueError) as error:
        raise Refusal("Unsupported live snapshot at %s: %s" % (session.endpoint, error)) from error


def matched_state(session, timeout=TIMEOUT):
    deadline = time.monotonic() + timeout
    while True:
        document, live = live_state(session)
        try:
            if disk_state(session) == live:
                return document, live
        except Refusal:
            if time.monotonic() >= deadline:
                raise
        if time.monotonic() >= deadline:
            raise Refusal("Saved snapshot does not match live topology/native IDs/cwd: " + str(session.directory))
        time.sleep(0.2)


def process_info(pid):
    result = subprocess.run(["ps", "-p", str(pid), "-o", "uid=", "-o", "lstart=", "-o", "state=", "-o", "command="],
                            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode == 1 and not result.stdout.strip():
        return None
    if result.returncode or not result.stdout.strip():
        raise Refusal("Cannot inspect server PID " + str(pid))
    fields = result.stdout.strip().split(None, 7)
    if len(fields) != 8:
        raise Refusal("Unrecognized ps output for server PID " + str(pid))
    if fields[6].startswith("Z"):
        return None
    return {"pid": pid, "uid": int(fields[0]), "started": " ".join(fields[1:6]), "command": fields[7]}


def server_identity(session):
    checked_path(session.endpoint)
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(3)
        connection.connect(str(session.endpoint))
        if sys.platform == "darwin":
            # Darwin SOL_LOCAL / LOCAL_PEERPID, independent of Python's exported constants.
            pid = struct.unpack("i", connection.getsockopt(0, 2, 4))[0]
        elif hasattr(socket, "SO_PEERCRED"):
            pid, _, _ = struct.unpack("3i", connection.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
        else:
            raise Refusal("Peer PID verification requires macOS or Linux")
    identity = process_info(pid)
    if not identity or identity["uid"] != os.getuid():
        raise Refusal("Server process is not owned by this user")
    command = shlex.split(identity["command"])
    if not command or "server" not in command[1:] or not os.path.samefile(command[0], session.context.binary):
        raise Refusal("Server PID does not belong to expected binary: " + str(identity))
    return identity


def wait_exit(identity, timeout=TIMEOUT):
    deadline = time.monotonic() + timeout
    while process_info(identity["pid"]) == identity:
        if time.monotonic() >= deadline:
            raise Refusal("Server PID has not exited: " + str(identity["pid"]))
        time.sleep(0.1)


def stop(session, identity):
    if server_identity(session) != identity:
        raise Refusal("Server identity changed before shutdown: " + str(session.endpoint))
    try:
        api(session, "server.stop")
    except (OSError, ValueError):
        # Some versions close the API stream while processing the accepted stop.
        pass
    wait_exit(identity)
    if running(session):
        raise Refusal("A replacement server appeared during shutdown: " + str(session.endpoint))


def validate_context(context):
    for path in (context.config, context.state):
        checked_path(path)
    if not context.binary.is_file() or not os.access(context.binary, os.X_OK):
        raise Refusal("Missing executable: " + str(context.binary))
    if context.config.name != "herdr" or context.state.name != "herdr":
        raise Refusal("Release binaries require config/state directories ending in herdr")
    if context.label == "inbox":
        text = checked_path(context.config / "config.toml").read_text()
        section = re.search(r"(?ms)^\[update\]\s*$(.*?)(?=^\[|\Z)", text)
        if not section or any(not re.search(r"(?m)^\s*" + key + r"\s*=\s*false\s*(?:#.*)?$", section[1])
                              for key in ("version_check", "manifest_check")):
            raise Refusal("Inbox config must disable update.version_check and update.manifest_check")


def plan(source, target):
    sources, targets = discover(source), discover(target)
    mapping = []
    names = set()
    for session in sources:
        name = ("agent-inbox" if session.name == "default" else session.name) if target.label == "inbox" else ("default" if session.name == "agent-inbox" else session.name)
        if name in names:
            raise Refusal("Default/name mapping collision at target session " + name)
        names.add(name)
        mapping.append((session, target.session(name)))
    # Every overwritten target, including a new empty destination, has a verified backup.
    known = {session.name for session in targets}
    targets += [destination for _, destination in mapping if destination.name not in known]
    if not sources:
        raise Refusal("No source sessions discovered in " + str(source.config))
    return mapping, targets


def attach_command(session):
    env = session.context.environment()
    unsets = ["HERDR_ENV", "HERDR_SOCKET_PATH", "HERDR_CLIENT_SOCKET_PATH", "HERDR_SESSION",
              "HERDR_CONFIG_PATH", "HERDR_WORKSPACE_ID", "HERDR_TAB_ID", "HERDR_PANE_ID",
              "HERDR_AGENT", "CODEX_THREAD_ID"]
    args = ["env"]
    for key in unsets:
        args += ["-u", key]
    args += ["XDG_CONFIG_HOME=" + env["XDG_CONFIG_HOME"], "XDG_STATE_HOME=" + env["XDG_STATE_HOME"]]
    if session.context.label == "inbox":
        args += ["HERDR_DISABLE_UPDATES=1"]
    return shlex.join(args + session.command())


class Switch:
    def __init__(self, source, target, backup_root, transaction=None):
        self.source, self.target = source, target
        self.backup_root = checked_path(backup_root)
        self.directory = checked_path(transaction) if transaction else None
        self.journal = read_json(self.directory / "journal.json") if transaction else None
        if self.journal and (self.journal["source"] != source.as_json() or self.journal["target"] != target.as_json()):
            raise Refusal("Recovery contexts do not match the transaction")
        if self.journal:
            self.validate_journal()

    def validate_journal(self):
        journal = self.journal
        if journal.get("version") != 1 or journal.get("phase") not in {"preparing", "ready", "stopping", "stopped", "installing", "starting", "complete", "rolling-back", "rolled-back"}:
            raise Refusal("Unsupported recovery journal")
        if self.directory.parent != self.backup_root:
            raise Refusal("Transaction is outside the backup root")
        known = set()
        backup_paths = set()
        directories = set()
        for record in journal["sessions"]:
            if record["side"] not in ("source", "target"):
                raise Refusal("Invalid journal session side")
            key = (record["side"], record["name"])
            if key in known or Path(record["directory"]) != self.session(record).directory:
                raise Refusal("Invalid/duplicate journal session path")
            known.add(key)
            directories.add(record["directory"])
            validate_entries(record["expected"])
            for field in ("pre", "final", "unverified_final"):
                if record.get(field):
                    path = checked_path(record[field])
                    if path.parent != self.directory:
                        raise Refusal("Backup path is outside the transaction")
                    backup_paths.add(str(path))
            if record.get("evidence") and checked_path(record["evidence"]["path"]).parent != self.directory:
                raise Refusal("Live evidence is outside the transaction")
        seen_targets = set()
        for src, dst in journal["mapping"]:
            expected = ("agent-inbox" if src == "default" else src) if self.target.label == "inbox" else ("default" if src == "agent-inbox" else src)
            if expected != dst or dst in seen_targets or ("source", src) not in known or ("target", dst) not in known:
                raise Refusal("Invalid journal session mapping")
            seen_targets.add(dst)
        for field in ("operations", "rollback_operations"):
            for operation in journal.get(field, []):
                destination = checked_path(operation["destination"])
                name = operation["name"]
                allowed = {record["directory"] for record in journal["sessions"] if field == "rollback_operations" or record["side"] == "target"}
                if name not in DURABLE or destination.name != name or str(destination.parent) not in allowed or operation["backup"] not in backup_paths:
                    raise Refusal("Operation is outside journalled session data")
                for location in ("stage", "displaced"):
                    path = checked_path(operation[location])
                    if path.parent != destination.parent or not path.name.startswith((".switch-", ".rollback-")):
                        raise Refusal("Invalid journal staging path")
                for state in ("old", "new"):
                    validate_entries(operation[state])
                    if any(path != name and not path.startswith(name + "/") for path in operation[state]):
                        raise Refusal("Operation includes another data root")
        for entry in journal["restarts"]:
            if ("target", entry["name"]) not in known:
                raise Refusal("Unknown restart target in journal")

    def save(self, phase=None):
        if phase:
            self.journal["phase"] = phase
        atomic_json(self.directory / "journal.json", self.journal)

    def session(self, record):
        return (self.source if record["side"] == "source" else self.target).session(record["name"])

    def prepare(self):
        validate_context(self.source)
        validate_context(self.target)
        mapping, targets = plan(self.source, self.target)
        self.directory = self.backup_root / (time.strftime("%Y%m%dT%H%M%S") + "-" + uuid.uuid4().hex)
        self.directory.mkdir(parents=True, mode=0o700)
        fsync_dir(self.backup_root)
        self.journal = {"version": 1, "source": self.source.as_json(), "target": self.target.as_json(),
                        "phase": "preparing", "sessions": [], "mapping": [[src.name, dst.name] for src, dst in mapping],
                        "operations": [], "restarts": []}
        self.save()
        for side, sessions in (("source", [src for src, _ in mapping]), ("target", targets)):
            for session in sessions:
                record = {"side": side, "name": session.name, "directory": str(session.directory),
                          "running": running(session), "stopped": False}
                if record["running"]:
                    record["identity"] = server_identity(session)
                    document, live = matched_state(session)
                    record["semantic"] = live
                    evidence = self.directory / (side + "-" + session.name + "-live.json")
                    atomic_json(evidence, document)
                    record["evidence"] = {"path": str(evidence), "sha256": digest(evidence)}
                elif (session.directory / "session.json").exists():
                    record["semantic"] = disk_state(session)
                record["pre"] = backup_session(session, self.directory / (side + "-" + session.name + "-pre"))
                record["expected"] = inventory(session.directory)
                self.journal["sessions"].append(record)
                self.save()
        self.save("ready")

    def verify_all_backups(self):
        for record in self.journal["sessions"]:
            if Path(record["directory"]) != self.session(record).directory:
                raise Refusal("Journal session path does not match its context")
            if verify_backup(Path(record["pre"]))["source"] != record["directory"]:
                raise Refusal("Backup source does not match journal")
            if record.get("final"):
                if verify_backup(Path(record["final"]))["source"] != record["directory"]:
                    raise Refusal("Final backup source does not match journal")
            if record.get("evidence") and digest(Path(record["evidence"]["path"])) != record["evidence"]["sha256"]:
                raise Refusal("Live snapshot evidence changed")

    def stop_all(self):
        self.verify_all_backups()
        self.save("stopping")
        for record in self.journal["sessions"]:
            session = self.session(record)
            if record.get("final"):
                if running(session) or inventory(session.directory) != record["expected"]:
                    raise Refusal("Stopped session changed since journal: " + str(session.directory))
                continue
            if record["running"]:
                if running(session):
                    document, live = matched_state(session)
                    if live != record["semantic"]:
                        raise Refusal("Session changed after backup; start a new dry run: " + str(session.directory))
                    record["stop_requested"] = True
                    self.save()
                    stop(session, record["identity"])
                elif not record.get("stop_requested"):
                    raise Refusal("Server stopped outside this transaction: " + str(session.directory))
                else:
                    wait_exit(record["identity"])
                record["stopped"] = True
                record["expected"] = inventory(session.directory)
                self.save()
                if disk_state(session) != record["semantic"]:
                    record["unverified_final"] = backup_session(session, self.directory / (record["side"] + "-" + record["name"] + "-unverified-final-" + uuid.uuid4().hex))
                    self.save()
                    raise Refusal("Final saved snapshot does not match pre-stop API evidence: " + str(session.directory))
            elif running(session) or inventory(session.directory) != record["expected"]:
                raise Refusal("Stopped session changed after backup: " + str(session.directory))
            final_path = self.directory / (record["side"] + "-" + record["name"] + "-final-" + uuid.uuid4().hex)
            record["final"] = backup_session(session, final_path)
            record["expected"] = inventory(session.directory)
            self.save()
        self.save("stopped")

    def check_stopped(self):
        for record in self.journal["sessions"]:
            session = self.session(record)
            if running(session):
                untouched = self.journal["phase"] == "rolling-back" and record["running"] and not record["stopped"] and (not record.get("stop_requested") or record.get("leave_running"))
                allowed_identity = record.get("left_identity", record.get("identity"))
                if not untouched or server_identity(session) != allowed_identity:
                    raise Refusal("Server appeared during transfer: " + record["directory"])
            if self.journal["phase"] != "rolling-back" and record["side"] == "source" and inventory(Path(record["directory"])) != record["expected"]:
                raise Refusal("Stopped source changed since backup: " + record["directory"])

    def make_operations(self):
        if self.journal["operations"]:
            return
        self.verify_all_backups()
        self.check_stopped()
        records = {(item["side"], item["name"]): item for item in self.journal["sessions"]}
        for source_name, target_name in self.journal["mapping"]:
            incoming = records[("source", source_name)]
            destination = records[("target", target_name)]
            if inventory(Path(destination["directory"])) != destination["expected"]:
                raise Refusal("Target changed before install: " + destination["directory"])
            manifest = verify_backup(Path(incoming["final"]))
            for name in DURABLE:
                old = {path: item for path, item in destination["expected"].items() if path == name or path.startswith(name + "/")}
                new = {path: item for path, item in manifest["entries"].items() if path == name or path.startswith(name + "/")}
                if old == new:
                    continue
                self.journal["operations"].append({"destination": str(Path(destination["directory"]) / name),
                    "backup": incoming["final"], "name": name, "old": old, "new": new,
                    "stage": str(Path(destination["directory"]) / (".switch-" + self.directory.name + "-" + name)),
                    "displaced": str(Path(destination["directory"]) / (".switch-original-" + self.directory.name + "-" + name)),
                    "done": False})
        self.save("installing")

    def install_operation(self, operation):
        destination = checked_path(operation["destination"])
        name = operation["name"]
        current = {path: item for path, item in inventory(destination.parent).items() if path == name or path.startswith(name + "/")}
        displaced = checked_path(operation["displaced"])
        stage = checked_path(operation["stage"])
        if operation["done"]:
            if current != operation["new"]:
                raise Refusal("Installed data changed: " + str(destination))
            return
        # A crash may occur between rename and the following journal write.
        if current == operation["new"] and (not operation["old"] or displaced.exists()):
            operation["done"] = True
            self.save()
            return
        if current != operation["old"] and not (not current and displaced.exists()):
            raise Refusal("Destination diverged from journal: " + str(destination))
        destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        if stage.exists():
            if root_inventory(stage, name) != operation["new"]:
                raise Refusal("Interrupted staging does not match journal: " + str(stage))
        elif operation["new"]:
            stage_parent = destination.parent / (".switch-stage-" + uuid.uuid4().hex)
            stage_parent.mkdir(mode=0o700)
            copy_inventory(Path(operation["backup"]) / "data", stage_parent, operation["new"])
            staged = stage_parent / name
            if inventory(stage_parent) != operation["new"]:
                raise Refusal("Staged data verification failed: " + str(stage_parent))
            os.replace(staged, stage)
            stage_parent.rmdir()
            fsync_dir(destination.parent)
        self.check_stopped()
        for record in self.journal["sessions"]:
            if Path(record["directory"]) == destination.parent and running(self.session(record)):
                raise Refusal("Destination server is running: " + str(destination.parent))
        if root_inventory(destination, name) != current:
            raise Refusal("Destination changed while staging: " + str(destination))
        if destination.exists():
            if displaced.exists():
                raise Refusal("Displaced data already exists: " + str(displaced))
            os.replace(destination, displaced)
            fsync_dir(destination.parent)
        if operation["new"]:
            self.check_stopped()
            checked_path(destination)
            if destination.exists():
                raise Refusal("Destination reappeared before install: " + str(destination))
            os.replace(stage, destination)
            fsync_dir(destination.parent)
        operation["done"] = True
        self.save()

    def start_session(self, session, expected, entry=None):
        if not running(session):
            log = self.directory / ("start-" + session.context.label + "-" + session.name + "-" + uuid.uuid4().hex + ".log")
            with log.open("xb") as stream:
                os.chmod(log, 0o600)
                process = subprocess.Popen(session.command("server"), env=session.context.environment(),
                                           stdin=subprocess.DEVNULL, stdout=stream, stderr=stream, start_new_session=True)
            if entry is not None:
                deadline = time.monotonic() + 2
                while time.monotonic() < deadline:
                    identity = process_info(process.pid)
                    if identity:
                        entry["identity"] = identity
                        self.save()
                        break
                    if process.poll() is not None:
                        break
                    time.sleep(0.01)
        deadline = time.monotonic() + TIMEOUT
        while True:
            try:
                if running(session):
                    identity = server_identity(session)
                    _, actual = live_state(session)
                    if actual == expected:
                        return identity
            except (OSError, ValueError, Refusal):
                pass
            if time.monotonic() >= deadline:
                raise Refusal("Restart did not restore expected topology/native IDs/cwd: " + str(session.directory))
            time.sleep(0.2)

    def restart_targets(self):
        self.save("starting")
        records = {(item["side"], item["name"]): item for item in self.journal["sessions"]}
        for source_name, target_name in self.journal["mapping"]:
            source = records[("source", source_name)]
            if not source["running"]:
                continue
            session = self.target.session(target_name)
            entry = next((item for item in self.journal["restarts"] if item["name"] == target_name), None)
            if entry is None:
                entry = {"name": target_name, "expected": source["semantic"]}
                self.journal["restarts"].append(entry)
                self.save()
            if "identity" in entry and running(session) and server_identity(session) != entry["identity"]:
                raise Refusal("Restarted server identity changed")
            entry["identity"] = self.start_session(session, entry["expected"], entry)
            entry["verified"] = True
            entry["current"] = inventory(session.directory)
            self.save()
        self.save("complete")

    def run(self):
        self.prepare()
        self.continue_run()

    def continue_run(self):
        phase = self.journal["phase"]
        if phase in ("complete", "rolled-back"):
            return
        if phase == "preparing":
            raise Refusal("Preparation was interrupted; no session was stopped. Start a new transaction.")
        if phase == "rolling-back":
            self.rollback()
            return
        if phase in ("ready", "stopping"):
            self.stop_all()
        try:
            if self.journal["phase"] in ("stopped", "installing"):
                self.make_operations()
                self.check_stopped()
                for operation in self.journal["operations"]:
                    self.install_operation(operation)
            self.restart_targets()
        except Exception:
            # Keep all pre/post-stop copies, including the original inbox metadata.
            self.rollback()
            raise

    def rollback(self):
        if self.journal["phase"] == "rolled-back":
            return
        self.verify_all_backups()
        self.save("rolling-back")
        for entry in self.journal["restarts"]:
            if entry.get("rollback_stopped"):
                continue
            session = self.target.session(entry["name"])
            identity = entry.get("identity")
            if identity and process_info(identity["pid"]) == identity and not running(session):
                # A closing or not-yet-bound socket does not prove the writer exited.
                wait_exit(identity)
            if running(session):
                identity = server_identity(session)
                if identity != entry.get("identity"):
                    raise Refusal("Rollback refused a replacement server")
                _, live = live_state(session)
                if entry.get("verified") and live != entry["expected"]:
                    raise Refusal("Restarted session changed; rollback would lose changes")
                # Save mutations made by our own restart before restoring original target data.
                backup_session(session, self.directory / ("rollback-restarted-" + entry["name"] + "-" + uuid.uuid4().hex))
                stop(session, identity)
                entry["current"] = inventory(session.directory)
                entry["rollback_stopped"] = True
                self.save()
        for record in self.journal["sessions"]:
            session = self.session(record)
            restoring = record.get("restoring", {}).get("identity")
            if restoring and process_info(restoring["pid"]) == restoring and not running(session):
                wait_exit(restoring)
            if record.get("stop_requested") and not record["stopped"]:
                if running(session):
                    identity = server_identity(session)
                    _, live = live_state(session)
                    if identity != record["identity"] or live != record["semantic"]:
                        record["leave_running"] = True
                        record["left_identity"] = identity
                        self.save()
                        continue
                    # The prior process may have crashed after journalling intent, before send.
                    stop(session, identity)
                wait_exit(record["identity"])
                if running(session):
                    raise Refusal("Replacement server appeared after original stop request")
                record["stopped"] = True
                record["expected"] = inventory(session.directory)
                backup = backup_session(session, self.directory / (record["side"] + "-" + record["name"] + "-recovered-final-" + uuid.uuid4().hex))
                try:
                    valid = disk_state(session) == record["semantic"]
                except Refusal:
                    valid = False
                record["final" if valid else "unverified_final"] = backup
                self.save()
            if running(session):
                if record.get("leave_running") and server_identity(session) == record["left_identity"]:
                    continue
                restored = record.get("restored_identity") or record.get("restoring", {}).get("identity")
                if restored and server_identity(session) == restored:
                    continue
                if record["running"] and not record["stopped"] and not record.get("stop_requested") and server_identity(session) == record["identity"]:
                    continue
                raise Refusal("Rollback requires stopped original servers: " + str(session.endpoint))
        # Journal restoration operations before changing any target file.
        rollback_operations = self.journal.get("rollback_operations")
        if rollback_operations is not None:
            for operation in rollback_operations:
                restored = next((record for record in self.journal["sessions"]
                                 if Path(record["directory"]) == Path(operation["destination"]).parent
                                 and (record.get("restored_identity") or record.get("restoring", {}).get("identity"))), None)
                if restored and running(self.session(restored)):
                    identity = restored.get("restored_identity") or restored["restoring"]["identity"]
                    if server_identity(self.session(restored)) != identity or live_state(self.session(restored))[1] != restored["semantic"]:
                        raise Refusal("Restored original changed after restart")
                    continue
                self.install_operation(operation)
        pending_operations = []
        for record in self.journal["sessions"]:
            if record["side"] != "target" and not (record["stopped"] and not record.get("final")):
                continue
            if running(self.session(record)):
                continue
            if rollback_operations is not None:
                continue
            session = self.session(record)
            original = Path(record.get("final", record["pre"]))
            if record["stopped"] and not record.get("final") and not record.get("unverified_final"):
                record["unverified_final"] = backup_session(session, self.directory / (record["side"] + "-" + record["name"] + "-unverified-final-" + uuid.uuid4().hex))
                self.save()
            expected = record["expected"]
            current = inventory(session.directory)
            restart = next((item for item in self.journal["restarts"] if item["name"] == session.name), None)
            if restart and "current" in restart:
                expected = restart["current"]
            else:
                expected = dict(expected)
                for operation in self.journal["operations"]:
                    if Path(operation["destination"]).parent != session.directory:
                        continue
                    root = operation["name"]
                    actual_root = {path: item for path, item in current.items() if path == root or path.startswith(root + "/")}
                    if actual_root not in (operation["old"], operation["new"], {} if Path(operation["displaced"]).exists() else operation["old"]):
                        raise Refusal("Rollback found external modifications: " + str(session.directory / root))
                    expected = {path: item for path, item in expected.items() if path != root and not path.startswith(root + "/")}
                    expected.update(actual_root)
            if current != expected:
                raise Refusal("Rollback found external modifications: " + str(session.directory))
            manifest = verify_backup(original)
            for name in DURABLE:
                before = {path: item for path, item in current.items() if path == name or path.startswith(name + "/")}
                after = {path: item for path, item in manifest["entries"].items() if path == name or path.startswith(name + "/")}
                if before == after:
                    continue
                operation = {"destination": str(session.directory / name), "backup": str(original), "name": name,
                             "old": before, "new": after,
                             "stage": str(session.directory / (".rollback-stage-" + uuid.uuid4().hex)),
                             "displaced": str(session.directory / (".rollback-preserved-" + uuid.uuid4().hex)), "done": False}
                pending_operations.append(operation)
        if rollback_operations is None:
            self.journal["rollback_operations"] = pending_operations
            self.save()
            for operation in self.journal["rollback_operations"]:
                self.install_operation(operation)
        for record in self.journal["sessions"]:
            if not record["running"]:
                continue
            session = self.session(record)
            if running(session):
                identity = server_identity(session)
                if record.get("leave_running") and identity == record["left_identity"]:
                    continue
                if ((record.get("restored_identity") or record.get("restoring", {}).get("identity")) == identity or
                    (not record["stopped"] and not record.get("stop_requested") and record["identity"] == identity)):
                    continue
                raise Refusal("Original server identity changed during rollback")
            if disk_state(session) != record["semantic"]:
                raise Refusal("Original saved snapshot is uncertain; restore verified backup manually: " + record["pre"])
            startup = record.setdefault("restoring", {"expected": record["semantic"]})
            self.save()
            record["restored_identity"] = self.start_session(session, record["semantic"], startup)
            self.save()
        self.save("rolled-back")


def defaults():
    user_root = Path.home()
    return (
        Context("stable", user_root / ".local/bin/herdr", user_root / ".config/herdr", user_root / ".local/state/herdr"),
        Context("inbox", user_root / ".local/share/herdr-inbox/bin/herdr", user_root / ".config/herdr-inbox/herdr", user_root / ".local/state/herdr-inbox/herdr"),
        user_root / ".local/state/herdr-inbox-switch/backups",
    )


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("direction", choices=("to-inbox", "to-stable"), nargs="?")
    recovery = parser.add_mutually_exclusive_group()
    recovery.add_argument("--resume", type=Path, metavar="TRANSACTION")
    recovery.add_argument("--rollback", type=Path, metavar="TRANSACTION")
    parser.add_argument("--apply", action="store_true", help="Perform shutdown and switch. Default is dry run.")
    parser.add_argument("--yes", action="store_true", help="Explicitly confirm shutdown without an interactive prompt.")
    args = parser.parse_args(argv)
    if fcntl is None:
        raise Refusal("This switcher requires macOS or Linux")
    stable, inbox, backup_root = defaults()
    transaction = args.resume or args.rollback
    if not args.direction and not transaction:
        parser.error("a direction or --resume/--rollback is required")
    if args.yes and not args.apply:
        parser.error("--yes requires --apply")
    if args.apply and os.environ.get("HERDR_ENV") == "1":
        raise Refusal("Run --apply from a separate terminal outside Herdr. This agent's session would be stopped.")
    source, target = (stable, inbox) if args.direction != "to-stable" else (inbox, stable)
    if transaction:
        transaction = checked_path(transaction)
        if transaction.parent != backup_root:
            raise Refusal("Recovery transaction must be directly inside " + str(backup_root))
        journal = read_json(transaction / "journal.json")
        contexts = (Context.from_json(journal["source"]), Context.from_json(journal["target"]))
        if {json.dumps(context.as_json(), sort_keys=True) for context in contexts} != {json.dumps(context.as_json(), sort_keys=True) for context in (stable, inbox)}:
            raise Refusal("Recovery journal does not match installed stable/inbox paths")
        source, target = contexts
    switch = Switch(source, target, backup_root, transaction)
    print("Local Unix sessions only; remote endpoints stay unchanged. Do not launch either app while switching.")
    print("Shutdown ends shell jobs and pane processes. Agent histories remain external/shared; agents may resume automatically.")
    print("Both profiles stop; matching destination sessions are replaced after backup. Destination-only sessions stay saved but stopped.")
    if transaction:
        print("Transaction:", transaction, "phase:", switch.journal["phase"])
    else:
        validate_context(source)
        validate_context(target)
        mapping, targets = plan(source, target)
        for src, dst in mapping:
            print("%s %s [%s] -> %s %s" % (source.label, src.name, "running" if running(src) else "stopped", target.label, dst.name))
        print("Target sessions backed up before overwrite:", ", ".join(session.name for session in targets))
    if not args.apply:
        print("Dry run. No servers stopped or session files changed. Use --apply from a separate terminal.")
        return 0
    if not args.yes:
        expected = "rollback" if args.rollback else "resume" if args.resume else args.direction
        if not sys.stdin.isatty() or input("Type %s to stop sessions and continue: " % expected) != expected:
            raise Refusal("Shutdown was not confirmed")
    backup_root.mkdir(parents=True, exist_ok=True, mode=0o700)
    checked_path(backup_root)
    os.chmod(backup_root, 0o700)
    fsync_dir(backup_root.parent)
    lock_path = backup_root.parent / "switch.lock"
    checked_path(lock_path)
    with lock_path.open("a") as lock:
        os.chmod(lock_path, 0o600)
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise Refusal("Another switch is already running") from error
        if not transaction:
            pending = []
            for directory in backup_root.iterdir():
                if directory.is_dir() and (directory / "journal.json").exists():
                    if read_json(directory / "journal.json")["phase"] not in ("complete", "rolled-back", "preparing"):
                        pending.append(str(directory))
            if pending:
                raise Refusal("Unfinished transaction; use --resume or --rollback: " + ", ".join(pending))
        try:
            if args.rollback:
                switch.rollback()
            elif args.resume:
                switch.continue_run()
            else:
                switch.run()
        except BaseException:
            if switch.directory and switch.journal:
                print("Backups/journal:", switch.directory, file=sys.stderr)
                stopped = [record["side"] + "/" + record["name"] for record in switch.journal["sessions"] if record["stopped"]]
                print("Stopped original sessions:", ", ".join(stopped) or "none", file=sys.stderr)
                print("Recovery:", shlex.join([sys.executable, str(Path(__file__).absolute()), "--rollback", str(switch.directory), "--apply"]), file=sys.stderr)
            raise
    print("Transaction", switch.journal["phase"] + ":", switch.directory)
    if switch.journal["phase"] == "complete":
        for _, name in switch.journal["mapping"]:
            print("Attach:", attach_command(target.session(name)))
        canonical = "agent-inbox" if target.label == "inbox" else "default"
        originally_running = {record["name"] for record in switch.journal["sessions"] if record["side"] == "source" and record["running"]}
        attach_names = [name for source_name, name in switch.journal["mapping"] if source_name in originally_running]
        if sys.stdin.isatty() and sys.stdout.isatty() and attach_names:
            session = target.session(canonical if canonical in attach_names else attach_names[0])
            sys.stdout.flush()
            os.execve(str(target.binary), session.command(), target.environment())
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (Refusal, OSError, ValueError) as error:
        print("Refused:", error, file=sys.stderr)
        sys.exit(1)
