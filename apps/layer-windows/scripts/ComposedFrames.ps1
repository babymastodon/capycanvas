Add-Type -AssemblyName System.Drawing
if(!('TabFrames' -as [type])){
$references=@([Drawing.Bitmap],[Drawing.Color],[Drawing.Graphics])|ForEach-Object {$_.Assembly;$_.GetInterfaces()|ForEach-Object Assembly}|Where-Object {$_ -ne [object].Assembly}|ForEach-Object Location|Select-Object -Unique
Add-Type -ReferencedAssemblies $references -TypeDefinition @'
using System;
using System.Drawing;
using System.Runtime.InteropServices;
public static class TabFrames {
 [DllImport("dwmapi.dll")] public static extern int DwmFlush();
 static int Delta(Color p,Color q){return Math.Max(Math.Abs(p.R-q.R),Math.Max(Math.Abs(p.G-q.G),Math.Abs(p.B-q.B)));}
 public static Bitmap Grab(int x,int y,int width,int height){
  var image=new Bitmap(width,height);
  using(var graphics=Graphics.FromImage(image))graphics.CopyFromScreen(x,y,0,0,image.Size);
  return image;
 }
 public sealed class Envelope {
  readonly int width,height;
  readonly int[] lo,hi;
  public Envelope(Bitmap preview,Bitmap final){
   width=preview.Width;height=preview.Height;int size=width*height*3;
   var minimum=new int[size];var maximum=new int[size];lo=new int[size];hi=new int[size];
   for(int y=0;y<height;y++)for(int x=0;x<width;x++){
    int i=(y*width+x)*3;var p=preview.GetPixel(x,y);var q=final.GetPixel(x,y);
    minimum[i]=Math.Min(p.R,q.R);minimum[i+1]=Math.Min(p.G,q.G);minimum[i+2]=Math.Min(p.B,q.B);
    maximum[i]=Math.Max(p.R,q.R);maximum[i+1]=Math.Max(p.G,q.G);maximum[i+2]=Math.Max(p.B,q.B);
   }
   for(int y=0;y<height;y++)for(int x=0;x<width;x++){
    int i=(y*width+x)*3;lo[i]=lo[i+1]=lo[i+2]=255;
    for(int yy=Math.Max(0,y-1);yy<=Math.Min(height-1,y+1);yy++)
     for(int xx=Math.Max(0,x-1);xx<=Math.Min(width-1,x+1);xx++){
      int j=(yy*width+xx)*3;
      for(int c=0;c<3;c++){lo[i+c]=Math.Min(lo[i+c],minimum[j+c]);hi[i+c]=Math.Max(hi[i+c],maximum[j+c]);}
     }
    for(int c=0;c<3;c++){lo[i+c]-=6;hi[i+c]+=6;}
   }
  }
  bool Invalid(Bitmap image,int x,int y){
   int i=(y*width+x)*3;var v=image.GetPixel(x,y);
   return v.R<lo[i]||v.R>hi[i]||v.G<lo[i+1]||v.G>hi[i+1]||v.B<lo[i+2]||v.B>hi[i+2];
  }
  public int Difference(Bitmap image){return Difference(image,Rectangle.Empty);}
  public int Difference(Bitmap image,Rectangle excluded){
   int count=0;
   for(int y=0;y<height;y++)for(int x=0;x<width;x++)if(!excluded.Contains(x,y)&&Invalid(image,x,y))count++;
   return count;
  }
  public bool ExtraTint(Bitmap image,Bitmap final,Rectangle strip,Rectangle excluded,Color tint){
   strip=Rectangle.Intersect(strip,new Rectangle(0,0,width,height));
   var band=Rectangle.Inflate(strip,2,2);var core=Rectangle.Inflate(strip,-4,-4);
   if(core.Width<=0||core.Height<=0)return false;
   double alpha=tint.A/255.0;
   Func<Color,Color,int,bool> layered=(f,v,tolerance)=>Over(f.R,tint.R,alpha,v.R,tolerance,false)
    &&Over(f.G,tint.G,alpha,v.G,tolerance,false)&&Over(f.B,tint.B,alpha,v.B,tolerance,false);
   Func<Color,Color,bool> partial=(f,v)=>Over(f.R,tint.R,alpha,v.R,6,true)&&Over(f.G,tint.G,alpha,v.G,6,true)&&Over(f.B,tint.B,alpha,v.B,6,true);
   if(!layered(Mode(final,core),Mode(image,core),3))return false;
   int outside=0,unexplained=0;
   for(int y=0;y<height;y++)for(int x=0;x<width;x++){
    if(excluded.Contains(x,y)||!Invalid(image,x,y))continue;
    if(!band.Contains(x,y)){outside++;continue;}
    var f=final.GetPixel(x,y);var v=image.GetPixel(x,y);
    if(Flat(final,x,y)?!layered(f,v,6):!partial(f,v))unexplained++;
   }
   return outside<=8&&unexplained<=8;
  }
  static bool Over(int final,int tint,double alpha,int value,int tolerance,bool partial){
   double layered=final+alpha*(tint-final);
   return partial?value>=Math.Min(final,layered)-tolerance&&value<=Math.Max(final,layered)+tolerance:Math.Abs(value-layered)<=tolerance;
  }
  bool Flat(Bitmap image,int x,int y){
   var center=image.GetPixel(x,y);
   for(int yy=Math.Max(0,y-1);yy<=Math.Min(height-1,y+1);yy++)
    for(int xx=Math.Max(0,x-1);xx<=Math.Min(width-1,x+1);xx++)if(image.GetPixel(xx,yy)!=center)return false;
   return true;
  }
  static Color Mode(Bitmap image,Rectangle area){
   var keys=new int[area.Width*area.Height];int n=0,best=0,mode=0;
   for(int y=area.Top;y<area.Bottom;y++)for(int x=area.Left;x<area.Right;x++)keys[n++]=image.GetPixel(x,y).ToArgb();
   Array.Sort(keys);
   for(int start=0,end=0;start<n;start=end){
    while(end<n&&keys[end]==keys[start])end++;
    if(end-start>best){best=end-start;mode=keys[start];}
   }
   return Color.FromArgb(mode);
  }
 }
 public static int[] Remaining(Bitmap image,Bitmap final,Rectangle overview){
  int native=0,paper=0;
  for(int y=0;y<image.Height;y++)for(int x=0;x<image.Width;x++){
   if(Delta(image.GetPixel(x,y),final.GetPixel(x,y))<=6)continue;
   if(overview.Contains(x,y))paper++;else native++;
  }
  return new[]{native,paper};
 }
 public static int FootDifference(Bitmap a,RectangleF from,Bitmap b,RectangleF to,double scale){
  int count=0,pad=(int)(8*scale),y=(int)(to.Bottom-5*scale),oldY=(int)(from.Bottom-5*scale);
  for(int x=pad;x<to.Width-pad;x++){
   int oldX=(int)(from.X+pad+(x-pad)*(from.Width-2*pad)/(to.Width-2*pad));
   if(Delta(a.GetPixel(oldX,oldY),b.GetPixel((int)to.X+x,y))>16)count++;
  }
  return count;
 }
}
'@
}

function Sample-Frames($Area,[scriptblock]$Action){
 $frames=[Collections.Generic.List[Drawing.Bitmap]]::new();$times=@()
 try{
  for($i=0;$i -lt 90;$i++){
   $null=[TabFrames]::DwmFlush();$actionAt=[Diagnostics.Stopwatch]::GetTimestamp();& $Action $i|Out-Null
   $begin=[Diagnostics.Stopwatch]::GetTimestamp()
   $frames.Add([TabFrames]::Grab([int]$Area.Left,[int]$Area.Top,[int]$Area.Width,[int]$Area.Height))
   $end=[Diagnostics.Stopwatch]::GetTimestamp()
   $times+=@{frame=$i;action=$actionAt;begin=$begin;end=$end}
  }
  @{frames=$frames;times=$times}
 }catch{foreach($image in $frames){$image.Dispose()};throw}
}
