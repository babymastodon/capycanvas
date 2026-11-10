using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;

public static class PocFunctionalNative {
    [StructLayout(LayoutKind.Sequential)] public struct Rect {public int Left,Top,Right,Bottom; public int Width {get{return Right-Left;}} public int Height {get{return Bottom-Top;}}}
    [StructLayout(LayoutKind.Sequential)] public struct Point {public int X,Y;}
    [StructLayout(LayoutKind.Sequential)] struct GuiInfo {public uint Size,Flags;public IntPtr Active,Focus,Capture,MenuOwner,MoveSize,Caret;public Rect CaretRect;}
    delegate bool EnumCallback(IntPtr window,IntPtr state);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumCallback callback,IntPtr state);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window,out uint process);
    [DllImport("user32.dll",CharSet=CharSet.Unicode,ExactSpelling=true,SetLastError=true,EntryPoint="GetClassNameW")] static extern int GetClassName(IntPtr window,StringBuilder name,int capacity);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window,out Rect rect);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr window,out Rect rect);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr window,ref Point point);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr window);
    [DllImport("user32.dll")] static extern bool GetGUIThreadInfo(uint thread,ref GuiInfo info);
    [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr window,IntPtr after,int x,int y,int width,int height,uint flags);
    [DllImport("user32.dll")] static extern bool IsZoomed(IntPtr window);
    [DllImport("user32.dll")] static extern IntPtr GetKeyboardLayout(uint thread);
    [DllImport("user32.dll",SetLastError=true)] static extern IntPtr SendMessageTimeout(IntPtr window,uint message,IntPtr w,IntPtr l,uint flags,uint timeout,out IntPtr result);
    [DllImport("imm32.dll")] static extern IntPtr ImmGetDefaultIMEWnd(IntPtr window);
    [StructLayout(LayoutKind.Sequential)] struct TitleBarInfo {public uint Size;public Rect Bar;[MarshalAs(UnmanagedType.ByValArray,SizeConst=6)]public uint[] State;[MarshalAs(UnmanagedType.ByValArray,SizeConst=6)]public Rect[] Bounds;}
    [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern IntPtr LoadLibrary(string path);
    [DllImport("kernel32.dll",CharSet=CharSet.Ansi,SetLastError=true)] static extern IntPtr GetProcAddress(IntPtr module,string name);
    [DllImport("kernel32.dll")] static extern bool FreeLibrary(IntPtr module);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate IntPtr SpecFunction();
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate UIntPtr RowFunction(uint row,[MarshalAs(UnmanagedType.I1)]bool visibility,[Out]byte[] output,UIntPtr capacity);
    public static void Own(IntPtr window,uint process){uint actual;GetWindowThreadProcessId(window,out actual);if(window==IntPtr.Zero||actual!=process)throw new InvalidOperationException("The HWND is not owned by the recorded process");}
    public static IntPtr[] Windows(uint process){var result=new List<IntPtr>();EnumWindows(delegate(IntPtr window,IntPtr state){uint actual;GetWindowThreadProcessId(window,out actual);if(actual==process&&IsWindowVisible(window))result.Add(window);return true;},IntPtr.Zero);return result.ToArray();}
    public static string ClassName(IntPtr window,uint process){Own(window,process);var name=new StringBuilder(256);if(GetClassName(window,name,name.Capacity)==0)throw new System.ComponentModel.Win32Exception();return name.ToString();}
    public static bool BoundsVisible(double[] bounds,double[] viewport,bool fully){if(bounds.Length!=4||viewport.Length!=4)throw new ArgumentException("Bounds require x,y,width,height");foreach(double value in bounds)if(double.IsNaN(value)||double.IsInfinity(value))return false;foreach(double value in viewport)if(double.IsNaN(value)||double.IsInfinity(value))return false;if(bounds[2]<=0||bounds[3]<=0||viewport[2]<=0||viewport[3]<=0)return false;return fully?bounds[0]>=viewport[0]&&bounds[1]>=viewport[1]&&bounds[0]+bounds[2]<=viewport[0]+viewport[2]&&bounds[1]+bounds[3]<=viewport[1]+viewport[3]:bounds[0]<viewport[0]+viewport[2]&&bounds[0]+bounds[2]>viewport[0]&&bounds[1]<viewport[1]+viewport[3]&&bounds[1]+bounds[3]>viewport[1];}
    public static string Spec(string path){IntPtr module=LoadLibrary(path);if(module==IntPtr.Zero)throw new System.ComponentModel.Win32Exception();try{var function=(SpecFunction)Marshal.GetDelegateForFunctionPointer(GetProcAddress(module,"poc_spec_json"),typeof(SpecFunction));IntPtr text=function();var bytes=new List<byte>();for(int i=0;Marshal.ReadByte(text,i)!=0;i++)bytes.Add(Marshal.ReadByte(text,i));return Encoding.UTF8.GetString(bytes.ToArray());}finally{FreeLibrary(module);}}
    public static string RowId(string path,uint row){IntPtr module=LoadLibrary(path);if(module==IntPtr.Zero)throw new System.ComponentModel.Win32Exception();try{var function=(RowFunction)Marshal.GetDelegateForFunctionPointer(GetProcAddress(module,"poc_spec_row_id"),typeof(RowFunction));byte[] bytes=new byte[128];int count=(int)function(row,false,bytes,new UIntPtr((uint)bytes.Length)).ToUInt64();if(count<1||count>=bytes.Length)throw new Exception("Shared row ID exceeded its buffer");return Encoding.UTF8.GetString(bytes,0,count-1);}finally{FreeLibrary(module);}}
    public static int HitTest(IntPtr window,uint process,int x,int y){Own(window,process);IntPtr result;long packed=((long)(ushort)y<<16)|(ushort)x;if(SendMessageTimeout(window,0x84,IntPtr.Zero,new IntPtr(packed),2,2000,out result)==IntPtr.Zero)throw new System.ComponentModel.Win32Exception();return result.ToInt32();}
    public static void Close(IntPtr window,uint process){Own(window,process);IntPtr result;if(SendMessageTimeout(window,0x10,IntPtr.Zero,IntPtr.Zero,2,2000,out result)==IntPtr.Zero)throw new System.ComponentModel.Win32Exception();}
    public static bool Maximized(IntPtr window,uint process){Own(window,process);return IsZoomed(window);}
    public static int[] MaximizeButton(IntPtr window,uint process){Own(window,process);var info=new TitleBarInfo{Size=(uint)Marshal.SizeOf(typeof(TitleBarInfo)),State=new uint[6],Bounds=new Rect[6]};IntPtr memory=Marshal.AllocHGlobal((int)info.Size);try{Marshal.StructureToPtr(info,memory,false);IntPtr result;if(SendMessageTimeout(window,0x33f,IntPtr.Zero,memory,2,2000,out result)==IntPtr.Zero)return null;info=(TitleBarInfo)Marshal.PtrToStructure(memory,typeof(TitleBarInfo));Rect bounds=info.Bounds[3];if((info.State[3]&0x8000)!=0||bounds.Width<=0||bounds.Height<=0)return null;return new[]{bounds.Left,bounds.Top,bounds.Width,bounds.Height};}finally{Marshal.DestroyStructure(memory,typeof(TitleBarInfo));Marshal.FreeHGlobal(memory);}}
    public static void ResizeClient(IntPtr window,uint process,int logicalWidth,int logicalHeight,int x,int y){Own(window,process);uint dpi=GetDpiForWindow(window);Rect outer,client;if(!GetWindowRect(window,out outer)||!GetClientRect(window,out client))throw new System.ComponentModel.Win32Exception();int width=(int)Math.Round(logicalWidth*dpi/96.0),height=(int)Math.Round(logicalHeight*dpi/96.0);if(!SetWindowPos(window,IntPtr.Zero,x,y,width+outer.Width-client.Width,height+outer.Height-client.Height,0x14)||!GetClientRect(window,out client))throw new System.ComponentModel.Win32Exception();if(client.Width!=width||client.Height!=height)throw new InvalidOperationException("Actual client dimensions differ from the requested physical dimensions");}
    public static int Language(IntPtr window,uint process){Own(window,process);uint owner;uint thread=GetWindowThreadProcessId(window,out owner);return (int)(GetKeyboardLayout(thread).ToInt64()&0xffff);}
    public static void ImeOpen(IntPtr window,uint process,bool open){Own(window,process);var ime=ImmGetDefaultIMEWnd(window);if(ime==IntPtr.Zero)throw new InvalidOperationException("The owned window has no IMM32 IME provider");Own(ime,process);IntPtr result;if(SendMessageTimeout(ime,0x283,new IntPtr(6),new IntPtr(open?1:0),2,2000,out result)==IntPtr.Zero)throw new System.ComponentModel.Win32Exception();}
    public static int[] Caret(IntPtr window,uint process){Own(window,process);uint owner;uint thread=GetWindowThreadProcessId(window,out owner);var info=new GuiInfo{Size=(uint)Marshal.SizeOf(typeof(GuiInfo))};if(!GetGUIThreadInfo(thread,ref info)||info.Caret==IntPtr.Zero)return null;Own(info.Caret,process);var origin=new Point();if(!ClientToScreen(info.Caret,ref origin))throw new System.ComponentModel.Win32Exception();return new[]{origin.X+info.CaretRect.Left,origin.Y+info.CaretRect.Top,Math.Max(1,info.CaretRect.Width),info.CaretRect.Height};}
}

