import csv,mpmath as m,json,collections
m.mp.dps=100;S=m.mpf(2)**64
N=lambda z:m.erfc(-z/m.sqrt(2))/2
stats=collections.Counter();badcond=[];baderr=[];rooterrs=[];worst=m.mpf(0)
for row in csv.reader(open('/private/tmp/optfix-iv-independent/conditioning-output.csv')):
 x=list(map(int,row[:8]));s,k,t,sig,r,q=[m.mpf(v)/S for v in x[:6]];put=x[6];target=m.mpf(x[7])/S
 stats[row[8]+(':'+row[9] if row[8]=='ERR' else '')]+=1
 if row[8]!='OK':continue
 got=m.mpf(row[9])/S
 def eval(v):
  a=v*m.sqrt(t);d1=(m.log(s/k)+(r-q+v*v/2)*t)/a;d2=d1-a;dq=m.exp(-q*t);dr=m.exp(-r*t);vega=s*dq*m.exp(-d1*d1/2)/m.sqrt(2*m.pi)*m.sqrt(t)
  p=k*dr*N(-d2)-s*dq*N(-d1) if put else s*dq*N(d1)-k*dr*N(d2)
  return p,vega
 try:
  root=got
  for _ in range(12):
   p,v=eval(root);step=(p-target)/v;root-=step
   if abs(step)<m.mpf('1e-80'):break
  p,v=eval(root)
  e=max(s,k)*m.mpf(2)**-48+64/S
  threshold=m.mpf(2)**28*e
  if v<threshold:badcond.append({'input':x,'vega_ratio':str(v/threshold),'returned':str(got),'root':str(root)})
  bound=4*e/v+m.mpf(2)**-40*root;ratio=abs(got-root)/bound;worst=max(worst,ratio)
  if ratio>1:baderr.append({'input':x,'ratio':str(ratio)})
 except Exception as ex:rooterrs.append((x,str(ex)))
print(json.dumps({'stats':dict(stats),'worst_sigma_ratio':str(worst),'bad_conditioning':badcond[:5],'bad_error':baderr[:5],'root_errors':rooterrs[:5]},indent=2))
