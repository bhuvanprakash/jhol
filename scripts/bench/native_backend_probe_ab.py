"""Focused saved-binary/candidate/Bun comparison. Reuses paper_matrix quality gates."""
import argparse, importlib.util, json, os, random, shutil, statistics, subprocess, tempfile, time
from pathlib import Path
repo = Path(__file__).resolve().parents[2]
ap = argparse.ArgumentParser(description=__doc__)
ap.add_argument('--baseline', required=True, type=Path)
ap.add_argument('--candidate', type=Path, default=repo/'target/release/jhol')
ap.add_argument('--bun', default='bun')
ap.add_argument('--fixture-report', type=Path, default=repo/'docs/architecture/experiments/cold-stage-baseline-20261003-233517.json')
cli = ap.parse_args()
sp = importlib.util.spec_from_file_location('matrix', repo / 'scripts/bench/paper_matrix.py')
m = importlib.util.module_from_spec(sp); sp.loader.exec_module(m)
baseline = cli.baseline.resolve()
candidate = cli.candidate.resolve(); bun = Path(shutil.which(cli.bun) or cli.bun).resolve()
inputs = json.loads(cli.fixture_report.read_text())['fixture_inputs']
report = {'purpose': 'focused native backend-probe removal; pilot, not 4x acceptance', 'binaries': {k: {'path': str(p), 'sha256': m.hash_file(p), 'version': m._version(str(p))} for k,p in [('before',baseline),('after',candidate),('bun',bun)]}, 'fixture_inputs': inputs, 'timing': 'subprocess.run capture; immediate completion, no 10ms RSS polling; peak RSS unavailable; outer memory watchdog', 'samples': [], 'summary': {}}
original = m._run_monitored
condition = 'registry-cold'
def measured(command, cwd, env, timeout):
    command = list(command)
    if Path(command[0]).name != 'bun': command += ['--no-scripts']
    else: command += ['--ignore-scripts']
    if condition == 'local-cache-cold': command += ['--offline']
    return subprocess.run(command, cwd=cwd, env=env, text=True, capture_output=True, timeout=timeout), None
m._run_monitored = measured
m.ARM_ENV['harvest'] = {'JHOL_NO_HARVEST':'1', 'JHOL_JGRID':'0', 'JHOL_JGRID_L0':'0', 'JHOL_JAGRS_MIRROR':'0'}
rng = random.Random(2026100413)
with tempfile.TemporaryDirectory(prefix='jhol-native-probe-ab-') as td:
    root = Path(td); fixture = root/'fixture'; fixture.mkdir()
    for name,data in inputs.items(): (fixture/name).write_text(json.dumps(data,indent=2)+'\n')
    # Causal negative control: a fake Bun records probes, without installing anything.
    report['probe_control'] = []
    for label,binary in [('before',baseline),('after',candidate)]:
        control = root/('control-'+label); control.mkdir(); (control/'bin').mkdir()
        (control/'package.json').write_text('{"name":"probe","version":"1.0.0"}')
        fake = control/'bin/bun'; fake.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$JHOL_BACKEND_PROBE_LOG"\nexit 0\n'); fake.chmod(0o755)
        env=os.environ.copy(); env.update({'HOME':str(control),'PATH':str(control/'bin')+os.pathsep+env.get('PATH',''),'JHOL_CACHE_DIR':str(control/'cache'),'JHOL_NO_DAEMON':'1','JHOL_BACKEND_PROBE_LOG':str(control/'probes')})
        r=subprocess.run([str(binary),'install','--offline'],cwd=control,env=env,capture_output=True,text=True,timeout=30)
        report['probe_control'].append({'variant':label,'exit':r.returncode,'probes':(control/'probes').read_text().splitlines() if (control/'probes').exists() else []})
    seeds={}
    for label,binary in [('harvest',candidate),('bun',bun)]:
        seed=root/('seed-'+label); seed.mkdir(); home=root/('seed-home-'+label);home.mkdir()
        env=os.environ.copy()
        for key in list(env):
            if key.startswith('JHOL_'):env.pop(key,None)
        env.update({'HOME':str(home),'XDG_CACHE_HOME':str(home/'.cache'),'JHOL_CACHE_DIR':str(seed),'BUN_INSTALL_CACHE_DIR':str(seed),'JHOL_NO_HARVEST':'1','JHOL_JGRID':'0','JHOL_JGRID_L0':'0','JHOL_JAGRS_MIRROR':'0','JHOL_PREBUILT':'0','JHOL_NO_DAEMON':'1','JHOL_NO_SPLASH':'1'})
        r=measured([str(binary),'install'],fixture,env,120)[0]
        if r.returncode:raise RuntimeError('cache seed failed: '+r.stderr[-1000:])
        shutil.rmtree(fixture/'node_modules'); seeds[label]=seed
    report['seed_coverage']={key:{'bytes':m.cache_bytes(p),'sha256':m._directory_digest(p),'source':'one install from identical lock'} for key,p in seeds.items()}
    for condition,rounds in [('local-cache-cold',5),('registry-cold',3)]:
        args=argparse.Namespace(fixture=fixture,fixture_lock_sha256=m.hash_file(fixture/'package-lock.json'),condition=condition,bun=str(bun),jhol=str(candidate),node='node',timeout=120,drift_policy='fail')
        for rnd in range(1,rounds+1):
            for label in rng.sample(['before','after','bun'],3):
                arm='bun' if label=='bun' else 'harvest'; args.jhol=str(baseline if label=='before' else candidate)
                sample=m.run_one(args,arm,rnd,root,seeds,m.DEFAULT_MARKERS);sample['variant']=label;sample['condition']=condition
                report['samples'].append(sample);print(condition,label,rnd,sample['status'],round(sample.get('elapsed_ms',0),2),flush=True)
        report['summary'][condition]={}
        for label in ['before','after','bun']:
            ss=[s for s in report['samples'] if s['condition']==condition and s['variant']==label];passed=[s['elapsed_ms'] for s in ss if s['status']=='pass']
            report['summary'][condition][label]={'passed':len(passed),'total':len(ss),'median_ms':statistics.median(passed) if passed else None,'range_ms':[min(passed),max(passed)] if passed else None}
m._run_monitored=original
out=repo/'docs/architecture/experiments'/('native-backend-probe-ab-'+time.strftime('%Y%m%d-%H%M%S')+'.json')
out.write_text(json.dumps(report,indent=2)+'\n');print(out);print(json.dumps(report['summary'],indent=2))
assert report['probe_control'][0]['probes']==['--version'] and report['probe_control'][1]['probes']==[]
raise SystemExit(0 if all(s['status']=='pass' for s in report['samples']) else 2)
