"""100-digit exact-target roots and root vega ratios for gen_conditioning.py's set.
Reads conditioning-inputs.csv, emits five 2001-row gzip CSV blocks. No optfix calls.
Schema: S,K,T,sigma_seed,r,q,kind,target,sigma_star_raw,vega_star_raw,ratio_rounded_E.
All first ten columns are integers; final column are 85-significant-digit decimal strings.
E_raw = ceil(max(S_raw,K_raw)/2^48)+64, exactly price_error_bound's Q64 rounding.
"""
import csv,mpmath as m,gzip,io,json,os
m.mp.dps=100;SF=m.mpf(2)**64
N=lambda z:m.erfc(-z/m.sqrt(2))/2
base=os.path.dirname(os.path.abspath(__file__));blocks=[];rows=[];ratios=[]
for idx,row in enumerate(csv.reader(open(base+'/conditioning-inputs.csv'))):
 x=list(map(int,row));s,k,t,sig,r,q=[m.mpf(v)/SF for v in x[:6]];put=x[6];target=m.mpf(x[7])/SF;dq=m.exp(-q*t);dr=m.exp(-r*t);rt=m.sqrt(t)
 def ev(v):
  a=v*rt;d1=(m.log(s/k)+(r-q+v*v/2)*t)/a;d2=d1-a
  p=k*dr*N(-d2)-s*dq*N(-d1) if put else s*dq*N(d1)-k*dr*N(d2)
  return p,s*dq*m.exp(-d1*d1/2)/m.sqrt(2*m.pi)*rt
 for _ in range(15):
  p,vg=ev(sig);step=(p-target)/vg;sig-=step
  if abs(step)<m.mpf('1e-85'):break
 p,vg=ev(sig);eraw=(max(x[0],x[1])+(1<<48)-1)//(1<<48)+64;ratio=vg/(m.mpf(eraw)/SF*(1<<28))
 assert abs(p-target)<m.mpf('1e-80')
 rows.append(x+[int(m.nint(sig*SF)),int(m.nint(vg*SF)),m.nstr(ratio,85)]);ratios.append(ratio)
 if len(rows)==2001:
  b=io.StringIO();csv.writer(b,lineterminator='\n').writerows(rows);name=f'conditioning-exact-{len(blocks)+1}.csv.gz';data=gzip.compress(b.getvalue().encode(),mtime=0);open(base+'/'+name,'wb').write(data);blocks.append({'filename':name,'rows':len(rows),'bytes':len(data)});rows=[]
assert len(blocks)==5 and not rows
print(json.dumps({'blocks':blocks,'rows':idx+1,'ratio_min':str(min(ratios)),'ratio_max':str(max(ratios))},indent=2))
