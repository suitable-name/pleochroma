// MIT, Copyright (c) 2026 Niklas Gussek. Full license: LICENSE-MIT.
// Arithmetic adapted from pleochroma simd/triple_solve.rs and
// geometry/stone_metrics/vertices.rs. Never compile with contraction/fast math.
// NVRTC provides device math built-ins without host C/C++ headers.
typedef unsigned long long Index;
struct V3 { double x, y, z; };
__device__ V3 add(V3 a,V3 b) {return {a.x+b.x,a.y+b.y,a.z+b.z};}
__device__ V3 mul(V3 a,double s) {return {a.x*s,a.y*s,a.z*s};}
__device__ V3 divide(V3 a,double s) {return {a.x/s,a.y/s,a.z/s};}
__device__ V3 cross(V3 a,V3 b) {
    return {a.y*b.z-b.y*a.z,a.z*b.x-b.z*a.x,a.x*b.y-b.x*a.y};
}
__device__ double dot(V3 a,V3 b) {return (a.x*b.x+a.y*b.y)+a.z*b.z;}
__device__ V3 normal(const double* p,int i) {return {p[4*i],p[4*i+1],p[4*i+2]};}
__device__ double maxabs(V3 a) {return fmax(fmax(fabs(a.x),fabs(a.y)),fabs(a.z));}
__device__ Index choose3(Index p) {return p<3?0:p*(p-1)*(p-2)/6;}
__device__ Index choose2(Index p) {return p<2?0:p*(p-1)/2;}
extern "C" __global__ void prune_pairs(const double* planes,int p,unsigned* active) {
    unsigned idx=blockIdx.x*blockDim.x+threadIdx.x;
    if(idx>=(unsigned)p*p)return;
    int a=idx/p,b=idx%p;active[idx]=0;if(b<=a)return;
    V3 na=normal(planes,a), nb=normal(planes,b), dir=cross(na,nb);
    double sine=sqrt(dot(dir,dir));
    if(sine<1e-3) {active[idx]=p-b-1;return;}
    double ma=planes[4*a+3],mb=planes[4*b+3];
    V3 origin=divide(add(mul(cross(nb,dir),ma),mul(cross(dir,na),mb)),sine*sine);
    V3 unit=divide(dir,sine);
    double margin=1e-5+2.0*1e-11/1e-6/sine;
    // Exact IEEE-754 infinity; INFINITY would require a host math.h macro.
    const double infinity=__longlong_as_double(0x7ff0000000000000LL);
    double lo=-infinity,hi=infinity;
    for(int c=0;c<p;c++) {
        if(c==a||c==b)continue;
        V3 nc=normal(planes,c);
        double slope=dot(nc,unit),room=planes[4*c+3]+margin-dot(nc,origin);
        if(slope>0.0)hi=fmin(hi,room/slope);
        else if(slope<0.0)lo=fmax(lo,room/slope);
        else if(room<0.0)return;
        if(lo>hi)return;
    }
    active[idx]=p-b-1;
}
// Prefix surviving pair lengths in place. Zero-weight pairs disappear from
// the logical triple sequence while their original lexicographic order stays.
extern "C" __global__ void pair_scan(unsigned* weights,unsigned n,unsigned* sums) {
    __shared__ unsigned prefix[256];
    unsigned idx=blockIdx.x*256+threadIdx.x,lane=threadIdx.x;
    prefix[lane]=idx<n?weights[idx]:0;__syncthreads();
    for(unsigned offset=1;offset<256;offset*=2) {
        unsigned v=lane>=offset?prefix[lane-offset]:0;__syncthreads();
        prefix[lane]+=v;__syncthreads();
    }
    if(idx<n)weights[idx]=prefix[lane];
    if(lane==255)sums[blockIdx.x]=prefix[lane];
}
extern "C" __global__ void pair_offsets(const unsigned* sums,unsigned n,Index* offsets) {
    if(blockIdx.x||threadIdx.x)return;Index sum=0;
    for(unsigned i=0;i<n;i++){offsets[i]=sum;sum+=sums[i];}offsets[n]=sum;
}
__device__ void active_triple(Index index,const unsigned* prefixes,const Index* offsets,
    unsigned blocks,int p,int& a,int& b,int& c) {
    unsigned lo=0,hi=blocks;
    while(lo+1<hi){unsigned mid=(lo+hi)/2;if(offsets[mid]<=index)lo=mid;else hi=mid;}
    unsigned block=lo,local=(unsigned)(index-offsets[block]);
    unsigned remaining=(unsigned)(p*p)-block*256;lo=0;hi=remaining<256?remaining:256;
    while(lo<hi){unsigned mid=(lo+hi)/2;if(prefixes[block*256+mid]<=local)lo=mid+1;else hi=mid;}
    unsigned pair=block*256+lo,previous=lo?prefixes[pair-1]:0;
    a=pair/p;b=pair%p;c=b+1+local-previous;
}
__device__ Index original_rank(int a,int b,int c,int p) {
    return choose3(p)-choose3(p-a)+choose2(p-a-1)-choose2(p-b)+(c-b-1);
}
__device__ bool solve(const double* p,int a,int b,int c,V3& v) {
    V3 na=normal(p,a), nb=normal(p,b), nc=normal(p,c);
    // Axes of DMat3::from_cols(na,nb,nc).transpose().
    V3 u={na.x,nb.x,nc.x},w={na.z,nb.z,nc.z},t={na.y,nb.y,nc.y};
    V3 t0=cross(t,w);double det=dot(u,t0);
    if(isnan(det)||fabs(det)<1e-6)return false;
    V3 t1=cross(w,u),t2=cross(u,t);double inv=1.0/det;
    double ma=p[4*a+3],mb=p[4*b+3],mc=p[4*c+3];
    v={((t0.x*inv)*ma+(t0.y*inv)*mb)+(t0.z*inv)*mc,
       ((t1.x*inv)*ma+(t1.y*inv)*mb)+(t1.z*inv)*mc,
       ((t2.x*inv)*ma+(t2.y*inv)*mb)+(t2.z*inv)*mc};
    return isfinite(v.x)&&isfinite(v.y)&&isfinite(v.z);
}
// One tile is a contiguous lexicographic range of surviving-pair triples. Each block first stores
// candidates in lane order. A second pass prefixes blocks, a third compacts.
extern "C" __global__ void vertices(const double* planes,const unsigned* active,
    const Index* pair_offsets,unsigned pair_blocks,int p,Index start,unsigned count,double* candidates,unsigned* rank,
    unsigned* counts,unsigned* escape,unsigned* escaped) {
    __shared__ unsigned prefix[256];
    unsigned idx=blockIdx.x*blockDim.x+threadIdx.x,lane=threadIdx.x,valid=0;
    V3 v={0.0,0.0,0.0};
    if(idx<count) {
        int a,b,c;active_triple(start+idx,active,pair_offsets,pair_blocks,p,a,b,c);
        if(solve(planes,a,b,c,v)&&maxabs(v)<=65.0) {
            bool feasible=true;
            for(int j=0;j<p;j++) {
                if(dot(normal(planes,j),v)-planes[4*j+3]>1e-5) {feasible=false;break;}
            }
            if(feasible) {
                if(maxabs(v)>63.0) {
                    atomicExch(escaped,1u);
                    if(a<p-6)atomicExch(escape+a,1u);
                    if(b<p-6)atomicExch(escape+b,1u);
                    if(c<p-6)atomicExch(escape+c,1u);
                } else valid=1;
            }
        }
    }
    prefix[lane]=valid;__syncthreads();
    for(unsigned offset=1;offset<256;offset*=2) {
        unsigned n=lane>=offset?prefix[lane-offset]:0;__syncthreads();
        prefix[lane]+=n;__syncthreads();
    }
    if(idx<count) {
        rank[idx]=valid?prefix[lane]:0;
        if(valid) {candidates[3*idx]=v.x;candidates[3*idx+1]=v.y;candidates[3*idx+2]=v.z;}
    }
    if(lane==255)counts[blockIdx.x]=prefix[lane];
}
extern "C" __global__ void prefix_blocks(unsigned* counts,unsigned blocks,const unsigned* escaped,unsigned* total) {
    if(blockIdx.x||threadIdx.x)return;
    unsigned sum=0;
    for(unsigned i=0;i<blocks;i++) {unsigned n=counts[i];counts[i]=sum;sum+=n;}
    total[0]=sum;total[1]=escaped[0];
}
extern "C" __global__ void compact(const double* candidates,const unsigned* rank,
    const unsigned* offsets,unsigned count,Index start,double* output,Index* ordinals,
    const unsigned* pair_prefix,const Index* pair_offsets,unsigned pair_blocks,int p,unsigned capture) {
    unsigned idx=blockIdx.x*blockDim.x+threadIdx.x;if(idx>=count||!rank[idx])return;
    unsigned dest=offsets[blockIdx.x]+rank[idx]-1;
    if(capture){int a,b,c;active_triple(start+idx,pair_prefix,pair_offsets,pair_blocks,p,a,b,c);
        ordinals[dest]=original_rank(a,b,c,p);}
    output[3*dest]=candidates[3*idx];output[3*dest+1]=candidates[3*idx+1];output[3*dest+2]=candidates[3*idx+2];
}
