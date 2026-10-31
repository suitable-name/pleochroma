// One thread per wavelength/path. Absorption coefficients are evaluated by the exact
// CPU band functions; these f64 expressions mirror tone::principal_transmittance and
// the occupied-bin mixture. No fast math or implicit FMA contraction is enabled.
// NVRTC provides device math built-ins without host C/C++ headers.
// MIT source: AbsorptionBand::evaluate and principal_alphas. Band sums retain order.
extern "C" __global__ void tone_band_alphas(const float* bands,const unsigned* offsets,
 unsigned candidates,unsigned wavelengths,double* alpha){
 unsigned job=blockIdx.x*blockDim.x+threadIdx.x;if(job>=candidates*wavelengths)return;
 unsigned candidate=job/wavelengths,wave=job%wavelengths;float lambda=380.0f+(float)wave;
 for(unsigned axis=0;axis<3;axis++){const unsigned* d=offsets+6*candidate+2*axis;float sum=0;
  for(unsigned i=0;i<d[1];i++){const float* b=bands+4*(d[0]+i);float t;
   if(b[3]==0)t=(lambda-b[0])/b[1];else {float delta=b[0]-lambda,nu=1.0e7f*delta/(lambda*b[0]);t=nu/b[1];}
   sum+=b[2]*expf(-0.5f*t*t);}
  alpha[(candidate*wavelengths+wave)*3+axis]=(double)fmaxf(sum,0.0f);
 }
}
extern "C" __global__ void tone_exponentials(
    const double* paths, const unsigned* meta, const double* alpha,
    unsigned rows, unsigned wavelengths, double* output)
{
    unsigned job = blockIdx.x * blockDim.x + threadIdx.x;
    unsigned count = rows * wavelengths;
    if (job >= count) return;
    unsigned wavelength = job % wavelengths;
    unsigned row = job / wavelengths;
    const double* p = paths + 5 * row;
    const unsigned* m = meta + 3 * row;
    const double* a = alpha + (m[0] * wavelengths + wavelength) * 3;
    double depth;
    if (m[1]) {
        depth = p[4] * fma(a[2], p[3], fma(a[1], p[2], a[0] * p[1]));
    } else {
        // Histogram p[1] is already multiplied by the absorption scale on the host,
        // exactly where the CPU reference rounds that multiplication.
        depth = a[m[2]] * p[1];
    }
    output[job] = exp(-depth);
}

// Resident compacted paths: group records are [begin,end,candidate,slot,channel,principal].
extern "C" __global__ void tone_resident_exponentials(const double* paths,const double* alpha,
 const unsigned* groups,unsigned group_count,unsigned row_start,unsigned rows,unsigned wavelengths,double* output){
 unsigned job=blockIdx.x*blockDim.x+threadIdx.x;if(job>=rows*wavelengths)return;
 unsigned row=row_start+job/wavelengths,wave=job%wavelengths,lo=0,hi=group_count;
 while(lo+1<hi){unsigned mid=(lo+hi)/2;if(groups[6*mid]<=row)lo=mid;else hi=mid;}
 const unsigned* g=groups+6*lo;const double* p=paths+5*(g[2]*4096+row-g[0]);const double* a=alpha+(g[2]*wavelengths+wave)*3;
 double depth=g[5]?p[4]*fma(a[2],p[3],fma(a[1],p[2],a[0]*p[1])):a[g[4]]*p[1];
 output[job]=exp(-depth);
}
extern "C" __global__ void tone_resident_reduce(const double* paths,const double* exponentials,
 const unsigned* groups,unsigned group_count,unsigned row_start,unsigned rows,unsigned wavelengths,double* sums){
 unsigned job=blockIdx.x*blockDim.x+threadIdx.x;if(job>=group_count*wavelengths)return;
 unsigned wave=job%wavelengths;const unsigned* g=groups+6*(job/wavelengths);
 unsigned begin=g[0]>row_start?g[0]:row_start,end=g[1]<row_start+rows?g[1]:row_start+rows;if(begin>=end)return;
 unsigned target=(g[2]*3+g[3])*wavelengths+wave;double sum=sums[target];
 for(unsigned row=begin;row<end;row++)sum=fma(paths[5*(g[2]*4096+row-g[0])],exponentials[(row-row_start)*wavelengths+wave],sum);
 sums[target]=sum;
}

// One thread owns the entire serial sum of one candidate/channel/wavelength. Paths are
// visited in the same order as the CPU reference, including the previous tile's accumulator.
// Neighboring wavelength threads read neighboring exponential entries for each path.
extern "C" __global__ void tone_ordered_reduce(
    const double* paths, const double* exponentials, const unsigned* groups,
    unsigned group_count, unsigned wavelengths, double* sums)
{
    unsigned job = blockIdx.x * blockDim.x + threadIdx.x;
    if (job >= group_count * wavelengths) return;
    unsigned wavelength = job % wavelengths;
    const unsigned* g = groups + 3 * (job / wavelengths);
    unsigned target = g[2] * wavelengths + wavelength;
    double sum = sums[target];
    for (unsigned row = g[0]; row < g[0] + g[1]; ++row) {
        sum = fma(paths[5 * row], exponentials[row * wavelengths + wavelength], sum);
    }
    sums[target] = sum;
}
