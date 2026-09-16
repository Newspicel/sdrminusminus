import argparse, hashlib, json, re, struct, subprocess, tempfile, urllib.request
from pathlib import Path
parser = argparse.ArgumentParser()
parser.add_argument("--cache", type=Path, required=True)
parser.add_argument("--out", type=Path, required=True)
args = parser.parse_args()
args.cache.mkdir(parents=True, exist_ok=True)
REFERENCES = {'map': ('https://raw.githubusercontent.com/gnuradio/gnuradio/aee9fd3f79389c4282a98e8d62c8405c73fd91df/gr-dtv/lib/dvbs2/dvbs2_modulator_bc_impl.cc', 'd4f9f7ec4684054b9b9b6f8ddb4084b922ce0792d63ad8508a9bfd61f37296a9'), 'physical': ('https://raw.githubusercontent.com/gnuradio/gnuradio/aee9fd3f79389c4282a98e8d62c8405c73fd91df/gr-dtv/lib/dvbs2/dvbs2_physical_cc_impl.cc', 'ee3c6844331b5a00ea5ac68af185e51b43471dfbb1e914f80ea3efc5fe869b76'), 'interleave': ('https://raw.githubusercontent.com/gnuradio/gnuradio/aee9fd3f79389c4282a98e8d62c8405c73fd91df/gr-dtv/lib/dvbs2/dvbs2_interleaver_bb_impl.cc', '0f632d6e15e404786e40c63e08541cc53a3cbe3c4d2d49beecd7fbfaf834f571'), 'ldpc': ('https://raw.githubusercontent.com/gnuradio/gnuradio/aee9fd3f79389c4282a98e8d62c8405c73fd91df/gr-dtv/lib/dvb/dvb_ldpc_bb_impl.cc', '686c73086c9daaea1ab30699c3a264755fd2e2f043060cc52cc1173a8c47d675')}
sources = {}
for name, (url, digest) in REFERENCES.items():
    path = args.cache / (name + ".cc")
    if not path.exists():
        urllib.request.urlretrieve(url, path)
    data = path.read_bytes()
    if hashlib.sha256(data).hexdigest() != digest:
        raise ValueError("Reference checksum mismatch: " + str(path))
    sources[name] = data.decode()

def balanced(text,start):
    begin=text.index('{',start);depth=1;end=begin+1
    while depth:
        depth+=(text[end]=='{')-(text[end]=='}');end+=1
    return text[begin+1:end-1]
mapbody=balanced(sources['map'],sources['map'].index('switch (constellation)'))
interbody=balanced(sources['interleave'],sources['interleave'].index('switch (constellation)'))
phy=sources['physical'];a=phy.index('    if (constellation == MOD_QPSK)');b=phy.index('    // Now create the PL header.',a);phybody=phy[a:b]
rates=sorted(set(re.findall(r'\bC\d+_\d+\b',mapbody+interbody+phybody)))
mods=sorted(set(re.findall(r'\bMOD_\w+\b',mapbody+interbody+phybody)))
vls=sorted(set(re.findall(r'\bC\w+_VLSNR\w*\b',phybody)))
constants='enum{'+','.join(rates+mods+vls+['FECFRAME_NORMAL','FECFRAME_SHORT'])+'};\n'
arrays='std::complex<float> m_bpsk[2][2], m_qpsk[4], m_8psk[8], m_16apsk[16], m_32apsk[32], m_64apsk[64], m_128apsk[128], m_256apsk[256];'
select='''auto points = mod==2 ? m_qpsk : mod==3 ? m_8psk : mod==4 ? m_16apsk : mod==5 ? m_32apsk : mod==6 ? m_64apsk : mod==7 ? m_128apsk : m_256apsk;'''
source='#include <cmath>\n#include <complex>\n#include <cstdio>\nusing gr_complex=std::complex<float>;\nconst double GR_M_PI=3.14159265358979323846;\nconst int FRAME_SIZE_NORMAL=64800,FRAME_SIZE_SHORT=16200;\n'+constants+'void set_output_multiple(int) {}\nvoid dump(int framesize,int rate,int constellation){\nint frame_size=framesize==FECFRAME_NORMAL?64800:16200;\nint slots=0,pilot_symbols=0,modcod=0;\n'+phybody+'''if(modcod<132 || (modcod<216 && framesize!=FECFRAME_NORMAL) || (modcod>=216 && framesize!=FECFRAME_SHORT)) return;
int mod=0,rows=0,packed_items=0;
int rowaddr0=0,rowaddr1=0,rowaddr2=0,rowaddr3=0,rowaddr4=0,rowaddr5=0,rowaddr6=0,rowaddr7=0;
switch(constellation){'''+interbody+'''}
double r0=0,r1=1,r2=0,r3=0,r4=0,r5=0,r6=0,r7=0,r8=0,m=1;
'''+arrays+'switch(constellation){'+mapbody+'}\n'+select+'''
int addresses[]={rowaddr0,rowaddr1,rowaddr2,rowaddr3,rowaddr4,rowaddr5,rowaddr6,rowaddr7};
printf("%d %d %d %d %d %d",modcod,framesize==FECFRAME_SHORT,rate,constellation,mod,slots);
for(int i=0;i<mod;i++) printf(" %d",addresses[i]/rows);
for(int i=0;i<(1<<mod);i++) printf(" %.9g %.9g",points[i].real(),points[i].imag());
printf("\\n");}
int main(){'''+''.join(f'dump({frame},{r},{m});\n' for frame in ['FECFRAME_NORMAL','FECFRAME_SHORT'] for r in rates for m in mods)+'}\n'
with tempfile.TemporaryDirectory(prefix="s2x-reference-") as temporary:
    source_path = Path(temporary) / "extract.cc"
    binary_path = Path(temporary) / "extract"
    source_path.write_text(source)
    subprocess.run(["clang++", "-O2", "-std=c++17", str(source_path), "-o", str(binary_path)], check=True)
    raw = subprocess.check_output([str(binary_path)], text=True)
