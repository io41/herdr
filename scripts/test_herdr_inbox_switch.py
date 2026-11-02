"""Switcher checks use temporary data and mocked servers, never user sessions."""

import contextlib
import copy
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts import herdr_inbox_switch as switch


def snapshot(cwd="/tmp/project", value="native-session-id", label="work"):
    return {
        "version": 3, "active": 0, "selected": 0,
        "workspaces": [{
            "id": "w1", "location_id": "untouched-location-id", "tab_location_ids": ["untouched-tab-id"],
            "custom_name": label, "identity_cwd": cwd, "active_tab": 0,
            "public_pane_numbers": {"42": 1, "87": 2}, "public_tab_numbers": [1],
            "tabs": [{"custom_name": "agents", "zoomed": False, "focused": 42, "root_pane": 42,
                      "layout": {"Split": {"direction": "Horizontal", "ratio": 0.4,
                                           "first": {"Pane": 42}, "second": {"Pane": 87}}},
                      "panes": {
                          "42": {"cwd": cwd, "label": "primary", "launch_argv": ["codex", "resume", value],
                                 "agent_session": {"source": "codex", "agent": "codex", "kind": "id", "value": value}},
                          "87": {"cwd": cwd, "agent_session": {"source": "pi", "agent": "pi", "kind": "path", "value": "/tmp/native/session.jsonl"}},
                      }}],
        }],
    }


def api_snapshot(document):
    document = copy.deepcopy(document)
    workspace = document["workspaces"][0]
    tab = workspace["tabs"][0]
    panes = []
    for raw, pid in (("42", "w1:p1"), ("87", "w1:p2")):
        pane = tab["panes"][raw]
        panes.append({"pane_id": pid, "workspace_id": "w1", "tab_id": "w1:t1",
                      "cwd": pane["cwd"], "label": pane.get("label"),
                      "agent_session": pane.get("agent_session"), "revision": 35,
                      "terminal_id": "runtime-terminal-" + raw})
    return {
        "version": "0.9.3", "protocol": 1, "focused_workspace_id": "w1",
        "workspaces": [{"workspace_id": "w1", "tab_count": 1, "pane_count": 2, "active_tab_id": "w1:t1"}],
        "tabs": [{"workspace_id": "w1", "tab_id": "w1:t1", "number": 1, "pane_count": 2}],
        "panes": panes,
        "layouts": [{"workspace_id": "w1", "tab_id": "w1:t1", "zoomed": False,
                     "focused_pane_id": "w1:p1", "panes": [{"pane_id": "w1:p1"}, {"pane_id": "w1:p2"}],
                     "splits": [{"id": "split_0_root", "direction": "right", "ratio": 0.4}]}],
    }


