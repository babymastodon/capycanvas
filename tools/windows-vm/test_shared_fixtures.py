from contextlib import contextmanager
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import MagicMock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
spec = importlib.util.spec_from_file_location("windows_vm_shared", Path(__file__).with_name("windows-vm.py"))
vm = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vm)


class FakeGuest:
    def __init__(self, run, seconds=1):
        self.run = run
        self.seconds = seconds
        self.logs = {}
        self.stopped = set()
        self.complete = set()

    def status(self, machine, command, **options):
        rows = {}
        for session in self.run.sessions:
            if session.machine is not machine or session.id not in command:
                continue
            log = self.logs.setdefault(session.id, [])
            if session.id in self.stopped:
                rows[session.id] = {"state": "Ready", "complete": False, "log": "\n".join(log), "error": "worker stopped"}
                continue
            done = session.id in self.complete
            if not done:
                claimed = self.run.queue.assignments[session.id]
                if not self.run.queue.pending() or session.id in self.run.queue.draining:
                    done = True
                    self.complete.add(session.id)
                else:
                    selector = self.run.queue.remaining.popleft()
                    claimed.append(selector)
                    log.append(json.dumps({"name": selector, "exit": 0, "seconds": self.seconds, "message": ""}))
            rows[session.id] = {"state": "Ready" if done else "Running", "complete": done, "log": "\n".join(log), "error": None}
        return SimpleNamespace(stdout=json.dumps(rows))


