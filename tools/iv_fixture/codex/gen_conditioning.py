import mpmath as m,json
m.mp.dps=100;S=1<<64;SF=m.mpf(S)
N=lambda z:m.erfc(-z/m.sqrt(2))/2
rows=[];meta={}
for vs,ts in [('.005','1'),('.2','1'),('.5','.01'),('1','.01'),('5','.0001')]:
 vr=int(m.floor(m.mpf(vs)*S));tr=int(m.floor(m.mpf(ts)*S));v=m.mpf(vr)/SF;t=m.mpf(tr)/SF;a=v*m.sqrt(t)
 def f(x):
  k=100*m.exp(x);d1=-x/a+a/2;vega=100*m.sqrt(t)*m.exp(-d1*d1/2)/m.sqrt(2*m.pi);E=k*m.mpf(2)**-48+64/SF
  return m.log(vega)-m.log(m.mpf(2)**28*E)
 xc=m.findroot(f,(a*4,a*6))
 for j in range(-1000,1001):
  x=xc+j*m.mpf('1e-9');kr=int(m.nint(100*m.exp(x)*SF));k=m.mpf(kr)/SF;d1=(m.log(100/k)+v*v*t/2)/a;d2=d1-a;p=100*N(d1)-k*N(d2);target=int(m.floor(p*SF));vega=100*m.sqrt(t)*m.exp(-d1*d1/2)/m.sqrt(2*m.pi);E=k*m.mpf(2)**-48+64/SF
  row=(100*S,kr,tr,vr,0,0,0,target);line=','.join(map(str,row));rows.append(line);meta[line]=str(vega/(m.mpf(2)**28*E))
p='/private/tmp/optfix-iv-independent/';open(p+'conditioning-inputs.csv','w').write('\n'.join(rows)+'\n');open(p+'inputs.csv','w').write(open(p+'conditioning-inputs.csv').read());json.dump(meta,open(p+'conditioning-meta.json','w'));print(len(rows),'threshold-straddling cases')
