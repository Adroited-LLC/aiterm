import hashlib,json,pathlib,secrets,tempfile,threading,unittest,urllib.request,urllib.error
from server import Server
class PrivateUpdatesTest(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.root=pathlib.Path(self.tmp.name)
  self.code='aiterm_'+secrets.token_urlsafe(32)
  self.invites=self.root/'invites.json';self.invites.write_text(json.dumps({'test':hashlib.sha256(self.code.encode()).hexdigest()}))
  (self.root/'app.apk').write_bytes(b'package')
  (self.root/'updates.json').write_text(json.dumps({'schema':1,'platforms':{'android-arm64':{'asset':'app.apk','size':7}}}))
  self.server=Server(('127.0.0.1',0),self.root,self.invites)
  threading.Thread(target=self.server.serve_forever,daemon=True).start()
 def tearDown(self): self.server.shutdown();self.server.server_close();self.tmp.cleanup()
 def get(self,path,code=None):
  request=urllib.request.Request(f'http://127.0.0.1:{self.server.server_port}'+path,headers={'Authorization':'Bearer '+code} if code else {})
  try:
   with urllib.request.urlopen(request) as r:return r.status,r.read()
  except urllib.error.HTTPError as e:
   with e:return e.code,e.read()
 def test_private_feed_and_assets_require_invite(self):
  for path in ['/updates/access','/updates/latest','/updates/assets/app.apk']:
   self.assertEqual(self.get(path)[0],401);self.assertEqual(self.get(path,'bad')[0],401)
  self.assertEqual(self.get('/updates/latest',self.code)[0],200)
  self.assertEqual(self.get('/updates/assets/app.apk',self.code),(200,b'package'))
 def test_revocation_is_immediate(self):
  self.assertEqual(self.get('/updates/access',self.code)[0],200)
  self.invites.write_text('{}');self.assertEqual(self.get('/updates/access',self.code)[0],401)
 def test_only_manifest_assets_are_accessible(self):
  for path in ['/updates/assets/invites.json','/updates/assets/../invites.json','/updates/assets/%2e%2e/invites.json','/updates/assets/app.apk?code=x']:
   self.assertEqual(self.get(path,self.code)[0],404)
 def test_incomplete_release_is_an_error(self):
  (self.root/'app.apk').unlink();self.assertEqual(self.get('/updates/latest',self.code)[0],503)
if __name__=='__main__':unittest.main()
