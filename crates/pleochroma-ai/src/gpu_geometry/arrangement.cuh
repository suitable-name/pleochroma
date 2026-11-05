// A one-way certificate: every production arrangement point is the rounded
// result of one real-surface triple. Scan a superset (without feasibility,
// clustering or pair pruning), enclosing the exact CPU SIMD operation order.
// Each junction needs at least one possible matching triple. An uncertain solve
// invalidates the certificate globally; it never becomes a refusal.
__device__ unsigned long long choose2(unsigned long long n) { return n*(n-1)/2; }
__device__ unsigned long long choose3(unsigned long long n) { return n*(n-1)*(n-2)/6; }
__device__ unsigned long long before_a(int n,int a) { return choose3(n)-choose3(n-a); }
__device__ unsigned long long before_b(int n,int a,int b) {
    return choose2(n-a-1)-choose2(n-b);
}
__device__ I component(I a,I b,I c,I inv,I x,I y,I z) {
    return ia(ia(im(im(a,inv),x),im(im(b,inv),y)),im(im(c,inv),z));
}
extern "C" __global__ void arrangement_proof(const double *normalized,const double *planes,
    const int *meta,const int *supports,const double *bounds,unsigned int *flags,
    int ns,int nj,double scale,unsigned long long start,unsigned int count) {
    unsigned int lane=blockIdx.x*blockDim.x+threadIdx.x;
    if(lane>=count) return;
    unsigned long long rank=start+lane;
    int lo=0,hi=ns-2;
    while(lo+1<hi) { int mid=(lo+hi)/2; if(before_a(ns,mid)<=rank) lo=mid; else hi=mid; }
    int a=lo; rank-=before_a(ns,a);
    lo=a+1; hi=ns-1;
    while(lo+1<hi) { int mid=(lo+hi)/2; if(before_b(ns,a,mid)<=rank) lo=mid; else hi=mid; }
    int b=lo, c=b+1+(int)(rank-before_b(ns,a,b));
    // CPU transposes the normal matrix before its cofactor inverse.
    IV u={exact(normalized[4*a]),exact(normalized[4*b]),exact(normalized[4*c])};
    IV v={exact(normalized[4*a+1]),exact(normalized[4*b+1]),exact(normalized[4*c+1])};
    IV w={exact(normalized[4*a+2]),exact(normalized[4*b+2]),exact(normalized[4*c+2])};
    IV t0=ivcross(v,w),t1=ivcross(w,u),t2=ivcross(u,v);
    I det=ivdot(u,t0);
    if(!bounded(det)) { atomicExch(flags+nj,1); return; }
    if(maxabs(det)<1e-6) return;
    if(det.lo<=0.&&det.hi>=0.) { atomicExch(flags+nj,1); return; }
    I inv=idiv(exact(1.),det);
    I x=exact(normalized[4*a+3]),y=exact(normalized[4*b+3]),z=exact(normalized[4*c+3]);
    IV point={im(component(t0.x,t0.y,t0.z,inv,x,y,z),exact(scale)),
              im(component(t1.x,t1.y,t1.z,inv,x,y,z),exact(scale)),
              im(component(t2.x,t2.y,t2.z,inv,x,y,z),exact(scale))};
    if(!bounded3(point)) { atomicExch(flags+nj,1); return; }
    // Flags only ever go from 0 to 1, so a plain (volatile) read is enough to skip
    // an already witnessed junction; a stale 0 only repeats work. An atomic read here
    // put every lane's every junction through the same few L2 atomic units.
    const volatile unsigned int *witnessed=flags;
    for(int j=0;j<nj;j++) {
        if(witnessed[j]) continue;
        const int *m=meta+6*j;
        bool possible=true;
        for(int k=m[3];k<m[4];k++) {
            int s=supports[k];
            I residual=isub(ivdot(ivnormal(planes,s),point),exact(planes[4*s+3]));
            if(!bounded(residual)) { atomicExch(flags+nj,1); return; }
            if(residual.lo>bounds[j]||residual.hi< -bounds[j]) { possible=false; break; }
        }
        if(possible) atomicExch(flags+j,1);
    }
}
