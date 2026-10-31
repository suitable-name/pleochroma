// Conservative necessary conditions for the original production meet gate.
// Every arithmetic interval endpoint uses CUDA IEEE directed rounding. A finite
// accepted point q must be within the original displacement box and each support
// slab. Invert the three basis slabs as intervals, intersect the displacement
// box, then test all support slabs/containment halfspaces against that enclosure.
// No overlap proves impossibility; overlap NEVER certifies preservation.
//
// The CPU gate evaluates dot/subtract, distance/divide and hypot in f64. Its
// rounded acceptance implies real constraints enlarged by gamma_16 times the
// absolute operands, plus subnormal allowance. 128*DBL_EPSILON is greater than
// gamma_16=16u/(1-16u), u=DBL_EPSILON/2; the extra +1 covers underflow. This also
// covers fused/unfused CPU dot evaluation and the final comparison subtraction.
// Non-finite interval endpoints or a determinant containing zero mean UNKNOWN.
// No such uncertainty is converted to a rejection.
// NVRTC provides device math built-ins without host C/C++ headers.
struct I { double lo,hi; };
struct IV { I x,y,z; };
struct Box { IV p; int possible; };
__device__ I exact(double a) {return {a,a};}
__device__ I ia(I a,I b) {return {__dadd_rd(a.lo,b.lo),__dadd_ru(a.hi,b.hi)};}
__device__ I isub(I a,I b) {return {__dsub_rd(a.lo,b.hi),__dsub_ru(a.hi,b.lo)};}
__device__ I im(I a,I b) {
    double lo=fmin(fmin(__dmul_rd(a.lo,b.lo),__dmul_rd(a.lo,b.hi)),fmin(__dmul_rd(a.hi,b.lo),__dmul_rd(a.hi,b.hi)));
    double hi=fmax(fmax(__dmul_ru(a.lo,b.lo),__dmul_ru(a.lo,b.hi)),fmax(__dmul_ru(a.hi,b.lo),__dmul_ru(a.hi,b.hi)));
    return {lo,hi};
}
__device__ I idiv(I a,I b) {return im(a,{__ddiv_rd(1.,b.hi),__ddiv_ru(1.,b.lo)});}
__device__ IV ivadd(IV a,IV b) {return {ia(a.x,b.x),ia(a.y,b.y),ia(a.z,b.z)};}
__device__ IV ivmul(IV a,I b) {return {im(a.x,b),im(a.y,b),im(a.z,b)};}
__device__ IV ivdiv(IV a,I b) {return {idiv(a.x,b),idiv(a.y,b),idiv(a.z,b)};}
__device__ IV ivcross(IV a,IV b) {return {isub(im(a.y,b.z),im(a.z,b.y)),isub(im(a.z,b.x),im(a.x,b.z)),isub(im(a.x,b.y),im(a.y,b.x))};}
__device__ I ivdot(IV a,IV b) {return ia(ia(im(a.x,b.x),im(a.y,b.y)),im(a.z,b.z));}
__device__ IV ivnormal(const double *p,int s) {return {exact(p[s*4]),exact(p[s*4+1]),exact(p[s*4+2])};}
__device__ double maxabs(I a) {return fmax(fabs(a.lo),fabs(a.hi));}
__device__ bool bounded(I a) {return isfinite(a.lo)&&isfinite(a.hi);}
__device__ bool bounded3(IV a) {return bounded(a.x)&&bounded(a.y)&&bounded(a.z);}
__device__ double round_margin(double magnitude) {
    // Exact binary64 epsilon (2^-52), independent of the host float.h macro.
    const double epsilon=2.220446049250313080847263336181640625e-16;
    return __dmul_ru(128.*epsilon,__dadd_ru(magnitude,1.));
}
__device__ I around(double origin,double radius) {
    double slack=round_margin(__dadd_ru(fabs(origin),fabs(radius)));
    double r=__dadd_ru(radius,slack);
    return {__dsub_rd(origin,r),__dadd_ru(origin,r)};
}
__device__ double plane_error(const double *p,int s,IV box) {
    double magnitude=__dadd_ru(__dmul_ru(fabs(p[s*4]),maxabs(box.x)),__dmul_ru(fabs(p[s*4+1]),maxabs(box.y)));
    magnitude=__dadd_ru(magnitude,__dmul_ru(fabs(p[s*4+2]),maxabs(box.z)));
    magnitude=__dadd_ru(magnitude,fabs(p[s*4+3]));
    return round_margin(magnitude);
}
__device__ I slab_rhs(const double *p,int s,IV box,double bound) {
    double radius=__dadd_ru(bound,plane_error(p,s,box));
    return {__dsub_rd(p[s*4+3],radius),__dadd_ru(p[s*4+3],radius)};
}
__device__ I intersect(I a,I b) {return {fmax(a.lo,b.lo),fmin(a.hi,b.hi)};}
__device__ Box meet_box(const double *p,const double *original,const int *m,const int *supports,
    double bound,double displacement,double tolerance,int fixed_outline,int ns) {
    IV initial={around(original[0],fixed_outline?tolerance:displacement),around(original[1],displacement),around(original[2],fixed_outline?tolerance:displacement)};
    if(!bounded3(initial)) return {initial,1};
    IV a=ivnormal(p,m[0]),b=ivnormal(p,m[1]),c=ivnormal(p,m[2]);
    IV bc=ivcross(b,c),ca=ivcross(c,a),ab=ivcross(a,b);
    I det=ivdot(a,bc);
    if(!bounded(det)||(det.lo<=0.&&det.hi>=0.)||fmin(fabs(det.lo),fabs(det.hi))<=1e-8) return {initial,1};
    IV box=ivadd(ivadd(ivmul(ivdiv(bc,det),slab_rhs(p,m[0],initial,bound)),ivmul(ivdiv(ca,det),slab_rhs(p,m[1],initial,bound))),ivmul(ivdiv(ab,det),slab_rhs(p,m[2],initial,bound)));
    if(!bounded3(box)) return {initial,1};
    box={intersect(initial.x,box.x),intersect(initial.y,box.y),intersect(initial.z,box.z)};
    if(box.x.lo>box.x.hi||box.y.lo>box.y.hi||box.z.lo>box.z.hi) return {initial,0};
    int member=m[3];
    for(int s=0;s<ns;s++) {
        while(member<m[4]&&supports[member]<s) member++;
        bool support=member<m[4]&&supports[member]==s;
        I lhs=ivdot(ivnormal(p,s),box),rhs=slab_rhs(p,s,initial,bound);
        if(!bounded(lhs)||!bounded(rhs)) continue;
        if(lhs.lo>rhs.hi || (support&&lhs.hi<rhs.lo)) return {box,0};
    }
    return {box,1};
}
__device__ I isquare(I a) {
    if(a.lo<=0.&&a.hi>=0.) return {0.,__dmul_ru(maxabs(a),maxabs(a))};
    double near=fmin(fabs(a.lo),fabs(a.hi)),far=maxabs(a);
    return {__dmul_rd(near,near),__dmul_ru(far,far)};
}
__device__ I radius_range(IV box) {
    I squared=ia(isquare(box.x),isquare(box.z));
    return {__dsqrt_rd(fmax(0.,squared.lo)),__dsqrt_ru(fmax(0.,squared.hi))};
}
