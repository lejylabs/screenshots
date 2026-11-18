#!/usr/bin/env python3
"""Synthetic Task clipboard source + real private HTTPS/SXCU; no native clipboard/keyring."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import ssl
import subprocess
import tempfile
import threading
import urllib.parse
import xml.etree.ElementTree as ET

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--test-binary', type=Path, required=True)
parser.add_argument('--runtime-directory', type=Path, required=True)
args = parser.parse_args()
spec = importlib.util.spec_from_file_location('sxcu_fixture', Path(__file__).with_name('test-sharex-upload.py'))
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)
with tempfile.TemporaryDirectory(prefix='screenshots-task-clipboard-private-') as directory:
    root = Path(directory)
    binary = root / 'screenshots-test'
    shutil.copy2(args.test_binary.resolve(), binary)
    (root / 'lib').mkdir()
    shutil.copytree(args.runtime_directory.resolve(), root / 'lib/screenshots-sxcu')
    ca_key, ca, key, cert, csr, extensions = [root / name for name in
        ('ca-key.pem', 'ca.pem', 'key.pem', 'server.pem', 'server.csr', 'extensions')]
    extensions.write_text('basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1,DNS:localhost\n')
    for command in [
        ['openssl','req','-x509','-newkey','rsa:2048','-nodes','-keyout',str(ca_key),'-out',str(ca),'-days','1','-subj','/CN=Private Task fixture CA','-addext','basicConstraints=critical,CA:TRUE'],
        ['openssl','req','-new','-newkey','rsa:2048','-nodes','-keyout',str(key),'-out',str(csr),'-subj','/CN=localhost'],
        ['openssl','x509','-req','-in',str(csr),'-CA',str(ca),'-CAkey',str(ca_key),'-CAcreateserial','-out',str(cert),'-days','1','-extfile',str(extensions)],
    ]:
        subprocess.run(command,check=True,capture_output=True,timeout=20)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert,key)
    records = []
    server = fixture.Server(('127.0.0.1',0),fixture.handler(records,'unused-synthetic-token'))
    server.context = context
    thread = threading.Thread(target=server.serve_forever,kwargs={'poll_interval':0.1})
    thread.start()
    try:
        modes = ['None','MultipartFormData','FormURLEncoded','JSON','XML','Binary']
        for mode in modes:
            sxcu = {'Version':'17.0.0','DestinationType':'TextUploader','RequestURL':f'https://127.0.0.1:{server.server_port}/clipboard/{mode}','RequestMethod':'POST','Body':mode,'FileFormName':'','Arguments':{'value':'{input}'},'URL':'{json:files[?(@.id == 2)].url}'}
            if mode == 'JSON': sxcu['Data'] = '{"text":"{input}"}'
            if mode == 'XML': sxcu['Data'] = '<text>{input}</text>'
            (root/f'{mode}.sxcu').write_text(json.dumps(sxcu))
            task = {'version':1,'source':{'type':'clipboard_text','allow_private_source':True},'actions':[{'type':'sharex_upload','profile':f'{mode}.sxcu'},{'type':'link','format':'plain'}]}
            (root/f'task-{mode}.json').write_text(json.dumps(task))
        env = dict(os.environ)
        for key in ['HOME','USERPROFILE','APPDATA','LOCALAPPDATA','XDG_CONFIG_HOME','XDG_DATA_HOME','XDG_CACHE_HOME']:
            env[key] = str(root / key)
        for key in ['DISPLAY','WAYLAND_DISPLAY','WAYLAND_SOCKET','DBUS_SESSION_BUS_ADDRESS']:
            env.pop(key,None)
        env['XDG_SESSION_TYPE'] = 'x11'
        env['SSL_CERT_FILE'] = str(ca)
        env['SCREENSHOTS_TASK_CLIPBOARD_HTTPS_FIXTURE'] = str(root)
        result = subprocess.run([str(binary),'--exact','task::clipboard_input::tests::private_text_task_upload_fixture'],env=env,capture_output=True,timeout=120)
        assert result.returncode == 0, (result.stdout.decode(errors='replace'),result.stderr.decode(errors='replace'))
        text = 'synthetic private text\n€'
        assert len(records) == len(modes)
        for mode, record in zip(modes,records):
            if mode == 'None': assert not record['body']
            elif mode == 'MultipartFormData': assert record['multipart']['value']['data'] == text.encode()
            elif mode == 'FormURLEncoded': assert urllib.parse.parse_qs(record['body'].decode())['value'] == [text]
            elif mode == 'JSON': assert json.loads(record['body'])['text'] == text
            elif mode == 'XML': assert ET.fromstring(record['body']).text == text
            else: assert record['body'] == text.encode()
        print(json.dumps({'private_source':'synthetic Rust executor','https_requests':len(records),'native_clipboard':False,'keyring':False}))
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        assert not thread.is_alive()
