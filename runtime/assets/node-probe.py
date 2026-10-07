import os,json,platform,socket,subprocess,shutil
def run(args):
 try: return subprocess.run(args,capture_output=True,text=True,timeout=5)
 except Exception: return None
release={}
try:
 for line in open('/etc/os-release'):
  if '=' in line:
   k,v=line.strip().split('=',1); release[k]=v.strip('"')
except OSError: pass
addresses=[]
r=run(['ip','-j','-4','address','show','scope','global'])
if r and r.returncode==0:
 for iface in json.loads(r.stdout):
  if 'UP' not in iface.get('flags',[]): continue
  if iface.get('ifname','').startswith(('docker','br-','cni','flannel','veth')): continue
  addresses += [a['local'] for a in iface.get('addr_info',[]) if a.get('family')=='inet']
memory=0
try: memory=os.sysconf('SC_PAGE_SIZE')*os.sysconf('SC_PHYS_PAGES')/1024**3
except (ValueError,OSError): pass
r=run(['sudo','-n','true']) if os.geteuid()!=0 else None
print(json.dumps(dict(name=socket.gethostname().split('.')[0].lower(),os=release.get('ID',platform.system().lower()),version=release.get('VERSION_ID',platform.release()),architecture=platform.machine(),cores=os.cpu_count() or 0,memoryGiB=round(memory,1),diskGiB=round(shutil.disk_usage('/').free/1024**3,1),addresses=addresses,sudo=os.geteuid()==0 or bool(r and r.returncode==0),existingCluster=os.path.exists('/etc/rancher/k3s') or os.path.exists('/var/lib/rancher/k3s'))))
