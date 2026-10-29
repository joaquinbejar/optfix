"""Validate conditioning-exact-*.csv.gz with independent mpmath at 100 digits.
Usage: python3 check_threshold_fixture.py [directory]
Columns: eight raw inputs, rounded sigma_star, rounded vega_star, 85-digit vega/threshold.
No optfix code or outputs are required. E uses the public Q64 upward rounding exactly.
"""
import csv,gzip,glob,os,sys,mpmath as m,json
m.mp.dps=100;SF=m.mpf(2)**64
base=sys.argv[1] if len(sys.argv)>1 else os.path.dirname(os.path.abspath(__file__))
N=lambda z:m.erfc(-z/m.sqrt(2))/2
rows=0;bad=[]
for path in sorted(glob.glob(os.path.join(base,'conditioning-exact-*.csv.gz'))):
 with gzip.open(path,'rt') as f:
  for row in csv.reader(f):
   x=list(map(int,row[:10]));s,k,t,sig,r,q=[m.mpf(v)/SF for v in x[:6]];target=m.mpf(x[7])/SF;rt=m.sqrt(t);dq=m.exp(-q*t);dr=m.exp(-r*t)
   def ev(v):
    a=v*rt;d1=(m.log(s/k)+(r-q+v*v/2)*t)/a;d2=d1-a
    p=k*dr*N(-d2)-s*dq*N(-d1) if x[6] else s*dq*N(d1)-k*dr*N(d2)
    return p,s*dq*m.exp(-d1*d1/2)/m.sqrt(2*m.pi)*rt
   for _ in range(15):
    p,vg=ev(sig);step=(p-target)/vg;sig-=step
    if abs(step)<m.mpf('1e-85'):break
   p,vg=ev(sig);eraw=(max(x[0],x[1])+(1<<48)-1)//(1<<48)+64;ratio=vg/(m.mpf(eraw)/SF*(1<<28));rows+=1
   if (int(m.nint(sig*SF))!=x[8] or int(m.nint(vg*SF))!=x[9] or abs(ratio-m.mpf(row[10]))>m.mpf('5e-84') or abs(p-target)>m.mpf('1e-80')):bad.append((path,rows))
print(json.dumps({'rows':rows,'mismatches':len(bad),'first':bad[:5]}));assert rows==10005 and not bad
