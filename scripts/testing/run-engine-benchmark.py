#!/usr/bin/env python3
"""Run immutable same-machine process measurements and retain admission deferrals."""
import argparse
import hashlib
import json
import pathlib
import statistics
import subprocess
import time
from datetime import datetime, timezone

parser = argparse.ArgumentParser()
parser.add_argument('--harness', required=True, type=pathlib.Path)
parser.add_argument('--baseline-host', required=True, type=pathlib.Path)
parser.add_argument('--candidate-host', type=pathlib.Path)
parser.add_argument('--reference-only', action='store_true')
parser.add_argument('--output', required=True, type=pathlib.Path)
parser.add_argument('--acceptance-only', action='store_true')
parser.add_argument('--environment-wait-seconds', type=int, default=3600,
                    help='Maximum aggregate time waiting for an idle host')
parser.add_argument('--scenarios', nargs='+', default=['startup', 'disconnected', 'connected', 'text'],
                    choices=['startup', 'disconnected', 'connected', 'text'])
args = parser.parse_args()
if len(set(args.scenarios)) != len(args.scenarios):
    parser.error('duplicate --scenarios values are not allowed')
if args.reference_only == (args.candidate_host is not None):
    parser.error('select --reference-only or provide --candidate-host')
if not 0 <= args.environment_wait_seconds <= 7200:
    parser.error('environment wait must be between 0 and 7200 seconds')

hosts = {'a': args.baseline_host.resolve()}
if args.candidate_host is not None:
    hosts['b'] = args.candidate_host.resolve()
provenance = {}
for role, binary in hosts.items():
    manifest = json.loads(binary.with_suffix('.manifest.json').read_text())
    if manifest['binary_sha256'] != hashlib.sha256(binary.read_bytes()).hexdigest():
        raise SystemExit('binary identity mismatch')
    if manifest['profile'] != 'release':
        raise SystemExit('release build required')
    provenance[role] = manifest
if not args.reference_only:
    for key in ['host_source_sha256', 'features', 'rustc', 'profile', 'release_profile',
                'cargo_config_sha256', 'rustflags_sha256']:
        if provenance['a'][key] != provenance['b'][key]:
            raise SystemExit(f'incompatible build: {key}')

args.output.mkdir(parents=True, exist_ok=False)
harness = str(args.harness.resolve())
runs = []
events = []
failures = []
reference_environment = None
wait_remaining = float(args.environment_wait_seconds)


def now():
    return datetime.now(timezone.utc).isoformat()


def summarize():
    result = {}
    for scenario in args.scenarios:
        result[scenario] = {}
        for role in hosts:
            metrics = {}
            for row in runs:
                if row['warmup'] or row['scenario'] != scenario or row['role'] != role or not row['valid']:
                    continue
                directory = args.output / row['directory']
                work = json.loads((directory / 'work.json').read_text())
                measured = json.loads((directory / 'metrics.json').read_text())
                for endpoint in ['a', 'b']:
                    values = {
                        f'{endpoint}_startup_ns': work[f'{endpoint}_startup_ns'],
                        f'{endpoint}_process_max_rss_bytes': measured[endpoint]['observed_max_rss'],
                        f'{endpoint}_process_median_rss_bytes': measured[endpoint]['median_rss'],
                        f'{endpoint}_process_cpu_ns': measured[endpoint]['cpu_delta_ns'],
                    }
                    for phase, label in [('warmup_start', 'warmup'), ('work_start', 'work'), ('work_end', 'recovery')]:
                        if phase in measured[endpoint]['phases']:
                            window = measured[endpoint]['phases'][phase]
                            values.update({f'{endpoint}_{label}_median_rss_bytes': window['median_rss'],
                                           f'{endpoint}_{label}_cpu_ns': window['cpu_delta_ns']})
                    for key, value in values.items():
                        metrics.setdefault(key, []).append(value)
            result[scenario][role] = {
                key: {'median': statistics.median(values), 'min': min(values),
                      'max': max(values), 'n': len(values)} for key, values in metrics.items()
            }
    return result


def validate_summary(summary):
    for scenario, roles in summary.items():
        for role, metrics in roles.items():
            for metric, value in metrics.items():
                if value['n'] != 6:
                    raise RuntimeError(
                        f'{scenario}/{role}/{metric}: expected 6 observations, got {value["n"]}')


