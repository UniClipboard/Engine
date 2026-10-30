"""使用真实 Git diff 验证分类、重命名、模式与回退边界。"""
import importlib.util
import itertools
import os
import json
import re
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('scope', Path(__file__).with_name('pr-scope.py'))
policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(policy)


class ScopeTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.git('init', '-q')
        self.git('config', 'user.name', 'tars623')
        self.git('config', 'user.email', 'github-tars-bot@uniclipboard.app')
        for name in ('docs/old.md', 'crates/product.rs', *policy.SNAPSHOT_FILES):
            self.write(name)
        self.commit()
        self.base = self.git('rev-parse', 'HEAD').decode().strip()

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.root), *args])

    def write(self, name, text='baseline\n'):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def commit(self):
        self.git('add', '.')
        self.git('commit', '-qm', 'test fixture')

    def result(self):
        self.commit()
        return policy.classify(self.git('diff', '--raw', '--no-abbrev', '-z', '--no-renames', self.base + '...HEAD'))

    def test_docs(self):
        self.write('docs/new name\nwith-newline.md')
        self.assertEqual(self.result(), 'docs')

    def test_example_347(self):
        self.write('docs/design-docs/local-builds.md')
        for name in policy.SNAPSHOT_FILES:
            self.write(name, 'changed\n')
        self.assertEqual(self.result(), 'snapshot')

    def test_mixed_product_and_docs(self):
        self.write('docs/new.md')
        self.write('crates/product.rs', 'changed\n')
        self.assertEqual(self.result(), 'full')

    def test_unknown_scripts_and_classifier(self):
        for name in ('scripts/new.py', 'scripts/build-cache/new.py', 'scripts/ci/pr-scope.py',
                     '.github/workflows/pr-check.yml', 'docs/generated/schema.md', 'docs/AGENTS.md',
                     'docs/design-docs/observability.md'):
            with self.subTest(name=name):
                raw = b':000000 100644 ' + b'0'*40 + b' ' + b'a'*40 + b' A\0' + name.encode() + b'\0'
                self.assertEqual(policy.classify(raw), 'full')

    def test_product_renamed_to_docs(self):
        self.git('mv', 'crates/product.rs', 'docs/moved.md')
        self.assertEqual(self.result(), 'full')

    def test_docs_renamed_to_product(self):
        self.git('mv', 'docs/old.md', 'crates/moved.rs')
        self.assertEqual(self.result(), 'full')

    def test_document_rename(self):
        self.git('mv', 'docs/old.md', 'docs/new.md')
        self.assertEqual(self.result(), 'docs')

    def test_type_change(self):
        path = self.root / 'docs/old.md'
        path.unlink()
        path.symlink_to('../crates/product.rs')
        self.assertEqual(self.result(), 'full')

    def test_executable_document(self):
        self.git('update-index', '--chmod=+x', 'docs/old.md')
        raw = self.git('diff', '--cached', '--raw', '--no-abbrev', '-z', '--no-renames')
        self.assertEqual(policy.classify(raw), 'full')

    def test_empty_and_malformed(self):
        for raw in (b'', b'broken\0', b'broken\0docs/x.md\0', b'broken'):
            self.assertEqual(policy.classify(raw), 'full')

    def test_event_and_full_ci_overrides(self):
        for event in ('push', 'workflow_dispatch', 'merge_group', 'schedule', ''):
            self.assertEqual(policy.scope(event, {}), 'full')
        self.assertEqual(policy.scope('pull_request', {'pull_request': {'labels': [{'name': 'full-ci'}]}}), 'full')

    def test_real_pr_event_and_missing_history(self):
        self.write('docs/new.md')
        self.commit()
        event = {'pull_request': {'labels': [], 'base': {'sha': self.base},
                                 'head': {'sha': self.git('rev-parse', 'HEAD').decode().strip()}}}
        original = os.getcwd()
        try:
            os.chdir(self.root)
            self.assertEqual(policy.scope('pull_request', event), 'docs')
        finally:
            os.chdir(original)
        with patch.object(policy.subprocess, 'check_output', side_effect=subprocess.CalledProcessError(128, 'git')):
            with self.assertRaises(subprocess.CalledProcessError):
                policy.scope('pull_request', event)


class WorkflowTests(unittest.TestCase):
    root = Path(__file__).resolve().parents[2]

    def test_scope_failure_defaults_and_cli(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'output'
            event = Path(directory) / 'event.json'
            for payload in ('invalid json', '{}', json.dumps({'pull_request': {'labels': []}})):
                event.write_text(payload)
                output.write_text('')
                subprocess.run(['python3', str(self.root / 'scripts/ci/pr-scope.py')], check=True,
                               env=dict(os.environ, GITHUB_EVENT_NAME='pull_request',
                                        GITHUB_EVENT_PATH=str(event), GITHUB_OUTPUT=str(output)))
                self.assertEqual(output.read_text(), 'scope=full\n')

    def test_product_job_conditions_fail_open_to_full_checks(self):
        text = (self.root / '.github/workflows/pr-check.yml').read_text()
        for job in ('upgrade-matrix-smoke', 'tests', 'peer-reachability', 'blob-connectivity', 'mobile-bindings'):
            # 直接执行实际 YAML 中的表达式，覆盖空输出、标签与失败/取消。
            expression = re.search(r'^    if: (.+)$', text.split('  ' + job + ':\n')[1], re.M)[1]
            for scope in ('full', '', 'unknown', 'docs', 'snapshot'):
                for action, label in (('synchronize', ''), ('labeled', 'full-ci'), ('labeled', 'other')):
                    for cancelled, scope_result in itertools.product((True, False), ('success', 'failure', 'skipped')):
                        values = {'always()': True, '!cancelled()': not cancelled,
                                  'needs.scope.result': scope_result, 'needs.scope.outputs.scope': scope, 'github.event.action': action,
                                  'github.event.label.name': label}
                        actual = expression
                        for key, value in values.items():
                            actual = actual.replace(key, repr(value))
                        actual = eval(actual.replace('&&', ' and ').replace('||', ' or '), {'__builtins__': {}})
                        self.assertEqual(actual, not cancelled and (action != 'labeled' or label == 'full-ci')
                                         and (scope_result != 'success' or scope not in ('docs', 'snapshot')))
        self.assertIn('needs: [scope, snapshot-linux]', text)
        self.assertIn('"$LINUX_RESULT" != success', text)

    def test_no_trigger_path_filter_or_permission_expansion(self):
        for filename in ('pr-check.yml', 'windows-build.yml'):
            text = (self.root / '.github/workflows' / filename).read_text()
            self.assertNotIn('paths:', text)
            self.assertNotIn('paths-ignore:', text)
            self.assertIn('merge_group:', text)
            self.assertIn('workflow_dispatch:', text)
            self.assertIn('contents: read', text)
            self.assertNotIn('pull_request_target:', text)
        scope = (self.root / '.github/workflows/pr-scope.yml').read_text()
        self.assertIn('git show "$BASE_SHA:scripts/ci/pr-scope.py"', scope)
        self.assertIn("echo 'scope=full'", scope)
        self.assertIn('persist-credentials: false', scope)
        sdk = (self.root / '.github/actions/snapshot-tests/action.yml').read_text()
        self.assertNotIn('continue-on-error', sdk)
        self.assertIn('boto3==1.40.0', sdk)
        self.assertIn('PYTHONPATH:', sdk)
        self.assertIn('mbx-snapshot.integration.py', sdk)


if __name__ == '__main__':
    unittest.main()
