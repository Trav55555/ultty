import hashlib,json,os,shutil,subprocess,tempfile,time,uuid
from pathlib import Path
base=Path('/home/travers/projects/tui-terminal/target/session-daily-use')
bundle=base/'headless-bundle-01'
prefix=base/'service-install-01'
tool=Path('/home/travers/projects/tui-terminal/.worktrees/session-daily-use/scripts/deploy/session-bundle.py')
name='verify-'+uuid.uuid4().hex[:8]
unit='ultty-session-'+name+'.service'
runtime=Path(tempfile.mkdtemp(prefix='usv-',dir='/tmp'))
exe=prefix/'current/bin/ultty-session'
out=base/'logs/service-gate-01.json'
record={'passed':False,'unit':unit,'source':json.loads((bundle/'manifest.json').read_text())['source_revision']}
child=owner=None

def run(args,**kw):
 return subprocess.run(args,text=True,capture_output=True,timeout=20,**kw)

def alive(pid):
 if pid is None:return False
 try: os.kill(pid,0);return True
 except ProcessLookupError:return False

try:
 release=run(['python3','-S',str(tool),'install',str(bundle),'--prefix',str(prefix)],check=True).stdout.strip()
 run(['python3','-S',str(tool),'activate',release,'--prefix',str(prefix)],check=True)
 command=['systemd-run','--user','--unit='+unit,'--collect','--property=Restart=no','--property=KillMode=control-group','--property=TimeoutStopSec=15s','--property=ExecStop='+str(exe)+' stop '+name,'--setenv=XDG_RUNTIME_DIR='+str(runtime),'--setenv=TMPDIR='+str(runtime),str(exe),'host',name,'-e','/bin/sleep','120']
 record['command']=command
 started=run(command,check=True)
 record['start_result']=started.stderr.strip()
 env=dict(os.environ,XDG_RUNTIME_DIR=str(runtime),TMPDIR=str(runtime))
 deadline=time.monotonic()+15
 while True:
  result=run([str(exe),'snapshot',name],env=env)
  if result.returncode==0:break
  if time.monotonic()>deadline:raise RuntimeError(result.stderr)
  time.sleep(.1)
 snap=json.loads(result.stdout)['reply']['snapshot']
 owner=snap['owner']['pid'];child=snap['panes'][0]['pid']
 props=run(['systemctl','--user','show',unit,'--property=MainPID,Restart,KillMode,ExecStop'],check=True).stdout
 record.update(owner_pid=owner,child_pid=child,unit_properties=props)
 assert f'MainPID={owner}\n' in props and 'Restart=no\n' in props and 'KillMode=control-group\n' in props
 assert str(exe) in props and f'stop {name}' in props
 run(['systemctl','--user','stop',unit],check=True)
 deadline=time.monotonic()+10
 while alive(owner) or alive(child):
  if time.monotonic()>deadline:raise RuntimeError('owned host or child survived service stop')
  time.sleep(.1)
 record['passed']=True
 record['binary_sha256']=hashlib.sha256(exe.read_bytes()).hexdigest()
finally:
 run(['systemctl','--user','stop',unit])
 record['owner_stopped']=not alive(owner)
 record['child_stopped']=not alive(child)
 shutil.rmtree(runtime)
 record['private_runtime_removed']=not runtime.exists()
 out.write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps(record,indent=2))