records=[]
for line in raw.splitlines():
    x=line.split();code,short,rate,mode,bits,slots=map(int,x[:6]);order=list(map(int,x[6:6+bits]));points=list(map(float,x[6+bits:]));records.append(dict(code=code,short=bool(short),rate=rates[rate][1:],mode=mods[mode-len(rates)],bits=bits,slots=slots,order=order,points=list(zip(points[::2],points[1::2]))))
records.sort(key=lambda r:r['code'])
assert len({r['code'] for r in records})==len(records)
args.cache.joinpath("modes.json").write_text(json.dumps(records))
root = args.out
modes = records
points=root/'s2x'/'points';points.mkdir(parents=True,exist_ok=True)
mods={2:'Qpsk',3:'Psk8',4:'Apsk16',5:'Apsk32',6:'Apsk64',7:'Apsk128',8:'Apsk256'}
lines=['use super::{Mode, Modulation, Rate};','']
for r in modes:
    code=r['code'];name=f'm{code}';lines.append(f'mod {name};')
    def number(v):
        target = struct.pack('<f', v)
        s = next(f'{v:.{digits}g}' for digits in range(1, 10) if struct.pack('<f', float(f'{v:.{digits}g}')) == target)
        return s if '.' in s or 'e' in s else s+'.0' 
    (points/(name+'.rs')).write_text('pub const POINTS: &[(f32, f32)] = &[\n'+''.join(f'    ({number(a)}, {number(b)}),\n' for a,b in r['points'])+'];\n')
lines+=['','pub const MODES: &[Mode] = &[']
for r in modes:
    code=r['code'];mod='Apsk8' if r['mode']=='MOD_8APSK' else mods[r['bits']]
    lines.append(f'    Mode {{ code: {code}, short: {str(r["short"]).lower()}, modulation: Modulation::{mod}, rate: Rate::R{r["rate"]}, order: &{r["order"]}, points: m{code}::POINTS }},')
lines+= ['];']
(points/'mod.rs').write_text('\n'.join(lines)+'\n')
ldpc=sources['ldpc']
pattern=r'ldpc_tab_(\d+_\d+)([NS])\s*\[\d+\]\s*\[\d+\]\s*=\s*\{(.*?)\};'
tables={}
for m in re.finditer(pattern,ldpc,re.S):
    rate,size,data=m.groups();rows=[]
    for row in re.findall(r'\{([^{}]+)\}',data):
        values=[int(n) for n in re.findall(r'\d+',row)];rows.append(values[1:1+values[0]])
    tables[(rate,size)]=rows
need=sorted(set((r['rate'],'S' if r['short'] else 'N') for r in modes))
existing={(rate,'N') for rate in ['1_4','1_3','2_5','1_2','3_5','2_3','3_4','4_5','5_6','8_9','9_10','2_9']}|{(rate,'S') for rate in ['1_4','1_3','2_5','1_2','3_5','2_3','3_4','4_5','5_6','8_9','11_45','4_15']}
folder=root/'s2x'/'tables';folder.mkdir(exist_ok=True)
lines=['use super::{Frame, Rate};','']
for rate,size in need:
    if (rate,size) in existing:continue
    rows=tables[(rate,size)];name='r'+rate+'_'+size.lower();lines.append(f'mod {name};')
    (folder/(name+'.rs')).write_text(f'pub const ADDRESSES: &[&[u16]] = &[\n'+''.join('    &['+', '.join(map(str,row))+'],\n' for row in rows)+'];\n')
lines+=['','pub fn addresses(rate: Rate, frame: Frame) -> Option<&\'static [&\'static [u16]]> {','    match (rate, frame) {']
for rate,size in need:
    if (rate,size) not in existing:lines.append(f'        (Rate::R{rate}, Frame::{"Short" if size=="S" else "Normal"}) => Some(r{rate}_{size.lower()}::ADDRESSES),')
lines+=['        _ => None,','    }','}']
(folder/'mod.rs').write_text('\n'.join(lines)+'\n')
