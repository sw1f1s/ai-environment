"""Native Rust dispatcher integration: fake CLI transport, real CAS and checks.

The fake replaces only inference. Python does not orchestrate the workflow.
"""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

REPOSITORY = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get('AI_ENV_BIN', REPOSITORY / 'skills/ai-env/scripts/bin/aienv'))
FAKE = r'''#!PYTHON
import hashlib, json, os, pathlib, sys, time
args = sys.argv[1:]
assert '--ephemeral' in args and '--json' in args
assert args[args.index('--sandbox')+1] == 'read-only'
assert args[args.index('-c')+1] == 'web_search="disabled"'
prompt = sys.stdin.read()
role = prompt.split('AI Env role: ', 1)[1].split('.', 1)[0]
metadata = None
for line in prompt.splitlines():
    if line.startswith('{'):
        try:
            data = json.loads(line)
            if 'snapshot' in data and 'goal' in data:
                metadata = data
                break
        except ValueError: pass
if metadata is None:
    start = prompt.find('{')
    metadata = json.JSONDecoder().raw_decode(prompt[start:])[0]
root = pathlib.Path(args[args.index('-C')+1])
artifacts = pathlib.Path(os.environ['AE_FAKE_ARTIFACTS'])
artifacts.mkdir(exist_ok=True)
identifier = metadata.get('task', {}).get('id', role)
label = role + '-' + identifier
counter = artifacts / (label + '.calls')
count = int(counter.read_text()) + 1 if counter.exists() else 1
counter.write_text(str(count))
(artifacts / (label + '-' + str(count) + '.prompt')).write_text(prompt)
schema_doc = json.loads(pathlib.Path(args[args.index('--output-schema')+1]).read_text())
assert schema_doc['properties']['snapshot']['enum'] == [metadata['snapshot']]
if role in ('reviewer', 'qa') and not metadata.get('metadata_repair'):
    assert schema_doc['properties']['tasks']['items']['enum'] == [task['id'] for task in metadata['task_contracts']]
    # Real controller state, not a second hand-written decision catalog: review
    # receives the applied executors' saved rationale even in a fresh session.
    saved = json.loads((root / '.ai-env/workflows/example.json').read_text())
    context = metadata['implementation_context']
    assert context['items'] and context['omitted'] == 0
    for row in context['items']:
        assert saved['summaries'][row['task']].startswith(row['summary'])
        assert row['status'] == saved['task_states'][row['task']]['status']
        assert row['completed_revision'] == saved['completed'].get(row['task'])
    planned = {task['id']: task for task in saved['plan']['tasks']}
    for task in metadata['task_contracts']:
        assert task['context'] == planned[task['id']].get('context', [])
if role in ('reviewer', 'qa') and not metadata.get('metadata_repair'):
    assert schema_doc['properties']['architecture']['properties']['issues']['items']['properties']['tasks']['items']['enum'] == [task['id'] for task in metadata['task_contracts']]
if role == 'qa' and not metadata.get('metadata_repair') and metadata['proof_ids']:
    assert schema_doc['properties']['proofs']['items']['enum'] == metadata['proof_ids']
scenario = os.environ.get('AE_FAKE_SCENARIO', 'single')
if metadata.get('metadata_repair'):
    spec = metadata['metadata_repair']
    report = dict(snapshot=metadata['snapshot'], selections=[dict(id=row['id'], refs=spec['choices'][row['id']][:1], unresolved=False) for row in spec['rows']])
    if scenario == 'metadata-repeat':
        report['selections'][0]['refs'] = ['unknown-ref']
    if scenario == 'metadata-unresolved':
        for row in report['selections']: row.update(refs=[], unresolved=True)
    pathlib.Path(args[args.index('-o')+1]).write_text(json.dumps(report))
    print(json.dumps(dict(type='turn.completed',usage=dict(input_tokens=100,cached_input_tokens=20,output_tokens=10))))
    sys.exit(0)
if scenario in ('timeout', 'parent-loss'):
    (artifacts / 'model.pid').write_text(str(os.getpid()))
    time.sleep(120)
def job(name, path, depends=(), context=()):
    return dict(id=name, title='Implement ' + name, instructions='Keep local style and reuse existing code.',
                paths=[path], read_refs=[next(row['id'] for row in metadata['planning_refs']['refs'] if row['path'] == name) for name in context], depends=list(depends))
def edits(paths):
    rows = []
    expanded = []
    for name in paths:
        expanded.extend(sorted(p.relative_to(root).as_posix() for p in (root / name).glob("*.py")) if (root / name).is_dir() else [name])
    for name in expanded:
        path = root / name
        old = path.read_bytes() if path.exists() else None
        desired = b'VALUE = 2\n' if name.endswith('a.py') else b'VALUE = 4\n'
        if scenario in ('noop-failure', 'noop-accept'): desired = b'VALUE = 1\n'
        if scenario == 'team-qa-failure' and role == 'developer' and identifier == 'a' and count == 1:
            desired = b'VALUE = 1\n'
        if name.endswith('a.py') and ((scenario in ('team-arch-block','team-arch-mixed') and role == 'developer' and identifier == 'a' and count == 1) or (scenario == 'single-arch-block' and role == 'coordinator')):
            desired = b'import subprocess\nVALUE = 2\n'
        if old == desired: continue
        rows.append(dict(path=name, source=desired.decode()))
    return rows
base = dict(snapshot=metadata['snapshot'])
if role in ('coordinator', 'architect'):
    team = scenario.startswith('team') or role == 'architect'
    tasks = [job('a','src/a.py'),job('b','src/b.py')]
    if not team: tasks = [job('change','src', context=['tests/unit/check_values.py','tests/integration/check_total.py'] if metadata['approved_checks'] else [])]
    report = dict(base, tasks=tasks, checks=metadata['approved_checks'], acceptance=['a=2 and b=4; existing unit and integration checks pass.'], architecture=dict(boundaries=['Value modules own their data only; tests and transport remain separate.'], dependencies=['Value modules must not import process/transport dependencies.'], extension_scenarios=[]))
    owners = ['a'] if scenario == 'team-qa-failure' else [task['id'] for task in tasks]
    report['verification'] = [dict(id='requested-values', acceptance=[0], tasks=owners,
        risk='The requested value helpers must keep their values and the configured unit/integration invariants.', checks=metadata['approved_checks'])]
    if role == 'coordinator':
        report.update(mode='team' if team else 'single', summary='Implement requested values with project style.', edits=[] if team else edits(['src/a.py','src/b.py']))
elif role == 'developer':
    task = metadata['task']
    if scenario == 'team-replay':
        (artifacts / (identifier + '-running-workflow.json')).write_bytes((root / '.ai-env/workflows/example.json').read_bytes())
    if scenario == 'team-fail' and task['id'] == 'b' and count == 1:
        time.sleep(.15)
        print(json.dumps(dict(type='turn.completed',usage=dict(input_tokens=100,cached_input_tokens=20,output_tokens=10))))
        sys.exit(3)
    report = dict(base, summary='Scoped contract satisfied.', edits=edits(task['paths']))
else:
    report = dict(base, verdict='accept', findings=[], tasks=[], assessment=['src/a.py and src/b.py use existing VALUE convention; configured checks cover requested values; no claim of exhaustive coverage.'])
    report['architecture'] = dict(assessment=['Current src/a.py and src/b.py preserve independent value ownership and existing tests; no speculative abstraction is needed.'], issues=[])
    if role == 'qa':
        if scenario == 'source-ref-replay' and count == 1:
            (artifacts / 'qa-running-workflow.json').write_bytes((root / '.ai-env/workflows/example.json').read_bytes())
        report['proofs'] = metadata['proof_ids']
        report['assessment'] = ['Current a=2 and b=4 satisfy the request; unit and integration proofs passed.']
        report['verification'] = []
        for obligation in metadata.get('verification', []):
            evidence = [result for result in metadata.get('check_results', []) if result['check'] in obligation['checks']]
            failed = any(not result['passed'] for result in evidence)
            status = 'gap' if failed else 'verified' if obligation['checks'] else 'inspected'
            report['verification'].append(dict(id=obligation['id'], status=status,
                proofs=[result['id'] for result in evidence], source_refs=metadata['source_refs']['by_obligation'][obligation['id']][:1],
                evidence='Current value source and approved unit/integration results support the contract.' if not failed else 'The actual unit/integration proof fails for the assigned value helper.'))
        if scenario != 'noop-accept' and any(not result['passed'] for result in metadata.get('check_results', [])):
            report.update(verdict='changes', findings=['Value contracts still fail; repair the assigned values.'],
                          tasks=['a'] if scenario == 'team-qa-failure' else ['change'])
            if scenario == 'team-qa-failure': report['proofs'] = []
def architecture_issue(severity, task, path):
    return dict(severity=severity,tasks=[task],path=path,line=1,
        problem='The data-only value module imports a process transport dependency.' if severity=='blocking' else 'A module comment could clarify the existing value contract.',
        consequence='Changing process transport would couple an independent value policy to that implementation.' if severity=='blocking' else 'An optional comment could help navigation; behavior and ownership are already correct.',
        fix='Remove the transport import and keep the existing VALUE declaration.' if severity=='blocking' else 'Optionally document the existing module responsibility without adding a new layer.',
        verification='Inspect the current source dependency and run the approved unit/integration checks; no extra execution claimed.')
if scenario in ('team-arch-block','team-arch-mixed') and role=='reviewer' and count==1:
    report.update(verdict='changes',findings=[],tasks=[])
    report['architecture']['issues']=[architecture_issue('blocking','a','src/a.py')]
    if scenario=='team-arch-mixed': report['architecture']['issues'].append(architecture_issue('advisory','b','src/b.py'))
if scenario=='team-arch-advisory' and role=='reviewer':
    report['architecture']['issues']=[architecture_issue('advisory','a','src/a.py')]
if scenario=='single-arch-block' and role=='qa' and count==1:
    report.update(verdict='changes',findings=[],tasks=[])
    report['architecture']['issues']=[architecture_issue('blocking','change','src/a.py')]
if scenario=='team-arch-missing' and role=='reviewer' and count==1: report.pop('architecture')
if scenario == 'bad-snapshot-once' and role == 'coordinator' and count == 1: report['snapshot'] = '0' * 64
if scenario == 'team-invalid-once' and role == 'developer' and identifier == 'b' and count == 1: report['snapshot'] = '0' * 64
if scenario == 'auto-bad-team-once' and role == 'coordinator' and count == 1:
    report.update(mode='team', tasks=[job('a','src/*.py'),job('b','src/b.py')], edits=[])
if scenario == 'bad-edit' and role == 'coordinator': report['edits'][0] = dict(path='src/a.py', replacements=[dict(old='VALUE = 9', new='VALUE = 2')])
if scenario in ('bad-scope-once', 'bad-scope-repeat') and role == 'coordinator' and (count == 1 or scenario.endswith('repeat')):
    report['tasks'][0]['paths'] = ['src/*.py']
if scenario == 'bad-shape-once' and role == 'coordinator' and count == 1:
    report['edits'][0]['old'] = 'VALUE = 1\n'
    report['edits'][0]['new'] = 'VALUE = 2\n'
if scenario == 'grouped' and role == 'coordinator':
    report['edits'][0] = dict(path='src/a.py', replacements=[dict(old='VALUE = 1',new='INTERMEDIATE = 2'),dict(old='INTERMEDIATE',new='VALUE')])
if scenario == 'team-review-once' and role == 'reviewer' and count == 1: report['assessment'] = []
if scenario == 'bad-proof-once' and role == 'qa' and count == 1: report['proofs'] = ['forged-proof']
if scenario in ('metadata-repeat','metadata-unresolved','source-ref-replay') and role == 'qa' and count == 1: report['verification'][0]['source_refs'] = ['unknown-ref']
if scenario == 'bad-read-once' and role == 'coordinator' and count == 1: report['tasks'][0]['read_refs'] = ['Approved check test runs python check.py.']
if role == 'qa' and count == 1:
    if scenario == 'missing-verification-once': report.pop('verification')
    if scenario == 'forged-verification-once': report['verification'][0]['proofs'] = ['forged-proof']
    if scenario == 'outside-verification-once': report['verification'][0]['source_refs'] = ['unknown-ref']
    if scenario == 'duplicate-verification-once': report['verification'].append(report['verification'][0])
answer = pathlib.Path(args[args.index('-o')+1])
answer.write_text('{broken' if scenario == 'malformed' or (scenario == 'malformed-once' and role == 'coordinator' and count == 1) else json.dumps(report))
if scenario == 'unknown-usage': sys.exit(0)
print(json.dumps(dict(type='turn.completed',usage=dict(input_tokens=100,cached_input_tokens=20,output_tokens=10))))
'''


class NativeOrchestrationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='ae-native-runner-')
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name).resolve()
        self.root = self.base / 'project'
        self.root.mkdir()
        self.write('src/a.py', 'VALUE = 1\n')
        self.write('src/b.py', 'VALUE = 1\n')
        self.write('.editorconfig', 'root = true\n[*]\nindent_style = space\nindent_size = 4\n')
        self.write('AGENTS.md', 'Keep existing naming and helper functions.\n')
        self.request = self.base / 'request.txt'
        self.request.write_text('Set a VALUE to 2 and b VALUE to 4; preserve project conventions.\n')
        self.fakebin = self.base / 'bin'
        self.fakebin.mkdir()
        fake = self.fakebin / 'codex'
        fake.write_text(FAKE.replace('#!PYTHON', '#!' + sys.executable, 1))
        fake.chmod(0o755)
        self.artifacts = self.base / 'fake-artifacts'
        self.env = dict(os.environ, PATH=str(self.fakebin) + os.pathsep + os.environ['PATH'],
                        AE_FAKE_ARTIFACTS=str(self.artifacts), AE_FAKE_SCENARIO='single')
        self.cli('init')
        self.write('tests/unit/check_values.py',
                   'from pathlib import Path\nimport runpy\n'
                   'r = Path(__file__).resolve().parents[2]\n'
                   'assert runpy.run_path(str(r / "src/a.py"))["VALUE"] == 2\n')
        self.write('tests/integration/check_total.py',
                   'from pathlib import Path\nimport runpy\n'
                   'r = Path(__file__).resolve().parents[2]\n'
                   'assert sum(runpy.run_path(str(r / p))["VALUE"] for p in ("src/a.py", "src/b.py")) == 6\n')
        self.configure()

    def write(self, path, text):
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)

    def configure(self):
        self.write('.ai-env/config.json', json.dumps({'checks': {
            'unit': {'argv': [sys.executable, '-I', 'tests/unit/check_values.py'], 'timeout_seconds': 10},
            'integration': {'argv': [sys.executable, '-I', 'tests/integration/check_total.py'], 'timeout_seconds': 10},
        }}))

    def cli(self, *args, success=True):
        process = subprocess.run([str(BINARY), '--root', str(self.root), *map(str,args)],
                                 env=self.env, capture_output=True, text=True, timeout=45)
        if success:
            self.assertEqual(process.returncode, 0, process.stderr + process.stdout)
            return json.loads(process.stdout)
        return process

    def run_workflow(self, *options, success=True):
        request = [] if '--resume' in options else ['--request', str(self.request)]
        return self.cli('orchestrate', 'example', *request, '--effort', 'low', *options, success=success)

    def calls(self, role):
        return sum(int(path.read_text()) for path in self.artifacts.glob(role + '-*.calls'))

    def test_single_path_has_no_architect_developer_or_reviewer_overhead(self):
        result = self.run_workflow()
        self.assertEqual(result['phase'], 'done')
        self.assertEqual(result['mode'], 'single')
        self.assertEqual(self.calls('coordinator'), 1)
        self.assertEqual(self.calls('qa'), 1)
        self.assertEqual(self.calls('architect') + self.calls('developer') + self.calls('reviewer'), 0)
        self.assertEqual(result['model_calls'], 2)
        self.assertEqual(result['observed_total_tokens'], 220)
        self.assertEqual(len(result['proofs']), 2)
        self.assertEqual((self.root / 'src/a.py').read_text(), 'VALUE = 2\n')
        self.assertEqual((self.root / 'src/b.py').read_text(), 'VALUE = 4\n')
        prompt = next(self.artifacts.glob('coordinator-*.prompt')).read_text()
        self.assertIn('Keep existing naming and helper functions.', prompt)
        self.assertIn('indent_size', prompt)
        resumed = self.run_workflow('--resume')
        self.assertEqual(resumed['model_calls'], 2)
        self.assertEqual(self.calls('coordinator') + self.calls('qa'), 2)

    def test_explicit_team_keeps_independent_review_and_real_qa(self):
        self.env['AE_FAKE_SCENARIO'] = 'team'
        result = self.run_workflow('--mode', 'team')
        self.assertEqual(result['phase'], 'done')
        self.assertEqual(self.calls('architect'), 1)
        self.assertEqual(self.calls('developer'), 2)
        self.assertEqual(self.calls('reviewer'), 1)
        self.assertEqual(self.calls('qa'), 1)
        self.assertEqual(result['model_calls'], 5)
        self.assertEqual(len(result['proofs']), 2)

    def test_auto_can_choose_team_without_repeating_architecture(self):
        self.env['AE_FAKE_SCENARIO'] = 'team'
        result = self.run_workflow()
        self.assertEqual(result['phase'], 'done')
        self.assertEqual(result['mode'], 'team')
        self.assertEqual(self.calls('coordinator'), 1)
        self.assertEqual(self.calls('architect'), 0)
        self.assertEqual(self.calls('developer'), 2)

    def test_completed_sibling_is_not_regenerated_after_other_failure(self):
        self.env['AE_FAKE_SCENARIO'] = 'team-fail'
        initial = self.run_workflow('--mode', 'team', success=False)
        # A transport error may return control or be retried by Rust immediately.
        if initial.returncode:
            result = self.run_workflow('--resume')
        else:
            result = json.loads(initial.stdout)
        self.assertEqual(result['phase'], 'done')
        self.assertEqual(int((self.artifacts / 'developer-a.calls').read_text()), 1)
        self.assertEqual(int((self.artifacts / 'developer-b.calls').read_text()), 2)
        self.assertEqual(result['model_calls'], 6)
        self.assertEqual(result['observed_total_tokens'], 660)

    def test_failed_check_is_attributed_without_regenerating_unaffected_module(self):
        self.env['AE_FAKE_SCENARIO'] = 'team-qa-failure'
        result = self.run_workflow('--mode', 'team', '--max-calls', '10')
        self.assertEqual(result['phase'], 'done')
        self.assertEqual(int((self.artifacts / 'developer-a.calls').read_text()), 2)
        self.assertEqual(int((self.artifacts / 'developer-b.calls').read_text()), 1)
        self.assertEqual(result['model_calls'], 8)
        second_attempt = (self.artifacts / 'developer-a-2.prompt').read_text()
        self.assertIn('check_values.py', second_attempt)
        self.assertIn('AssertionError', second_attempt)
        self.assertEqual(len(result['proofs']), 2)

    def test_architecture_blockers_repair_only_owner_and_preserve_sibling(self):
        for scenario in ('team-arch-block','team-arch-mixed'):
            with self.subTest(scenario=scenario):
                other=NativeOrchestrationTests(); other.setUp()
                try:
                    other.env['AE_FAKE_SCENARIO']=scenario
                    result=other.run_workflow('--mode','team')
                    self.assertEqual(result['phase'],'done')
                    self.assertEqual(int((other.artifacts/'developer-a.calls').read_text()),2)
                    self.assertEqual(int((other.artifacts/'developer-b.calls').read_text()),1)
                    self.assertEqual(result['model_calls'],7)
                    self.assertNotIn('subprocess',(other.root/'src/a.py').read_text())
                    retry=(other.artifacts/'developer-a-2.prompt').read_text()
                    self.assertIn('transport',retry)
                    self.assertIn('boundaries',retry)
                    qa=(other.artifacts/'qa-qa-1.prompt').read_text()
                    self.assertIn('architecture_review',qa)
                    self.assertIn('architecture_report',result)
                    self.assertTrue(result['architecture_report_fresh'])
                finally: other.doCleanups()

    def test_architecture_advisory_is_persisted_without_extra_calls_or_rounds(self):
        self.env['AE_FAKE_SCENARIO']='team-arch-advisory'
        result=self.run_workflow('--mode','team')
        self.assertEqual(result['phase'],'done')
        self.assertEqual(result['round'],1)
        self.assertEqual(result['model_calls'],5)
        self.assertEqual(result['architecture_report']['issues'][0]['severity'],'advisory')
        self.assertEqual(self.calls('developer'),2)
        resumed=self.run_workflow('--resume')
        self.assertEqual(resumed['model_calls'],5)
        self.assertEqual(resumed['architecture_report'],result['architecture_report'])
        self.assertIn('advisory',(self.artifacts/'qa-qa-1.prompt').read_text())

    def test_single_qa_architecture_repair_does_not_add_independent_reviewer(self):
        self.env['AE_FAKE_SCENARIO']='single-arch-block'
        result=self.run_workflow()
        self.assertEqual(result['phase'],'done')
        self.assertEqual(result['model_calls'],4)
        self.assertEqual(self.calls('coordinator'),1)
        self.assertEqual(self.calls('developer'),1)
        self.assertEqual(self.calls('qa'),2)
        self.assertEqual(self.calls('reviewer')+self.calls('architect'),0)
        self.assertNotIn('subprocess',(self.root/'src/a.py').read_text())
        self.assertTrue(result['architecture_report_fresh'])

    def test_missing_required_architecture_review_retries_only_review(self):
        self.env['AE_FAKE_SCENARIO']='team-arch-missing'
        result=self.run_workflow('--mode','team')
        self.assertEqual(result['phase'],'done')
        self.assertEqual(result['model_calls'],6)
        self.assertEqual(self.calls('developer'),2)
        self.assertEqual(self.calls('reviewer'),2)
        self.assertIn('architecture',(self.artifacts/'reviewer-reviewer-2.prompt').read_text())

    def test_discovered_tests_without_checks_stop_before_workflow_and_models(self):
        self.write('.ai-env/config.json', '{"checks":{}}')
        failed = self.run_workflow(success=False)
        self.assertNotEqual(failed.returncode, 0)
        self.assertEqual(list(self.artifacts.glob('*.calls')), [])
        self.assertFalse((self.root / '.ai-env/workflows/example.json').exists())
        self.configure()
        self.assertEqual(self.run_workflow()['phase'], 'done')

    def test_cumulative_call_budget_survives_resume(self):
        failed = self.run_workflow('--max-calls', '1', success=False)
        self.assertNotEqual(failed.returncode, 0)
        self.assertEqual(self.calls('coordinator'), 1)
        repeated = self.run_workflow('--resume', '--max-calls', '1', success=False)
        self.assertNotEqual(repeated.returncode, 0)
        self.assertEqual(self.calls('coordinator'), 1)
        result = self.run_workflow('--resume', '--max-calls', '2')
        self.assertEqual(result['phase'], 'done')
        self.assertEqual(result['model_calls'], 2)

    def test_failing_real_checks_never_complete_and_loop_is_bounded(self):
        self.env['AE_FAKE_SCENARIO'] = 'noop-failure'
        result = self.run_workflow('--rounds', '2', '--max-calls', '8', success=False)
        self.assertNotEqual(result.returncode, 0)
        state = self.cli('workflow', 'status', 'example')
        self.assertNotEqual(state['phase'], 'done')
        self.assertLessEqual(self.calls('coordinator') + self.calls('developer') + self.calls('qa'), 8)
        self.assertEqual(state['completed'], {})

    def test_resume_cannot_change_approved_checks(self):
        self.run_workflow()
        failed = self.run_workflow('--resume', '--check', 'unit', success=False)
        self.assertNotEqual(failed.returncode, 0)
        self.assertEqual(self.calls('coordinator') + self.calls('qa'), 2)

    def test_invalid_model_answer_is_charged_and_no_source_is_applied(self):
        self.env['AE_FAKE_SCENARIO'] = 'malformed'
        failed = self.run_workflow(success=False)
        self.assertNotEqual(failed.returncode, 0)
        self.assertEqual((self.root / 'src/a.py').read_text(), 'VALUE = 1\n')
        self.assertEqual(self.calls('coordinator'), 2)
        self.assertIn('without progress', failed.stderr)
        self.env['AE_FAKE_SCENARIO'] = 'single'
        result = self.run_workflow('--resume')
        self.assertEqual(result['phase'], 'done')
        self.assertEqual(result['model_calls'], 4)
        self.assertEqual(result['observed_total_tokens'], 440)

    def test_rejected_coordinator_edit_keeps_plan_and_repairs_task_automatically(self):
        self.env['AE_FAKE_SCENARIO'] = 'bad-edit'
        result = self.run_workflow()
        self.assertEqual(result['phase'], 'done')
        self.assertEqual(self.calls('coordinator'), 1)
        self.assertEqual(self.calls('developer'), 1)
        self.assertEqual(result['model_calls'], 3)
        prompt = next(self.artifacts.glob('developer-*.prompt')).read_text()
        self.assertIn('old must match exactly once', prompt)
        self.assertIn('response_diagnostics', prompt)

    def test_invalid_planning_response_repairs_without_manual_resume(self):
        for scenario in ('bad-scope-once', 'malformed-once', 'bad-shape-once', 'bad-snapshot-once'):
            with self.subTest(scenario=scenario):
                # Give each trajectory its own normal fixture.
                other = NativeOrchestrationTests()
                other.setUp()
                try:
                    other.env['AE_FAKE_SCENARIO'] = scenario
                    result = other.run_workflow()
                    self.assertEqual(result['phase'], 'done')
                    self.assertEqual(result['model_calls'], 3)
                    self.assertEqual(result['observed_total_tokens'], 330)
                    role = 'coordinator'
                    prompts = sorted(other.artifacts.glob(role + '-*.prompt'))
                    self.assertIn('response_diagnostics', prompts[-1].read_text())
                    rejection = list((other.root / '.ai-env/workflow-runs/example').glob('call-*/rejection.json'))
                    self.assertEqual(len(rejection), 1)
                finally:
                    other.doCleanups()

    def test_rejected_team_plan_does_not_commit_routing_before_validation(self):
        self.env['AE_FAKE_SCENARIO'] = 'auto-bad-team-once'
        result = self.run_workflow()
        self.assertEqual(result['mode'], 'single')
        self.assertEqual(self.calls('coordinator'), 2)
        self.assertEqual(self.calls('architect'), 0)
        self.assertEqual(result['model_calls'], 3)

    def test_invalid_developer_response_repairs_without_regenerating_sibling(self):
        self.env['AE_FAKE_SCENARIO'] = 'team-invalid-once'
        result = self.run_workflow('--mode', 'team')
        self.assertEqual(result['phase'], 'done')
        self.assertEqual(int((self.artifacts/'developer-a.calls').read_text()), 1)
        self.assertEqual(int((self.artifacts/'developer-b.calls').read_text()), 2)
        self.assertEqual(result['model_calls'], 6)
        self.assertIn('mismatched snapshot', (self.artifacts/'developer-b-2.prompt').read_text())

    def test_repeated_invalid_scope_stops_without_source_or_check_changes(self):
        self.env['AE_FAKE_SCENARIO'] = 'bad-scope-repeat'
        failed = self.run_workflow(success=False)
        self.assertNotEqual(failed.returncode, 0)
        self.assertIn('without progress', failed.stderr)
        self.assertEqual(self.calls('coordinator'), 2)
        self.assertEqual((self.root/'src/a.py').read_text(), 'VALUE = 1\n')
        self.assertEqual(self.calls('qa'), 0)
        self.assertEqual(self.cli('workflow','status','example')['completed'], {})

    def test_repair_budget_is_cumulative_and_resume_can_accept_corrected_answer(self):
        self.env['AE_FAKE_SCENARIO'] = 'bad-scope-once'
        failed = self.run_workflow('--max-calls', '1', success=False)
        self.assertNotEqual(failed.returncode, 0)
        self.assertEqual(self.calls('coordinator'), 1)
        result = self.run_workflow('--resume', '--max-calls', '3')
        self.assertEqual(result['phase'], 'done')
        self.assertEqual(result['model_calls'], 3)
        self.assertEqual(result['observed_total_tokens'], 330)

    def test_grouped_local_edits_share_original_hash_and_apply_sequentially(self):
        self.env['AE_FAKE_SCENARIO'] = 'grouped'
        result = self.run_workflow()
        self.assertEqual(result['phase'], 'done')
        self.assertEqual(result['model_calls'], 2)
        self.assertEqual((self.root/'src/a.py').read_text(), 'VALUE = 2\n')

    def test_hashless_model_answer_is_bound_from_private_durable_inputs(self):
        self.run_workflow()
        directory = self.root / '.ai-env/workflow-runs/example'
        ledger = json.loads((directory / 'budget.json').read_text())
        folder = directory / ledger['entries'][0]['artifact']
        answer = json.loads((folder / 'answer.json').read_text())
        inputs = json.loads((folder / 'inputs.json').read_text())
        self.assertTrue(all('expected_hash' not in edit for edit in answer['edits']))
        self.assertEqual(inputs['edit_base']['files']['src/a.py'], hashlib.sha256(b'VALUE = 1\n').hexdigest())
        prompt = (self.artifacts / 'coordinator-coordinator-1.prompt').read_text()
        self.assertNotIn(inputs['edit_base']['digest'], prompt)
        self.assertNotIn(inputs['edit_base']['files']['src/a.py'], prompt)
        state = self.cli('workflow', 'status', 'example')
        self.assertTrue(state['verification_report_fresh'])
        self.assertEqual(state['verification_report']['results'][0]['status'], 'verified')

    def test_saved_hashless_answer_replays_without_redispatch(self):
        self.cli('workflow', 'start', 'example', '--request', self.request, '--mode', 'auto')
        state_path = self.root / '.ai-env/workflows/example.json'
        planning = state_path.read_bytes()
        failed = self.run_workflow('--resume', '--max-calls', '1', success=False)
        self.assertNotEqual(failed.returncode, 0)
        directory = self.root / '.ai-env/workflow-runs/example'
        ledger_path = directory / 'budget.json'
        ledger = json.loads(ledger_path.read_text())
        folder = directory / ledger['entries'][0]['artifact']
        original_inputs = (folder / 'inputs.json').read_bytes()
        # Restore the exact durable state at a crash after the model receipt,
        # before plan/proposal/application. The saved raw answer has no hashes.
        self.write('src/a.py', 'VALUE = 1\n')
        self.write('src/b.py', 'VALUE = 1\n')
        state_path.write_bytes(planning)
        ledger['entries'][0]['processed'] = False
        ledger_path.write_text(json.dumps(ledger))
        result = self.run_workflow('--resume', '--max-calls', '2')
        self.assertEqual(result['phase'], 'done')
        self.assertEqual(result['model_calls'], 2)
        self.assertEqual(self.calls('coordinator'), 1)
        self.assertEqual((folder / 'inputs.json').read_bytes(), original_inputs)

    def test_invalid_obligation_report_retries_only_qa_and_reuses_checks(self):
        for scenario in ('missing-verification-once', 'forged-verification-once',
                         'outside-verification-once', 'duplicate-verification-once'):
            with self.subTest(scenario=scenario):
                other = NativeOrchestrationTests(); other.setUp()
                try:
                    other.env['AE_FAKE_SCENARIO'] = scenario
                    result = other.run_workflow()
                    self.assertEqual(result['phase'], 'done')
                    self.assertEqual(result['model_calls'], 3)
                    self.assertEqual(other.calls('coordinator'), 1)
                    self.assertEqual(other.calls('qa'), 2)
                    self.assertEqual(len(list((other.root / '.ai-env/proofs').glob('*.json'))), 2)
                    self.assertTrue(result['verification_report_fresh'])
                finally: other.doCleanups()

    def test_planning_read_repair_preserves_original_implementation(self):
        self.env['AE_FAKE_SCENARIO'] = 'bad-read-once'
        result = self.run_workflow()
        self.assertEqual(result['phase'], 'done')
        self.assertEqual(result['model_calls'], 3)
        self.assertEqual(self.calls('coordinator'), 2)
        self.assertEqual(self.calls('developer'), 0)
        directory = self.root / '.ai-env/workflow-runs/example'
        ledger = json.loads((directory / 'budget.json').read_text())
        first, second = [directory / e['artifact'] for e in ledger['entries'][:2]]
        original = json.loads((first / 'answer.json').read_text())
        correction = json.loads((second / 'answer.json').read_text())
        inputs = json.loads((second / 'inputs.json').read_text())
        self.assertIn('metadata_repair', inputs)
        self.assertEqual(inputs['metadata_repair']['origin'], first.name)
        self.assertEqual(inputs['metadata_repair']['answer_sha256'], hashlib.sha256((first / 'answer.json').read_bytes()).hexdigest())
        self.assertEqual(set(correction), {'snapshot', 'selections'})
        self.assertIn('edits', original)
        self.assertNotIn('edits', correction)
        prompt = (self.artifacts / 'coordinator-coordinator-2.prompt').read_text()
        self.assertNotIn('Current source packet:', prompt)
        self.assertNotIn('VALUE = 2', prompt)
        self.assertNotIn('Project style and tests', prompt)
        self.assertEqual((self.root / 'src/a.py').read_text(), 'VALUE = 2\n')

    def test_qa_reference_repair_replays_without_rerunning_assessment_or_checks(self):
        self.env['AE_FAKE_SCENARIO'] = 'source-ref-replay'
        first = self.run_workflow()
        self.assertEqual(first['phase'], 'done')
        self.assertEqual(first['model_calls'], 3)
        directory = self.root / '.ai-env/workflow-runs/example'
        ledger_path = directory / 'budget.json'
        ledger = json.loads(ledger_path.read_text())
        entry = ledger['entries'][-1]
        folder = directory / entry['artifact']
        inputs_before = (folder / 'inputs.json').read_bytes()
        self.assertIn('metadata_repair', json.loads(inputs_before))
        self.assertNotIn('Current source packet:', (folder / 'prompt.txt').read_text())
        (self.root / '.ai-env/workflows/example.json').write_bytes((self.artifacts / 'qa-running-workflow.json').read_bytes())
        entry['processed'] = False
        ledger_path.write_text(json.dumps(ledger))
        resumed = self.run_workflow('--resume', '--max-calls', '3')
        self.assertEqual(resumed['phase'], 'done')
        self.assertEqual(resumed['model_calls'], 3)
        self.assertEqual(resumed['verification_report'], first['verification_report'])
        self.assertEqual((folder / 'inputs.json').read_bytes(), inputs_before)
        self.assertEqual(len(list((self.root / '.ai-env/proofs').glob('*.json'))), 2)

    def test_changed_origin_cannot_be_substituted_during_reference_repair_replay(self):
        self.env['AE_FAKE_SCENARIO'] = 'source-ref-replay'
        self.run_workflow()
        directory = self.root / '.ai-env/workflow-runs/example'
        ledger_path = directory / 'budget.json'
        ledger = json.loads(ledger_path.read_text())
        entry = ledger['entries'][-1]
        inputs = json.loads((directory / entry['artifact'] / 'inputs.json').read_text())
        origin = directory / inputs['metadata_repair']['origin'] / 'answer.json'
        original = json.loads(origin.read_text()); original['assessment'] = ['Substituted conclusion']
        origin.write_text(json.dumps(original))
        (self.root / '.ai-env/workflows/example.json').write_bytes((self.artifacts / 'qa-running-workflow.json').read_bytes())
        entry['processed'] = False; ledger_path.write_text(json.dumps(ledger))
        failed = self.run_workflow('--resume', '--max-calls', '3', success=False)
        self.assertNotEqual(failed.returncode, 0)
        self.assertNotEqual(self.cli('workflow','status','example')['phase'], 'done')
        self.assertEqual(self.calls('qa'), 2)
        self.assertEqual((self.root / 'src/a.py').read_text(), 'VALUE = 2\n')

    def test_changed_contract_rejects_saved_source_references_without_source_changes(self):
        self.env['AE_FAKE_SCENARIO'] = 'source-ref-replay'
        self.run_workflow()
        directory = self.root / '.ai-env/workflow-runs/example'
        ledger_path = directory / 'budget.json'
        ledger = json.loads(ledger_path.read_text())
        original = (self.artifacts / 'qa-running-workflow.json').read_text()
        for field in ('risk', 'instructions', 'architecture'):
            with self.subTest(field=field):
                workflow = json.loads(original)
                if field == 'risk':
                    workflow['plan']['verification'][0]['risk'] += ' Changed contract.'
                elif field == 'instructions':
                    workflow['plan']['tasks'][0]['instructions'] += ' Changed contract.'
                else:
                    workflow['plan']['architecture']['boundaries'].append('Changed contract.')
                (self.root / '.ai-env/workflows/example.json').write_text(json.dumps(workflow))
                ledger['entries'][-1]['processed'] = False
                ledger_path.write_text(json.dumps(ledger))
                failed = self.run_workflow('--resume', '--max-calls', '3', success=False)
                self.assertNotEqual(failed.returncode, 0)
                rejection = json.loads((directory / ledger['entries'][-1]['artifact'] / 'rejection.json').read_text())
                self.assertIn('contract changed since dispatch', rejection['error'])
                self.assertNotEqual(self.cli('workflow', 'status', 'example')['phase'], 'done')
                self.assertEqual(self.calls('qa'), 2)
                self.assertEqual((self.root / 'src/a.py').read_text(), 'VALUE = 2\n')

    def test_unresolved_reference_needs_substantive_qa_and_reuses_fresh_checks(self):
        self.env['AE_FAKE_SCENARIO'] = 'metadata-unresolved'
        result = self.run_workflow()
        self.assertEqual(result['phase'], 'done')
        self.assertEqual(result['model_calls'], 4)
        self.assertEqual(self.calls('coordinator'), 1)
        self.assertNotIn('Current source packet:', (self.artifacts/'qa-qa-2.prompt').read_text())
        self.assertIn('Current source packet:', (self.artifacts/'qa-qa-3.prompt').read_text())
        self.assertEqual(len(list((self.root/'.ai-env/proofs').glob('*.json'))), 2)

    def test_repeated_bad_reference_patch_stops_without_reimplementing_code(self):
        self.env['AE_FAKE_SCENARIO'] = 'metadata-repeat'
        result = self.run_workflow(success=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('without progress', result.stderr)
        self.assertEqual(self.calls('coordinator'), 1)
        self.assertLessEqual(self.calls('qa'), 3)
        self.assertNotEqual(self.cli('workflow','status','example')['phase'], 'done')
        self.assertEqual((self.root/'src/a.py').read_text(), 'VALUE = 2\n')

    def test_developer_receipts_replay_from_original_private_bases_and_reject_races(self):
        for concurrent_change in (False, True):
            with self.subTest(concurrent_change=concurrent_change):
                other = NativeOrchestrationTests(); other.setUp()
                try:
                    other.env['AE_FAKE_SCENARIO'] = 'team-replay'
                    paused = other.run_workflow('--mode', 'team', '--max-calls', '3', success=False)
                    self.assertNotEqual(paused.returncode, 0)
                    directory = other.root / '.ai-env/workflow-runs/example'
                    ledger_path = directory / 'budget.json'
                    ledger = json.loads(ledger_path.read_text())
                    saved = {}
                    for entry in ledger['entries']:
                        if entry['role'] == 'developer':
                            entry['processed'] = False
                            inputs = directory / entry['artifact'] / 'inputs.json'
                            saved[inputs] = inputs.read_bytes()
                    self.assertEqual(len(saved), 2)
                    # Receipt/answer are durable, both attempts were running,
                    # and no host application was committed at this crashpoint.
                    other.write('src/a.py', 'VALUE = 99\n' if concurrent_change else 'VALUE = 1\n')
                    other.write('src/b.py', 'VALUE = 1\n')
                    (other.root / '.ai-env/workflows/example.json').write_bytes(
                        (other.artifacts / 'a-running-workflow.json').read_bytes())
                    ledger_path.write_text(json.dumps(ledger))
                    if concurrent_change:
                        failed = other.run_workflow('--resume', '--max-calls', '5', success=False)
                        self.assertNotEqual(failed.returncode, 0)
                        self.assertEqual((other.root / 'src/a.py').read_text(), 'VALUE = 99\n')
                        self.assertEqual((other.root / 'src/b.py').read_text(), 'VALUE = 1\n')
                        self.assertEqual(other.calls('reviewer') + other.calls('qa'), 0)
                    else:
                        result = other.run_workflow('--resume', '--max-calls', '5')
                        self.assertEqual(result['phase'], 'done')
                        self.assertEqual(result['model_calls'], 5)
                        self.assertEqual((other.root / 'src/a.py').read_text(), 'VALUE = 2\n')
                    self.assertEqual(other.calls('developer'), 2)
                    for path, contents in saved.items():
                        self.assertEqual(path.read_bytes(), contents)
                finally: other.doCleanups()

    def test_qa_packet_targets_risks_and_source_without_global_test_inventory(self):
        for index in range(80):
            self.write(f'tests/unrelated/test_inventory_{index:03}.py', '# unrelated discovery fixture\n')
        self.write('unrelated/decoy.py', 'UNRELATED_SOURCE_SENTINEL = 1\n')
        self.run_workflow()
        prompt = (self.artifacts / 'qa-qa-1.prompt').read_text()
        metadata = next(json.loads(line) for line in prompt.splitlines()
                        if line.startswith('{') and '"goal"' in line)
        self.assertEqual(metadata['qa_focus']['selected_obligation_ids'], ['requested-values'])
        self.assertEqual(metadata['changed_paths'], ['src/a.py', 'src/b.py'])
        self.assertNotIn('test_inventory_079.py', prompt)
        self.assertNotIn('UNRELATED_SOURCE_SENTINEL', prompt)
        self.assertIn('VALUE = 2', prompt)
        self.assertIn('VALUE = 4', prompt)
        self.assertIn('check_values.py', prompt)
        self.assertIn('check_total.py', prompt)

    def test_project_without_checks_is_reported_as_static_inspection(self):
        (self.root / 'tests/unit/check_values.py').unlink()
        (self.root / 'tests/integration/check_total.py').unlink()
        self.write('.ai-env/config.json', '{"checks":{}}')
        result = self.run_workflow()
        self.assertEqual(result['phase'], 'done')
        self.assertEqual(result['verification_execution'], 'static_only')
        report = result['verification_report']['results'][0]
        self.assertEqual(report['status'], 'inspected')
        self.assertEqual(report['proofs'], [])
        self.assertEqual(result['proofs'], [])
        self.assertTrue(result['verification_report_fresh'])

    def test_rejected_review_evidence_retries_only_reviewer(self):
        self.env['AE_FAKE_SCENARIO'] = 'team-review-once'
        result = self.run_workflow('--mode','team')
        self.assertEqual(result['phase'], 'done')
        self.assertEqual(self.calls('architect'), 1)
        self.assertEqual(self.calls('developer'), 2)
        self.assertEqual(self.calls('reviewer'), 2)
        self.assertEqual(self.calls('qa'), 1)
        self.assertEqual(result['model_calls'], 6)

    def test_unknown_qa_proof_is_rejected_then_repaired_without_implementation_replay(self):
        self.env['AE_FAKE_SCENARIO'] = 'bad-proof-once'
        result = self.run_workflow()
        self.assertEqual(result['phase'], 'done')
        self.assertEqual(self.calls('coordinator'), 1)
        self.assertEqual(self.calls('qa'), 2)
        self.assertEqual(result['model_calls'], 3)
        self.assertIn('absent from actual', (self.artifacts/'qa-qa-2.prompt').read_text())

    def test_rejected_replayed_report_does_not_wedge_future_resumes(self):
        self.env['AE_FAKE_SCENARIO'] = 'noop-accept'
        failed = self.run_workflow(success=False)
        self.assertNotEqual(failed.returncode, 0)
        ledger_path = self.root / '.ai-env/workflow-runs/example/budget.json'
        ledger = json.loads(ledger_path.read_text())
        self.assertEqual(ledger['entries'][-1]['role'], 'qa')
        # Simulate losing the host immediately after the model receipt, before
        # the response's rejection was acknowledged in the dispatch ledger.
        ledger['entries'][-1]['processed'] = False
        ledger_path.write_text(json.dumps(ledger))
        replay = self.run_workflow('--resume', success=False)
        self.assertNotEqual(replay.returncode, 0)
        self.assertTrue(json.loads(ledger_path.read_text())['entries'][-1]['processed'])
        self.env['AE_FAKE_SCENARIO'] = 'single'
        result = self.run_workflow('--resume')
        self.assertEqual(result['phase'], 'done')
        self.assertEqual(self.calls('coordinator'), 1)

    def test_unknown_usage_is_not_zero_and_blocks_token_limited_dispatch(self):
        self.env['AE_FAKE_SCENARIO'] = 'unknown-usage'
        failed = self.run_workflow('--max-tokens', '1000', success=False)
        self.assertNotEqual(failed.returncode, 0)
        self.assertEqual(self.calls('coordinator'), 1)
        self.assertEqual(self.calls('qa'), 0)
        self.env['AE_FAKE_SCENARIO'] = 'single'
        still_blocked = self.run_workflow('--resume', '--max-tokens', '1000', success=False)
        self.assertNotEqual(still_blocked.returncode, 0)
        self.assertEqual(self.calls('qa'), 0)
        result = self.run_workflow('--resume')
        self.assertEqual(result['phase'], 'done')
        self.assertIsNone(result['observed_total_tokens'])

    def test_model_timeout_retains_failed_receipt_and_allows_resume(self):
        self.env['AE_FAKE_SCENARIO'] = 'timeout'
        failed = self.run_workflow('--timeout', '1', success=False)
        self.assertNotEqual(failed.returncode, 0)
        receipts = list((self.root / '.ai-env/workflow-runs/example').glob('call-*/result.json'))
        self.assertEqual(len(receipts), 1)
        receipt = json.loads(receipts[0].read_text())
        self.assertIn('timed out', receipt['error'])
        self.assertIsNone(receipt['usage'])
        self.env['AE_FAKE_SCENARIO'] = 'single'
        result = self.run_workflow('--resume')
        self.assertEqual(result['phase'], 'done')
        self.assertEqual(result['model_calls'], 3)
        self.assertIsNone(result['observed_total_tokens'])

    def test_parent_loss_reaps_model_and_parallel_runner_preserves_ledger(self):
        self.env['AE_FAKE_SCENARIO'] = 'parent-loss'
        command = [str(BINARY), '--root', str(self.root), 'orchestrate', 'example',
                   '--request', str(self.request), '--timeout', '30']
        process = subprocess.Popen(command, env=self.env, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, text=True)
        try:
            pidfile = self.artifacts / 'model.pid'
            deadline = time.monotonic() + 10
            while not pidfile.exists() and time.monotonic() < deadline:
                time.sleep(.03)
            self.assertTrue(pidfile.exists(), 'Model did not start')
            model_pid = int(pidfile.read_text())
            ledger = self.root / '.ai-env/workflow-runs/example/budget.json'
            before = ledger.read_bytes()
            competing = self.run_workflow('--resume', success=False)
            self.assertNotEqual(competing.returncode, 0)
            self.assertEqual(ledger.read_bytes(), before)
            process.kill()
            process.communicate(timeout=5)
            self.env['AE_FAKE_SCENARIO'] = 'single'
            result = self.run_workflow('--resume')
            self.assertEqual(result['phase'], 'done')
            self.assertEqual(result['model_calls'], 3)
            with self.assertRaises(ProcessLookupError):
                os.kill(model_pid, 0)
            receipts = [json.loads(p.read_text()) for p in
                        (self.root / '.ai-env/workflow-runs/example').glob('call-*/result.json')]
            self.assertTrue(any(r.get('error') and r['usage'] is None and r['exit_code'] != 0
                                for r in receipts))
        finally:
            if process.poll() is None:
                process.kill()
            process.communicate(timeout=10)

    def test_linked_budget_file_cannot_overwrite_outside_file(self):
        runs = self.root / '.ai-env/workflow-runs/example'
        runs.mkdir(parents=True)
        sentinel = self.base / 'sentinel.json'
        content = '{"entries":[]}\n'
        sentinel.write_text(content)
        (runs / 'budget.json').symlink_to(sentinel)
        failed = self.run_workflow(success=False)
        self.assertNotEqual(failed.returncode, 0)
        self.assertEqual(sentinel.read_text(), content)
        self.assertEqual(self.calls('coordinator'), 0)

    def test_reject_unsafe_runner_artifact_paths(self):
        outside = self.base / 'outside'
        outside.mkdir()
        runs = self.root / '.ai-env/workflow-runs'
        runs.mkdir(exist_ok=True)
        (runs / 'example').symlink_to(outside, target_is_directory=True)
        failed = self.run_workflow(success=False)
        self.assertNotEqual(failed.returncode, 0)
        self.assertEqual(list(outside.iterdir()), [])
        self.assertEqual(self.calls('coordinator'), 0)


if __name__ == '__main__':
    unittest.main()