class SharedRunTests(unittest.TestCase):
    def setUp(self):
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        state = self.root / "state"
        (state / "vms").mkdir(parents=True)
        self.runtime = self.root / "runtime"
        self.runtime.mkdir()
        self.clock = [0.0]

        def advance(seconds):
            self.clock[0] += seconds
            if self.clock[0] > 100000:
                raise AssertionError("The scheduler did not finish")
        for patcher in (patch.multiple(vm, STATE=state, VMS=state / "vms", RUNTIME=self.runtime, SETTINGS={}, ROOT=self.root),
                        patch.object(vm.time, "sleep", side_effect=advance),
                        patch.object(vm.time, "monotonic", side_effect=lambda: self.clock[0]), patch("builtins.print")):
            patcher.start()
            self.addCleanup(patcher.stop)
        self.started = []
        self.alive = set()

    def build(self, selectors, workers=None, durations=None):
        queue_directory = self.root / "queue"
        queue_directory.mkdir(exist_ok=True)
        queue = vm.fixture_shards.Queue(selectors, [], durations or {}, queue_directory / "queue.json")
        args = SimpleNamespace(profile="release", timeout_minutes=20, workers=workers)
        self.demands = []
        run = vm.SharedRun(args, self.root / "evidence", "run-a", queue, self.demands.append, {})
        run.polled = -100
        return run

    def machine_up(self, machine):
        self.started.append(machine.name)
        self.alive.add(machine.name)

    @contextmanager
    def scheduled(self, run, fake, ready):
        def accounts(machine):
            return ready.get(machine.name, []) if machine.name in self.alive else []

        def lifetime(machine):
            return machine.name in self.alive

        class Executor:
            def submit(self, function, *args):
                future = MagicMock()
                try:
                    function(*args)
                    future.exception.return_value = None
                except (Exception, SystemExit) as error:
                    future.exception.return_value = error
                future.done.return_value = True
                return future
        run.executor = Executor()
        with patch.object(vm, "ready_accounts", side_effect=accounts), patch.object(vm, "host_alive", side_effect=lifetime), \
                patch.object(vm, "start_shared_vm", side_effect=self.machine_up), \
                patch.object(run, "start_session"), patch.object(run, "close_session", side_effect=self.closed), \
                patch.object(vm, "guest", side_effect=fake.status), patch.object(vm, "running", return_value=True):
            with patch.object(vm.session_pool.Pool, "runs", return_value=self.demand_rows):
                yield

    def closed(self, session):
        session.lock.close()
        session.closed = True

    def test_a_small_list_starts_one_vm_and_takes_only_the_sessions_it_needs(self):
        self.demand_rows = {"run-a": {"demand": 3}}
        accounts = vm.shared_accounts()
        run = self.build(["a", "b", "c"])
        with run.queue:
            fake = FakeGuest(run)
            with self.scheduled(run, fake, {"shared-gpu-1": accounts, "shared-gpu-2": accounts}):
                run.schedule(run.executor)
        self.assertEqual(self.started, ["shared-gpu-1"])
        self.assertLessEqual(len(run.sessions), 3)
        self.assertEqual(sorted(sum(run.queue.assignments.values(), [])), ["a", "b", "c"])
        rows, errors = vm.fixture_shards.merge(["a", "b", "c"], run.shards())
        self.assertEqual(errors, [])

    def test_a_large_list_brings_up_both_vms_and_runs_each_selector_once(self):
        names = [f"row-{index}" for index in range(40)]
        self.demand_rows = {"run-a": {"demand": 40}}
        run = self.build(names)
        accounts = vm.shared_accounts()
        with run.queue:
            fake = FakeGuest(run)
            with self.scheduled(run, fake, {"shared-gpu-1": accounts, "shared-gpu-2": accounts}):
                run.schedule(run.executor)
        self.assertEqual(self.started, ["shared-gpu-1", "shared-gpu-2"])
        self.assertEqual({session.machine.name for session in run.sessions}, {"shared-gpu-1", "shared-gpu-2"})
        self.assertEqual(sorted(sum(run.queue.assignments.values(), [])), sorted(names))
        _, errors = vm.fixture_shards.merge(names, run.shards())
        self.assertEqual(errors, [])

    def test_a_new_run_makes_a_large_run_yield_sessions_after_their_current_fixture(self):
        names = [f"row-{index}" for index in range(200)]
        self.demand_rows = {"run-a": {"demand": 200}}
        run = self.build(names)
        accounts = vm.shared_accounts()
        with run.queue:
            fake = FakeGuest(run)
            calls = []

            def status(machine, command, **options):
                calls.append(machine.name)
                if len(calls) == 3:
                    self.demand_rows["run-b"] = {"demand": 5}
                if len(calls) == 6:
                    del self.demand_rows["run-b"]
                return fake.status(machine, command, **options)
            fake_status = SimpleNamespace(status=status)
            with self.scheduled(run, fake_status, {"shared-gpu-1": accounts}):
                run.schedule(run.executor)
        self.assertGreaterEqual(len(run.queue.draining), 5)
        self.assertEqual(sorted(sum(run.queue.assignments.values(), [])), sorted(names))
        self.assertGreater(len(run.sessions), 15)
        self.assertEqual(vm.fixture_shards.merge(names, run.shards())[1], [])

    def test_workers_caps_sessions_and_published_demand(self):
        names = [f"row-{index}" for index in range(10)]
        self.demand_rows = {"run-a": {"demand": 2}}
        run = self.build(names, workers=2)
        accounts = vm.shared_accounts()
        with run.queue:
            with self.scheduled(run, FakeGuest(run), {"shared-gpu-1": accounts}):
                run.schedule(run.executor)
        self.assertLessEqual(max(self.demands), 2)
        self.assertEqual(len({session.id for session in run.sessions}), 2)

    def test_a_session_that_cannot_start_is_retired_and_takes_no_selector(self):
        self.demand_rows = {"run-a": {"demand": 1}}
        run = self.build(["a"])
        accounts = vm.shared_accounts()
        with run.queue:
            fake = FakeGuest(run)
            with self.scheduled(run, fake, {"shared-gpu-1": accounts}):
                attempts = []

                def start(session):
                    attempts.append(session.id)
                    if len(attempts) == 1:
                        raise RuntimeError("no shell")
                run.start_session.side_effect = start
                run.schedule(run.executor)
        self.assertEqual(run.broken, {"shared-gpu-1-capy01"})
        self.assertEqual(run.queue.assignments[run.sessions[0].id], [])
        self.assertEqual(vm.fixture_shards.merge(["a"], run.shards())[1], [])
        self.assertIn("no shell", run.errors[0])

    def test_a_worker_that_stops_leaves_its_claimed_selector_missing(self):
        self.demand_rows = {"run-a": {"demand": 2}}
        run = self.build(["a", "b"], workers=1)
        accounts = vm.shared_accounts()
        with run.queue:
            fake = FakeGuest(run)
            def status(machine, command, **options):
                if not fake.stopped and run.sessions:
                    session = run.sessions[0]
                    run.queue.assignments[session.id].append(run.queue.remaining.popleft())
                    fake.stopped.add(session.id)
                return fake.status(machine, command, **options)
            with self.scheduled(run, SimpleNamespace(status=status), {"shared-gpu-1": accounts}):
                run.schedule(run.executor)
        rows, errors = vm.fixture_shards.merge(["a", "b"], run.shards())
        self.assertIn("worker stopped", " ".join(errors))
        self.assertEqual(sum(row["exit"] == "missing" for row in rows), 1)
        self.assertEqual(run.broken, {"shared-gpu-1-capy01"})
        self.assertEqual(len(run.sessions), 2)

    def test_a_retired_session_keeps_its_timeout_and_only_unclaimed_work_moves(self):
        self.demand_rows = {"run-a": {"demand": 2}}
        run = self.build(["a", "b"], workers=1)
        accounts = vm.shared_accounts()
        with run.queue:
            fake = FakeGuest(run)

            def status(machine, command, **options):
                if not fake.stopped and run.sessions:
                    session = run.sessions[0]
                    selector = run.queue.remaining.popleft()
                    run.queue.assignments[session.id].append(selector)
                    fake.logs[session.id] = [json.dumps({"name": selector, "exit": "timeout", "seconds": 1230,
                                                        "message": "Session retired after timeout: review did not exit"})]
                    fake.stopped.add(session.id)
                return fake.status(machine, command, **options)

            with self.scheduled(run, SimpleNamespace(status=status), {"shared-gpu-1": accounts}):
                run.schedule(run.executor)
        rows, errors = vm.fixture_shards.merge(["a", "b"], run.shards())
        self.assertEqual([(row["name"], row["exit"]) for row in rows], [("a", "timeout"), ("b", 0)])
        self.assertIn("Session retired after timeout", rows[0]["message"])
        self.assertIn("worker stopped", " ".join(errors))
        self.assertEqual(run.broken, {"shared-gpu-1-capy01"})
        self.assertEqual([run.queue.assignments[session.id] for session in run.sessions], [["a"], ["b"]])

    def test_a_run_stops_instead_of_waiting_when_every_session_failed(self):
        self.demand_rows = {"run-a": {"demand": 1}}
        run = self.build(["a"])
        accounts = vm.shared_accounts()
        with run.queue:
            with self.scheduled(run, FakeGuest(run), {"shared-gpu-1": accounts}):
                run.start_session.side_effect = RuntimeError("no shell")
                with self.assertRaisesRegex(RuntimeError, "Every shared GPU session failed.*no shell"):
                    run.schedule(run.executor)
        self.assertEqual(len(run.sessions), len(accounts))

    def test_a_run_with_no_share_left_gives_back_every_session_after_its_fixture(self):
        names = [f"row-{index}" for index in range(30)]
        self.demand_rows = {"run-a": {"demand": 30}}
        run = self.build(names)
        with run.queue:
            fake = FakeGuest(run)
            calls = []

            def status(machine, command, **options):
                calls.append(machine.name)
                if len(calls) == 2:
                    self.demand_rows["run-b"] = {"demand": 1}
                return fake.status(machine, command, **options)
            with self.scheduled(run, SimpleNamespace(status=status), {"shared-gpu-1": ["capy01"]}):
                with patch.object(vm.session_pool.Pool, "claim", side_effect=lambda *a: None if "run-b" in self.demand_rows else MagicMock()):
                    with self.assertRaises(AssertionError):
                        run.schedule(run.executor)
        self.assertEqual(len(run.sessions), 1)
        self.assertIn(run.sessions[0].id, run.queue.draining)
        self.assertEqual(len(run.queue.assignments[run.sessions[0].id]), 2)

    def test_cancelling_waits_for_running_startups_before_releasing_their_accounts(self):
        run = self.build(["a"])
        events = []
        session = SimpleNamespace(started=MagicMock(), closing=None, id="s", failure=None)
        session.started.cancel.return_value = False
        run.sessions = [session]
        with run.queue, patch.object(vm, "wait", side_effect=lambda futures: events.append("joined")), \
                patch.object(run, "close_session", side_effect=lambda target: events.append("closed")):
            run.abandon()
        self.assertEqual(events, ["joined", "closed"])

    def test_status_reads_completion_before_results_so_the_last_row_is_never_lost(self):
        session = SimpleNamespace(id="s", root="C:\\capyrun\\r\\capy01", task="capycanvas-capy01")
        command = vm.session_status_command([session])
        self.assertLess(command.index("Get-ScheduledTask"), command.index("complete"))
        self.assertLess(command.index("$complete=Test-Path"), command.index('results.jsonl'))


    @contextmanager
    def session_view_transport(self, run, *, view_failure=False, protection_failure=False):
        machine=run.machines[0]
        source=self.root/'source.tar.gz';source.write_bytes(b'owned source archive')
        build=self.root/'build.tar.gz';build.write_bytes(b'owned build archive')
        run.payload={'source':source,'build':build,'record':{'controls':{'LayerDrop':{'CapyCanvas.exe':'control'}}},'hashes':{'owned':'source'}}
        helper=self.root/'guest-files';helper.mkdir(exist_ok=True)
        actual=vm.GUEST_FILES/'session-view.ps1'
        body=actual.read_text() if actual.exists() else 'function New-FixtureSessionView {param($Payload,$Repository,$BuildDirectory)}\n'
        (helper/'session-view.ps1').write_text(body)
        events=[];archives=[];scripts=[];views=[];protected=[]
        ssh_command=vm.ssh_command
        def transport(*arguments,**options):
            archives.append((options['stdin'].name,arguments[-1]))
            return subprocess.CompletedProcess(arguments,0)
        def publish(*arguments,**options):
            self.assertNotIn(machine.name,run.prepared)
            events.append('published')
        def script(target,text,**options):
            self.assertIs(target,machine)
            scripts.append(text)
            if 'published' in events and machine.name not in run.prepared:
                protected.append(text)
                events.append('protected')
                if protection_failure:
                    raise SystemExit('A PowerShell script in the %s VM exited with status 17.'%target.name)
            invocation=[line for line in text.splitlines() if line.strip().startswith('New-FixtureSessionView ')]
            if invocation:
                views.append((text,invocation[0]))
                events.append('view')
                if view_failure:
                    raise SystemExit('A PowerShell script in the %s VM exited with status 17.'%target.name)
        self.addCleanup(run.queue.server.server_close)
        with patch.object(vm,'USER','owned-ssh-maintainer'),patch.object(vm,'GUEST_FILES',helper),patch.object(vm,'guest_script',side_effect=script), \
                patch.object(vm,'ssh_command',side_effect=lambda target,command:['ssh','owned',command]), \
                patch.object(vm,'run',side_effect=transport),patch.object(vm,'build_inputs',return_value={'native':'input'}), \
                patch.object(vm,'build_files',return_value={'CapyCanvas.exe':'binary'}) as files, \
                patch.object(vm,'validate_build') as validate,patch.object(vm,'validate_controls') as controls, \
                patch.object(vm,'publish_build_record',side_effect=publish) as published, \
                patch.object(vm,'desktop',side_effect=lambda *args,**kwargs:events.append('launch')) as desktop:
            ssh=ssh_command(SimpleNamespace(server=True,ssh_port=SimpleNamespace(read_text=lambda:'22001')),'exit 0')
            yield SimpleNamespace(machine=machine,events=events,archives=archives,scripts=scripts,views=views,protected=protected,helper_source=body,ssh=ssh,
                                  files=files,validate=validate,controls=controls,published=published,desktop=desktop)

    def test_fifteen_sessions_reuse_one_validated_payload_and_keep_private_launch_paths(self):
        run=self.build(['owned'])
        with self.session_view_transport(run) as transport:
            sessions=[vm.SharedSession(transport.machine,'capy%02d'%index,MagicMock(),run.id,index) for index in range(1,16)]
            for session in sessions:
                run.queue.add_runner(session.id)
                run.start_session(session)
            self.assertEqual(len(transport.views),15,'Session startup copied fifteen complete build trees instead of creating private linked views')
            self.assertEqual(len(transport.archives),2)
            self.assertEqual([Path(path).name for path,command in transport.archives],['source.tar.gz','build.tar.gz'])
            self.assertIn(run.payload_root(),transport.archives[0][1])
            self.assertIn(run.payload_root()+'\\artifacts\\windows',transport.archives[1][1])
            transport.files.assert_called_once()
            transport.validate.assert_called_once()
            transport.controls.assert_called_once()
            transport.published.assert_called_once()
            self.assertEqual(len(transport.protected),1,'Validated immutable payload was not protected once before session creation')
            self.assertLess(transport.events.index('published'),transport.events.index('protected'))
            self.assertLess(transport.events.index('protected'),transport.events.index('view'))
            protection=transport.protected[0]
            self.assertTrue(protection.startswith(transport.helper_source))
            calls=[line for line in protection.splitlines() if line.strip().startswith('Protect-FixtureSessionPayload ')]
            self.assertEqual(len(calls),1,'Preparation did not invoke the shared payload ACL producer once')
            self.assertIn("-Path '%s'"%run.payload_root().rsplit('\\',1)[0],calls[0])
            self.assertIn("-User 'owned-ssh-maintainer'",calls[0])
            self.assertIn('owned-ssh-maintainer@127.0.0.1',transport.ssh)
            self.assertNotIn('icacls.exe',protection[len(transport.helper_source):])
            for session in sessions:
                fullcopy="& robocopy.exe '%s' '%s' /E /MT:4 /NFL /NDL /NJH /NJS /NP"%(run.payload_root(),session.repo)
                self.assertFalse(any(fullcopy in text for text in transport.scripts),
                                 'Session startup still makes a full per-lease payload copy')
            self.assertEqual(transport.desktop.call_count,15)
            for session,(text,invocation),call in zip(sessions,transport.views,transport.desktop.call_args_list):
                self.assertTrue(text.startswith(transport.helper_source))
                canonical=run.prepared[transport.machine.name]
                private=canonical.replace(run.payload_root(),session.repo)
                command=text[text.index(invocation):]
                self.assertIn(run.payload_root(),command)
                self.assertIn(session.repo,command)
                self.assertIn(canonical,command)
                grant="/grant '%s:(OI)(CI)F'"%session.account
                self.assertIn(grant,text)
                self.assertLess(text.index(grant),text.index(invocation))
                self.assertNotIn('/grant',text[text.index(invocation)+len(invocation):])
                self.assertIn('-Executable '+private+'\\CapyCanvas.exe',call.args[1])
                self.assertIn('-BuildRecord '+private+'\\fixture-build.json',call.args[1])
                self.assertEqual(call.kwargs['directory'],session.repo)
                self.assertTrue(session.launched)

    def test_failed_session_view_never_launches_or_rebuilds_the_shared_payload(self):
        for phase in ('protection','view'):
            with self.subTest(phase=phase):
                run=self.build(['owned'])
                with self.session_view_transport(run,view_failure=phase=='view',protection_failure=phase=='protection') as transport:
                    session=vm.SharedSession(transport.machine,'capy01',MagicMock(),run.id,1)
                    run.queue.add_runner(session.id)
                    with self.assertRaisesRegex(SystemExit,'PowerShell script in .* VM exited with status 17'):
                        run.start_session(session)
                    transport.desktop.assert_not_called()
                    self.assertFalse(session.launched)
                    self.assertEqual(len(transport.views),int(phase=='view'))
                    self.assertEqual(len(transport.archives),2)
                    self.assertEqual(len(transport.protected),1)
                    self.assertEqual(transport.machine.name in run.prepared,phase=='view')
                    transport.validate.assert_called_once()
                    transport.controls.assert_called_once()



    def test_graphics_withdrawal_drains_only_its_account_and_collects_before_cleanup(self):
        run=self.build(["lost","healthy","later"],workers=2)
        machine=run.machines[0]
        machine.runtime.mkdir(parents=True,exist_ok=True)
        vm.write_session_health(machine,"boot","boot-a")
        bad=vm.SharedSession(machine,"capy01",MagicMock(),run.id,1)
        good=vm.SharedSession(machine,"capy02",MagicMock(),run.id,2)
        identity={"account":"HOST\\capy01","session_id":1,"boot_utc":"boot-a","logon_sid":"S-1-5-5-1-1"}
        good.graphics_identity=dict(identity,account="HOST\\capy02",session_id=2,logon_sid="S-1-5-5-1-2")
        good.results=[{"name":"healthy","exit":0,"seconds":1}]
        good.last_passed=dict(good.results[-1],observed_utc=vm.time.time())
        for session in (bad,good):run.queue.add_runner(session.id)
        calls=[]
        def capture(session,proof,target,role):
            self.assertEqual(vm.withdrawn_accounts(machine),{"capy01"})
            self.assertTrue(vm.session_evidence_pending(machine))
            self.assertFalse(bad.closed)
            self.assertFalse(good.closed)
            calls.append((session.account,role))
            return {"status":"retrieved","role":role}
        with patch.object(run,"capture_session_graphics",side_effect=capture),patch.object(vm,"guest") as guest:
            run.withdraw_graphics_session(bad,identity,[good])
        self.assertCountEqual(calls,[("capy01","affected"),("capy02","healthy")])
        self.assertEqual(run.queue.draining,{bad.id})
        self.assertFalse(good.draining)
        self.assertNotIn(bad.slot,run.broken,"Fresh boots must be able to readmit the account")
        self.assertIn("graphics-evidence-complete",guest.call_args.args[1])
        self.assertFalse(vm.session_evidence_pending(machine))
        self.assertEqual(vm.withdrawn_accounts(machine),{"capy01"})
        run.queue.server.server_close()

    def test_graphics_poll_retains_one_failed_result_and_never_replays_it(self):
        run=self.build(["lost","healthy","later"],workers=2)
        machine=run.machines[0];machine.runtime.mkdir(parents=True,exist_ok=True)
        vm.write_session_health(machine,"boot","boot-a")
        bad=vm.SharedSession(machine,"capy01",MagicMock(),run.id,1)
        good=vm.SharedSession(machine,"capy02",MagicMock(),run.id,2)
        class Executor:
            def submit(self,function,*args):
                function(*args)
                return MagicMock()
        for session in (bad,good):
            session.started=MagicMock();session.started.done.return_value=True;session.started.exception.return_value=None
            run.queue.add_runner(session.id)
        run.queue.assignments[bad.id]=["lost"]
        run.queue.assignments[good.id]=["healthy"]
        run.queue.remaining.clear();run.queue.remaining.append("later")
        run.sessions=[bad,good]
        identity={"account":"capy01","session_id":1,"boot_utc":"boot-a","logon_sid":"S-1-5-5-1-1"}
        row={"name":"lost","exit":-2003304307,"seconds":2,"message":"MILERR_NO_HARDWARE_DEVICE","infrastructure_invalid":True,"infrastructure_cause":"MILERR_NO_HARDWARE_DEVICE"}
        statuses={bad.id:{"state":"Running","complete":False,"log":json.dumps(row),"error":None,"graphics_withdrawal":json.dumps(identity)},
                  good.id:{"state":"Running","complete":False,"log":json.dumps({"name":"healthy","exit":0,"seconds":1}),"error":None,
                           "graphics_identity":json.dumps(dict(identity,account="capy02",session_id=2))}}
        with patch.object(vm,"guest",return_value=SimpleNamespace(stdout=json.dumps(statuses))),patch.object(run,"capture_session_graphics",return_value={"status":"retrieved"}) as capture:
            run.poll(Executor());run.poll(Executor())
        self.assertEqual(bad.results,[row])
        self.assertEqual(list(run.queue.remaining),["later"])
        self.assertEqual(capture.call_count,2,"Each affected and healthy capture runs exactly once")
        self.assertEqual(run.queue.draining,{bad.id})
        self.assertFalse(good.draining)
        run.queue.server.server_close()

    def test_post_lock_recheck_rejects_a_stale_withdrawn_account(self):
        run=self.build(["later"])
        machine=run.machines[0];run.prepared[machine.name]="ready"
        machine.runtime.mkdir(parents=True,exist_ok=True)
        (machine.runtime/"sessions.json").write_text(json.dumps(["capy01","capy02"]))
        vm.write_session_health(machine,"capy01",{"boot_utc":"boot-a"})
        with patch.object(vm,"host_alive",return_value=True),patch.object(run,"start_session") as start:
            run.acquire(MagicMock(),1,{machine.name:["capy01"],run.machines[1].name:[]})
        self.assertEqual(run.sessions,[])
        start.assert_not_called()
        lease=run.pool.claim(machine.name,"capy01","control",self.root)
        self.assertIsNotNone(lease,"Rejected stale readiness must release its provisional slot")
        lease.close();run.queue.server.server_close()


    def test_one_slow_capture_does_not_delay_another_accounts_withdrawal(self):
        from threading import Event,Thread
        run=self.build(["lost-a","lost-b"])
        machine=run.machines[0];machine.runtime.mkdir(parents=True,exist_ok=True)
        vm.write_session_health(machine,"boot","boot-a")
        sessions=[vm.SharedSession(machine,f"capy0{i}",MagicMock(),run.id,i) for i in (1,2)]
        statuses={}
        for session in sessions:
            session.started=MagicMock();session.started.done.return_value=True;session.started.exception.return_value=None
            run.queue.add_runner(session.id)
            identity={"account":session.account,"session_id":int(session.account[-1]),"boot_utc":"boot-a","logon_sid":"S-1-5-5-1-"+session.account[-1]}
            statuses[session.id]={"state":"Running","complete":False,"log":json.dumps({"name":"lost-"+session.account,"exit":-2003304307,"seconds":1,"message":"lost"}),"error":None,"graphics_withdrawal":json.dumps(identity)}
        run.sessions=sessions
        entered=Event();release=Event();second=Event();errors=[]
        def capture(session,identity,target,role):
            if session.account=="capy01":entered.set();release.wait(10)
            else:second.set()
            return {"status":"retrieved"}
        def poll():
            try:run.poll(MagicMock())
            except BaseException as error:errors.append(error)
        with patch.object(vm,"guest",return_value=SimpleNamespace(stdout=json.dumps(statuses))),patch.object(run,"capture_session_graphics",side_effect=capture):
            worker=Thread(target=poll);worker.start()
            try:
                self.assertTrue(entered.wait(2),"The first affected capture never started")
                self.assertTrue(second.wait(2),"A slow capture delayed another account until its cleanup bound")
            finally:release.set();worker.join(15);run.queue.server.server_close()
        self.assertFalse(worker.is_alive());self.assertEqual(errors,[])


