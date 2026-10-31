// MIT, Copyright (c) 2026 Niklas Gussek. Full license: LICENSE-MIT.
// Sutherland-Hodgman operation order from stone_metrics/concave.rs.
// Standalone NVRTC source; no host headers are needed.
__device__ bool same_vertex(const double* a,const double* b) {
    return a[0]==b[0]&&a[1]==b[1]&&a[2]==b[2];
}
__device__ bool append(double* output,int& count,int capacity,const double* v) {
    if(count&&same_vertex(output+3*(count-1),v))return true;
    if(count>=capacity)return false;
    for(int k=0;k<3;k++)output[3*count+k]=v[k];
    count++;return true;
}
__device__ double distance(const double* plane,const double* v) {
    return ((plane[0]*v[0]+plane[1]*v[1])+plane[2]*v[2])-plane[3];
}
extern "C" __global__ void tool_faces(const double* planes,const double* seeds,
    unsigned n,unsigned stride,double* scratch_a,double* scratch_b,
    unsigned* counts,unsigned* error) {
    unsigned face=blockIdx.x*blockDim.x+threadIdx.x;if(face>=n)return;
    double* src=scratch_a+face*stride*3;double* dst=scratch_b+face*stride*3;
    for(int k=0;k<12;k++)src[k]=seeds[face*12+k];
    int size=4;
    for(unsigned p=0;p<n&&size>=3;p++) {
        if(p==face)continue;
        const double* plane=planes+4*p;
        bool inside=true;for(int i=0;i<size;i++)if(!(distance(plane,src+3*i)<=0.0))inside=false;
        if(inside)continue;
        int out=0;
        for(int i=0;i<size;i++) {
            int j=(i+1)%size;const double* a=src+3*i;const double* b=src+3*j;
            double da=distance(plane,a),db=distance(plane,b);
            if(da<=0.0) {
                if(!append(dst,out,stride,a)) {atomicExch(error,1u);counts[face]=0;return;}
                if(db>0.0) {
                    double t=da/(da-db);double v[3];for(int k=0;k<3;k++)v[k]=a[k]+(b[k]-a[k])*t;
                    if(!append(dst,out,stride,v)) {atomicExch(error,1u);counts[face]=0;return;}
                }
            } else if(db<=0.0) {
                double t=da/(da-db);double v[3];for(int k=0;k<3;k++)v[k]=a[k]+(b[k]-a[k])*t;
                if(!append(dst,out,stride,v)) {atomicExch(error,1u);counts[face]=0;return;}
            }
        }
        while(out>1&&same_vertex(dst,dst+3*(out-1)))out--;
        size=out;double* tmp=src;src=dst;dst=tmp;
    }
    counts[face]=size>=3?size:0;
    // Canonical buffer regardless of how many clips changed the polygon.
    if(src!=scratch_a+face*stride*3)for(int i=0;i<size*3;i++)scratch_a[face*stride*3+i]=src[i];
}
extern "C" __global__ void face_offsets(const unsigned* counts,unsigned n,unsigned* offsets) {
    if(blockIdx.x||threadIdx.x)return;unsigned sum=0;
    for(unsigned i=0;i<n;i++) {offsets[i]=sum;sum+=counts[i];}offsets[n]=sum;
}
extern "C" __global__ void pack_faces(const double* source,const unsigned* counts,
    const unsigned* offsets,unsigned n,unsigned stride,double* output) {
    unsigned face=blockIdx.x;if(face>=n)return;
    for(unsigned v=threadIdx.x;v<counts[face];v+=blockDim.x)
        for(int k=0;k<3;k++)output[3*(offsets[face]+v)+k]=source[3*(face*stride+v)+k];
}
