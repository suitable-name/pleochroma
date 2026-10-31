// MIT source provenance: pleochroma/color/metrics/lighting.rs and
// optics/raytracer/environment/rig.rs. Observer=ZERO, exposure=1, tray=0 exactly
// match the analytical metric's radiance probe (not a beauty estimator).
__device__ float step(float a,float b,float x){float t=clamp((x-a)/(b-a),0,1);return t*t*fmaf(-2.0f,t,3.0f);}
__device__ float ipow(float x,unsigned exponent){float result=1.0f;while(exponent){if(exponent&1)result*=x;x*=x;exponent>>=1;}return result;}
__device__ float metric_radiance(V dir,const float* l){
 V d=norm(dir);int model=(int)l[0];float spot=l[1],power=l[2];V key=loadv(l+12),fill=loadv(l+15);
 if(model==0){
  float r=l[3]!=0?fmaxf(fmaf(0.012f,fmaf(d.y,0.5f,0.5f),0.015f),0.005f)*power:0.0f;
  float kd=fmaxf(dot(d,key),0);if(kd>0)r=fmaf(ipow(kd,28)*12.0f*spot,power,r);
  float fd=fmaxf(dot(d,fill),0);if(fd>0)r=fmaf(ipow(fd,18)*4.5f,power,r);
  for(int i=0;i<16;i++){float rd=fmaxf(dot(d,loadv(l+18+3*i)),0);if(rd>0.96f){float spark=(rd-0.96f)/0.04f;r=fmaf(ipow(spark,6)*22.0f*spot,power,r);}}
  return r;
 }
 float horizon=step(-0.05f,0.05f,d.y);
 if(model==1)return horizon*power;
 if(model==2){
  float walls=fmaf(0.08f,fmaxf(d.y,0),0.14f)*l[4];
  if(l[8]>0)walls=fmaf(0.18f*l[4],l[8],walls*(1-l[8]));
  float card=0;for(int i=4;i<=12;i+=4)card=fmaxf(card,step(0.898794f,0.9612617f,dot(d,loadv(l+18+3*i))));
  walls*=fmaf(card*l[5],-0.9f,1.0f);
  float k=step(0.7660444f,0.9396926f,dot(d,key))*(1.4f*spot);
  float spark=step(0.9961947f,0.9993908f,dot(d,fill))*(5.0f*spot)*l[6];
  return (((walls+k)+spark)*horizon+l[7]*(1-horizon))*power;
 }
 // ASET is mapped to the ISO grading illumination in host preparation.
 if(model==3||model==5){
  float sun=dot(d,key),sky=fmaf(0.08f,1-fmaxf(d.y,0),0.10f),glow=fmaxf(sun,0);
  float g2=glow*glow,g4=g2*g2,aureole=(g4*g4)*0.30f;
  float r=(sky+aureole)*horizon+0.04f*(1-horizon);
  if(model==5)r+=sun>=0.9999889f?l[11]:0.0f;
  return r*power;
 }
 return -1.0f;
}
