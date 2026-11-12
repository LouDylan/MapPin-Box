"""Local preview. Search/maps are live; all device operations use sample state."""
import copy
import json
import os
import queue
import re
import threading
import time
import tomllib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1] / 'firmware' / 'src'
VERSION = tomllib.loads((ROOT.parent / 'Cargo.toml').read_text())['package']['version']
PREVIEW_DATA = ROOT.parents[1] / '.private' / 'preview'
SAVED = PREVIEW_DATA / 'favorites.json'
MAP_FILE = PREVIEW_DATA / 'maps.json'
LOCK = threading.Lock()
JOBS = queue.Queue()
WIFI_JOBS = queue.Queue()
EVENTS = []
NEXT_WIFI = 1
START = time.monotonic()
PLACES = [
    dict(id=1, name='上海市中心', latitude=31.2304, longitude=121.4737, address='上海市', source='manual'),
    dict(id=2, name='上海虹桥站', latitude=31.1961169, longitude=121.3161097, address='上海市 闵行区 新虹街道 申贵路', source='openstreetmap'),
    dict(id=3, name='伦敦大本钟', latitude=51.5007, longitude=-0.1246, address='Westminster, London', source='openstreetmap'),
]

PROVIDERS = ('amap', 'osm', 'google', 'tianditu')
def valid_key(value): return isinstance(value, str) and bool(re.fullmatch(r'[A-Za-z0-9_-]{16,128}', value))
def configured(provider):
    if provider == 'osm': return True
    if provider == 'amap': return all(valid_key(MAP_CONFIG['amap'].get(field)) for field in ('key', 'security_code'))
    return valid_key(MAP_CONFIG[provider].get('key'))
def regional_providers():
    result = {}
    for region, group in [('domestic', ('amap', 'tianditu')), ('global', ('osm', 'google'))]:
        candidates = [MAP_CONFIG.get(region+'_provider'), *group, MAP_CONFIG['provider']] if region == 'global' else [MAP_CONFIG.get(region+'_provider'), MAP_CONFIG['provider'], *group]
        result[region] = next((p for p in candidates if p in group and configured(p)), None)
    return result

def public_map_config():
    provider = MAP_CONFIG['provider']
    return dict(provider=provider, **MAP_CONFIG[provider], configured=configured(provider), configured_providers={p: configured(p) for p in PROVIDERS},regional_providers=regional_providers())
def map_config(value):
    if not isinstance(value, dict) or value.get('provider') not in PROVIDERS: raise ValueError('请选择地图服务')
    result = copy.deepcopy(MAP_CONFIG)
    old = MAP_CONFIG['provider']
    region = value.get('region')
    groups = {'domestic': ('amap', 'tianditu'), 'global': ('osm', 'google')}
    if region is not None and value['provider'] not in groups.get(region, ()):
        raise ValueError('地图源与所选地区不匹配')
    result['version'] = 3
    result['global_provider'] = result.get('global_provider') or 'osm'
    if old not in groups['global'] and not result.get('domestic_provider'):
        result['domestic_provider'] = old
    provider = value['provider']
    if region is None or old in groups[region]: result['provider'] = provider
    result[('global' if provider in groups['global'] else 'domestic')+'_provider'] = provider
    fields = ('key', 'security_code') if provider == 'amap' else ('key',) if provider != 'osm' else ()
    entered = {field: str(value.get(field) or '').strip() for field in fields}
    if any(entered.values()):
        if not all(valid_key(text) for text in entered.values()): raise ValueError('请填写所选地图服务的完整凭证')
        result[provider] = entered
    if provider != 'osm' and not all(valid_key(result[provider].get(f)) for f in fields): raise ValueError('请填写所选地图服务的凭证')
    return result

