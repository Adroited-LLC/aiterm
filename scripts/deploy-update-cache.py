#!/usr/bin/env python3
"""Deploy a complete verified private release cache; GitHub credentials stay on the publisher."""
import argparse,hashlib,json,pathlib,re,shlex,subprocess,uuid
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('cache',type=pathlib.Path);p.add_argument('--host',required=True);p.add_argument('--identity',required=True)
a=p.parse_args()
manifest=json.loads((a.cache/'updates.json').read_text())
assert manifest['schema']==1
files=[a.cache/'updates.json']
for entry in manifest['platforms'].values():
 assert re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]{0,180}',entry['asset'])
 f=a.cache/entry['asset'];assert f.stat().st_size==entry['size']
 with f.open('rb') as stream:assert hashlib.file_digest(stream,'sha256').hexdigest()==entry['sha256']
 files.append(f)
key='release-'+uuid.uuid4().hex
remote='/tmp/aiterm-'+key
ssh=['ssh','-o','BatchMode=yes','-i',a.identity,a.host]
subprocess.run(ssh+['mkdir -m 700 '+shlex.quote(remote)],check=True)
subprocess.run(['scp','-q','-i',a.identity,*map(str,files),a.host+':'+remote+'/'],check=True)
script=f'''set -eu
root=/var/lib/aiterm-updates
mv {remote} "$root/{key}"
chown -R root:aiterm-updates "$root/{key}"
chmod 0750 "$root/{key}"
chmod 0640 "$root/{key}"/*
ln -s "$root/{key}" "$root/.next"
mv -Tf "$root/.next" "$root/current"
'''
subprocess.run(ssh+['sudo -n bash -s'],input=script,text=True,check=True)
print('Activated verified release cache:',key)
