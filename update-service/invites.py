#!/usr/bin/env python3
"""Manage per-person invite hashes. Plaintext codes are written only to a private output file."""
import argparse,hashlib,json,os,pathlib,secrets

def save(path,data):
 path=pathlib.Path(path);path.parent.mkdir(parents=True,exist_ok=True)
 tmp=path.with_name(path.name+'.tmp')
 fd=os.open(tmp,os.O_WRONLY|os.O_CREAT|os.O_TRUNC,0o600)
 with os.fdopen(fd,'w') as f:json.dump(data,f,indent=2);f.write('\n')
 os.chmod(tmp,0o600);tmp.replace(path)
def main():
 p=argparse.ArgumentParser(description=__doc__)
 p.add_argument('--store',type=pathlib.Path,required=True)
 p.add_argument('--output',type=pathlib.Path)
 p.add_argument('action',choices=['create','revoke','list']);p.add_argument('names',nargs='*')
 a=p.parse_args();store=json.loads(a.store.read_text()) if a.store.exists() else {}
 if a.action=='list':print('\n'.join(store));return
 if a.action=='create':
  if a.output is None:p.error('--output is required for create')
  codes=json.loads(a.output.read_text()) if a.output.exists() else {}
  for name in a.names:
   if name in store:p.error(f'{name} already has an invite; revoke before replacing it')
   code='aiterm_'+secrets.token_urlsafe(32);store[name]=hashlib.sha256(code.encode()).hexdigest();codes[name]=code
  save(a.output,codes)
 else:
  for name in a.names:store.pop(name,None)
 save(a.store,store)
 print('Updated invite hashes. Deploy this store to apply changes.')
if __name__=='__main__':main()
