#!/usr/bin/env python3
"""Stage a complete, verified three-platform release in the private distribution repo.
Requires gh authentication; never embeds or prints credentials. Draft by default.
"""
import argparse, hashlib, json, pathlib, shutil, subprocess, tempfile
REPO = 'Adroited-LLC/aiterm-releases'
def gh(*args):
    return subprocess.check_output(['gh', *args], text=True)
def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--rpm', required=True, type=pathlib.Path)
    parser.add_argument('--apk', required=True, type=pathlib.Path)
    parser.add_argument('--windows', required=True, type=pathlib.Path)
    parser.add_argument('--android-version', required=True)
    parser.add_argument('--android-code', required=True, type=int)
    parser.add_argument('--windows-version', required=True)
    parser.add_argument('--notes', required=True, type=pathlib.Path)
    parser.add_argument('--publish', action='store_true')
    args = parser.parse_args()
    assert json.loads(gh('repo','view',REPO,'--json','visibility'))['visibility'] == 'PRIVATE', 'Distribution repository must be private'
    assert subprocess.check_output(['rpm','-qp','--qf','%{NAME} %{ARCH}',str(args.rpm)],text=True) == 'aiterm x86_64', 'Wrong RPM package or architecture'
    rpm_version = subprocess.check_output(['rpm','-qp','--qf','%{VERSION}',str(args.rpm)],text=True)
    import re
    for version in (rpm_version,args.android_version,args.windows_version):
        assert re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+',version), 'Invalid version'
    assert args.android_code > 0
    notes = args.notes.read_text()
    with tempfile.TemporaryDirectory(prefix='aiterm-release-') as tmp:
        folder = pathlib.Path(tmp)
        platforms = {}
        definitions = [
            ('linux-x86_64',args.rpm,rpm_version,f'aiterm-{rpm_version}-1.x86_64.rpm'),
            ('windows-x86_64',args.windows,args.windows_version,f'aiterm-windows-{args.windows_version}-x64-setup.exe'),
            ('android-arm64',args.apk,args.android_version,f'aiterm-android-{args.android_version}.apk')]
        for platform, source, version, name in definitions:
            assert source.is_file() and source.stat().st_size > 0, f'Missing artifact: {source}'
            dest = folder/name; shutil.copyfile(source,dest)
            entry = dict(version=version,asset=name,size=dest.stat().st_size,sha256=hashlib.file_digest(dest.open('rb'),'sha256').hexdigest(),notes=notes)
            if platform == 'android-arm64': entry['versionCode'] = args.android_code
            platforms[platform] = entry
        manifest = folder/'updates.json'
        manifest.write_text(json.dumps(dict(schema=1,platforms=platforms),indent=2)+'\n')
        gh('release','create',args.tag,'--repo',REPO,'--draft','--title',f'AITerm {args.tag}','--notes-file',str(args.notes))
        gh('release','upload',args.tag,'--repo',REPO,*map(str,folder.iterdir()))
        release = json.loads(gh('api',f'repos/{REPO}/releases'))
        release = next(r for r in release if r['tag_name']==args.tag)
        assets = {a['name']:a for a in release['assets']}
        for path in folder.iterdir():
            expected = 'sha256:'+hashlib.file_digest(path.open('rb'),'sha256').hexdigest()
            assert assets[path.name]['size']==path.stat().st_size and assets[path.name]['digest']==expected, f'Upload verification failed: {path.name}'
        if args.publish:
            assert json.loads(gh('repo','view',REPO,'--json','visibility'))['visibility']=='PRIVATE'
            gh('release','edit',args.tag,'--repo',REPO,'--draft=false','--latest')
        print(gh('release','view',args.tag,'--repo',REPO,'--json','url,isDraft'))
if __name__ == '__main__': main()
