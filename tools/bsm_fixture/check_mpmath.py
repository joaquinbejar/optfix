"""Codex's independent 100-digit mpmath checker for #11: reads the exporter's output (inputs,
then price, delta, gamma, vega, theta as raw Q64.64, or ERR) and counts envelope violations.

    python check_mpmath.py OUTPUT.csv RESULT.json
"""
import mpmath as mp,csv,json,sys
mp.mp.dps=100;S=mp.mpf(2)**64
N=lambda z:mp.erfc(-z/mp.sqrt(2))/2
phi=lambda z:mp.exp(-z*z/2)/mp.sqrt(2*mp.pi)
counts=[0]*5;worst=[(mp.mpf(0),None)]*5;errors=[];first={}
for row in csv.reader(open(sys.argv[1])):
 raw=list(map(int,row[:7]));s,k,t,sig,r,q=[mp.mpf(v)/S for v in raw[:6]];put=raw[6]
 dq=mp.exp(-q*t);dr=mp.exp(-r*t);rt=mp.sqrt(t);a=sig*rt;d1=(mp.log(s/k)+(r-q+sig*sig/2)*t)/a;d2=d1-a;pdf=phi(d1)
 if put:
  price=k*dr*N(-d2)-s*dq*N(-d1);delta=-dq*N(-d1);theta=-s*dq*pdf*sig/(2*rt)+r*k*dr*N(-d2)-q*s*dq*N(-d1)
 else:
  price=s*dq*N(d1)-k*dr*N(d2);delta=dq*N(d1);theta=-s*dq*pdf*sig/(2*rt)-r*k*dr*N(d2)+q*s*dq*N(d1)
 wants=[price,delta,dq*pdf/(s*a),s*dq*pdf*rt,theta]
 if row[7]=='ERR':errors.append((raw,row[8],str(max(abs(v) for v in wants))));continue
 scales=[max(s,k),1,dq/(s*a),s*dq*rt,s*dq*sig/rt+abs(r)*k*dr+abs(q)*s*dq]
 bounds=[scales[0]*mp.mpf(2)**-48+64/S,mp.mpf(2)**-49,scales[2]*mp.mpf(2)**-40+64/S,scales[3]*mp.mpf(2)**-43+64/S,scales[4]*mp.mpf(2)**-42+64/S]
 for j,(got,want,bound) in enumerate(zip(map(int,row[7:]),wants,bounds)):
  err=abs(mp.mpf(got)/S-want);ratio=err/bound
  if ratio>worst[j][0]:worst[j]=(ratio,raw)
  if ratio>1:
   counts[j]+=1
   if j not in first:first[j]={'input_raw':raw,'got_raw':got,'expected':mp.nstr(want,70),'error':mp.nstr(err,35),'allowed':mp.nstr(bound,35),'ratio':mp.nstr(ratio,15),'d1':mp.nstr(d1,20)}
print('violations price/delta/gamma/vega/theta',counts)
print('worst',[(str(a),b) for a,b in worst]);print('errors',errors[:5], 'count',len(errors));print(json.dumps(first,indent=2))
open(sys.argv[2],'w').write(json.dumps({'violations':counts,'first':first,'errors':errors},indent=2))
