import io, sys, zipfile, os, json
src, dst = sys.argv[1], sys.argv[2]
for f in sorted(os.listdir(src)):
    if not f.endswith('.app'): continue
    b = open(os.path.join(src, f), 'rb').read()
    i = b.find(b'PK\x03\x04')
    z = zipfile.ZipFile(io.BytesIO(b[i:]))
    names = z.namelist()
    al = [n for n in names if n.lower().endswith('.al')]
    print(f, 'al files:', len(al), 'has app.json:', 'app.json' in names, 'NavxManifest' in ' '.join(names))
    if not al: continue
    out = os.path.join(dst, f[:-4].replace(' ', '_'))
    for n in al + [n for n in names if n == 'app.json']:
        p = os.path.join(out, n.replace('%20', ' '))
        os.makedirs(os.path.dirname(p), exist_ok=True)
        open(p, 'wb').write(z.read(n))