MAP_CONFIG = dict(version=3,provider='amap',global_provider='osm',amap=dict(key='', security_code=''), osm={}, google={}, tianditu={})
try:
    value = json.loads(MAP_FILE.read_text())
    if 'provider' in value:
        if value['provider'] not in PROVIDERS: raise ValueError('invalid provider')
        MAP_CONFIG.update(value)
        if value.get('version', 2) < 3:
            MAP_CONFIG['global_provider'] = 'osm'
            if MAP_CONFIG['provider'] in ('google', 'osm'): MAP_CONFIG['provider'] = 'osm'
        MAP_CONFIG['version'] = 3
    elif all(valid_key(value.get(field)) for field in ('key', 'security_code')):
        MAP_CONFIG['amap'] = value
    if MAP_FILE.exists(): MAP_FILE.chmod(0o600)
except (FileNotFoundError, ValueError, TypeError, AttributeError):
    pass

def place(value, stored=False):
    import math
    p = {key: value[key] for key in ('name',)}
    google = value.get('source') == 'google'
    reference = google and value.get('latitude') is None and value.get('longitude') is None
    if not reference: p.update(latitude=value['latitude'], longitude=value['longitude'])
    p['name'] = str(p['name']).strip()
    if not p['name'] or len(p['name']) > 64: raise ValueError('地点名称需要1至64个字')
    for key, limit in ([] if reference else [('latitude', 90), ('longitude', 180)]):
        number = p[key]
        if isinstance(number, bool) or not isinstance(number, (int, float)) or not math.isfinite(number) or abs(number) > limit:
            raise ValueError('地点位置无效')
        p[key] = round(number, 7)
    for key, limit in [('address', 96), ('source', 32)]:
        text = str(value.get(key) or '').strip()
        if len(text) > limit or any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in text): raise ValueError('地点信息格式无效')
        if text: p[key] = text
    if any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in p['name']): raise ValueError('名称不能包含换行')
    if google:
        pid, query = value.get('google_place_id'), str(value.get('query') or '').strip()
        if not isinstance(pid,str) or not re.fullmatch(r'[A-Za-z0-9_-]{1,256}',pid) or not 1<=len(query)<=64 or any(ord(c)<32 or 127<=ord(c)<=159 for c in query): raise ValueError('Google 地点引用无效')
        p.update(google_place_id=pid, query=query)
        if stored: p = dict(name=query, source='google', google_place_id=pid, query=query)
    return p

def same(a, b):
    if a.get('source')=='google' or b.get('source')=='google': return a.get('source')==b.get('source') and a.get('google_place_id')==b.get('google_place_id')
    return (a['latitude'], a['longitude']) == (b['latitude'], b['longitude'])

try:
    saved = json.loads(SAVED.read_text())
    if not isinstance(saved, list) or len(saved) > 20: raise ValueError('invalid sample favorites')
    PLACES = [dict(id=i+1, **place(p, stored=True)) for i,p in enumerate(saved)]
except (FileNotFoundError, ValueError, KeyError, TypeError):
    pass

STATE = dict(preview=True, boot_id=int(time.time()), version=VERSION, revision=1, paired=True,
             maps_configured=configured(MAP_CONFIG['provider']), map_provider=MAP_CONFIG['provider'], configured_providers={p:configured(p) for p in PROVIDERS},
             network_busy=False, button=None, phone=dict(name='示例 iPhone',product_type='iPhone15,3',product_version='27.0.1'),
             mode='real', location=None, operation=None, restore_queued=False, result=None,
             favorites=PLACES, recent=[place(PLACES[1])] if len(PLACES)>1 else [],
             limits=dict(favorites=20, recent=6, name=64))
GOOGLE_REPLAY = None
NEXT_JOB = 1
NEXT_FAVORITE = max((p['id'] for p in PLACES), default=0)+1


WIFI = dict(ssid='MapPin-DEMO', address='192.168.71.1', channel=6, client_count=1,
            clients=[dict(index=1,id='12:34:56',rssi=-43)], local_only=True,
            upstream=dict(connected=False,saved_ssid=None,address=None,dns=None),
            operation=None,result=None,networks=[])
DEVICE = dict(chip='ESP32-S3',cores=2,configured_cpu_mhz=240,idf_version='v5.5.3',flash_bytes=16777216,
              psram_bytes=8388608,internal_free=172032,internal_min_free=143360,internal_largest_block=98304,
              psram_free=7143424,reset_reason='演示启动',reset_code=1)

