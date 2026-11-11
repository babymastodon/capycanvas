using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Text;
using System.Threading;
using System.Windows.Automation;
public sealed class NavigatorDrawerRead {public string id,status,error;public double[] bounds;public long begin,end;}
public sealed class NavigatorRead {public int index;public long begin,end;public int raw;public string error;}
public sealed class NavigatorObserver : IDisposable {
 readonly AutomationElement workspace;readonly string drawerId,workspaceId,workspaceName;readonly List<NavigatorDrawerRead> drawers=new List<NavigatorDrawerRead>();readonly object gate=new object();readonly List<NavigatorRead> reads=new List<NavigatorRead>();readonly List<string> raw=new List<string>();
 Thread worker;volatile bool stopping;bool overflow;int rawBytes;
 public NavigatorObserver(AutomationElement element,string frameId){
  workspace=element;drawerId=frameId;
  if(drawerId!=null){workspaceId=workspace.Current.AutomationId;workspaceName=workspace.Current.Name;if(workspaceId.Length==0&&workspaceName.Length==0)throw new InvalidOperationException("Navigator workspace has no cache filter identity");}
 }
 public void Start(){worker=new Thread(Observe);worker.IsBackground=true;worker.SetApartmentState(ApartmentState.MTA);worker.Start();}
 CacheRequest DrawerCache(){
  var request=new CacheRequest{TreeScope=TreeScope.Subtree,AutomationElementMode=AutomationElementMode.None};
  request.TreeFilter=new OrCondition(new AndCondition(new PropertyCondition(AutomationElement.AutomationIdProperty,workspaceId),new PropertyCondition(AutomationElement.NameProperty,workspaceName)),new PropertyCondition(AutomationElement.AutomationIdProperty,drawerId));
  request.Add(AutomationElement.BoundingRectangleProperty);request.Add(AutomationElement.ItemStatusProperty);request.Add(AutomationElement.AutomationIdProperty);request.Add(AutomationElement.NameProperty);
  return request;
 }
 void FindDrawer(AutomationElement element,ref AutomationElement found,bool root){
  if(root){if(element.Cached.AutomationId!=workspaceId||element.Cached.Name!=workspaceName)throw new InvalidOperationException("Navigator cached workspace identity changed");}
  else{if(element.Cached.AutomationId!=drawerId||found!=null)throw new InvalidOperationException("Navigator cached drawer identity is ambiguous");found=element;}
  foreach(AutomationElement child in element.CachedChildren)FindDrawer(child,ref found,false);
 }
 void Observe(){
  var request=drawerId==null?null:DrawerCache();
  while(!stopping){var read=new NavigatorRead{begin=Stopwatch.GetTimestamp(),raw=-1};string value=null;NavigatorDrawerRead shape=null;
   try{
    if(request==null)value=(string)workspace.GetCurrentPropertyValue(AutomationElement.ItemStatusProperty);
    else{
     shape=new NavigatorDrawerRead{id=drawerId,begin=read.begin};
     var cached=workspace.GetUpdatedCache(request);shape.end=Stopwatch.GetTimestamp();
     value=cached.Cached.ItemStatus;AutomationElement drawer=null;FindDrawer(cached,ref drawer,true);
     if(drawer!=null){var bounds=drawer.Cached.BoundingRectangle;shape.bounds=new[]{bounds.Left,bounds.Top,bounds.Width,bounds.Height};shape.status=drawer.Cached.ItemStatus;}
    }
   }catch(Exception error){read.error=error.ToString();if(shape!=null){shape.error=read.error;shape.end=Stopwatch.GetTimestamp();}}
   read.end=Stopwatch.GetTimestamp();lock(gate){
    bool changed=value!=null&&(raw.Count==0||raw[raw.Count-1]!=value||shape!=null&&(drawers[drawers.Count-1].status!=shape.status||drawers[drawers.Count-1].error!=shape.error||!SameBounds(drawers[drawers.Count-1].bounds,shape.bounds)));int bytes=changed?Encoding.UTF8.GetByteCount(value)+(shape!=null&&shape.status!=null?Encoding.UTF8.GetByteCount(shape.status):0):0;
    if(reads.Count>=4096||bytes>33554432-rawBytes){overflow=true;stopping=true;break;}
    read.index=reads.Count;if(value!=null){if(changed){raw.Add(value);drawers.Add(shape);rawBytes+=bytes;}read.raw=raw.Count-1;}reads.Add(read);
   }Thread.Sleep(1);
  }
 }
 static bool SameBounds(double[] a,double[] b){if(a==null||b==null)return a==b;for(int i=0;i<4;i++)if(a[i]!=b[i])return false;return true;}
 public bool Overflow {get{lock(gate)return overflow;}}
 public NavigatorRead[] Reads(){lock(gate)return reads.ToArray();}
 public NavigatorDrawerRead[] DrawerRaw(){lock(gate)return drawers.ToArray();}
 public string[] Raw(){lock(gate)return raw.ToArray();}
 public void Dispose(){stopping=true;if(worker!=null&&!worker.Join(10000))throw new TimeoutException("Navigator metadata observer did not stop");if(Overflow)throw new InvalidOperationException("Navigator metadata observer exceeded bounded evidence buffers");}

}
