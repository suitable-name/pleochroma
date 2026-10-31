// CUDA mirror of geometry/tool/{interval.rs,tool.rs::sd_normal} and intersect_stone.rs.
struct Hit {float t;V normal;int facet;bool found;};
struct Range {float lo,hi;bool valid;};
__device__ Range clip(float lo,float hi,float s0,float s1){lo=fmaxf(lo,s0);hi=fminf(hi,s1);return {lo,hi,lo<=hi};}
__device__ Range roots(float a,float b,float c){
 float disc=fmaf(b,b,-a*c);if(disc<=1e-7f*fmaf(b,b,fabsf(a*c)))return {0,0,false};
 float q=-(b+copysignf(sqrtf(disc),b)),r1=q/a,r2=c/q;return {fminf(r1,r2),fmaxf(r1,r2),true};
}
__device__ Range interval(const float* p,V origin,V dir){
 int kind=(int)p[0];V o=origin-loadv(p+1),a=loadv(p+4);float hl=p[7],r0=p[8],r1=p[9];
 if(kind==0)return roots(dot(dir,dir),dot(o,dir),fmaf(r0,-r0,dot(o,o)));
 float z=dot(o,a),dz=dot(dir,a),s0=-1e30f,s1=1e30f;
 if(kind==2){
  V d=loadv(p+11),w=cross(a,d);float k=(r1-r0)/(2*hl),rm=0.5f*(r0+r1),stroke=p[10];
  V ns[6]={a,-a,d,-d,w-a*k,-w-a*k};float es[6]={-hl,-hl,-stroke,-stroke,-rm,-rm};
  for(int i=0;i<6;i++){float den=dot(ns[i],dir),side=dot(ns[i],o)+es[i];
   if(fabsf(den)>1e-7f){float t=-side/den;if(den<0)s0=fmaxf(s0,t);else s1=fminf(s1,t);}
   else if(side>0)return {0,0,false};}
  return {s0,s1,s0<=s1};
 }
 if(fabsf(dz)>1e-7f){float x=(-hl-z)/dz,y=(hl-z)/dz;s0=fminf(x,y);s1=fmaxf(x,y);}
 else if(fabsf(z)>hl)return {0,0,false};
 float rm=0.5f*(r0+r1),k=(r1-r0)/(2*hl);V q=o-a*z,r=dir-a*dz;
 float rz=fmaf(k,z,rm),rate=k*dz;
 float qa=fmaf(rate,-rate,dot(r,r)),qb=fmaf(-rate,rz,dot(q,r)),qc=fmaf(rz,-rz,dot(q,q));
 if(fabsf(qa)<=1e-7f){if(fabsf(qb)<=1e-7f)return {s0,s1,qc<=0};float t=-qc/(2*qb);return qb>0?clip(-1e30f,t,s0,s1):clip(t,1e30f,s0,s1);}
 Range rr=roots(qa,qb,qc);if(!rr.valid)return rr;
 if(qa>0)return clip(rr.lo,rr.hi,s0,s1);
 Range x=clip(-1e30f,rr.lo,s0,s1),y=clip(rr.hi,1e30f,s0,s1);
 if(x.valid&&y.valid)return {fminf(x.lo,y.lo),fmaxf(x.hi,y.hi),true};return x.valid?x:y;
}
struct Distance {float sd;V normal;};
__device__ Distance distance_normal(const float* p,V point){
 int kind=(int)p[0];V q=point-loadv(p+1),a=loadv(p+4);float hl=p[7],r0=p[8],r1=p[9];
 if(kind==0){float l=sqrtf(len2(q));return {l-r0,l>1e-12f?q/l:v(0,0,1)};}
 float z=dot(q,a);Distance best=z>=0?Distance{z-hl,a}:Distance{-z-hl,-a};
 if(kind==1){
  V perp=q-a*z;float rho=sqrtf(len2(perp));V rh=rho>1e-12f?perp/rho:basis(a);
  float dr=r1-r0,l=sqrtf(fmaf(4*hl,hl,dr*dr)),rm=0.5f*(r0+r1);
  float sd=fmaf(z,-dr,(rho-rm)*2*hl)/l;V n=rh*(2*hl/l)-a*(dr/l);
  return sd>best.sd?Distance{sd,n}:best;
 }
 V d=loadv(p+11),w=cross(a,d);float x=dot(q,d),y=dot(q,w),k=(r1-r0)/(2*hl),rm=0.5f*(r0+r1),kap=1/sqrtf(fmaf(k,k,1.0f));
 Distance along=x>=0?Distance{x-p[10],d}:Distance{-x-p[10],-d};if(along.sd>best.sd)best=along;
 Distance pos={(y-rm-k*z)*kap,(w-a*k)*kap};if(pos.sd>best.sd)best=pos;
 Distance neg={(-y-rm-k*z)*kap,(-w-a*k)*kap};if(neg.sd>best.sd)best=neg;return best;
}
__device__ V tool_normal(const float* tools,int tool,V point){
 const float* t=tools+85*tool;float best=1e30f;V n=v(0,0,1);
 for(int j=0;j<(int)t[0];j++){Distance d=distance_normal(t+1+14*j,point);if(fabsf(d.sd)<best){best=fabsf(d.sd);n=d.normal;}}
 return -n;
}
template<bool HAS_TOOLS> __device__ __forceinline__ Hit intersect(const float* planes,int np,const float* tools,int nt,V origin,V dir){
 float near=-1e30f,far=1e30f;int ni=-1,fi=-1;
 for(int i=0;i<np;i++){const float* p=planes+i*4;V n=loadv(p);float den=dot(n,dir),side=p[3]+dot(n,origin);
  if(fabsf(den)>1e-7f){float t=-side/den;if(den<0){if(t>near){near=t;ni=i;}}else if(t<far){far=t;fi=i;}}
  else if(side>0)return {0,v(0,0,0),0,false};}
 if(near>far)return {0,v(0,0,0),0,false};
 if constexpr(!HAS_TOOLS){int idx=near>1e-4f?ni:fi;float t=near>1e-4f?near:far;return {t,idx>=0?loadv(planes+4*idx):v(0,0,0),idx>=0?idx:0,t>1e-4f};}
 float ts[258];int keys[258];int n=2;ts[0]=near;keys[0]=0;ts[1]=far;keys[1]=1;
 for(int i=0;i<nt;i++){const float* t=tools+85*i;Range hull={0,0,false};
  for(int j=0;j<(int)t[0];j++){Range r=interval(t+1+j*14,origin,dir);if(r.valid){if(!hull.valid)hull=r;else{hull.lo=fminf(hull.lo,r.lo);hull.hi=fmaxf(hull.hi,r.hi);}}}
  if(hull.valid){ts[n]=hull.lo;keys[n++]=2+2*i;ts[n]=hull.hi;keys[n++]=3+2*i;}}
 // Insertion sort preserves exact equal-parameter key ordering from the CPU carve.
 for(int i=1;i<n;i++){float t=ts[i];int key=keys[i],j=i-1;while(j>=0&&(ts[j]>t||(ts[j]==t&&keys[j]>key))){ts[j+1]=ts[j];keys[j+1]=keys[j];j--;}ts[j+1]=t;keys[j+1]=key;}
 int pd=0,td=0;bool inside=false;
 for(int i=0;i<n;i++){int key=keys[i];if(key==0)pd++;else if(key==1)pd--;else if(key%2==0)td++;else td--;
  bool now=pd>0&&td<=0;if(now==inside)continue;inside=now;if(ts[i]<=1e-4f)continue;
  if(key<2){int idx=key==0?ni:fi;return {ts[i],idx>=0?loadv(planes+4*idx):v(0,0,0),idx>=0?idx:0,true};}
  int ti=(key-2)/2;return {ts[i],tool_normal(tools,ti,origin+dir*ts[i]),np+ti,true};}
 return {0,v(0,0,0),0,false};
}
