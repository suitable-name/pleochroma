// Mirror of metrics/frosted.rs and surface/{ground,ggx,micro}.rs.
// GroundSample.weight is deliberately ignored by the metrics oracle too.
__device__ unsigned hash32(unsigned x){x*=0x85ebca6bu;x^=x>>13;x*=0xc2b2ae35u;x^=x>>16;return x;}
__device__ unsigned path_seed(V point,V dir){
 float p[6]={point.x,point.y,point.z,dir.x,dir.y,dir.z};unsigned s=0x9e3779b9u;
 for(int i=0;i<6;i++)s=hash32(s^__float_as_uint(p[i]));return s;
}
__device__ float draw(unsigned seed,unsigned bounce,unsigned salt){return (float)hash32(seed^hash32(bounce^salt))/4294967295.0f;}
struct Frost {V dir;bool crossed;};
__device__ Frost frosted(float n1,float n2,float ci,float sin2,V normal,V incident,unsigned seed,unsigned bounce){
 V t=basis(normal),b=cross(normal,t),neg=-incident;
 V i=v(dot(neg,t),dot(neg,b),fmaxf(dot(neg,normal),1e-4f));i=norm(i);
 float eta=n1/n2,st=eta*eta*fmaf(ci,-ci,1.0f),r=1.0f;
 if(st<1.0f){float ct=sqrtf(1-st),rs=fmaf(n1,ci,-n2*ct)/fmaf(n1,ci,n2*ct),rp=fmaf(n2,ci,-n1*ct)/fmaf(n2,ci,n1*ct);r=0.5f*fmaf(rp,rp,rs*rs);}
 r=sin2>1.0f?0.98f:clamp(r,0.02f,0.98f);
 bool reflect=draw(seed,bounce,0x9e3779b1u)<r;
 float u1=draw(seed,bounce,0x27d4eb2fu),u2=draw(seed,bounce,0x165667b1u);
 V vh=norm(v(0.45f*i.x,0.45f*i.y,i.z));float l=fmaf(vh.x,vh.x,vh.y*vh.y);
 V t1=l>0?v(-vh.y,vh.x,0)/sqrtf(l):v(1,0,0),t2=cross(vh,t1);
 float rr=sqrtf(u1),phi=6.2831853071795864769f*u2,p1=rr*cosf(phi),p2disk=rr*sinf(phi),s=0.5f*(1+vh.z);
 float p2=fmaf(1-s,sqrtf(fmaxf(1-p1*p1,0)),s*p2disk);
 V nh=t1*p1+t2*p2+vh*sqrtf(fmaxf(1-p1*p1-p2*p2,0));V m=norm(v(0.45f*nh.x,0.45f*nh.y,fmaxf(nh.z,0)));
 float c=dot(i,m);V o;bool valid=true;
 if(reflect)o=m*(2*c)-i;
 else{float k=1-eta*eta*fmaf(c,-c,1.0f);valid=k>=0;o=m*fmaf(eta,c,-sqrtf(fmaxf(k,0)))-i*eta;}
 valid=valid&&c>0&&(reflect?o.z>0:o.z<0);
 if(!valid){o=v(-i.x,-i.y,i.z);return {norm(t*o.x+b*o.y+normal*o.z),false};}
 o=norm(o);return {norm(t*o.x+b*o.y+normal*o.z),!reflect};
}