def record_event(message, level='info'):
    EVENTS.append(dict(uptime=int(time.monotonic()-START),level=level,message=message))
    del EVENTS[:-32]

def snapshot():
    value=copy.deepcopy(STATE)
    value['regional_providers']=regional_providers()
    value['uptime']=int(time.monotonic()-START)
    value['last_location']=copy.deepcopy(value['recent'][0]) if value['recent'] else None
    if value['operation']:
        value['operation']['elapsed_ms']=int((time.monotonic()-value['operation'].pop('started'))*1000)
    return value

def wifi_snapshot():
    value=copy.deepcopy(WIFI)
    if value['operation']:
        value['operation']['elapsed_ms']=int((time.monotonic()-value['operation'].pop('started'))*1000)
    return value

def validate_wifi(value):
    if not isinstance(value,dict): raise ValueError('Wi-Fi 配置格式无效')
    ssid,password=value.get('ssid'),value.get('password')
    if not isinstance(ssid,str) or not 1<=len(ssid.encode())<=32 or any(ord(c)<32 or 127<=ord(c)<=159 for c in ssid):
        raise ValueError('Wi-Fi 名称需要1至32字节，不能包含控制字符')
    if not isinstance(password,str) or (password and not 8<=len(password.encode())<=63) or any(ord(c)<32 or 127<=ord(c)<=159 for c in password):
        raise ValueError('Wi-Fi 密码需要8至63字节，开放网络请留空')
    if ssid == WIFI['ssid']: raise ValueError('请选择其他 Wi-Fi，不能连接盒子自身热点')
    return dict(ssid=ssid,password=password)

def validate_hotspot(value):
    if not isinstance(value,dict): raise ValueError('热点配置格式无效')
    ssid,password=value.get('ssid'),value.get('password')
    if not isinstance(ssid,str) or not ssid.strip() or len(ssid.encode())>32 or any(ord(c)<32 or 127<=ord(c)<=159 for c in ssid):
        raise ValueError('热点名称需要1至32字节，不能全为空格或包含控制字符')
    if password is not None and (not isinstance(password,str) or (password and (not 8<=len(password)<=63 or any(not 32<=ord(c)<=126 for c in password)))):
        raise ValueError('热点密码需要8至63位英文、数字或符号，留空保持原密码')
    if ssid==WIFI['upstream']['saved_ssid']: raise ValueError('热点名称不能与已保存的上游 Wi-Fi 相同')
    return dict(ssid=ssid,password=password or None)

ADMIN_PAGES=dict(system='系统信息',network='Wi-Fi 连接',maps='地图服务',location='定位调试',help='使用帮助')

def admin_page(page):
    return ((ROOT/'admin.html').read_text()
            .replace('{{page}}',page).replace('{{title}}',ADMIN_PAGES[page])
            .replace('{{content}}',(ROOT/'admin'/(page+'.html')).read_text())).encode()

def wifi_worker():
    while True:
        job=WIFI_JOBS.get()
        time.sleep(1.5)
        with LOCK:
            kind=job['kind']
            if kind=='scan':
                WIFI['networks']=[dict(ssid='测试网络',rssi=-40,channel=6,secured=True),dict(ssid='测试开放网络',rssi=-65,channel=11,secured=False)]
                message='演示扫描完成，找到2个样例网络'
            elif kind=='connect':
                WIFI['upstream']=dict(connected=True,saved_ssid=job['ssid'],address='192.168.100.188',dns='192.168.100.1')
                message='演示连接成功，没有修改实际网络'
            elif kind=='hotspot':
                WIFI['ssid']=job['ssid']
                message='演示热点设置已保存，没有修改实际网络'
            else:
                WIFI['upstream']=dict(connected=False,saved_ssid=None,address=None,dns=None)
                message='演示已断开并忘记网络'
            WIFI['result']=dict(id=job['id'],kind=kind,ok=True,message=message)
            WIFI['operation']=None
            STATE['network_busy']=False
            record_event(message)
        WIFI_JOBS.task_done()