@unittest.skipIf(switch.fcntl is None, "local Unix switcher")
class SwitchTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.stable = self.context("stable")
        self.inbox = self.context("inbox")
        self.backups = self.root / "backups"
        self.runtime = {}
        self.stops = []
        self.current_switch = None
        self.expected_backup_count = None
        self.real_wait_exit = switch.wait_exit
        self.patch("running", self.running)
        self.patch("server_identity", self.identity)
        self.patch("live_state", self.live_state)
        self.patch("stop", self.stop)
        self.patch("wait_exit", lambda identity, timeout=switch.TIMEOUT: None)
        self.patch("process_info", lambda pid: None)
        self.patch("start_session", self.start_session, owner=switch.Switch)

    def patch(self, name, value, owner=switch):
        patcher = mock.patch.object(owner, name, value)
        patcher.start()
        self.addCleanup(patcher.stop)

    def context(self, label):
        binary = self.root / label / "bin/herdr"
        binary.parent.mkdir(parents=True)
        binary.write_text("#!/bin/sh\nexit 1\n")
        binary.chmod(0o700)
        context = switch.Context(label, binary, self.root / label / "config/herdr", self.root / label / "state/herdr")
        context.config.mkdir(parents=True)
        if label == "inbox":
            (context.config / "config.toml").write_text("[update]\nversion_check = false\nmanifest_check = false\n")
        return context

    def write(self, context, name="default", document=None, running=False):
        document = snapshot() if document is None else document
        session = context.session(name)
        session.directory.mkdir(parents=True, exist_ok=True)
        (session.directory / "session.json").write_text(json.dumps(document))
        (session.directory / "session.json").chmod(0o640)
        (session.directory / "session-history.json").write_text('{"history": "screen contents"}')
        (session.directory / "agent-conversations.json").write_text('{"location_id": "untouched-location-id", "records": []}')
        (session.directory / "agent-conversations.json.bak").write_text('{"previous": "registry"}')
        for name in switch.TREES:
            (session.directory / name).mkdir(exist_ok=True)
            (session.directory / name / "saved.json").write_text(json.dumps(document))
        if running:
            self.runtime[(context.label, session.name)] = api_snapshot(document)
        return session

    def running(self, session):
        return (session.context.label, session.name) in self.runtime

    def identity(self, session):
        return {"pid": 1234, "uid": os.getuid(), "started": "fixture", "command": str(session.context.binary) + " server"}

    def live_state(self, session):
        document = copy.deepcopy(self.runtime[(session.context.label, session.name)])
        return document, switch.semantic_live(document)

    def stop(self, session, identity):
        if self.current_switch:
            journal = self.current_switch.journal
            if self.expected_backup_count is not None:
                self.assertEqual(len(journal["sessions"]), self.expected_backup_count)
            for record in journal["sessions"]:
                switch.verify_backup(Path(record["pre"]))
        self.stops.append((session.context.label, session.name))
        self.runtime.pop((session.context.label, session.name))

    def start_session(self, session, expected, entry=None):
        self.assertEqual(switch.disk_state(session), expected)
        self.runtime[(session.context.label, session.name)] = api_snapshot(switch.read_json(session.directory / "session.json"))
        if entry is not None:
            entry["identity"] = self.identity(session)
            if self.current_switch is not None:
                self.current_switch.save()
        return self.identity(session)

    def create_switch(self, source=None, target=None, transaction=None):
        self.current_switch = switch.Switch(source or self.stable, target or self.inbox, self.backups, transaction)
        return self.current_switch

    def test_live_disk_semantics_include_layout_cwd_and_native_refs(self):
        document = snapshot()
        self.assertEqual(switch.semantic_disk(document), switch.semantic_live(api_snapshot(document)))
        original = switch.semantic_disk(document)
        changed = api_snapshot(document)
        changed["panes"][0]["agent_session"]["value"] = "other-session"
        self.assertNotEqual(original, switch.semantic_live(changed))
        changed = api_snapshot(document)
        changed["layouts"][0]["splits"][0]["direction"] = "down"
        self.assertNotEqual(original, switch.semantic_live(changed))

    def test_roundtrip_preserves_native_ids_absolute_paths_and_unknown_metadata(self):
        source = self.write(self.stable, running=True)
        named = self.write(self.stable, "other", snapshot(value="second-id"))
        original = switch.inventory(source.directory)
        named_original = switch.inventory(named.directory)
        target = self.write(self.inbox, "agent-inbox", snapshot(value="preexisting-test-id"), running=True)
        target_original = switch.inventory(target.directory)
        self.expected_backup_count = 4
        forward = self.create_switch()
        forward.run()
        self.assertEqual(forward.journal["phase"], "complete")
        self.assertEqual(switch.inventory(target.directory), original)
        target_record = next(record for record in forward.journal["sessions"] if record["side"] == "target" and record["name"] == "agent-inbox")
        self.assertEqual(switch.verify_backup(Path(target_record["pre"]))["entries"], target_original)
        self.assertFalse(self.running(source))
        self.assertTrue(self.running(target))
        reverse = self.create_switch(self.inbox, self.stable)
        reverse.run()
        self.assertEqual(switch.inventory(source.directory), original)
        self.assertEqual(switch.inventory(named.directory), named_original)
        self.assertTrue(self.running(source))
        self.assertFalse(self.running(named))
        self.assertEqual(json.loads((target.directory / "session.json").read_text())["workspaces"][0]["location_id"], "untouched-location-id")

    def test_collision_and_symlink_refused_before_stop(self):
        self.write(self.stable, running=True)
        self.write(self.stable, "agent-inbox")
        with self.assertRaisesRegex(switch.Refusal, "collision"):
            self.create_switch().run()
        self.assertEqual(self.stops, [])
        with tempfile.TemporaryDirectory() as outside:
            (self.stable.config / "session.json").unlink()
            (self.stable.config / "session.json").symlink_to(Path(outside) / "data")
            with self.assertRaisesRegex(switch.Refusal, "Symlink"):
                switch.discover(self.stable)
        self.assertEqual(self.stops, [])

    def test_invalid_later_backup_never_stops_earlier_source(self):
        self.write(self.stable, running=True)
        bad = self.write(self.inbox, "agent-inbox")
        (bad.directory / "session-history.json").unlink()
        (bad.directory / "session-history.json").symlink_to("session.json")
        with self.assertRaisesRegex(switch.Refusal, "Symlink"):
            self.create_switch().run()
        self.assertEqual(self.stops, [])

    def test_backup_tamper_prevents_stop_and_modes_are_private(self):
        self.write(self.stable, running=True)
        transaction = self.create_switch()
        transaction.prepare()
        directory = Path(transaction.journal["sessions"][0]["pre"])
        self.assertEqual(directory.stat().st_mode & 0o077, 0)
        self.assertEqual((directory / "data/session.json").stat().st_mode & 0o077, 0)
        (directory / "data/session.json").write_text("tampered")
        with self.assertRaisesRegex(switch.Refusal, "verification"):
            transaction.continue_run()
        self.assertEqual(self.stops, [])

    def test_stale_or_invalid_disk_snapshot_refused_before_stop(self):
        session = self.write(self.stable, running=True)
        document = snapshot(value="stale-id")
        (session.directory / "session.json").write_text(json.dumps(document))
        with self.assertRaisesRegex(switch.Refusal, "does not match"):
            switch.matched_state(session, timeout=0)
        (session.directory / "session.json").write_text("{broken")
        with self.assertRaisesRegex(switch.Refusal, "Invalid persisted"):
            switch.matched_state(session, timeout=0)
        self.assertEqual(self.stops, [])

    def test_stop_waits_for_process_exit_even_if_socket_disappears(self):
        with mock.patch.object(switch, "process_info", side_effect=[{"pid": 7}, {"pid": 7}, None]), mock.patch.object(switch.time, "sleep") as waited:
            self.real_wait_exit({"pid": 7}, timeout=1)
            self.assertEqual(waited.call_count, 2)

    def test_partial_stop_resume_reloads_semantics_without_tuple_drift(self):
        source = self.write(self.stable, running=True)
        transaction = self.create_switch()
        transaction.prepare()
        transaction.stop_all()
        self.assertFalse(self.running(source))
        resumed = self.create_switch(transaction=transaction.directory)
        resumed.continue_run()
        self.assertEqual(resumed.journal["phase"], "complete")
        self.assertTrue(self.running(self.inbox.session("agent-inbox")))

    def test_interrupted_atomic_install_resumes_after_original_rename(self):
        self.write(self.stable)
        target = self.write(self.inbox, "agent-inbox", snapshot(value="target-original"))
        transaction = self.create_switch()
        transaction.prepare()
        transaction.stop_all()
        transaction.make_operations()
        real_replace = switch.os.replace

        def interrupt(source, destination):
            if Path(source) == target.directory / "session.json":
                real_replace(source, destination)
                raise KeyboardInterrupt()
            return real_replace(source, destination)

        with mock.patch.object(switch.os, "replace", side_effect=interrupt):
            with self.assertRaises(KeyboardInterrupt):
                transaction.install_operation(transaction.journal["operations"][0])
        resumed = self.create_switch(transaction=transaction.directory)
        resumed.continue_run()
        self.assertEqual(resumed.journal["phase"], "complete")
        self.assertEqual(switch.disk_state(target), switch.disk_state(self.stable.session("default")))

    def test_install_failure_rolls_back_target_and_restarts_original_sources(self):
        source = self.write(self.stable, running=True)
        target = self.write(self.inbox, "agent-inbox", snapshot(value="original-inbox"), running=True)
        original = switch.inventory(target.directory)
        transaction = self.create_switch()
        real_install = transaction.install_operation
        failed = False

        def fail_once(operation):
            nonlocal failed
            if not failed:
                failed = True
                real_install(operation)
                raise OSError("fixture failed install")
            return real_install(operation)

        with mock.patch.object(transaction, "install_operation", side_effect=fail_once):
            with self.assertRaisesRegex(OSError, "fixture failed"):
                transaction.run()
        self.assertEqual(transaction.journal["phase"], "rolled-back")
        self.assertEqual(switch.inventory(target.directory), original)
        self.assertTrue(self.running(source))
        self.assertTrue(self.running(target))

    def test_partial_stop_rollback_leaves_untouched_target_running(self):
        source = self.write(self.stable, running=True)
        target = self.write(self.inbox, "agent-inbox", snapshot(value="target"), running=True)
        transaction = self.create_switch()
        transaction.prepare()
        original_stop = self.stop

        def interrupt(session, identity):
            if session.context.label == "inbox":
                raise KeyboardInterrupt()
            original_stop(session, identity)

        with mock.patch.object(switch, "stop", side_effect=interrupt):
            with self.assertRaises(KeyboardInterrupt):
                transaction.stop_all()
        resumed = self.create_switch(transaction=transaction.directory)
        resumed.rollback()
        self.assertTrue(self.running(source))
        self.assertTrue(self.running(target))
        self.assertEqual(resumed.journal["phase"], "rolled-back")

    def test_external_modification_refuses_resume_and_preserves_backups(self):
        self.write(self.stable)
        target = self.write(self.inbox, "agent-inbox", snapshot(value="original"))
        transaction = self.create_switch()
        transaction.prepare()
        transaction.stop_all()
        transaction.make_operations()
        (target.directory / "session.json").write_text("external modification")
        with self.assertRaisesRegex(switch.Refusal, "external modifications"):
            transaction.continue_run()
        self.assertEqual((target.directory / "session.json").read_text(), "external modification")
        transaction.verify_all_backups()

    def test_cli_dry_run_writes_nothing_and_herdr_apply_is_refused(self):
        self.write(self.stable, running=True)
        with mock.patch.object(switch, "defaults", return_value=(self.stable, self.inbox, self.backups)):
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(switch.main(["to-inbox"]), 0)
            self.assertFalse(self.backups.exists())
            with mock.patch.dict(os.environ, {"HERDR_ENV": "1"}):
                with self.assertRaisesRegex(switch.Refusal, "separate terminal"):
                    switch.main(["to-inbox", "--apply", "--yes"])
        self.assertEqual(self.stops, [])

    def test_environment_clears_inherited_socket_and_config_overrides(self):
        with mock.patch.dict(os.environ, {"HERDR_ENV": "1", "HERDR_SOCKET_PATH": "/tmp/wrong.sock",
                                         "HERDR_SESSION": "wrong", "HERDR_CONFIG_PATH": "/tmp/wrong.toml"}):
            environment = self.inbox.environment()
        self.assertNotIn("HERDR_ENV", environment)
        self.assertNotIn("HERDR_SESSION", environment)
        self.assertNotIn("HERDR_SOCKET_PATH", environment)
        self.assertNotIn("HERDR_CONFIG_PATH", environment)
        self.assertEqual(environment["HERDR_DISABLE_UPDATES"], "1")
        self.assertEqual(environment["XDG_CONFIG_HOME"], str(self.inbox.config.parent))

    def test_concurrent_target_edit_during_staging_is_never_overwritten(self):
        self.write(self.stable)
        target = self.write(self.inbox, "agent-inbox", snapshot(value="old-target"))
        transaction = self.create_switch()
        transaction.prepare()
        transaction.stop_all()
        transaction.make_operations()
        original_copy = switch.copy_inventory

        def concurrent_edit(*args, **kwargs):
            original_copy(*args, **kwargs)
            (target.directory / "session.json").write_text("concurrent-user-change")

        with mock.patch.object(switch, "copy_inventory", side_effect=concurrent_edit):
            with self.assertRaisesRegex(switch.Refusal, "changed while staging"):
                transaction.install_operation(transaction.journal["operations"][0])
        self.assertEqual((target.directory / "session.json").read_text(), "concurrent-user-change")

    def test_server_launch_during_staging_is_refused(self):
        self.write(self.stable)
        target = self.write(self.inbox, "agent-inbox", snapshot(value="old-target"))
        original = switch.inventory(target.directory)
        transaction = self.create_switch()
        transaction.prepare()
        transaction.stop_all()
        transaction.make_operations()
        original_copy = switch.copy_inventory

        def concurrent_start(*args, **kwargs):
            original_copy(*args, **kwargs)
            self.runtime[("inbox", "agent-inbox")] = api_snapshot(snapshot(value="old-target"))

        with mock.patch.object(switch, "copy_inventory", side_effect=concurrent_start):
            with self.assertRaisesRegex(switch.Refusal, "Server appeared"):
                transaction.install_operation(transaction.journal["operations"][0])
        self.assertEqual(switch.inventory(target.directory), original)

    def test_bad_final_save_aborts_before_target_edit_and_rollback_uses_verified_pre(self):
        source = self.write(self.stable, running=True)
        target = self.write(self.inbox, "agent-inbox", snapshot(value="old-target"))
        target_original = switch.inventory(target.directory)
        source_original = switch.inventory(source.directory)
        transaction = self.create_switch()
        original_stop = self.stop

        def bad_save(session, identity):
            original_stop(session, identity)
            (session.directory / "session.json").write_text(json.dumps(snapshot(value="stale-final-native-id")))

        with mock.patch.object(switch, "stop", side_effect=bad_save):
            with self.assertRaisesRegex(switch.Refusal, "Final saved snapshot"):
                transaction.run()
        self.assertEqual(switch.inventory(target.directory), target_original)
        resumed = self.create_switch(transaction=transaction.directory)
        resumed.rollback()
        self.assertEqual(switch.inventory(source.directory), source_original)
        self.assertTrue(self.running(source))

    def test_failed_restart_rolls_back_both_contexts(self):
        source = self.write(self.stable, running=True)
        target = self.write(self.inbox, "agent-inbox", snapshot(value="old-target"), running=True)
        original = switch.inventory(target.directory)
        transaction = self.create_switch()
        first = True

        def failed_start(session, expected, entry=None):
            nonlocal first
            if first:
                first = False
                self.runtime[(session.context.label, session.name)] = api_snapshot(snapshot(value="failed-restored-native-id"))
                entry["identity"] = self.identity(session)
                transaction.save()
                raise switch.Refusal("fixture restart failed")
            return self.start_session(session, expected, entry)

        with mock.patch.object(transaction, "start_session", side_effect=failed_start):
            with self.assertRaisesRegex(switch.Refusal, "fixture restart failed"):
                transaction.run()
        self.assertEqual(transaction.journal["phase"], "rolled-back")
        self.assertEqual(switch.inventory(target.directory), original)
        self.assertTrue(self.running(source))
        self.assertTrue(self.running(target))

    def test_rollback_resume_after_original_restart_accepts_normal_reserialization(self):
        self.write(self.stable, running=True)
        target = self.write(self.inbox, "agent-inbox", snapshot(value="old-target"), running=True)
        transaction = self.create_switch()
        transaction.run()
        original_start = transaction.start_session

        def interrupt_after_start(session, expected, entry=None):
            identity = original_start(session, expected, entry)
            if session.context.label == "inbox":
                document = switch.read_json(session.directory / "session.json")
                # Stable/new writer serialization can change bytes without changing topology.
                (session.directory / "session.json").write_text(json.dumps(document, indent=4))
                raise KeyboardInterrupt()
            return identity

        with mock.patch.object(transaction, "start_session", side_effect=interrupt_after_start):
            with self.assertRaises(KeyboardInterrupt):
                transaction.rollback()
        resumed = self.create_switch(transaction=transaction.directory)
        resumed.rollback()
        self.assertEqual(resumed.journal["phase"], "rolled-back")
        self.assertTrue(self.running(target))

    def test_recovery_rejects_journal_destination_escape_before_mutation(self):
        self.write(self.stable)
        transaction = self.create_switch()
        transaction.prepare()
        transaction.stop_all()
        transaction.make_operations()
        transaction.journal["operations"][0]["destination"] = str(self.root / "outside/session.json")
        transaction.save()
        with self.assertRaisesRegex(switch.Refusal, "outside journalled"):
            self.create_switch(transaction=transaction.directory)
        self.assertFalse((self.root / "outside").exists())

    def test_manifest_path_escape_is_refused(self):
        session = self.write(self.stable)
        backup = self.root / "private-backup"
        switch.backup_session(session, backup)
        manifest = switch.read_json(backup / "manifest.json")
        manifest["entries"]["../outside"] = manifest["entries"]["session.json"]
        switch.atomic_json(backup / "manifest.json", manifest)
        with self.assertRaisesRegex(switch.Refusal, "Unsafe manifest"):
            switch.verify_backup(backup)

    def test_live_owned_pid_without_socket_blocks_rollback_file_changes(self):
        self.write(self.stable, running=True)
        target = self.write(self.inbox, "agent-inbox", snapshot(value="old-target"))
        transaction = self.create_switch()
        transaction.run()
        current = switch.inventory(target.directory)
        identity = transaction.journal["restarts"][0]["identity"]
        self.runtime.pop(("inbox", "agent-inbox"))
        with mock.patch.object(switch, "process_info", return_value=identity), mock.patch.object(switch, "wait_exit", side_effect=switch.Refusal("fixture PID still alive")):
            with self.assertRaisesRegex(switch.Refusal, "PID still alive"):
                transaction.rollback()
        self.assertEqual(switch.inventory(target.directory), current)

    def test_changed_target_after_stop_intent_is_left_running_during_rollback(self):
        source = self.write(self.stable, running=True)
        target = self.write(self.inbox, "agent-inbox", snapshot(value="old-target"), running=True)
        transaction = self.create_switch()
        transaction.prepare()
        original_stop = self.stop

        def interrupt(session, identity):
            if session.context.label == "inbox":
                self.runtime[("inbox", "agent-inbox")] = api_snapshot(snapshot(value="new-user-native-id"))
                raise KeyboardInterrupt()
            original_stop(session, identity)

        with mock.patch.object(switch, "stop", side_effect=interrupt):
            with self.assertRaises(KeyboardInterrupt):
                transaction.stop_all()
        resumed = self.create_switch(transaction=transaction.directory)
        resumed.rollback()
        self.assertTrue(self.running(source))
        self.assertTrue(self.running(target))
        self.assertNotIn(("inbox", "agent-inbox"), self.stops)
        self.assertEqual(self.runtime[("inbox", "agent-inbox")]["panes"][0]["agent_session"]["value"], "new-user-native-id")

    def test_interactive_apply_never_attaches_originally_stopped_default(self):
        self.write(self.stable)
        output = io.StringIO()
        with mock.patch.object(switch, "defaults", return_value=(self.stable, self.inbox, self.backups)), mock.patch.dict(os.environ, {"HERDR_ENV": ""}), mock.patch.object(switch.sys.stdin, "isatty", return_value=True), mock.patch.object(output, "isatty", return_value=True), mock.patch.object(switch.os, "execve") as execute:
            with contextlib.redirect_stdout(output):
                self.assertEqual(switch.main(["to-inbox", "--apply", "--yes"]), 0)
            execute.assert_not_called()

    def test_repeated_cli_switch_preserves_newer_stopped_target_without_new_transaction(self):
        self.write(self.stable, running=True)
        with mock.patch.object(switch, "defaults", return_value=(self.stable, self.inbox, self.backups)), mock.patch.dict(os.environ, {"HERDR_ENV": ""}), contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(switch.main(["to-inbox", "--apply", "--yes"]), 0)
            target = self.inbox.session("agent-inbox")
            (target.directory / "session.json").write_text(json.dumps(snapshot(value="newer-inbox-native-id")))
            self.runtime.pop(("inbox", "agent-inbox"))
            before = switch.inventory(target.directory)
            transactions = set(self.backups.iterdir())
            stopped = list(self.stops)
            with self.assertRaisesRegex(switch.Refusal, "inbox is already active"):
                switch.main(["to-inbox", "--apply", "--yes"])
            self.assertEqual(switch.inventory(target.directory), before)
            self.assertEqual(set(self.backups.iterdir()), transactions)
            self.assertEqual(self.stops, stopped)
            self.assertEqual(switch.main(["to-stable", "--apply", "--yes"]), 0)
        self.assertEqual(switch.read_json(self.stable.session("default").directory / "session.json")["workspaces"][0]["tabs"][0]["panes"]["42"]["agent_session"]["value"], "newer-inbox-native-id")

    def test_changed_stopped_source_can_start_fresh_transfer(self):
        source = self.write(self.stable)
        first = self.create_switch()
        first.run()
        (source.directory / "session.json").write_text(json.dumps(snapshot(value="reopened-source-native-id")))
        second = self.create_switch()
        second.run()
        self.assertNotEqual(second.directory, first.directory)
        self.assertEqual(switch.disk_state(self.inbox.session("agent-inbox")), switch.disk_state(source))

    def test_rollback_marks_original_source_active_for_direction_guard(self):
        self.write(self.stable)
        self.write(self.inbox, "agent-inbox", snapshot(value="original-inbox"))
        transaction = self.create_switch()
        transaction.run()
        transaction.rollback()
        count = len(list(self.backups.iterdir()))
        with self.assertRaisesRegex(switch.Refusal, "stable is already active"):
            self.create_switch(self.inbox, self.stable).run()
        self.assertEqual(len(list(self.backups.iterdir())), count)
        self.create_switch().run()

    def test_update_opt_out_survives_stable_context_and_inbox_forces_it(self):
        with mock.patch.dict(os.environ, {"HERDR_DISABLE_UPDATES": "1"}):
            self.assertEqual(self.stable.environment()["HERDR_DISABLE_UPDATES"], "1")
        with mock.patch.dict(os.environ, {"HERDR_DISABLE_UPDATES": "0"}):
            self.assertEqual(self.inbox.environment()["HERDR_DISABLE_UPDATES"], "1")


if __name__ == "__main__":
    unittest.main()