class SharedImageTests(unittest.TestCase):
    def test_server_answer_file_selects_desktop_standard_and_sets_the_administrator_password(self):
        text = vm.server_unattend((Path(vm.__file__).with_name("autounattend.xml")).read_text()).replace("@PASSWORD@", "secret")
        self.assertIn("<Key>/IMAGE/INDEX</Key>\n              <Value>2</Value>", text)
        import xml.etree.ElementTree as ET
        tree = ET.fromstring(text)
        ns = {"u": "urn:schemas-microsoft-com:unattend"}
        accounts = tree.find(".//u:UserAccounts", ns)
        self.assertEqual(accounts.find("u:AdministratorPassword/u:Value", ns).text, "secret")
        self.assertEqual([child.tag.split("}")[1] for child in accounts], ["AdministratorPassword", "LocalAccounts"])

    def test_create_server_keeps_its_own_seed_password_and_base(self):
        with TemporaryDirectory() as directory:
            state = Path(directory)
            paths = {"STATE": state, "FIRMWARE": state / "firmware.json", "KEY": state / "id_ed25519",
                     "SERVER_BASE": state / "base-server", "SERVER_INSTALL": state / "install-server",
                     "SERVER_ISO_RECORD": state / "iso-server", "SERVER_PASSWORD": state / "password-server",
                     "BASE": state / "base", "INSTALL": state / "install", "PASSWORD": state / "password",
                     "RUNTIME": state / "runtime"}
            iso = state / "server.iso"
            iso.touch()
            paths["SERVER_ISO_RECORD"].write_text(str(iso))
            paths["KEY"].touch()
            paths["KEY"].with_suffix(".pub").write_text("key")
            variables = state / "vars"
            variables.touch()
            with patch.multiple(vm, **paths), patch.object(vm, "running", return_value=False), \
                    patch.object(vm, "firmware", return_value={"vars": str(variables)}), \
                    patch.object(vm, "run", return_value=subprocess.CompletedProcess([], 0)), \
                    patch.object(vm, "launch", side_effect=InterruptedError) as launch, \
                    self.assertRaises(InterruptedError):
                vm.create(SimpleNamespace(gui=False, server=True))
            self.assertTrue(launch.call_args.args[0].server)
            seed = paths["SERVER_INSTALL"] / "seed"
            self.assertIn("RDS-RD-Server", (seed / "provision.ps1").read_text())
            self.assertEqual((seed / "password-policy.ps1").read_text(),
                             (vm.GUEST_FILES / "password-policy.ps1").read_text())
            self.assertFalse(paths["PASSWORD"].exists())
            password = paths["SERVER_PASSWORD"].read_text().strip()
            self.assertTrue(password.endswith("aA1!"))
            self.assertIn(f"<Value>{password}</Value>", (seed / "autounattend.xml").read_text())

    def test_host_keys_of_the_two_images_stay_apart(self):
        desktop = vm.ssh_command(SimpleNamespace(server=False, ssh_port=SimpleNamespace(read_text=lambda: "1")))
        server = vm.ssh_command(SimpleNamespace(server=True, ssh_port=SimpleNamespace(read_text=lambda: "1")))
        self.assertIn("HostKeyAlias=capycanvas-windows-vm", desktop)
        self.assertIn("HostKeyAlias=capycanvas-windows-server", server)

    def test_shared_names_select_the_server_image(self):
        self.assertTrue(vm.Machine("shared-gpu-1", Path("/x")).server)
        self.assertEqual(vm.Machine("shared-gpu-1", Path("/x")).base, vm.SERVER_BASE)
        self.assertFalse(vm.Machine("capycanvas5", Path("/x")).server)
        self.assertEqual(vm.Machine("capycanvas5", Path("/x")).base, vm.BASE)