public static class PocFunctionalPixels {
    static byte[] Read(Bitmap bitmap){var data=bitmap.LockBits(new Rectangle(0,0,bitmap.Width,bitmap.Height),ImageLockMode.ReadOnly,PixelFormat.Format32bppArgb);try{var bytes=new byte[bitmap.Width*bitmap.Height*4];for(int y=0;y<bitmap.Height;y++)Marshal.Copy(IntPtr.Add(data.Scan0,y*data.Stride),bytes,y*bitmap.Width*4,bitmap.Width*4);return bytes;}finally{bitmap.UnlockBits(data);}}
    public static int[] Difference(string before,string after,int left,int top,int width,int height){using(var a=new Bitmap(before))using(var b=new Bitmap(after)){if(a.Size!=b.Size)throw new ArgumentException("Snapshots changed dimensions");var x=Read(a);var y=Read(b);int minX=a.Width,minY=a.Height,maxX=-1,maxY=-1,count=0;for(int py=0;py<a.Height;py++)for(int px=0;px<a.Width;px++){if(px>=left&&px<left+width&&py>=top&&py<top+height)continue;int i=(py*a.Width+px)*4;if(x[i]==y[i]&&x[i+1]==y[i+1]&&x[i+2]==y[i+2])continue;minX=Math.Min(minX,px);minY=Math.Min(minY,py);maxX=Math.Max(maxX,px);maxY=Math.Max(maxY,py);count++;}return count==0?null:new[]{minX,minY,maxX-minX+1,maxY-minY+1,count};}}
    public sealed class Region {public int X,Y,Width,Height,ChangedPixels;}
    public static Region[] Regions(string before,string after,int[] masks){
        if(masks.Length%4!=0)throw new ArgumentException("Masks require x,y,width,height tuples");
        using(var a=new Bitmap(before))using(var b=new Bitmap(after)){
            if(a.Size!=b.Size)throw new ArgumentException("Snapshots changed dimensions");
            var x=Read(a);var y=Read(b);int width=a.Width,height=a.Height;var changed=new bool[width*height];var queue=new int[width*height];var result=new List<Region>();
            for(int py=0;py<height;py++)for(int px=0;px<width;px++){
                bool masked=false;for(int m=0;m<masks.Length;m+=4)if(px>=masks[m]&&py>=masks[m+1]&&px<masks[m]+masks[m+2]&&py<masks[m+1]+masks[m+3]){masked=true;break;}
                int at=py*width+px,i=at*4;changed[at]=!masked&&(x[i]!=y[i]||x[i+1]!=y[i+1]||x[i+2]!=y[i+2]);
            }
            for(int at=0;at<changed.Length;at++)if(changed[at]){
                int head=0,tail=1,minX=at%width,maxX=minX,minY=at/width,maxY=minY;queue[0]=at;changed[at]=false;
                while(head<tail){int current=queue[head++],cx=current%width,cy=current/width;minX=Math.Min(minX,cx);maxX=Math.Max(maxX,cx);minY=Math.Min(minY,cy);maxY=Math.Max(maxY,cy);
                    for(int dy=-1;dy<=1;dy++)for(int dx=-1;dx<=1;dx++){int nx=cx+dx,ny=cy+dy;if(nx<0||ny<0||nx>=width||ny>=height)continue;int next=ny*width+nx;if(changed[next]){changed[next]=false;queue[tail++]=next;}}
                }
                result.Add(new Region{X=minX,Y=minY,Width=maxX-minX+1,Height=maxY-minY+1,ChangedPixels=tail});
            }
            return result.ToArray();
        }
    }
    public static int[] CandidateRegions(Region[] regions,double[] caret,double lineHeight){
        if(lineHeight<=0||double.IsNaN(lineHeight))throw new ArgumentException("Actual caret line height is required");
        var indices=new List<int>();for(int i=0;i<regions.Length;i++){var r=regions[i];if(r.Width>=lineHeight&&r.Height>=lineHeight&&EdgeDistance(new double[]{r.X,r.Y,r.Width,r.Height},caret)<=2*lineHeight)indices.Add(i);}return indices.ToArray();
    }
    public static void DifferenceImage(string before,string after,string output){
        using(var a=new Bitmap(before))using(var b=new Bitmap(after))using(var image=new Bitmap(a.Width,a.Height,PixelFormat.Format32bppArgb)){
            if(a.Size!=b.Size)throw new ArgumentException("Snapshots changed dimensions");var x=Read(a);var y=Read(b);var bytes=new byte[x.Length];for(int i=0;i<bytes.Length;i+=4){bool changed=false;for(int c=0;c<3;c++){bytes[i+c]=(byte)Math.Abs(y[i+c]-x[i+c]);changed|=bytes[i+c]!=0;}bytes[i+3]=(byte)(changed?255:0);}
            var data=image.LockBits(new Rectangle(0,0,image.Width,image.Height),ImageLockMode.WriteOnly,PixelFormat.Format32bppArgb);try{for(int row=0;row<image.Height;row++)Marshal.Copy(bytes,row*image.Width*4,IntPtr.Add(data.Scan0,row*data.Stride),image.Width*4);}finally{image.UnlockBits(data);}image.Save(output,ImageFormat.Png);
        }
    }
    public static void Crop(string input,string output,int x,int y,int width,int height){using(var bitmap=new Bitmap(input)){var rect=Rectangle.Intersect(new Rectangle(0,0,bitmap.Width,bitmap.Height),new Rectangle(x,y,width,height));if(rect.IsEmpty)throw new ArgumentException("Snapshot crop is outside the image");using(var crop=bitmap.Clone(rect,PixelFormat.Format32bppArgb))crop.Save(output,ImageFormat.Png);}}
    public static int[] ColorRuns(string path,int x,int r,int g,int b){using(var bitmap=new Bitmap(path)){if(x<0||x>=bitmap.Width)throw new ArgumentOutOfRangeException("x");var runs=new List<int>();int start=-1;for(int y=0;y<=bitmap.Height;y++){bool match=false;if(y<bitmap.Height){Color c=bitmap.GetPixel(x,y);match=c.R==r&&c.G==g&&c.B==b;}if(match&&start<0)start=y;if(!match&&start>=0){runs.Add(start);runs.Add(y-start);start=-1;}}return runs.ToArray();}}
    public static bool Marker(string path,int x,int y){using(var image=new Bitmap(path)){if(x<1||y<1||x+96>=image.Width||y+4>=image.Height)return false;for(int py=y-1;py<=y+4;py++)for(int px=x-1;px<=x+96;px++){Color c=image.GetPixel(px,py);bool border=px==x-1||px==x+96||py==y-1||py==y+4;if(border){if(c.R!=17||c.G!=238||c.B!=68)return false;}else{bool black=c.R==0&&c.G==0&&c.B==0;bool white=c.R==255&&c.G==255&&c.B==255;if(!black&&!white)return false;}}for(int cell=0;cell<24;cell++){Color first=image.GetPixel(x+cell*4,y);for(int py=y;py<y+4;py++)for(int px=x+cell*4;px<x+(cell+1)*4;px++)if(image.GetPixel(px,py).ToArgb()!=first.ToArgb())return false;}return true;}}
    public static int Colors(string path){using(var image=new Bitmap(path)){var colors=new HashSet<int>();var bytes=Read(image);for(int i=0;i<bytes.Length;i+=4){colors.Add(bytes[i]|bytes[i+1]<<8|bytes[i+2]<<16);if(colors.Count>=16)return colors.Count;}return colors.Count;}}
    public static double EdgeDistance(double[] box,double[] caret){double left=box[0],top=box[1],right=left+box[2],bottom=top+box[3];double cLeft=caret[0],cTop=caret[1],cRight=cLeft+caret[2],cBottom=cTop+caret[3];double vertical=Math.Max(Math.Max(top-cBottom,cTop-bottom),0),horizontal=Math.Max(Math.Max(left-cRight,cLeft-right),0);double x1=Math.Max(Math.Max(cLeft-left,left-cRight),0),x2=Math.Max(Math.Max(cLeft-right,right-cRight),0),y1=Math.Max(Math.Max(cTop-top,top-cBottom),0),y2=Math.Max(Math.Max(cTop-bottom,bottom-cBottom),0);return Math.Min(Math.Min(Math.Sqrt(x1*x1+vertical*vertical),Math.Sqrt(x2*x2+vertical*vertical)),Math.Min(Math.Sqrt(y1*y1+horizontal*horizontal),Math.Sqrt(y2*y2+horizontal*horizontal)));}
}
