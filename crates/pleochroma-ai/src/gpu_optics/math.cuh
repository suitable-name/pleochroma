// Native CUDA mirror of pleochroma/src/optics/birefringence/{uniaxial,biaxial,helpers}.rs.
// Explicit fmaf calls correspond to Rust mul_add. NVRTC auto contraction is disabled.
struct V { float x,y,z; };
__device__ V v(float x,float y,float z){return {x,y,z};}
__device__ V operator+(V a,V b){return v(a.x+b.x,a.y+b.y,a.z+b.z);}
__device__ V operator-(V a,V b){return v(a.x-b.x,a.y-b.y,a.z-b.z);}
__device__ V operator-(V a){return v(-a.x,-a.y,-a.z);}
__device__ V operator*(V a,float s){return v(a.x*s,a.y*s,a.z*s);}
__device__ V operator/(V a,float s){return a*(1.0f/s);}
__device__ float dot(V a,V b){return (a.x*b.x+a.y*b.y)+a.z*b.z;}
__device__ float dotf(V a,V b){return fmaf(a.x,b.x,fmaf(a.y,b.y,a.z*b.z));}
__device__ V cross(V a,V b){return v(a.y*b.z-a.z*b.y,a.z*b.x-a.x*b.z,a.x*b.y-a.y*b.x);}
__device__ V crossf(V a,V b){return v(fmaf(a.y,b.z,-a.z*b.y),fmaf(a.z,b.x,-a.x*b.z),fmaf(a.x,b.y,-a.y*b.x));}
__device__ float len2(V a){return dot(a,a);}
__device__ V norm(V a){float q=len2(a);return q>0.0f?a/sqrtf(q):v(0,0,0);}
__device__ float clamp(float x,float a,float b){return fminf(fmaxf(x,a),b);}
__device__ V loadv(const float* a){return v(a[0],a[1],a[2]);}
__device__ V basis(V n){V a=fabsf(n.x)>0.9f?v(0,1,0):v(1,0,0);return norm(a-n*dot(n,a));}
__device__ V canonical(V a){float ax=fabsf(a.x),ay=fabsf(a.y),az=fabsf(a.z);float b=ax>=ay&&ax>=az?a.x:ay>=az?a.y:a.z;return b<0?-a:a;}
// Material: kind,no,ne,na,nb,ng,c[3], optical axes[9], absorption axes[9], principal.
struct Mat {
 int kind; float no,ne,na,nb,ng; V c,a,b,g,pa,pb,pc; bool principal;
};
__device__ Mat material(const float* p){return {(int)p[0],p[1],p[2],p[3],p[4],p[5],loadv(p+6),loadv(p+9),loadv(p+12),loadv(p+15),loadv(p+18),loadv(p+21),loadv(p+24),p[27]!=0};}
__device__ V local(Mat m,V k){return v(dot(m.a,k),dot(m.b,k),dot(m.g,k));}
__device__ V world(Mat m,V k){return m.a*k.x+m.b*k.y+m.g*k.z;}
__device__ float effective(float no,float ne,V k,V c,bool exact_axis){
 float co=fminf(fabsf(dot(k,c)),1.0f);
 if(exact_axis&&co>=1.0f-1e-7f)return no;
 if(fabsf(no-ne)<1e-5f)return no;
 float theta=acosf(co),si=sinf(theta);co=cosf(theta);
 float d=fmaf(ne*co,ne*co,(no*si)*(no*si));
 return d<=1e-8f?no:(no*ne)/sqrtf(d);
}
__device__ bool degenerate(float a,float b){return fabsf(a-b)<=sqrtf(1.1920928955078125e-7f)*fmaxf(fmaxf(fabsf(a),fabsf(b)),1.0f);}
__device__ float wave_index(Mat m,V k,int mode){
 if(m.kind==0)return m.no;
 if(m.kind==1)return mode==0?m.no:effective(m.no,m.ne,k,m.c,true);
 k=norm(k);bool ab=degenerate(m.na,m.nb),bg=degenerate(m.nb,m.ng);
 if(ab&&bg)return (m.na+m.nb+m.ng)/3.0f;
 if(ab||bg){float no=ab?(m.na+m.nb)*0.5f:(m.nb+m.ng)*0.5f;
 float ne=ab?m.ng:m.na;float e=effective(no,ne,k,ab?m.g:m.a,false);return mode?fmaxf(no,e):fminf(no,e);}
 V l=local(m,k);float a=l.x*l.x,b=l.y*l.y,g=l.z*l.z;
 float b1=1.0f/(m.na*m.na),b2=1.0f/(m.nb*m.nb),b3=1.0f/(m.ng*m.ng);
 float bigb=fmaf(a,b2+b3,fmaf(b,b1+b3,g*(b1+b2)));
 float bigc=fmaf(a,b2*b3,fmaf(b,b1*b3,g*b1*b2));
 float disc=sqrtf(fmaxf(fmaf(bigb,bigb,-4.0f*bigc),0.0f));
 float x=mode?0.5f*(bigb-disc):0.5f*(bigb+disc);
 return 1.0f/sqrtf(fmaxf(x,1e-12f));
}
__device__ V biaxial_e(Mat m,V k,int mode){
 k=norm(k);V l=local(m,k);
 float b0=1.0f/(m.na*m.na),b1=1.0f/(m.nb*m.nb),b2=1.0f/(m.ng*m.ng);
 float n=wave_index(m,k,mode),x=1.0f/(n*n);
 float a=l.x*l.x,b=l.y*l.y,c=l.z*l.z;
 float bigb=fmaf(a,b1+b2,fmaf(b,b0+b2,c*(b0+b1)));
 float xd=b0-b1,yd=b2-b0,ac=a+c,ab=a+b;
 float ds=fmaf(ac*ac,xd*xd,fmaf(ab*ab,yd*yd,2.0f*fmaf(b,-c,a)*xd*yd));
 float d=sqrtf(fmaxf(ds,0.0f)),lo=0.5f*(bigb-d),hi=0.5f*(bigb+d);
 x=fabsf(x-lo)<=fabsf(x-hi)?lo:hi;
 float s=fmaf(b0,a,fmaf(b1,b,b2*c));
 float m00=fmaf(a,fmaf(2.0f,-b0,s),b0-x),m11=fmaf(b,fmaf(2.0f,-b1,s),b1-x),m22=fmaf(c,fmaf(2.0f,-b2,s),b2-x);
 float m01=l.x*l.y*(s-b0-b1),m02=l.x*l.z*(s-b0-b2),m12=l.y*l.z*(s-b1-b2);
 V r0=v(m00,m01,m02),r1=v(m01,m11,m12),r2=v(m02,m12,m22);
 V c01=crossf(r0,r1),c02=crossf(r0,r2),c12=crossf(r1,r2);
 V dv=world(m,c01+c02*(dotf(c01,c02)<0?-1.0f:1.0f)+c12*(dotf(c01,c12)<0?-1.0f:1.0f));
 dv=len2(dv)>1e-12f?norm(canonical(dv)):basis(k);
 V dl=local(m,dv);return norm(world(m,v(dl.x*b0,dl.y*b1,dl.z*b2)));
}
__device__ V travel(Mat m,V k,int mode){
 if(m.kind==0||(m.kind==1&&mode==0))return k;
 if(m.kind==2){V e=biaxial_e(m,k,mode);V q=k-e*dot(e,k);return len2(q)>1e-10f?norm(q):k;}
 float co=clamp(dot(k,m.c),-1,1),ab=fabsf(co),si=sqrtf(fmaxf((1-ab)*(1+ab),0));
 if(fabsf(m.no-m.ne)<1e-5f)return k;
 float no2=m.no*m.no,ne2=m.ne*m.ne;
 float delta=atanf(((no2-ne2)*si*ab)/fmaxf(fmaf(ne2*ab,ab,no2*si*si),1e-6f));
 if(fabsf(delta)<1e-5f)return k;V cp=norm(m.c-k*co);if(len2(cp)<1e-6f)return k;
 return norm(k*cosf(delta)-cp*((co>=0?1.0f:-1.0f)*sinf(delta)));
}
__device__ V efield(Mat m,V k,int mode){
 if(m.kind==2)return biaxial_e(m,k,mode);
 V kc=cross(k,m.c);V o=len2(kc)>1e-8f?norm(kc):basis(norm(k));
 if(mode==0)return o;
 if(len2(kc)<1e-8f)return norm(cross(k,o));
 V s=travel(m,k,1);return norm(cross(kc,s));
}
__device__ V weights(Mat m,V k,int mode,float index){
 V e=efield(m,k,mode);V p=v(dot(m.pa,e),dot(m.pb,e),dot(m.pc,e));return v(p.x*p.x,p.y*p.y,p.z*p.z);
}
__device__ float fresnel(float n1,float n2,float ci,float ct){
 float ds=fmaf(n2,ct,n1*ci),dp=fmaf(n2,ci,n1*ct);
 if(fabsf(ds)<1e-6f||fabsf(dp)<1e-6f)return 0;
 float rs=fmaf(n2,-ct,n1*ci)/ds,rp=fmaf(n2,-ci,n1*ct)/dp;
 return clamp(1.0f-(rs*rs+rp*rp)*0.5f,0,1);
}
__device__ V refract(V incoming,V n,float ci,float index){
 float si=(1.0f/(index*index))*fmaf(ci,-ci,1.0f),ct=sqrtf(1.0f-si),eta=1.0f/index;
 return norm(incoming*eta+n*fmaf(eta,ci,-ct));
}
