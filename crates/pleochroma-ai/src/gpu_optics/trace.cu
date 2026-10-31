// Native CUDA analytical d-line fan, source: color/metrics/{evaluate,classify,ray_trace,birefringent}.rs.
// Each thread is one pose/cell/aperture/eigenmode, not a beauty/pathtrace estimator.
// Record: fate, transmittance, path, principal[3], exit[3], facet, cosine, bounce.
// fate 0=miss,1=blocked,2=windowed,3=absorbed,4=upward (host exact lighting gate).
template<bool HAS_TOOLS> __device__ __forceinline__ void trace_one(const float* planes,int np,const float* tools,int nt,
 const unsigned* finishes,int nf,const float* params,const float* poses,int pose_count,
 const float* cells,int cell_count,float* output,unsigned job){
 Mat m=material(params);int modes=m.kind==0?1:2;
 unsigned perpose=cell_count*5*modes;if(job>=(unsigned)pose_count*perpose)return;
 int pose=job/perpose,localjob=job%perpose,mode=localjob%modes,sample=(localjob/modes)%5,cell=localjob/(modes*5);
 float* out=output+12*job;for(int j=0;j<12;j++)out[j]=0;
 const float* pp=poses+pose*12;m.principal=m.principal&&pp[11]!=0.0f;V forward=loadv(pp),right=loadv(pp+3),up=loadv(pp+6);
 float u=cells[2*cell],vv=cells[2*cell+1],lateral=pp[10];
 V origin=-forward*pp[9]+right*(u*lateral)+up*(vv*lateral);
 float dx=sample==1?0.08f:sample==2?-0.08f:0,dz=sample==3?0.08f:sample==4?-0.08f:0;
 V incoming=norm(forward+right*dx+up*dz);
 Hit entry=intersect<HAS_TOOLS>(planes,np,tools,nt,origin,incoming);if(!entry.found)return;
 out[0]=3;V point=origin+incoming*entry.t;float ci=clamp(dot(-incoming,entry.normal),0,1);
 V k=incoming;float index=m.kind==2?m.nb:m.no,et=1;unsigned seed=nf>0?path_seed(point,incoming):0;
 bool anisotropic=m.kind!=0,entry_frost=entry.facet<nf&&finishes[entry.facet]!=0;
 if(entry_frost){
  Frost event=frosted(1,index,ci,(1/(index*index))*fmaf(ci,-ci,1.0f),entry.normal,incoming,seed,0);
  if(!event.crossed)return;k=event.dir;anisotropic=false;
 }else{
  int steps=m.kind==2?2:m.kind==1&&mode==1?3:1;
  for(int j=0;j<steps;j++){k=refract(incoming,entry.normal,ci,index);if(anisotropic)index=wave_index(m,k,mode);}
  if(m.kind==2&&dot(k,entry.normal)>=0){out[0]=1;return;}
  float ct=anisotropic&&(m.kind==2||mode==1)?clamp(-dot(k,entry.normal),0,1):sqrtf(1-(1/(index*index))*fmaf(ci,-ci,1.0f));
  et=fresnel(1,index,ci,ct);
 }
 V raydir=anisotropic?travel(m,k,mode):k;point=point+raydir*1e-4f;
 float path=0;V principal=v(0,0,0);
 for(int bounce=0;bounce<10;bounce++){
  if(anisotropic)index=wave_index(m,k,mode);
  Hit hit=intersect<HAS_TOOLS>(planes,np,tools,nt,point,raydir);if(!hit.found)return;
  path+=hit.t;
  if(m.principal){V w;
   if(anisotropic)w=weights(m,k,mode,index);
   else {Mat pm=m;pm.kind=params[28];pm.no=index;pm.ne=index+params[29];V w0=weights(pm,k,0,index),w1=weights(pm,k,1,index);w=(w0+w1)*0.5f;}
   principal=v(fmaf(hit.t,w.x,principal.x),fmaf(hit.t,w.y,principal.y),fmaf(hit.t,w.z,principal.z));}
  V next=point+raydir*hit.t;float co=clamp(dot(k,hit.normal),0,1),si=sqrtf(fmaxf(fmaf(co,-co,1.0f),0));
  V outdir;float ct=0,xt=0;bool escaping=false;
  if(hit.facet<nf&&finishes[hit.facet]!=0){
   Frost event=frosted(index,1,co,(index*index)*fmaf(co,-co,1.0f),-hit.normal,k,seed,bounce+1);
   if(event.crossed){outdir=event.dir;ct=dot(outdir,hit.normal);xt=1;escaping=true;}
   else {k=event.dir;raydir=anisotropic?travel(m,k,mode):k;point=next+raydir*1e-4f;continue;}
  }else if(si<1/index){float st=(index*index)*fmaf(co,-co,1.0f);if(st<=1){ct=sqrtf(1-st);outdir=norm(k*index+hit.normal*fmaf(index,-co,ct));xt=fresnel(index,1,co,ct);escaping=true;}}
  if(escaping){
   if(hit.normal.y<-0.05f){out[0]=2;return;}
   if(outdir.y<=0.05f)return;
   out[0]=4;out[1]=et*xt;out[2]=path;out[3]=principal.x;out[4]=principal.y;out[5]=principal.z;
   out[6]=outdir.x;out[7]=outdir.y;out[8]=outdir.z;out[9]=hit.facet;out[10]=ct;out[11]=bounce;return;
  }
  k=norm(k-hit.normal*(2*co));raydir=anisotropic?travel(m,k,mode):k;point=next+raydir*1e-4f;
 }
}
template<bool HAS_TOOLS> __device__ __forceinline__ void trace_pose_tile(const float* planes,int np,const float* tools,int nt,
 const unsigned* finishes,int nf,const float* params,const float* poses,int pose_count,
 const float* cells,int cell_count,float* output){
 trace_one<HAS_TOOLS>(planes,np,tools,nt,finishes,nf,params,poses,pose_count,cells,cell_count,output,blockIdx.x*blockDim.x+threadIdx.x);
}
#define POSE_KERNEL(NAME,TOOLS) extern "C" __global__ void NAME(const float* planes,int np,const float* tools,int nt, const unsigned* finishes,int nf,const float* params,const float* poses,int pose_count,const float* cells,int cell_count,float* output){trace_pose_tile<TOOLS>(planes,np,tools,nt,finishes,nf,params,poses,pose_count,cells,cell_count,output);}
POSE_KERNEL(optical_trace_planar,false)
POSE_KERNEL(optical_trace_csg,true)
// Planar and CSG candidates occupy separate contiguous ray ranges. Planar kernels
// compile without the CSG boundary arrays (258 floats and 258 keys per thread).
template<bool HAS_TOOLS> __device__ __forceinline__ void trace_candidate_batch(const float* planes,const float* tools,
 const unsigned* finishes,const float* params,const float* poses,const unsigned* descriptors,
 unsigned candidate_count,unsigned begin,unsigned count,const float* cells,int cell_count,float* output){
 unsigned local=blockIdx.x*blockDim.x+threadIdx.x;if(local>=count)return;unsigned job=begin+local;
 unsigned lo=0,hi=candidate_count;
 while(lo+1<hi){unsigned mid=(lo+hi)/2;if(descriptors[12*mid]<=job)lo=mid;else hi=mid;}
 const unsigned* d=descriptors+12*lo;unsigned localjob=job-d[0];
 trace_one<HAS_TOOLS>(planes+d[2],d[3],tools+d[4],d[5],finishes+d[6],d[7],params+d[8],poses+d[9],1,cells,cell_count,output+12*d[0],localjob);
}
#define BATCH_KERNEL(NAME,TOOLS) extern "C" __global__ void NAME(const float* planes,const float* tools,const unsigned* finishes,const float* params,const float* poses,const unsigned* descriptors,unsigned candidate_count,unsigned begin,unsigned count,const float* cells,int cell_count,float* output){trace_candidate_batch<TOOLS>(planes,tools,finishes,params,poses,descriptors,candidate_count,begin,count,cells,cell_count,output);}
BATCH_KERNEL(optical_trace_batch_planar,false)
BATCH_KERNEL(optical_trace_batch_csg,true)
