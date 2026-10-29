"""Codex's #11 sweep (review of 2026-10-29): d1 in {−9, −5, −1, 0, 1, 5, 9} at 26 expiries from
1e-12 s to one year, five volatilities, four spot scales and three rate/carry sets (K solved from
d1), plus 3,000 broad long-expiry points. Needs mpmath (a local dev venv; not a crate dependency).

    python gen_sweep.py OUT.csv
"""
import mpmath as m,random,sys
from pathlib import Path
m.mp.dps=90;S=1<<64;rng=random.Random(939342)
min_s=18446744073709;max_s=10**9*S
secs=['1e-12','1e-10','1e-8','1e-6','1e-4','.001','.01','.03','.1','.3','1','2','3','5','10','20','30','60','120','300','600','1800','3600','86400','2592000','31536000']
rows=[];meta={}
for sec in secs:
 t=max(1,int(m.mpf(sec)/31536000*S));meta[str(t)]=sec
 for sig in ['.005','.2','.5','1','5']:
  vr=int(m.mpf(sig)*S);v=m.mpf(vr)/S;T=m.mpf(t)/S
  for sr in [min_s,S,100*S,max_s]:
   s=m.mpf(sr)/S
   for rr,qq in [('0','0'),('.05','.02'),('-.1','.5')]:
    rraw=int(m.floor(m.mpf(rr)*S));qraw=int(m.floor(m.mpf(qq)*S));r=m.mpf(rraw)/S;q=m.mpf(qraw)/S
    for d1 in [-9,-5,-1,0,1,5,9]:
     k=s*m.exp((r-q+v*v/2)*T-d1*v*m.sqrt(T));kr=int(m.nint(k*S))
     if min_s<=kr<=max_s and sr*20>=kr and kr*20>=sr:
      for kind in [0,1]: rows.append((sr,kr,t,vr,rraw,qraw,kind))
# Broad long-expiry coverage, including both tails and carry extremes.
for _ in range(3000):
 s=m.mpf(10)**rng.uniform(-6,9);k=min(m.mpf(10)**9,max(m.mpf('1e-6'),s*m.mpf(10)**rng.uniform(-1.30,1.30)))
 t=m.mpf(10)**rng.uniform(-3,m.log10(30));v=m.mpf(10)**rng.uniform(m.log10(m.mpf('.005')),m.log10(5));r=rng.uniform(-.1,.5);q=rng.uniform(-.1,.5)
 rows.append((int(s*S),int(k*S),int(t*S),int(v*S),int(r*S),int(q*S),rng.randrange(2)))
Path(sys.argv[1]).write_text(''.join(','.join(map(str,x))+'\n' for x in rows));print(len(rows))
