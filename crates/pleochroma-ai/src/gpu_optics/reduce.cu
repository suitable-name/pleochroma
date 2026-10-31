// MIT source provenance: analytical metrics/{classify,evaluate,doubling,accumulate}.
// Integer mode counts are exact: every mode share is 1 or 1/2 and totals are <2^24.
// The final floating operations retain the CPU's division/multiplication order.
extern "C" __global__ void classify_records(float* records,const float* poses,const float* lighting,
 const unsigned* descriptors,unsigned descriptor_count,unsigned ray_count){
 unsigned job=blockIdx.x*blockDim.x+threadIdx.x;if(job>=ray_count)return;
 unsigned lo=0,hi=descriptor_count;while(lo+1<hi){unsigned mid=(lo+hi)/2;if(descriptors[12*mid]<=job)lo=mid;else hi=mid;}
 const unsigned* d=descriptors+12*lo;float* r=records+12*job;
 for(int j=0;j<12;j++){if(!isfinite(r[j])){r[0]=-1;return;}}
 if(r[0]!=4)return;
 V dir=loadv(r+6),observer=-loadv(poses+d[9]);const float* light=lighting+d[10];
 float radiance=metric_radiance(dir,light);
 if(!isfinite(radiance)||radiance<0){r[0]=-1;return;}
 if(dot(dir,observer)>light[9]||radiance<=light[10])r[0]=3;
}

// One owner per face-up candidate visits returned rays in the reference offer order.
// This preserves every f32 histogram/FMA rounding; compaction and f64 denominator
// accumulation preserve the principal-ray list order. No ray records leave the GPU.
extern "C" __global__ void compact_tone_paths(const float* records,const double* config,
 unsigned candidate_count,double* paths,double* metadata){
 unsigned candidate=blockIdx.x*blockDim.x+threadIdx.x;if(candidate>=candidate_count)return;
 const double* c=config+6*candidate;unsigned begin=(unsigned)c[0],end=(unsigned)c[1],modes=(unsigned)c[2];
 float width=(float)c[3],share=1.0f/(float)modes,weighted=0,weights=0;bool principal=c[5]!=0;
 unsigned returned=0,total=0,count=0;double denominator=0;float bins[128];for(int b=0;b<128;b++)bins[b]=0;
 double* output=paths+candidate*4096*5;double invalid=0;
 for(unsigned ray=begin;ray<end;ray++){const float* r=records+12*ray;if(r[0]==0)continue;total++;
  if(r[0]<0){invalid=1;continue;}if(r[0]!=4)continue;
  float weight=share*r[1],length=r[2];weighted=fmaf(length,weight,weighted);weights+=weight;returned++;
  if(principal){if(count>=4096){invalid=1;continue;}double* p=output+5*count++;
   p[0]=(double)weight;p[1]=(double)r[3];p[2]=(double)r[4];p[3]=(double)r[5];p[4]=c[4];denominator+=(double)weight;
  }else {float ratio=length/width;unsigned bin=isnan(ratio)||ratio<=0?0:ratio>=127?127:(unsigned)ratio;bins[bin]+=weight;}
 }
 if(!principal){if(weights>0){for(unsigned b=0;b<128;b++){if(bins[b]<=0)continue;float centre=((float)b+0.5f)*width;double* p=output+5*count++;
   p[0]=(double)bins[b]/(double)weights;p[1]=(double)centre*c[4];p[2]=0;p[3]=0;p[4]=c[4];}}denominator=1;}
 double* m=metadata+7*candidate;m[0]=(double)weighted;m[1]=(double)weights;m[2]=(double)returned;m[3]=(double)total;
 m[4]=(double)count;m[5]=denominator;m[6]=invalid;
}
extern "C" __global__ void reduce_pose_counts(const float* records,const unsigned* descriptors,
 unsigned descriptor_count,float* metrics){
 unsigned pose=blockIdx.x;if(pose>=descriptor_count)return;const unsigned* d=descriptors+12*pose;
 unsigned hit=0,windowed=0,extinct=0,returned=0,invalid=0;
 for(unsigned job=d[0]+threadIdx.x;job<d[1];job+=blockDim.x){int fate=(int)records[12*job];
  hit+=fate!=0;windowed+=fate==2;extinct+=fate==3;returned+=fate==4;invalid+=fate<0||fate>4;}
 __shared__ unsigned sums[5][128];unsigned t=threadIdx.x;
 sums[0][t]=hit;sums[1][t]=windowed;sums[2][t]=extinct;sums[3][t]=returned;sums[4][t]=invalid;__syncthreads();
 for(unsigned stride=64;stride>0;stride>>=1){if(t<stride){for(int i=0;i<5;i++)sums[i][t]+=sums[i][t+stride];}__syncthreads();}
 if(t==0){float share=1.0f/(float)d[11],total=(float)(sums[0][0]/d[11]);total=fmaxf(total,1.0f);
  metrics[4*pose]=clamp(((float)sums[3][0]*share)/total*100.0f,0,100);
  metrics[4*pose+1]=clamp(((float)sums[2][0]*share)/total*100.0f,0,100);
  metrics[4*pose+2]=clamp(((float)sums[1][0]*share)/total*100.0f,0,100);
  metrics[4*pose+3]=(float)sums[4][0];}
}