threading.Thread(target=wifi_worker,daemon=True).start()


def persist():
    PREVIEW_DATA.mkdir(mode=0o700, parents=True, exist_ok=True)
    temp = SAVED.with_suffix('.tmp')
    temp.write_text(json.dumps(STATE['favorites'], ensure_ascii=False, indent=2))
    temp.replace(SAVED)


def worker():
    global GOOGLE_REPLAY
    while True:
        job = JOBS.get()
        with LOCK:
            STATE['operation'] = dict(id=job['id'], kind=job['kind'], label=job['label'], phase='working', elapsed_ms=0, started=time.monotonic())
            if job['kind'] == 'restore': STATE['restore_queued'] = False
        time.sleep(2)
        with LOCK:
            if job['kind'] == 'set':
                p = job['place']
                STATE['location'] = place(p, stored=True)
                if not job['replay']: GOOGLE_REPLAY = (time.monotonic()+600,copy.deepcopy(p)) if p.get('source')=='google' else None
                STATE['mode'] = 'simulating'
                STATE['recent'] = ([place(p, stored=True)] + [x for x in STATE['recent'] if not same(x,p)])[:6]
                message = '演示已设置到“'+STATE['location']['name']+'”，未修改手机位置'
            else:
                STATE['location'] = None
                STATE['mode'] = 'real'
                message = '演示已恢复真实定位，未操作手机'
            record_event(message)
            STATE['result'] = dict(id=job['id'],ok=True,message=message,detail=None)
            STATE['operation'] = None
            STATE['revision'] += 1
        JOBS.task_done()
threading.Thread(target=worker, daemon=True).start()


