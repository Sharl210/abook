#!/usr/bin/env python3
"""Local fixture test for upgrade replacement and bad-package preservation."""
import hashlib,json,os,pathlib,shutil,subprocess,sys,tempfile

def main(binary):
 src=pathlib.Path(binary).resolve(); payload=src.read_bytes(); digest=hashlib.sha256(payload).hexdigest()
 ver=subprocess.check_output([str(src),'version'],text=True).strip(); version=ver.split()[1]
 target='aarch64-linux-android' if 'android/aarch64' in ver else 'x86_64-unknown-linux-musl'
 with tempfile.TemporaryDirectory(prefix='abook-upgrade-fixture-') as td:
  root=pathlib.Path(td); install=root/'install'; install.mkdir(); exe=install/('abook.exe' if os.name=='nt' else 'abook'); shutil.copy2(src,exe); exe.chmod(0o755)
  env=os.environ.copy(); env['ABOOK_ROOT']=str(root/'memory-root')
  init=subprocess.run([str(src),'init','--root',env['ABOOK_ROOT']],env=env,capture_output=True,text=True,timeout=60); assert init.returncode==0,init.stdout+init.stderr
  records=pathlib.Path(env['ABOOK_ROOT'])/'canonical/records'; records.mkdir(parents=True,exist_ok=True)
  for i in range(5): (records/f'C{i}.md').write_text(f'[CANONICAL_MEMORY]\nFORMAT=CANONICAL_MEMORY_V2\nID=C{i}\nTITLE=fixture {i}\nSTATUS=ACTIVE\nKEYS=k{i}\nREAD_WHEN=always\n[/CANONICAL_MEMORY]\n[MEMORY_BODY]\nbody {i}\n[/MEMORY_BODY]\n')
  def fixture(name,data,digest_value):
   base=root/name; rel=pathlib.Path(target)/exe.name; f=base/rel; f.parent.mkdir(parents=True); f.write_bytes(data); f.chmod(0o755)
   (base/'manifest.json').write_text(json.dumps({'schema':'abook-downloads/1','version':version,'files':[{'target':target,'os':'android' if 'android' in target else 'linux','arch':'aarch64' if 'aarch64' in target else 'x86_64','file':rel.as_posix(),'size':len(data),'sha256':digest_value}]}))
   return base
  good=subprocess.run([str(src),'upgrade','--dir',str(install),'--url',str(fixture('good',payload,digest)),'--force','--json'],env=env,capture_output=True,text=True,timeout=180)
  assert good.returncode==0,good.stdout+good.stderr; assert hashlib.sha256(exe.read_bytes()).hexdigest()==digest
  bad_data=bytearray(payload); bad_data[-16]^=1; before=hashlib.sha256(exe.read_bytes()).hexdigest()
  bad=subprocess.run([str(src),'upgrade','--dir',str(install),'--url',str(fixture('bad',bytes(bad_data),digest)),'--force','--json'],env=env,capture_output=True,text=True,timeout=180)
  after=hashlib.sha256(exe.read_bytes()).hexdigest(); assert bad.returncode==4,bad.stdout+bad.stderr; assert '"tag":"BLOCKED"' in bad.stdout; assert before==after==digest; assert not (install/'.abook.upgrade').exists()
  print(json.dumps({'status':'PASS','version':ver,'target':target,'sha256':digest,'good_exit':good.returncode,'bad_exit':bad.returncode,'old_binary_preserved':True},ensure_ascii=False))
if __name__=='__main__': main(sys.argv[1])
