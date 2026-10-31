// f64 port of meet_preservation/network.rs::residuals_solved and
// network_topology.rs::support_rows. Production f32 planes arrive unchanged.
// NVRTC provides device math built-ins without host C/C++ headers.
struct V { double x,y,z; };
__device__ V crossv(V a,V b) { return {a.y*b.z-a.z*b.y,a.z*b.x-a.x*b.z,a.x*b.y-a.y*b.x}; }
__device__ double dotv(V a,V b) { return a.x*b.x+a.y*b.y+a.z*b.z; }
__device__ V normal(const double *p,int i) { return {p[4*i],p[4*i+1],p[4*i+2]}; }
extern "C" __global__ void residuals(
    const double *planes,const int *meta,const int *supports,const double *targets,
    const double *original,const double *bounds,const double *displacements,
    double *out,double *points,int count,int nj,int ns,int rows,double scale,int precheck,int fixed_outline) {
    int task=blockIdx.x*blockDim.x+threadIdx.x;
    if(task>=count*nj) return;
    int batch=task/nj,j=task%nj;
    const double *p=planes+batch*ns*4;
    const int *m=meta+j*6;
    int a=m[0],b=m[1],c=m[2],start=m[3],end=m[4],row=m[5];
    V na=normal(p,a),nb=normal(p,b),nc=normal(p,c);
    V bc=crossv(nb,nc),ca=crossv(nc,na),ab=crossv(na,nb);
    double det=dotv(na,bc);
    // Exact IEEE-754 quiet NaN; NAN would require a host math.h macro.
    const double invalid=__longlong_as_double(0x7ff8000000000000LL);
    V point={(bc.x*p[4*a+3]+ca.x*p[4*b+3]+ab.x*p[4*c+3])/det,
             (bc.y*p[4*a+3]+ca.y*p[4*b+3]+ab.y*p[4*c+3])/det,
             (bc.z*p[4*a+3]+ca.z*p[4*b+3]+ab.z*p[4*c+3])/det};
    if(fabs(det)<=1e-8) point={invalid,invalid,invalid};
    points[task*7]=sqrt(point.x*point.x+point.z*point.z);
    points[task*7+1]=point.y;
    points[task*7+2]=1.;
    if(precheck) {
        Box box=meet_box(p,original+j*3,m,supports,bounds[j],displacements[batch],scale*2e-6,fixed_outline,ns);
        I radial=radius_range(box.p);
        points[task*7+2]=box.possible;
        points[task*7+3]=radial.lo;
        points[task*7+4]=radial.hi;
        points[task*7+5]=box.p.y.lo;
        points[task*7+6]=box.p.y.hi;
    }
    for(int i=start;i<end;i++) {
        int s=supports[i];
        if(s==a||s==b||s==c) continue;
        out[batch*(rows+1)+row]=(dotv(normal(p,s),point)-p[4*s+3])/scale-targets[row];
        row++;
    }
    double outside=0.;
    // Supports are sorted and deduplicated by canonical capture. Walk that
    // immutable list once; retain ascending plane order and identical f64 math.
    int member=start;
    for(int s=0;s<ns;s++) {
        while(member<end && supports[member]<s) member++;
        if(member>=end || supports[member]!=s)
            outside=fmax(outside,(dotv(normal(p,s),point)-p[4*s+3])/scale);
    }
    out[batch*(rows+1)+row]=isfinite(point.x)&&isfinite(point.y)&&isfinite(point.z)?outside:invalid;
}
extern "C" __global__ void gauge(
    const double *points,const int *members,double *out,int count,int nj,int ng,
    int rows,double radial,double height,double scale,int precheck,int lock_gauge) {
    int batch=blockIdx.x*blockDim.x+threadIdx.x;
    if(batch>=count) return;
    const double *p=points+batch*nj*7;
    double radius=0.,sum=0.;
    for(int j=0;j<nj;j++) radius=fmax(radius,p[j*7]);
    for(int j=0;j<ng;j++) sum+=p[members[j]*7+1];
    out[batch*(rows+1)+rows-2]=(radius-radial)/scale;
    out[batch*(rows+1)+rows-1]=(sum/(ng>0?ng:1)-height)/scale;
    int possible=1,finite=1;
    I range={0.,0.},mean={0.,0.};
    if(precheck) {
        for(int j=0;j<nj;j++) {
            if(p[j*7+2]==0.) possible=0;
            I r={p[j*7+3],p[j*7+4]};
            finite=finite&&bounded(r);
            range={fmax(range.lo,r.lo),fmax(range.hi,r.hi)};
        }
        for(int j=0;j<ng;j++) mean=ia(mean,{p[members[j]*7+5],p[members[j]*7+6]});
        mean=idiv(mean,exact(ng>0?ng:1));
        // Interval sum/divide encloses the CPU's ordered rounded mean. Expand the
        // hypot and final comparison by the same conservative gamma_16 budget.
        if(lock_gauge&&finite&&bounded(mean)) {
            double rtol=__dadd_ru(scale*2e-6,round_margin(__dadd_ru(maxabs(range),fabs(radial))));
            double htol=__dadd_ru(scale*2e-6,round_margin(__dadd_ru(maxabs(mean),fabs(height))));
            if(range.lo>__dadd_ru(radial,rtol)||range.hi<__dsub_rd(radial,rtol)
                ||mean.lo>__dadd_ru(height,htol)||mean.hi<__dsub_rd(height,htol)) possible=0;
        }
    }
    out[batch*(rows+1)+rows]=possible;
}