class SharedHostTests(unittest.TestCase):
    def test_clients_receive_the_password_only_on_stdin_and_restart_when_they_stop(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            machine = SimpleNamespace(runtime=root, rdp_port=SimpleNamespace(read_text=lambda: "4242"))
            commands = []

            def popen(command, **options):
                commands.append(command)
                child = MagicMock()
                child.args = command
                child.pid = 100000 + len(commands)
                child.poll.return_value = None
                if "Xvfb" in str(command[0]):
                    (root / "rdp" / "capy01-display").write_text("7\n")
                else:
                    child.stdin = MagicMock()
                return child
            password = root / "password"
            password.write_text("secret\n")
            with patch.object(vm, "rdp_tools", return_value=("/usr/bin/xfreerdp", "/usr/bin/Xvfb")), \
                    patch.object(vm, "SERVER_PASSWORD", password), patch.object(vm.subprocess, "Popen", side_effect=popen):
                clients = vm.RdpClients(machine)
                clients.keep_connected(["capy01"])
                self.assertEqual(len(commands), 2)
                client = commands[1]
                self.assertTrue(all("secret" not in str(item) for item in client))
                for flag in ("/v:127.0.0.1:4242", "/u:capy01", "/size:2560x1600", "/scale-desktop:150", "-suppress-output"):
                    self.assertIn(flag, client)
                clients.children["capy01"][1].stdin.write.assert_called_once_with(b"secret\n")
                clients.keep_connected(["capy01"])
                self.assertEqual(len(commands), 2)
                clients.children["capy01"][1].poll.return_value = 1
                clients.keep_connected(["capy01"])
                self.assertEqual(len(commands), 4)

    def test_boot_reserves_budgeted_cpu_slots_and_forwards_rdp(self):
        with TemporaryDirectory() as directory:
            state = Path(directory)
            base = state / "base-server"
            base.mkdir()
            (base / "disk.qcow2").touch()
            machine = vm.Machine("shared-gpu-1", state / "vms" / "shared-gpu-1")
            machine.base = base
            machine.runtime = state / "runtime"
            machine.runtime.mkdir()
            (state / "vms").mkdir()
            calls = {}

            @contextmanager
            def reserve(state_dir, name, owner, slot_count=1):
                calls.update(owner=owner, slot_count=slot_count)
                yield SimpleNamespace(cpus=4 * slot_count, memory=f"{8 * slot_count}G"), (MagicMock(),)

            @contextmanager
            def card(state_dir, name, requested):
                yield SimpleNamespace(address="0000:01:00.0", functions=["0000:01:00.0"]), MagicMock()

            def launch(target, **options):
                calls.update(cpus=options["cpus"], memory=options["memory"], gpu=options["gpu"].address)
            with patch.multiple(vm, STATE=state, SETTINGS={}), patch.object(vm, "running", return_value=False), \
                    patch.object(vm.resource, "getrlimit", return_value=(vm.resource.RLIM_INFINITY, vm.resource.RLIM_INFINITY)), \
                    patch.object(vm.cpu_pool, "reserve", side_effect=reserve), patch.object(vm.gpu_pool, "reserve", side_effect=card), \
                    patch.object(vm, "run"), patch.object(vm, "launch", side_effect=launch), patch.object(vm, "wait_for_ssh"), \
                    patch.object(vm, "prepare"), patch.object(vm, "prepare_gpu_driver"), \
                    patch.object(vm, "prepare_gpu_display", return_value={}):
                vm.boot(machine, gui=False, gpu="auto")
            self.assertEqual(calls["slot_count"], 8)
            self.assertEqual((calls["cpus"], calls["memory"]), (32, "64G"))
            self.assertEqual(calls["owner"], vm.SHARED_OWNER / "shared-gpu-1")


class FixtureRoutingTests(unittest.TestCase):
    def route(self, *arguments):
        with patch.object(vm.sys, "argv", ["windows-vm.py", "fixtures", *arguments]), \
                patch.object(vm, "shared_fixtures") as shared, patch.object(vm, "sharded_fixtures") as warp, \
                patch.object(vm, "start"), patch.object(vm, "selected", side_effect=RuntimeError("exclusive")):
            try:
                vm.main()
            except SystemExit as error:
                if str(error) != "exclusive":
                    raise
                return "exclusive"
        return "shared" if shared.called else "warp" if warp.called else None

    def test_default_is_shared_and_warp_and_exclusive_stay_available(self):
        self.assertEqual(self.route("layers"), "shared")
        self.assertEqual(self.route(), "shared")
        self.assertEqual(self.route("--workers", "4", "layers"), "shared")
        self.assertEqual(self.route("--warp", "layers"), "warp")
        self.assertEqual(self.route("--runners", "6", "layers"), "warp")
        self.assertEqual(self.route("--hardware-gpu", "layers"), "exclusive")

    def test_conflicting_modes_fail_before_any_vm_work(self):
        for arguments, message in ((("--hardware-gpu", "--warp"), "cannot be used"), (("--hardware-gpu", "--runners", "2"), "cannot be used"),
                                   (("--workers", "2", "--warp"), "--workers"), (("--workers", "2", "--hardware-gpu"), "--workers")):
            with self.subTest(arguments=arguments), self.assertRaises(SystemExit) as failure:
                self.route(*arguments)
            self.assertRegex(str(failure.exception), message)

    def test_an_impossible_cpu_budget_fails_before_building(self):
        with TemporaryDirectory() as directory:
            state = Path(directory)
            (state / "base-server").mkdir()
            (state / "base-server" / "disk.qcow2").touch()
            (state / "cpu-pool.json").write_text(json.dumps({"vcpu_budget": 96, "memory_budget_gib": 192,
                                                             "runner_cpus": 4, "runner_memory_gib": 8}))
            args = SimpleNamespace(names=[], vm="owner", no_build=False, profile="release", timeout_minutes=20, workers=None)
            with patch.multiple(vm, STATE=state, SERVER_BASE=state / "base-server", VMS=state, SETTINGS={}), \
                    patch.object(vm, "rdp_tools", return_value=("x", "y")), patch.object(vm.gpu_pool, "devices", return_value=[1]), \
                    patch.object(vm, "boot", side_effect=AssertionError("built")), \
                    self.assertRaisesRegex(RuntimeError, "per_owner_limit"):
                vm.shared_fixtures(args)

    def test_explicit_stop_holds_every_session_closed_until_the_vm_is_down(self):
        with TemporaryDirectory() as directory:
            runtime = Path(directory)
            with patch.multiple(vm, RUNTIME=runtime, SETTINGS={}):
                machine = vm.Machine("shared-gpu-1", runtime / "vm")
                pool = vm.shared_pool()
                lease = pool.claim("shared-gpu-1", "capy03", "run", "/worktree")
                with patch.object(vm, "selected", return_value=machine), patch.object(vm, "run") as service, \
                        self.assertRaisesRegex(SystemExit, "hold sessions"):
                    vm.stop(SimpleNamespace())
                service.assert_not_called()
                lease.close()
                claims = []

                def down(target):
                    claims.append(pool.claim("shared-gpu-1", "capy05", "late", "/worktree"))
                with patch.object(vm, "selected", return_value=machine), patch.object(vm, "run") as service, \
                        patch.object(vm, "stop_machine", side_effect=down):
                    vm.stop(SimpleNamespace())
                self.assertEqual(claims, [None])
                self.assertIn("stop", service.call_args.args)

    def test_a_missing_shared_image_points_at_setup_and_warp(self):
        with TemporaryDirectory() as directory:
            state = Path(directory)
            args = SimpleNamespace(names=[], vm="owner", no_build=False, profile="release", timeout_minutes=20, workers=None)
            with patch.multiple(vm, SERVER_BASE=state / "missing", VMS=state), \
                    self.assertRaisesRegex(RuntimeError, r"setup --server.*--warp"):
                vm.shared_fixtures(args)


class SharedHostTimeoutTests(unittest.TestCase):
    def test_ssh_health_timeout_reports_and_exits_through_normal_cleanup(self):
        with TemporaryDirectory() as directory:
            machine = SimpleNamespace(name="shared-gpu-1", server=True, runtime=Path(directory))
            clients = MagicMock()
            cleanup = MagicMock()
            cleanup.attach_mock(clients.close, "close")
            with patch.object(vm, "selected", return_value=machine), \
                    patch.object(vm.cpu_pool, "lock_file", return_value=MagicMock()), \
                    patch.object(vm.signal, "signal"), patch.object(vm, "shared_pool"), \
                    patch.object(vm, "shared_settings", return_value={"idle_minutes": 10}), \
                    patch.object(vm, "shared_accounts", return_value=["capy01"]), \
                    patch.object(vm, "RdpClients", return_value=clients), \
                    patch.object(vm, "running", return_value=True), patch.object(vm, "boot") as boot, \
                    patch.object(vm, "initialize_session_health"), \
                    patch.object(vm, "stop_machine") as stop, \
                    patch.object(vm, "guest", side_effect=subprocess.TimeoutExpired("liveness", 20)), \
                    patch.object(vm, "session_shells", side_effect=subprocess.TimeoutExpired("health", 60)) as shells:
                cleanup.attach_mock(stop, "stop")
                vm.shared_host(SimpleNamespace())
            self.assertEqual((machine.runtime / "startup-error").read_text(),
                             "shared-gpu-1 stopped responding to SSH health checks (timeout 60 s)")
            self.assertEqual([call[0] for call in cleanup.mock_calls], ["close", "stop"])
            stop.assert_called_once_with(machine)
            boot.assert_called_once_with(machine, gui=False, gpu="auto")
            shells.assert_called_once_with(machine)
            self.assertFalse((machine.runtime / "sessions.json").exists())

class SharedHostHealthTests(unittest.TestCase):
    @contextmanager
    def host(self, runtime, shells, running, probe):
        machine = SimpleNamespace(name="shared-gpu-1", server=True, runtime=runtime)
        with patch.object(vm, "selected", return_value=machine), \
                patch.object(vm.cpu_pool, "lock_file", return_value=MagicMock()), \
                patch.object(vm.signal, "signal"), patch.object(vm, "shared_pool") as pool, \
                patch.object(vm, "shared_settings", return_value={"idle_minutes": 10}), \
                patch.object(vm, "shared_accounts", return_value=["capy01"]), \
                patch.object(vm, "RdpClients") as clients, \
                patch.object(vm, "running", side_effect=running), patch.object(vm, "boot"), \
                patch.object(vm, "initialize_session_health"), \
                patch.object(vm, "stop_machine") as stop, \
                patch.object(vm, "session_shells", side_effect=shells), \
                patch.object(vm, "guest", side_effect=probe) as guest:
            pool.return_value.holders.return_value = {"capy01": {}}
            yield machine, clients.return_value, stop, guest

    def test_readiness_timeout_with_live_transport_preserves_ready_record_and_logs_once(self):
        with TemporaryDirectory() as directory:
            runtime=Path(directory);clock=[0];calls=[];saved=[]
            def readiness(machine):
                calls.append(machine)
                if len(calls)==1:
                    return {"capy01"}
                clock[0]+=60
                raise subprocess.TimeoutExpired("explorer readiness",60)
            def sleep(seconds):
                record=runtime/"sessions.json"
                identity=(record.read_bytes(),record.stat().st_ino,record.stat().st_mtime_ns)
                if not saved:
                    saved.append(identity)
                else:
                    self.assertEqual(identity,saved[0])
                    clients.close.assert_not_called()
                    stop.assert_not_called()
                    self.assertFalse((runtime/"startup-error").exists())
                    rows=[json.loads(line) for line in (runtime/"slow-health.jsonl").read_text().splitlines()]
                    self.assertEqual(len(rows),1)
                    self.assertEqual(rows[0]["command"],vm.SESSION_SHELLS_COMMAND)
                    self.assertEqual(rows[0]["elapsed"],60)
                    self.assertRegex(rows[0]["utc"],r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
                clock[0]+=seconds
            probes=[]
            def liveness(*args,**options):
                probes.append(options)
                if len(probes)==2:
                    raise subprocess.TimeoutExpired("liveness",20)
                clock[0]+=3
                return SimpleNamespace(returncode=0)
            with self.host(runtime,readiness,[True,True,True,True],liveness) as (machine,clients,stop,probe), \
                    patch.object(vm.time,"monotonic",side_effect=lambda:clock[0]),patch.object(vm.time,"sleep",side_effect=sleep) as sleeps:
                vm.shared_host(SimpleNamespace())
                self.assertEqual(sleeps.call_count,2,"Readiness timeout stopped a VM whose SSH transport was alive")
                self.assertEqual(probe.call_count,2)
                for call in probe.call_args_list:
                    self.assertEqual(call.args,(machine,"exit 0"))
                    self.assertEqual(call.kwargs,dict(stdin=subprocess.DEVNULL,capture_output=True,text=True,timeout=20))
                self.assertEqual((runtime/"startup-error").read_text(),"shared-gpu-1 stopped responding to SSH health checks (timeout 60 s)")
                clients.close.assert_called_once()
                stop.assert_called_once_with(machine)

    def test_failed_liveness_transport_stops_with_original_dead_message(self):
        with TemporaryDirectory() as directory:
            runtime=Path(directory)
            with self.host(runtime,subprocess.TimeoutExpired("readiness",60),[True,True],subprocess.CalledProcessError(255,"liveness")) as (machine,clients,stop,probe):
                vm.shared_host(SimpleNamespace())
                self.assertEqual((runtime/"startup-error").read_text(),"shared-gpu-1 stopped responding to SSH health checks (timeout 60 s)")
                self.assertFalse((runtime/"slow-health.jsonl").exists())
                clients.close.assert_called_once()
                stop.assert_called_once_with(machine)
                self.assertEqual(probe.call_count,1)

    def test_readiness_maps_only_accounts_whose_own_session_has_explorer(self):
        sessions=[" USERNAME SESSIONNAME ID STATE IDLE TIME LOGON TIME",">CaPy01 rdp-tcp#2 1 Active none 11/9/2026", " capy02 rdp-tcp#3 2 Active none 11/9/2026"," capy03 4 Disc 1:00 11/9/2026"," DOMAIN\\CAPY04 console 8 Active none 11/9/2026"," capy01 rdp-tcp#2 1 Active none 11/9/2026"]
        for shells,rows,expected in [([1,4,8,9],sessions,{"capy01","capy03","capy04"}),([2],sessions,{"capy02"}),([],sessions,set()),([1],sessions[:1],set()),([1],[],set())]:
            with self.subTest(shells=shells,rows=rows):
                def query(machine,command,**options):
                    if "-IncludeUserName" in command:
                        return SimpleNamespace(stdout=",".join("DOMAIN\\"+name for name in expected))
                    return SimpleNamespace(stdout=json.dumps({"shells":shells,"sessions":rows}))
                with patch.object(vm,"guest",side_effect=query) as guest:
                    self.assertEqual(vm.session_shells(SimpleNamespace()),expected)
                command=guest.call_args.args[1]
                self.assertNotIn("-IncludeUserName",command)
                self.assertIn(".SessionId",command)
                self.assertIn("quser",command)


class SessionGraphicsTests(unittest.TestCase):
    def test_withdrawal_filters_a_stale_ready_record_without_hiding_healthy_accounts(self):
        with TemporaryDirectory() as directory:
            machine=SimpleNamespace(name="shared-gpu-1",runtime=Path(directory))
            (machine.runtime/"sessions.json").write_text(json.dumps(["capy01","capy02"]))
            health=machine.runtime/"session-health";health.mkdir()
            (health/"boot.json").write_text(json.dumps("boot-a"))
            (health/"capy01.json").write_text(json.dumps({"boot_utc":"boot-a","account":"capy01","logon_sid":"S-1-5-5-1-2"}))
            with patch.object(vm,"host_alive",return_value=True):
                self.assertEqual(vm.ready_accounts(machine),["capy02"],"A withdrawn account was readmitted from the stale explorer record")

    def test_only_a_new_guest_boot_clears_withdrawal(self):
        with TemporaryDirectory() as directory:
            machine=SimpleNamespace(runtime=Path(directory))
            vm.write_session_health(machine,"boot","boot-a")
            vm.write_session_health(machine,"capy01",{"boot_utc":"boot-a","logon_sid":"old"})
            with patch.object(vm,"guest",return_value=SimpleNamespace(stdout="boot-a")):
                vm.initialize_session_health(machine)
            self.assertEqual(vm.withdrawn_accounts(machine),{"capy01"})
            with patch.object(vm,"guest",return_value=SimpleNamespace(stdout="boot-b")):
                vm.initialize_session_health(machine)
            self.assertEqual(vm.withdrawn_accounts(machine),set())

    def test_pending_capture_holds_idle_stop_only_within_its_recorded_bound(self):
        with TemporaryDirectory() as directory:
            machine=SimpleNamespace(runtime=Path(directory))
            vm.write_session_health(machine,"capy01",{"evidence_state":"pending","evidence_deadline":100})
            with patch.object(vm.time,"time",return_value=99):self.assertTrue(vm.session_evidence_pending(machine))
            with patch.object(vm.time,"time",return_value=100):self.assertFalse(vm.session_evidence_pending(machine))
            vm.write_session_health(machine,"capy01",{"evidence_state":"finished","evidence_deadline":100})
            with patch.object(vm.time,"time",return_value=99):self.assertFalse(vm.session_evidence_pending(machine))

    def test_host_waits_for_pending_capture_before_its_normal_idle_shutdown(self):
        with TemporaryDirectory() as directory:
            runtime=Path(directory);machine=SimpleNamespace(name="shared-gpu-1",server=True,runtime=runtime)
            vm.write_session_health(machine,"capy01",{"evidence_state":"pending","evidence_deadline":10000})
            clock=[0];observations=[]
            def sleep(seconds):
                clients.close.assert_not_called();stop.assert_not_called()
                observations.append(clock[0]);clock[0]+=601
                if len(observations)==2:
                    vm.write_session_health(machine,"capy01",{"evidence_state":"finished","evidence_deadline":10000})
            pool=vm.session_pool.Pool(runtime/"pool");lock_file=vm.cpu_pool.lock_file
            def lock(path):return MagicMock() if path.name=="host-lock" else lock_file(path)
            with patch.object(vm,"selected",return_value=machine),patch.object(vm.cpu_pool,"lock_file",side_effect=lock), \
                    patch.object(vm.signal,"signal"),patch.object(vm,"shared_pool",return_value=pool), \
                    patch.object(vm,"shared_settings",return_value={"idle_minutes":10}),patch.object(vm,"shared_accounts",return_value=["capy01"]), \
                    patch.object(vm,"RdpClients") as clients_type,patch.object(vm,"running",return_value=True), \
                    patch.object(vm,"boot"),patch.object(vm,"initialize_session_health"),patch.object(vm,"stop_machine") as stop, \
                    patch.object(vm,"session_shells",return_value={"capy01"}),patch.object(vm.time,"time",return_value=0), \
                    patch.object(vm.time,"monotonic",side_effect=lambda:clock[0]),patch.object(vm.time,"sleep",side_effect=sleep):
                clients=clients_type.return_value
                vm.shared_host(SimpleNamespace())
            self.assertEqual(observations,[0,601])
            stop.assert_called_once_with(machine)
            clients.close.assert_called_once()


    def test_foreign_lease_survives_another_accounts_withdrawal(self):
        with TemporaryDirectory() as directory:
            root=Path(directory);machine=SimpleNamespace(name="shared-gpu-1",runtime=root/"runtime")
            pool=vm.session_pool.Pool(root/"pool")
            foreign=pool.claim(machine.name,"capy02","foreign",root)
            own=pool.claim(machine.name,"capy01","ours",root)
            try:
                with patch.object(vm,"shared_pool",return_value=pool),patch.object(vm,"shared_machines",return_value=[machine]):
                    run=vm.SharedRun(SimpleNamespace(),root/"evidence","ours",MagicMock(),lambda _:None,{})
                bad=vm.SharedSession(machine,"capy01",own,"ours",1)
                vm.write_session_health(machine,"boot","boot-a")
                with patch.object(run,"capture_session_graphics",return_value={"status":"retrieved"}), \
                        patch.object(vm,"guest"),patch.object(vm,"stop_machine") as stop:
                    run.withdraw_graphics_session(bad,{"account":"capy01","session_id":1,"logon_sid":"S-1-5-5-1-1","boot_utc":"boot-a"},[])
                stop.assert_not_called()
                self.assertEqual(set(pool.holders(machine.name,["capy01","capy02"])),{"capy01","capy02"})
                with pool.closed(machine.name,["capy01","capy02"]) as closed:self.assertFalse(closed)
                self.assertFalse(bad.closed)
                self.assertEqual(vm.withdrawn_accounts(machine),{"capy01"})
            finally:
                own.close();foreign.close()


if __name__ == "__main__":
    unittest.main()
