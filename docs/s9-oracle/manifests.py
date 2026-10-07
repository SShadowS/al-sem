import io, sys, zipfile, os, json, re
import xml.etree.ElementTree as ET
src, dst = sys.argv[1], sys.argv[2]
for f in sorted(os.listdir(src)):
    if not f.endswith('.app'): continue
    out = os.path.join(dst, f[:-4].replace(' ', '_'))
    if not os.path.isdir(out): continue
    b = open(os.path.join(src, f), 'rb').read()
    z = zipfile.ZipFile(io.BytesIO(b[b.find(b'PK\x03\x04'):]))
    x = ET.fromstring(z.read('NavxManifest.xml'))
    ns = {'n': x.tag.split('}')[0].strip('{')}
    app = x.find('n:App', ns)
    deps = [{'id': d.get('Id'), 'name': d.get('Name'), 'publisher': d.get('Publisher'),
             'version': d.get('MinVersion') or d.get('Version')}
            for d in x.findall('n:Dependencies/n:Dependency', ns)]
    ranges = [{'from': int(r.get('MinObjectId')), 'to': int(r.get('MaxObjectId'))}
              for r in x.findall('n:IdRanges/n:IdRange', ns)]
    j = {'id': app.get('Id'), 'name': app.get('Name'), 'publisher': app.get('Publisher'),
         'version': app.get('Version'), 'dependencies': deps, 'idRanges': ranges,
         'runtime': app.get('Runtime'), 'target': app.get('Target') or 'Cloud'}
    if app.get('Platform'): j['platform'] = app.get('Platform')
    if app.get('Application'): j['application'] = app.get('Application')
    json.dump(j, open(os.path.join(out, 'app.json'), 'w'), indent=1)
    print(out.split(os.sep)[-1], j['id'], len(deps), j['runtime'], j.get('platform'), j.get('application'))
