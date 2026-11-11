using System;
using System.Drawing;
using System.Collections.Generic;
public sealed class NavigatorGeometry {
 public long qpc;public int source_index=-1;
 public double[] overview,header,clip,footer,frame,screen,corners;
}
public sealed class NavigatorFrameResult {
 public string classification;
 public int paper,placeholder,canvas,expected,outside;
 public double header_error;
 public long qpc;
 public double[] image,visible,frame,corners;
}
public sealed class NavigatorFrameClassifier : IDisposable {
 readonly Bitmap reference;readonly int width,height;readonly double scale;readonly double[] view,header;
 byte[] artwork;double[] artworkImage;
 readonly int[] paper,bg,panel;readonly bool[] white;readonly byte[] headerRgb,footerRgb;readonly int tx,ty,tw,th,fw,fh;
 static int Delta(int a,int b,int c,int[] p){return Math.Max(Math.Abs(a-p[0]),Math.Max(Math.Abs(b-p[1]),Math.Abs(c-p[2])));}
 public NavigatorFrameClassifier(string path,double[] overview,double[] tab,double pixelScale,int[] paperColor,int[] background,int[] panelColor,double[] footer){
  using(var loaded=new Bitmap(path))reference=new Bitmap(loaded);width=reference.Width;height=reference.Height;view=overview;header=tab;scale=pixelScale;paper=paperColor;bg=background;panel=panelColor;
  tx=(int)Math.Ceiling(tab[0]+8*scale);ty=(int)Math.Ceiling(tab[1]+(tab[3]-12*scale)/2);tw=(int)(36*scale);th=(int)(12*scale);
  if(tx<0||ty<0||tx+tw>width||ty+th>height)throw new ArgumentException("Header reference outside capture");
  headerRgb=new byte[tw*th*3];
  for(int y=0;y<th;y++)for(int x=0;x<tw;x++){var color=reference.GetPixel(tx+x,ty+y);int i=(y*tw+x)*3;headerRgb[i]=color.R;headerRgb[i+1]=color.G;headerRgb[i+2]=color.B;}
  fw=(int)(16*scale);fh=(int)(16*scale);footerRgb=new byte[fw*fh*3];
  int fx=(int)Math.Floor(footer[0]+(footer[2]-fw)/2),fy=(int)Math.Floor(footer[1]+(footer[3]-fh)/2);
  if(fx<0||fy<0||fx+fw>width||fy+fh>height)throw new ArgumentException("Action-row reference outside capture");
  for(int y=0;y<fh;y++)for(int x=0;x<fw;x++){var color=reference.GetPixel(fx+x,fy+y);int i=(y*fw+x)*3;footerRgb[i]=color.R;footerRgb[i+1]=color.G;footerRgb[i+2]=color.B;}
  white=new bool[width*height];
  for(int y=3;y<height-3;y++)for(int x=3;x<width-3;x++){
   bool valid=true;for(int yy=-3;yy<=3&&valid;yy++)for(int xx=-3;xx<=3;xx++){var c=reference.GetPixel(x+xx,y+yy);if(Delta(c.R,c.G,c.B,paper)>3){valid=false;break;}}
   white[y*width+x]=valid;
  }
 }
 double[] Image(double[] v,double aspect){double w=Math.Min(v[2]-8*scale,(v[3]-8*scale)*aspect),h=w/aspect;return new[]{v[0]+(v[2]-w)/2,v[1]+(v[3]-h)/2,w,h};}
 byte[] Pixels(Bitmap bitmap){
  var data=bitmap.LockBits(new Rectangle(0,0,width,height),System.Drawing.Imaging.ImageLockMode.ReadOnly,System.Drawing.Imaging.PixelFormat.Format32bppArgb);
  var bytes=new byte[width*height*4];
  try{for(int y=0;y<height;y++){var row=new byte[width*4];System.Runtime.InteropServices.Marshal.Copy(IntPtr.Add(data.Scan0,y*data.Stride),row,0,row.Length);for(int x=0;x<width;x++){int j=(y*width+x)*4;bytes[j]=row[x*4+2];bytes[j+1]=row[x*4+1];bytes[j+2]=row[x*4];bytes[j+3]=row[x*4+3];}}}finally{bitmap.UnlockBits(data);}
  return bytes;
 }
 public NavigatorFrameResult ClassifyBitmap(Bitmap bitmap,long qpc,NavigatorGeometry[] states,double aspect){return Classify(Pixels(bitmap),0,qpc,states,aspect);}
 public void ClearArtwork(){artwork=null;artworkImage=null;}
 public void SetArtwork(Bitmap bitmap,NavigatorFrameResult endpoint){
  if(endpoint.classification!="I"||endpoint.image==null||!Equal(endpoint.image,endpoint.visible))throw new InvalidOperationException("Artwork endpoint must be an originally filled, fully visible image");
  var shape=new NavigatorGeometry{frame=endpoint.frame,corners=endpoint.corners};
  foreach(double x in new[]{endpoint.image[0],endpoint.image[0]+endpoint.image[2]})foreach(double y in new[]{endpoint.image[1],endpoint.image[1]+endpoint.image[3]})if(!FrameInterior(shape,x,y,0))throw new InvalidOperationException("Artwork endpoint is clipped by the rounded frame");
  artwork=Pixels(bitmap);artworkImage=(double[])endpoint.image.Clone();
 }
 bool ArtworkMatch(byte[] pixels,int offset,int x,int y,double[] image){
  if(artwork==null)return false;
  int ax=(int)(artworkImage[0]+(x+.5-image[0])*artworkImage[2]/image[2]),ay=(int)(artworkImage[1]+(y+.5-image[1])*artworkImage[3]/image[3]);
  if(ax<0||ay<0||ax>=width||ay>=height)return false;int a=(ay*width+ax)*4,p=offset+(y*width+x)*4;
  return Delta(artwork[a],artwork[a+1],artwork[a+2],paper)>3&&Delta(artwork[a],artwork[a+1],artwork[a+2],bg)>3&&Delta(artwork[a],artwork[a+1],artwork[a+2],panel)>3&&Math.Max(Math.Abs(pixels[p]-artwork[a]),Math.Max(Math.Abs(pixels[p+1]-artwork[a+1]),Math.Abs(pixels[p+2]-artwork[a+2])))<=3;
 }
 bool ArtworkVisible(byte[] pixels,int offset,NavigatorGeometry[] states,double aspect){
  if(artwork==null)return false;
  foreach(var state in states){var image=Image(state.overview,aspect);var visible=state.clip==null?image:Intersect(image,state.clip);int matches=0;
   for(int y=Math.Max(0,(int)Math.Ceiling(visible[1]+3));y<Math.Min(height,(int)Math.Floor(visible[1]+visible[3]-3));y++)for(int x=Math.Max(0,(int)Math.Ceiling(visible[0]+3));x<Math.Min(width,(int)Math.Floor(visible[0]+visible[2]-3));x++)if(FrameInterior(state,x+.5,y+.5,3)&&ArtworkMatch(pixels,offset,x,y,image)&&++matches>=20)return true;
  }
  return false;
 }
 double FooterError(byte[] pixels,int offset,NavigatorGeometry state,int headerX,int headerY){
  if(state.footer==null)return Double.PositiveInfinity;
  double dx=headerX-(state.header[0]+tx-header[0]),dy=headerY-(state.header[1]+ty-header[1]);
  int left=(int)Math.Floor(state.footer[0]+(state.footer[2]-fw)/2+dx),top=(int)Math.Floor(state.footer[1]+(state.footer[3]-fh)/2+dy);double best=Double.PositiveInfinity;
  for(int sy=-1;sy<=1;sy++)for(int sx=-1;sx<=1;sx++){
   long sum=0;int count=0,low=255,high=0;
   for(int y=0;y<fh;y++)for(int x=0;x<fw;x++){
    int px=left+x+sx,py=top+y+sy;if(px<0||py<0||px>=width||py>=height)continue;
    if(state.clip!=null&&(px<state.clip[0]||py<state.clip[1]||px>=state.clip[0]+state.clip[2]||py>=state.clip[1]+state.clip[3]))continue;
    int p=offset+(py*width+px)*4,t=(y*fw+x)*3;
    for(int c=0;c<3;c++){sum+=Math.Abs(pixels[p+c]-footerRgb[t+c]);low=Math.Min(low,footerRgb[t+c]);high=Math.Max(high,footerRgb[t+c]);}count++;
   }
   if(count>0&&high>low)best=Math.Min(best,sum/(3.0*count));
  }
  return best;
 }
 static bool Equal(double[] a,double[] b,double tolerance=.01){if(a==null||b==null)return a==b;for(int i=0;i<4;i++)if(Math.Abs(a[i]-b[i])>tolerance)return false;return true;}
 static double[] Mix(double[] a,double[] b,double fraction){if(a==null)return null;var result=new double[4];for(int i=0;i<4;i++)result[i]=a[i]+fraction*(b[i]-a[i]);return result;}
 static double[] Intersect(double[] a,double[] b){double left=Math.Max(a[0],b[0]),top=Math.Max(a[1],b[1]);return new[]{left,top,Math.Max(0,Math.Min(a[0]+a[2],b[0]+b[2])-left),Math.Max(0,Math.Min(a[1]+a[3],b[1]+b[3])-top)};}
 public static bool FrameInterior(NavigatorGeometry state,double x,double y,double margin){
  if(state.corners==null)return true;
  if(state.frame==null||state.corners.Length!=4)throw new InvalidOperationException("Rounded Navigator frame geometry is missing");
  var f=state.frame;double left=f[0],top=f[1],right=left+f[2],bottom=top+f[3];
  if(x<left+margin||y<top+margin||x>right-margin||y>bottom-margin)return false;
  for(int i=0;i<4;i++){
   double r=state.corners[i];if(Double.IsNaN(r)||Double.IsInfinity(r)||r<0)throw new InvalidOperationException("Invalid Navigator corner radius");
   r=Math.Min(r,Math.Min(f[2],f[3])/2);bool east=i==1||i==2,south=i>=2;
   double cx=east?right-r:left+r,cy=south?bottom-r:top+r;
   if((east?x>cx:x<cx)&&(south?y>cy:y<cy)&&Math.Sqrt((x-cx)*(x-cx)+(y-cy)*(y-cy))>r-margin)return false;
  }
  return true;
 }
 static bool Horizontal(NavigatorGeometry a,NavigatorGeometry b){
  if((a.source_index>=0||b.source_index>=0)&&(a.source_index<0||b.source_index!=a.source_index+1))return false;
  if(!Equal(a.corners,b.corners))return false;
  if(a.screen==null||!Equal(a.screen,b.screen)||(a.frame==null)!=(b.frame==null)||(a.footer==null)!=(b.footer==null))return false;
  if(Math.Abs(a.header[1]-b.header[1])>.01||Math.Abs(a.header[2]-b.header[2])>.01||Math.Abs(a.header[3]-b.header[3])>.01)return false;
  if(Math.Abs(a.overview[2]-b.overview[2])>.01||Math.Abs(a.overview[3]-b.overview[3])>.01)return false;
  for(int i=0;i<2;i++)if(Math.Abs((a.overview[i]-a.header[i])-(b.overview[i]-b.header[i]))>.01)return false;
  if(a.footer!=null)for(int i=0;i<4;i++)if(Math.Abs((a.footer[i]-(i<2?a.header[i]:0))-(b.footer[i]-(i<2?b.header[i]:0)))>.01)return false;
  if(a.frame!=null&&Math.Abs((a.frame[1]-a.header[1])-(b.frame[1]-b.header[1]))>.01)return false;
  return true;
 }
 NavigatorGeometry BetweenHeaders(NavigatorGeometry[] states,int x,int y){
  double target=x-tx+header[0],targetY=y-ty+header[1];NavigatorGeometry found=null;
  for(int i=0;i+1<states.Length;i++){
   var a=states[i];var b=states[i+1];double dx=b.header[0]-a.header[0];
   if(b.qpc<=a.qpc||Math.Abs(dx)<.01||!Horizontal(a,b)||Math.Abs(a.header[1]-targetY)>.51)continue;
   double fraction=(target-a.header[0])/dx;if(fraction<0||fraction>1)continue;
   var current=new NavigatorGeometry{overview=Mix(a.overview,b.overview,fraction),header=Mix(a.header,b.header,fraction),footer=Mix(a.footer,b.footer,fraction),frame=Mix(a.frame,b.frame,fraction),screen=a.screen,corners=a.corners};
   current.clip=current.frame==null?current.screen:Intersect(current.frame,current.screen);
   if(found!=null&&(!Equal(found.overview,current.overview,.51)||!Equal(found.clip,current.clip,.51)))throw new InvalidOperationException("Displayed horizontal geometry has ambiguous native brackets");
   found=current;
  }
  if(found==null)throw new InvalidOperationException("Displayed horizontal geometry has no adjacent native bracket");return found;
 }
 public NavigatorFrameResult Classify(byte[] rgba,int offset,long qpc,NavigatorGeometry[] states,double aspect){
  var result=new NavigatorFrameResult{qpc=qpc,header_error=Double.MaxValue};int bestX=0,bestY=0;
  var rows=new HashSet<int>();for(int d=-3;d<=3;d++)rows.Add(ty+d);
  foreach(var state in states)for(int d=-1;d<=1;d++)rows.Add((int)Math.Round(state.header[1]+ty-header[1])+d);
  foreach(int y in rows){if(y<0||y+th>height)continue;for(int x=0;x<=width-tw;x++){
   long sum=0;for(int yy=0;yy<th;yy+=2)for(int xx=0;xx<tw;xx+=2){int i=offset+((y+yy)*width+x+xx)*4;int t=(yy*tw+xx)*3;sum+=Math.Abs(rgba[i]-headerRgb[t])+Math.Abs(rgba[i+1]-headerRgb[t+1])+Math.Abs(rgba[i+2]-headerRgb[t+2]);}
   double error=sum/(3.0*((th+1)/2)*((tw+1)/2));if(error<result.header_error){result.header_error=error;bestX=x;bestY=y;}
  }
  }
  for(int i=offset;i<offset+width*height*4;i+=4)if(Delta(rgba[i],rgba[i+1],rgba[i+2],paper)<=3)result.paper++;
  if(result.header_error>=10){result.classification=(result.paper>0||ArtworkVisible(rgba,offset,states,aspect))?"image-before-panel":"A";return result;}
  NavigatorGeometry chosen=null;double position=Double.MaxValue,anchor=Double.MaxValue,time=Double.MaxValue;bool sizesDiffer=false;double[] firstSize=null;
  foreach(var state in states){double d=Math.Abs(state.header[0]+tx-header[0]-bestX)+Math.Abs(state.header[1]+ty-header[1]-bestY);if(d<position){position=d;firstSize=state.overview;sizesDiffer=false;}else if(d==position&&(state.overview[2]!=firstSize[2]||state.overview[3]!=firstSize[3]))sizesDiffer=true;}
  foreach(var state in states){
   double d=Math.Abs(state.header[0]+tx-header[0]-bestX)+Math.Abs(state.header[1]+ty-header[1]-bestY);if(d!=position)continue;
   double error=sizesDiffer?FooterError(rgba,offset,state,bestX,bestY):0,stamp=Math.Abs((double)state.qpc-qpc);
   if(chosen==null||error<anchor||(error==anchor&&stamp<time)){chosen=state;anchor=error;time=stamp;}
  }
  if(sizesDiffer&&anchor>=10)throw new InvalidOperationException("Displayed native size has no confident action-row anchor");
  if(chosen==null)throw new InvalidOperationException("No arranged Navigator geometry");
  if(Math.Abs(chosen.header[0]+tx-header[0]-bestX)>.51||Math.Abs(chosen.header[1]+ty-header[1]-bestY)>.51)chosen=BetweenHeaders(states,bestX,bestY);
  double[] image=Image(chosen.overview,aspect),original=Image(view,aspect);int white=0,matchedArtwork=0;
  double[] visible=(double[])image.Clone();
  if(chosen.clip!=null){double right=Math.Min(image[0]+image[2],chosen.clip[0]+chosen.clip[2]),bottom=Math.Min(image[1]+image[3],chosen.clip[1]+chosen.clip[3]);visible[0]=Math.Max(image[0],chosen.clip[0]);visible[1]=Math.Max(image[1],chosen.clip[1]);visible[2]=Math.Max(0,right-visible[0]);visible[3]=Math.Max(0,bottom-visible[1]);}
  result.image=image;result.visible=visible;result.frame=chosen.frame;result.corners=chosen.corners;
  if(visible[0]<0||visible[1]<0||visible[0]+visible[2]>width||visible[1]+visible[3]>height)throw new InvalidOperationException("Expected visible Navigator image extends outside capture");
  for(int y=Math.Max(0,(int)Math.Ceiling(visible[1]+3));y<Math.Min(height,(int)Math.Floor(visible[1]+visible[3]-3));y++)for(int x=Math.Max(0,(int)Math.Ceiling(visible[0]+3));x<Math.Min(width,(int)Math.Floor(visible[0]+visible[2]-3));x++){
   if(!FrameInterior(chosen,x+.5,y+.5,3))continue;
   int rx=(int)(original[0]+(x+.5-image[0])*original[2]/image[2]),ry=(int)(original[1]+(y+.5-image[1])*original[3]/image[3]);
   if(rx<0||ry<0||rx>=width||ry>=height||!this.white[ry*width+rx])continue;result.expected++;int i=offset+(y*width+x)*4;if(ArtworkMatch(rgba,offset,x,y,image))matchedArtwork++;
   if(Delta(rgba[i],rgba[i+1],rgba[i+2],paper)<=3)white++;
   if(Delta(rgba[i],rgba[i+1],rgba[i+2],panel)<=3)result.placeholder++;
   if(y>image[1]+image[3]*.45&&y<image[1]+image[3]*.55&&Delta(rgba[i],rgba[i+1],rgba[i+2],bg)<=3)result.canvas++;
  }
  if(result.expected<20)throw new InvalidOperationException("Insufficient known paper pixels");
  for(int y=0;y<height;y++)for(int x=0;x<width;x++){
   if(x>=visible[0]-3&&x<=visible[0]+visible[2]+3&&y>=visible[1]-3&&y<=visible[1]+visible[3]+3)continue;
   int i=offset+(y*width+x)*4;if(Delta(rgba[i],rgba[i+1],rgba[i+2],paper)<=3)result.outside++;
  }
  result.classification=result.canvas>0?"C":(white==0&&matchedArtwork!=result.expected)?"P":result.placeholder>0||result.outside>0?"M":"I";return result;
 }
 public void Dispose(){reference.Dispose();}
}