class Handler(BaseHTTPRequestHandler):
    def send(self, value, code=200, mime='application/json; charset=utf-8'):
        data = json.dumps(value,ensure_ascii=False).encode() if not isinstance(value,bytes) else value
        self.send_response(code)
        self.send_header('Content-Type',mime)
        self.send_header('Cache-Control','no-store')
        self.send_header('Content-Length',str(len(data)))
        self.end_headers()
        try: self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError): pass

    def do_GET(self):
        path=urlparse(self.path).path
        redirects={'/connect':'/admin/help#iphone','/admin/backup':'/admin/maps#backup','/admin/events':'/admin/location#events'}
        if path in redirects:
            self.send_response(302)
            self.send_header('Location',redirects[path])
            self.send_header('Content-Length','0')
            self.end_headers()
            return
        page='system' if path=='/admin' else path.removeprefix('/admin/') if path.startswith('/admin/') and path!='/admin/system' else None
        if page in ADMIN_PAGES:
            self.send(admin_page(page),mime='text/html; charset=utf-8')
            return
        if path=='/google-sdk.mjs':
            self.send((ROOT/'google-sdk.mjs').read_bytes(),mime='text/javascript; charset=utf-8')
            return
        if path=='/current-location.mjs':
            self.send((ROOT/'current-location.mjs').read_bytes(),mime='text/javascript; charset=utf-8')
            return
        if path=='/hotspot.mjs':
            self.send((ROOT/'hotspot.mjs').read_bytes(),mime='text/javascript; charset=utf-8')
            return
        if path == '/api/maps/config':
            with LOCK: value=public_map_config()
            self.send(value)
            return
        if path == '/api/status':
            with LOCK: value=snapshot()
            self.send(value)
            return
        if path == '/api/admin/status':
            with LOCK:
                value=dict(device=DEVICE,app=snapshot(),wifi=wifi_snapshot(),debug=dict(phone_ip='192.168.71.2',events=copy.deepcopy(EVENTS)))
            self.send(value)
            return
        files={'/landmarks.mjs':('landmarks.mjs','text/javascript'),'/admin':('admin.html','text/html'),'/admin.css':('admin.css','text/css'),'/admin.js':('admin.js','text/javascript'),'/api.mjs':('api.mjs','text/javascript'),'/':('portal.html','text/html'),'/app.css':('portal.css','text/css'),'/app.js':('portal.js','text/javascript'),'/coordinates.mjs':('coordinates.mjs','text/javascript'),'/places.mjs':('places.mjs','text/javascript'),'/map-service.mjs':('map-service.mjs','text/javascript'),'/map-coordinates.mjs':('map-coordinates.mjs','text/javascript'),'/coord-transform.mjs':('coord-transform.mjs','text/javascript'),'/place-picker.mjs':('place-picker.mjs','text/javascript'),'/map-view.mjs':('map-view.mjs','text/javascript'),'/assets/maplibre-gl.mjs':('assets/maplibre-gl.mjs','text/javascript'),'/assets/maplibre-gl-worker.mjs':('assets/maplibre-gl-worker.mjs','text/javascript'),'/assets/maplibre-gl-shared.mjs':('assets/maplibre-gl-shared.mjs','text/javascript'),'/assets/maplibre-gl.css':('assets/maplibre-gl.css','text/css'),'/lucide-icons.mjs':('lucide-icons.mjs','text/javascript'),'/favicon.svg':('assets/favicon.svg','image/svg+xml'),'/licenses.txt':('assets/licenses.txt','text/plain')}
        if path not in files: self.send({'ok':False},404); return
        filename,mime=files[path]
        self.send((ROOT/filename).read_bytes(),mime=mime+'; charset=utf-8')

    def do_POST(self):
        try:
            size=int(self.headers.get('Content-Length',0))
            if not 0 < size <= 16384: raise ValueError('请求过大或缺少数据')
            value=json.loads(self.rfile.read(size))
            with LOCK: result=self.action(urlparse(self.path).path,value)
            self.send(result,202 if urlparse(self.path).path in ['/api/set','/api/restore','/api/replay','/api/wifi/scan','/api/wifi/connect','/api/wifi/disconnect','/api/wifi/hotspot'] else 200)
        except (ValueError, KeyError, TypeError) as error:
            self.send(dict(ok=False,message=str(error)),400)

    def action(self,path,value):
        global NEXT_JOB,NEXT_FAVORITE,MAP_CONFIG,NEXT_WIFI,GOOGLE_REPLAY
        if path == '/api/backup/export':
            raise ValueError('请在盒子网页上导出完整备份')
        if path in ['/api/wifi/scan','/api/wifi/connect','/api/wifi/disconnect','/api/wifi/hotspot']:
            if STATE['network_busy']: raise ValueError('已有 Wi-Fi 操作正在执行，请稍候')
            if STATE['operation'] or STATE['mode']=='simulating': raise ValueError('请先恢复真实定位，再扫描或更改 Wi-Fi')
            kind=path.rsplit('/',1)[-1]
            config=validate_wifi(value) if kind=='connect' else validate_hotspot(value) if kind=='hotspot' else {}
            job=dict(id=NEXT_WIFI,kind=kind,**config)
            NEXT_WIFI+=1
            WIFI['operation']=dict(id=job['id'],kind=kind,elapsed_ms=0,started=time.monotonic())
            STATE['network_busy']=True
            record_event(dict(scan='开始演示 Wi-Fi 扫描',connect='开始演示上游连接',disconnect='开始演示断开网络',hotspot='开始演示热点修改')[kind])
            WIFI_JOBS.put(job)
            return dict(ok=True,job_id=job['id'],**(dict(ssid=config['ssid']) if kind=='hotspot' else {}))
        if path == '/api/events/clear':
            EVENTS.clear()
            return dict(ok=True)
        replay = path == '/api/replay'
        if replay:
            if not STATE['recent']: raise ValueError('还没有成功定位记录，请先在网页设置一次定位')
            value=copy.deepcopy(STATE['recent'][0])
            if value.get('source')=='google':
                if not GOOGLE_REPLAY or time.monotonic()>=GOOGLE_REPLAY[0] or not same(value,GOOGLE_REPLAY[1]):
                    GOOGLE_REPLAY=None
                    raise ValueError('Google 地点需要联网更新，请在网页重新定位该地点')
                value=copy.deepcopy(GOOGLE_REPLAY[1])
            path='/api/set'
        if path == '/api/maps/config':
            config=map_config(value)
            PREVIEW_DATA.mkdir(mode=0o700, parents=True, exist_ok=True)
            temp=MAP_FILE.with_suffix('.tmp')
            with os.fdopen(os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), 'w') as handle:
                json.dump(config, handle)
            temp.chmod(0o600)
            temp.replace(MAP_FILE)
            MAP_CONFIG=config
            STATE['maps_configured']=configured(MAP_CONFIG['provider'])
            STATE['map_provider']=MAP_CONFIG['provider']
            STATE['configured_providers']={p:configured(p) for p in PROVIDERS}
            STATE['revision']+=1
            return dict(ok=True,message='地图服务配置已保存')
        if path in ['/api/set','/api/restore']:
            if STATE['network_busy']: raise ValueError('正在调整 Wi-Fi，完成后再操作定位')
            current=STATE['operation']
            if path=='/api/set' and current: raise ValueError('正在执行上一项操作，请稍候再切换地点')
            if path=='/api/restore' and current:
                if current['kind']=='restore': return dict(ok=True,job_id=current['id'])
                if STATE['restore_queued']: return dict(ok=True)
            p=place(value) if path=='/api/set' else None
            if p and ('latitude' not in p or 'longitude' not in p): raise ValueError('此地点需要联网解析，请在网页重新选择')
            job=dict(id=NEXT_JOB,kind='set' if p else 'restore',place=p,label=(p.get('query') or p['name']) if p else '真实定位',replay=replay)
            NEXT_JOB+=1
            if current: STATE['restore_queued']=True
            else: STATE['operation']=dict(id=job['id'],kind=job['kind'],label=job['label'],phase='queued',elapsed_ms=0,started=time.monotonic())
            record_event('演示定位操作已提交' if p else '演示恢复操作已提交')
            JOBS.put(job)
            return dict(ok=True,job_id=job['id'])
        if path=='/api/favorites':
            p=place(value, stored=True)
            duplicate=next((x for x in STATE['favorites'] if x['id']!=value.get('id') and same(x,p)),None)
            if duplicate: raise ValueError('这个地点已收藏为“'+duplicate['name']+'”')
            old=next((x for x in STATE['favorites'] if x['id']==value.get('id')),None)
            if value.get('id') and not old: raise ValueError('收藏不存在，请刷新')
            if old: old.update(p)
            else:
                if len(STATE['favorites'])>=20: raise ValueError('常用地点已满，请先删除不用的地点')
                STATE['favorites'].append(dict(id=NEXT_FAVORITE,**p));NEXT_FAVORITE+=1
            persist()
        elif path=='/api/favorites/delete':
            STATE['favorites']=[x for x in STATE['favorites'] if x['id']!=value['id']];persist()
        elif path=='/api/history/clear': STATE['recent']=[]
        elif path=='/api/favorites/import':
            if value.get('version') not in (1,2) or not isinstance(value.get('favorites'),list) or len(value['favorites'])>20: raise ValueError('请选择有效的收藏备份')
            incoming=[place(p, stored=True) for p in value['favorites']]
            merged=copy.deepcopy(STATE['favorites']); added=skipped=0
            for p in incoming:
                if any(same(x,p) for x in merged): skipped+=1;continue
                if len(merged)>=20: raise ValueError('合并后超过20个常用地点，未导入任何数据')
                merged.append(dict(id=NEXT_FAVORITE+added,**p));added+=1
            NEXT_FAVORITE+=added;STATE['favorites']=merged;persist()
            return dict(ok=True,message=f'已导入 {added} 个地点，跳过 {skipped} 个重复地点')
        else: raise ValueError('未知接口')
        STATE['revision']+=1
        return dict(ok=True)


if __name__ == '__main__':
    import argparse
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port',type=int,default=8765)
    parser.add_argument('--unpaired',action='store_true')
    args=parser.parse_args()
    if args.unpaired: STATE.update(paired=False, phone=None, mode='unknown')
    print(f'MapPin-Box 网页预览 http://127.0.0.1:{args.port}/',flush=True)
    ThreadingHTTPServer(('127.0.0.1',args.port),Handler).serve_forever()