def checkpoint(complete=False):
    summary = summarize()
    if complete:
        validate_summary(summary)
    value = {'schema_version': 1, 'passed': complete and not failures, 'complete': complete,
             'acceptance_only': args.acceptance_only, 'reference_only': args.reference_only,
             'reference_approved': False, 'state': 'passed' if complete and not failures else 'failed' if failures else 'running',
             'updated_utc': now(), 'runs': runs,
             'environment_events': events, 'failures': failures, 'summary': summary,
             'provenance': provenance, 'environment_wait_remaining_seconds': wait_remaining}
    pending = args.output / 'suite.pending'
    pending.write_text(json.dumps(value, indent=2))
    pending.replace(args.output / 'suite.json')
    lines = ['# Engine host benchmark', '',
             f'Complete suite: {complete}. Acceptance only: {args.acceptance_only}.',
             'Reference approval is manual. Failed invocations and admission deferrals remain recorded.', '',
             'Six measured runs per executable and scenario are required. Warmups are excluded.', '']
    if args.reference_only:
        lines += ['| Scenario | Metric | Reference median | Min | Max | n |',
                  '|---|---|---:|---:|---:|---:|']
        for scenario, roles in summary.items():
            for key, metric in roles['a'].items():
                lines.append(f"| {scenario} | {key} | {metric['median']} | {metric['min']} | {metric['max']} | {metric['n']} |")
    else:
        lines += ['| Scenario | Metric | Main median | Candidate median | Delta | n (Main/Candidate) |',
                  '|---|---|---:|---:|---:|---|']
        for scenario, roles in summary.items():
            for key in sorted(roles['a'].keys() | roles['b'].keys()):
                a, b = roles['a'].get(key), roles['b'].get(key)
                if a is not None and b is not None:
                    lines.append(f"| {scenario} | {key} | {a['median']} | {b['median']} | {b['median']-a['median']} | {a['n']}/{b['n']} |")
    (args.output / 'report.md').write_text('\n'.join(lines) + '\n')


def environment():
    response = subprocess.run([harness, 'environment'], check=True, capture_output=True, text=True, timeout=30)
    value = json.loads(response.stdout)
    if not isinstance(value['blocking_workloads'], int) or value['blocking_workloads'] < 0:
        raise RuntimeError('invalid environment probe')
    return value['blocking_workloads']


def wait_for_environment(scenario, index):
    global wait_remaining
    if args.acceptance_only:
        return
    blocked = False
    quiet_since = None
    while True:
        started = time.monotonic()
        count = environment()
        if count == 0:
            if not blocked:
                return
            if quiet_since is None:
                quiet_since = time.monotonic()
            if time.monotonic() - quiet_since >= 10:
                events.append({'scenario': scenario, 'index': index, 'state': 'resumed', 'at': now()})
                checkpoint()
                return
        else:
            quiet_since = None
            if not blocked:
                events.append({'scenario': scenario, 'index': index, 'state': 'deferred',
                               'blocking_workloads': count, 'at': now()})
                checkpoint()
            blocked = True
        if wait_remaining <= 0:
            raise RuntimeError('environment wait exhausted')
        time.sleep(min(5, wait_remaining))
        wait_remaining -= time.monotonic() - started


def run_one(scenario, index, role):
    global reference_environment
    output = args.output / f'{scenario}-{index:02}-{role}'
    while True:
        wait_for_environment(scenario, index)
        action = ['self-test', '--case', 'full-text' if scenario == 'text' else scenario] if args.acceptance_only else ['run', '--scenario', scenario]
        try:
            completed = subprocess.run([harness, *action, '--host', str(hosts[role]), '--output', str(output.resolve())], timeout=960)
        except subprocess.TimeoutExpired:
            runs.append({'scenario': scenario, 'role': role,
                         'warmup': index < (1 if args.reference_only else 2),
                         'directory': output.name, 'exit_code': None,
                         'valid': False, 'failure': 'controller_timeout'})
            checkpoint()
            raise
        if completed.returncode and not output.exists() and not args.acceptance_only and environment() > 0:
            events.append({'scenario': scenario, 'index': index, 'state': 'admission_race',
                           'exit_code': completed.returncode, 'at': now(), 'host_started': False})
            checkpoint()
            continue
        row = {'scenario': scenario, 'role': role, 'warmup': index < (1 if args.reference_only else 2),
               'directory': output.name, 'exit_code': completed.returncode, 'valid': False}
        runs.append(row)
        if completed.returncode:
            raise RuntimeError('benchmark workload failed; original evidence retained')
        subprocess.run([harness, 'verify', '--output', str(output)], check=True, timeout=30)
        fingerprint = json.loads((output / 'environment.json').read_text())
        if fingerprint.get('cpu_time_format') != 'mach-timebase-ns-v1':
            raise RuntimeError('legacy CPU units require corrected analysis')
        if fingerprint['host_sha256'] != provenance[role]['binary_sha256']:
            raise RuntimeError('binary changed during suite')
        stable = {key: fingerprint[key] for key in ['os', 'architecture', 'cpu', 'logical_cpus',
                  'machine_id', 'harness_sha256', 'scenario_version', 'cpu_time_format', 'dataset_version', 'network', 'power']}
        if reference_environment is None:
            reference_environment = stable
        if stable != reference_environment:
            raise RuntimeError('environment changed during suite')
        row['valid'] = True
        checkpoint()
        return


checkpoint()
try:
    order = ['a'] * 7 if args.reference_only else ['a', 'b'] + list('abba' * 3)
    for scenario in args.scenarios:
        for index, role in enumerate(order):
            run_one(scenario, index, role)
        if not args.reference_only:
            for block in range(3):
                base = 2 + block * 4
                subprocess.run([harness, 'compare', '--baseline', str(args.output / f'{scenario}-{base:02}-a'),
                                '--candidate', str(args.output / f'{scenario}-{base+1:02}-b'),
                                '--output', str(args.output / f'{scenario}-comparison-{block}')], check=True, timeout=30)
except (Exception, KeyboardInterrupt) as error:
    failures.append({'category': type(error).__name__, 'at': now(), 'completed_runs': sum(row['valid'] for row in runs)})
    checkpoint()
    raise
checkpoint(complete=True)
