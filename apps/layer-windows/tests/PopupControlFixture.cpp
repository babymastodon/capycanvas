#include "pch.h"
#include "WindowRegistry.h"
#include "ControlContext.h"
#include "NativeMenus.h"
#include "ChromeVisibility.h"
#include "WorkspaceManagerView.h"
#include "WorkspaceDialogs.h"
#include "WorkspaceView.h"
#include "CanvasModelUpdate.h"
#include "BubblePresentation.h"
#include "PressureCalibration.h"
#include "CanvasActionBar.h"
#include "EffectControls.h"
#include "SettingsView.h"
#include "ShortcutPage.h"
#include "PalettesView.h"
#include "TextureDialog.h"
#include "HeaderView.h"
#include "HeaderInput.h"
#include "ColorStrip.h"
#include <future>
#include <msctf.h>
#pragma comment(lib,"uuid.lib")
#include <winrt/Microsoft.UI.Xaml.Automation.Peers.h>
#include <winrt/Microsoft.UI.Xaml.Automation.Provider.h>
#include <microsoft.ui.xaml.window.h>
#include <exception>
#include <fstream>
using namespace CapyUi;
namespace {
struct TextServicesLibrary{
    using Current=HRESULT(WINAPI*)(ITfThreadMgr**);
    HMODULE module=nullptr;Current current=nullptr;DWORD loadError=0,lookupError=0,worker=0;std::wstring path;
    TextServicesLibrary(){
        std::jthread startup([this]{
            worker=GetCurrentThreadId();SetLastError(ERROR_SUCCESS);
            module=LoadLibraryExW(L"msctf.dll",nullptr,LOAD_LIBRARY_SEARCH_SYSTEM32);
            if(!module){loadError=GetLastError();return;}
            wchar_t name[32768]{};auto length=GetModuleFileNameW(module,name,32768);if(length&&length<32768)path.assign(name,length);
            SetLastError(ERROR_SUCCESS);current=reinterpret_cast<Current>(GetProcAddress(module,"TF_GetThreadMgr"));
            if(!current)lookupError=GetLastError();
        });
        startup.join();
    }
    ~TextServicesLibrary(){if(module)FreeLibrary(module);}
};
struct CompositionEdit:implements<CompositionEdit,ITfEditSession>{
    std::function<void(TfEditCookie)> run;
    explicit CompositionEdit(std::function<void(TfEditCookie)> value):run(std::move(value)){}
    HRESULT __stdcall DoEditSession(TfEditCookie cookie) noexcept override{
        try{run(cookie);return S_OK;}catch(...){return to_hresult();}
    }
};
struct CompositionSink:implements<CompositionSink,ITfCompositionSink>{
    using Trace=std::function<void(hstring,HRESULT,uintptr_t)>;
    Trace trace;bool terminated=false;
    explicit CompositionSink(Trace observer):trace(std::move(observer)){}
    HRESULT __stdcall OnCompositionTerminated(TfEditCookie cookie,ITfComposition* value) noexcept override{
        terminated=true;
        try{trace(L"ITfCompositionSink::OnCompositionTerminated",S_OK,reinterpret_cast<uintptr_t>(value));trace(L"CompositionTerminationCookie",S_OK,uintptr_t(cookie));return S_OK;}catch(...){return to_hresult();}
    }
};
struct ScopedComposition{
    using Trace=CompositionSink::Trace;
    TextServicesLibrary const& library;Trace trace;
    com_ptr<ITfThreadMgr> manager;com_ptr<ITfContext> context;com_ptr<ITfComposition> composition;com_ptr<CompositionSink> sink;
    TfClientId client=TF_CLIENTID_NULL;bool activated=false;std::map<std::wstring,std::pair<HRESULT,uintptr_t>> observations;unsigned probes=0;
    ScopedComposition(TextServicesLibrary const& services,Trace observer):library(services),trace(std::move(observer)),sink(make_self<CompositionSink>(trace)){
        auto loaded=library.module?S_OK:HRESULT_FROM_WIN32(library.loadError?library.loadError:ERROR_MOD_NOT_FOUND);
        checked(L"LoadLibraryExW(msctf,system32)",loaded,reinterpret_cast<uintptr_t>(library.module));
        auto found=library.current?S_OK:HRESULT_FROM_WIN32(library.lookupError?library.lookupError:ERROR_PROC_NOT_FOUND);
        checked(L"GetProcAddress(TF_GetThreadMgr)",found,reinterpret_cast<uintptr_t>(library.current));
    }
    ScopedComposition(ScopedComposition const&)=delete;
    ScopedComposition& operator=(ScopedComposition const&)=delete;
    void record(wchar_t const* api,HRESULT result,uintptr_t pointer=0){
        auto value=std::pair{result,pointer};auto found=observations.find(api);
        if(found==observations.end()||found->second!=value){observations[api]=value;trace(api,result,pointer);}
    }
    void checked(wchar_t const* api,HRESULT result,uintptr_t pointer=0){record(api,result,pointer);if(FAILED(result))throw hresult_error(result,api);}
    bool focused(TextBox const& entry,XamlRoot const& root){
        ++probes;auto focus=FocusManager::GetFocusedElement(root);
        record(L"XamlFocusedTextBox",focus==entry?S_OK:S_FALSE,reinterpret_cast<uintptr_t>(get_abi(focus)));
        if(focus!=entry)return false;
        if(!manager){
            auto result=library.current(manager.put());checked(L"TF_GetThreadMgr",result,reinterpret_cast<uintptr_t>(manager.get()));
            if(!manager){result=CoCreateInstance(CLSID_TF_ThreadMgr,nullptr,CLSCTX_INPROC_SERVER,__uuidof(ITfThreadMgr),manager.put_void());checked(L"CoCreateInstance(CLSID_TF_ThreadMgr)",result,reinterpret_cast<uintptr_t>(manager.get()));}
            if(!manager)return false;
            result=manager->Activate(&client);activated=SUCCEEDED(result);checked(L"ITfThreadMgr::Activate",result,uintptr_t(client));
        }
        BOOL threadFocus=FALSE;auto threadResult=manager->IsThreadFocus(&threadFocus);checked(L"ITfThreadMgr::IsThreadFocus",threadResult,uintptr_t(threadFocus));
        com_ptr<ITfDocumentMgr> document;auto result=manager->GetFocus(document.put());
        checked(L"ITfThreadMgr::GetFocus",result,reinterpret_cast<uintptr_t>(document.get()));
        if(result==S_FALSE||!document)return false;
        com_ptr<ITfContext> candidate;result=document->GetTop(candidate.put());checked(L"ITfDocumentMgr::GetTop",result,reinterpret_cast<uintptr_t>(candidate.get()));
        if(result==S_FALSE||!candidate||!threadFocus)return false;
        context=std::move(candidate);return true;
    }
    void edit(wchar_t const* phase,std::function<void(TfEditCookie)> run){
        auto session=make_self<CompositionEdit>(std::move(run));HRESULT result=E_FAIL;
        auto request=context->RequestEditSession(client,session.get(),TF_ES_SYNC|TF_ES_READWRITE,&result);
        checked((hstring(L"ITfContext::RequestEditSession/")+phase).c_str(),request,reinterpret_cast<uintptr_t>(context.get()));
        checked((hstring(L"ITfEditSession::DoEditSession/")+phase).c_str(),result,reinterpret_cast<uintptr_t>(context.get()));
    }
    void begin(hstring text=L"\u5019\u88dc"){
        edit(L"begin",[this,text](TfEditCookie cookie){
            TF_SELECTION selection{};ULONG count=0;auto result=context->GetSelection(cookie,TF_DEFAULT_SELECTION,1,&selection,&count);
            com_ptr<ITfRange> range;range.attach(selection.range);checked(L"ITfContext::GetSelection",result,reinterpret_cast<uintptr_t>(range.get()));
            checked(L"SelectionRange",count==1&&range?S_OK:E_UNEXPECTED,uintptr_t(count));
            com_ptr<ITfContextComposition> compositions;result=context->QueryInterface(__uuidof(ITfContextComposition),compositions.put_void());checked(L"QueryInterface(ITfContextComposition)",result,reinterpret_cast<uintptr_t>(compositions.get()));
            record(L"StartCompositionSink",S_OK,reinterpret_cast<uintptr_t>(static_cast<ITfCompositionSink*>(sink.get())));
            result=compositions->StartComposition(cookie,range.get(),sink.get(),composition.put());
            checked(L"ITfContextComposition::StartComposition",result,reinterpret_cast<uintptr_t>(composition.get()));
            checked(L"StartedComposition",composition?S_OK:E_UNEXPECTED,reinterpret_cast<uintptr_t>(composition.get()));
            checked(L"ITfRange::SetText",range->SetText(cookie,0,text.c_str(),LONG(text.size())),reinterpret_cast<uintptr_t>(range.get()));
        });
    }
    void end(){if(composition){if(!sink->terminated)edit(L"end",[this](TfEditCookie cookie){if(!sink->terminated)checked(L"ITfComposition::EndComposition",composition->EndComposition(cookie));});composition=nullptr;}}
    void deactivate(){if(activated){activated=false;client=TF_CLIENTID_NULL;checked(L"ITfThreadMgr::Deactivate",manager->Deactivate());}}
    void finish(){end();deactivate();}
    ~ScopedComposition(){try{end();}catch(...){}try{deactivate();}catch(...){}}
};

int resultCode=0;
struct Fixture:std::enable_shared_from_this<Fixture> {
    Window window,other;StackPanel root;Button anchor,untagged;
    std::shared_ptr<WorkspaceData> data=std::make_shared<WorkspaceData>();
    ControlContext context;std::shared_ptr<TextServicesLibrary> textServices;unsigned imeChecks=0;std::shared_ptr<std::vector<J>> imeTrace=std::make_shared<std::vector<J>>();
    bool windowClosed=false;OwnedPopup owner;std::optional<OwnedPopup::Ticket> origin;std::vector<MenuFlyout> popups;HWND hwnd=nullptr;unsigned checks=0,keys=0,presses=0,blurs=0,opened=0,closedCount=0,originPresses=0;
    std::filesystem::path output;hstring phase;
    winrt::apartment_context apartment;
    static void require(bool value,wchar_t const* text){if(!value)throw hresult_error(E_FAIL,text);}
    static HWND handle(Window const& value){HWND result=nullptr;check_hresult(value.as<IWindowNative>()->get_WindowHandle(&result));return result;}
    void privacy(){
        wchar_t expected[256],actual[256];DWORD bytes=0;
        auto length=GetEnvironmentVariableW(L"CAPY_PRIVATE_DESKTOP",expected,256);
        require(length&&length<256,L"Private desktop name required");
        check_bool(GetUserObjectInformationW(GetThreadDesktop(GetCurrentThreadId()),UOI_NAME,actual,sizeof actual,&bytes));
        require(std::wstring(expected)==actual,L"Fixture is not on its named private desktop");
        DWORD pid=0;GetWindowThreadProcessId(GetForegroundWindow(),&pid);
        require(pid==GetCurrentProcessId()&&GetForegroundWindow()==hwnd,L"Fixture does not own foreground input");
    }
    void nativePress(FrameworkElement const& target,bool right=false,std::optional<Point> local={}){
        privacy();auto position=target.TransformToVisual(nullptr).TransformPoint(local.value_or(Point{float(target.ActualWidth()/2),float(target.ActualHeight()/2)}));
        bool hit=false;for(auto const& element:VisualTreeHelper::FindElementsInHostCoordinates(position,root))if(element==target)hit=true;
        require(hit,L"Native point does not hit the intended XAML source");
        auto scale=root.XamlRoot().RasterizationScale();POINT point{LONG(position.X*scale),LONG(position.Y*scale)};check_bool(ClientToScreen(hwnd,&point));
        require(GetAncestor(WindowFromPoint(point),GA_ROOT)==hwnd,L"Native press target is outside owned window");
        check_bool(SetCursorPos(point.x,point.y));INPUT input[2]{};input[0].type=input[1].type=INPUT_MOUSE;
        input[0].mi.dwFlags=right?MOUSEEVENTF_RIGHTDOWN:MOUSEEVENTF_LEFTDOWN;input[1].mi.dwFlags=right?MOUSEEVENTF_RIGHTUP:MOUSEEVENTF_LEFTUP;
        require(SendInput(2,input,sizeof(INPUT))==2,L"Native pointer injection failed");
    }
    winrt::Windows::Foundation::IAsyncAction settle(){co_await winrt::resume_after(std::chrono::milliseconds(120));co_await apartment;}
    winrt::Windows::Foundation::IAsyncAction wait(std::function<bool()> ready,wchar_t const* message){
        auto deadline=std::chrono::steady_clock::now()+std::chrono::seconds(5);
        while(!ready()){require(std::chrono::steady_clock::now()<deadline,message);co_await winrt::resume_after(std::chrono::milliseconds(10));co_await apartment;}
    }
    MenuFlyout menu(){MenuFlyout popup;MenuFlyoutItem item;item.Text(L"Fixture action");popup.Items().Append(item);popup.Opened([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock())++self->opened;});popup.Closed([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock())++self->closedCount;});popups.push_back(popup);return popup;}
    bool visiblePopup(){for(auto const& popup:VisualTreeHelper::GetOpenPopupsForXamlRoot(root.XamlRoot()))if(popup.IsOpen())return true;return false;}
    void pending(){require(owner.Pending()&&!owner.Open()&&data->transients.size()==1,L"Pending popup lacks immediate unique cancellation registration");}
    void empty(){require(!owner.Active()&&!owner.Pending()&&!owner.Open()&&data->transients.empty()&&data->popupCount==0,L"Popup cancellation left retained state or registrations");}
    winrt::Windows::Foundation::IAsyncAction colorStripPresentation(){
        phase=L"color-strip-setup";
        try{
        auto source=std::make_shared<WorkspaceData>();context.apply(source);
        auto opened=colorUi(source->localization.get(),O({{L"type",S(L"editor_open")},{L"slot",S(L"foreground")}})).GetObject();
        require(object(opened,L"editor").Size()&&object(opened,L"view").Size()&&opened.GetNamedValue(L"error",JsonValue::CreateNullValue()).ValueType()==JsonValueType::Null,L"Color strip editor_open setup failed");
        auto edited=colorUi(source->localization.get(),O({{L"type",S(L"editor")},{L"editor",object(opened,L"editor")},{L"action",O({{L"op",S(L"text")},{L"text",S(L"#000000")}})}})).GetObject();
        require(object(edited,L"editor").Size()&&object(edited,L"view").Size()&&edited.GetNamedValue(L"error",JsonValue::CreateNullValue()).ValueType()==JsonValueType::Null,L"Color strip edited draft setup failed");
        auto editor=object(edited,L"editor"),original=object(object(edited,L"view"),L"value");
        require(array(original,L"rgba").Size()==4&&array(original,L"rgba").GetNumberAt(0)==0&&array(original,L"rgba").GetNumberAt(1)==0&&array(original,L"rgba").GetNumberAt(2)==0&&array(original,L"rgba").GetNumberAt(3)==1,L"Color strip setup did not produce the black editor draft");
        auto sample=J::Parse(LR"({"space":"Srgb","rgba":[0.95686274766922,0.9450980424880981,0.9098039865493774,1]})");
        auto shown=colorUi(source->localization.get(),O({{L"type",S(L"editor_strip")},{L"editor",editor},{L"sample",sample}})).GetObject();
        require(shown.GetNamedValue(L"hex").ValueType()==JsonValueType::String&&shown.GetNamedValue(L"label").ValueType()==JsonValueType::String&&array(shown,L"values").Size()==3,L"Color strip shared presentation setup failed");
        A colors;colors.Append(original);colors.Append(sample);
        auto previews=colorUi(source->localization.get(),O({{L"type",S(L"preview")},{L"colors",colors}})).GetArray();
        require(previews.Size()==2&&array(previews.GetObjectAt(0),L"rgba").Size()==4&&array(previews.GetObjectAt(1),L"rgba").Size()==4,L"Color strip display preview setup failed");
        std::wstring values;for(auto value:array(shown,L"values")){if(!values.empty())values+=L"  ";values+=value.GetString().c_str();}
        auto intensity=shown.GetNamedValue(L"intensity",JsonValue::CreateNullValue());
        auto content=O({{L"original",array(previews.GetObjectAt(0),L"rgba")},{L"sample",array(previews.GetObjectAt(1),L"rgba")},{L"hex",S(str(shown,L"hex"))},{L"intensity",S(intensity.ValueType()==JsonValueType::String?intensity.GetString():hstring{})},{L"label",S(str(shown,L"label"))},{L"values",S(hstring(values))}});
        auto paint=J::Parse(object(object(opened,L"editor"),L"picker").Stringify());paint.Insert(L"temporary",original);
        require(paint.Size()&&paint.HasKey(L"foreground")&&paint.HasKey(L"background"),L"Color strip paint state setup failed");
        source->state=O({{L"theme",S(root.RequestedTheme()==ElementTheme::Light?L"light":L"dark")},{L"colors",paint}});
        auto picker=J::Parse(LR"({"calibrating":false,"style":"glass","layer":false,"sample_width":1,"can_sample_layer":true,"sample_sizes":[1,5,15,51,101],"editor":true,"picked":null,"sample_point":[400,200],"loupe_point":[400,200]})");picker.Insert(L"preview",sample);
        source->colorPreview=O({{L"picker",picker}});
        unsigned dispatches=0;source->send=[&](std::string){++dispatches;};
        auto savedColors=object(source->state,L"colors").Stringify(),savedPicker=source->colorPreview.Stringify();
        Canvas surface;surface.Width(380);surface.Height(160);root.Children().InsertAt(0,surface);
        auto strip=std::make_shared<ColorStrip>();strip->data=source;strip->init(surface);
        auto cleanup=[&]{uint32_t at;if(root.Children().IndexOf(surface,at))root.Children().RemoveAt(at);};
        auto camera=[&]{auto scale=surface.XamlRoot().RasterizationScale();double width=surface.ActualWidth()*scale,height=surface.ActualHeight()*scale;A viewport,area;viewport.Append(N(width));viewport.Append(N(height));for(double value:{0.,0.,width,height})area.Append(N(value));source->state.Insert(L"camera",O({{L"viewport",viewport},{L"work_area",area}}));};
        auto equivalent=[](Geometry const& first,Geometry const& second){
            auto a=first.as<PathGeometry>(),b=second.as<PathGeometry>();if(a==b||a.Figures().Size()!=1||b.Figures().Size()!=1)return false;
            auto x=a.Figures().GetAt(0),y=b.Figures().GetAt(0);if(x.StartPoint().X!=y.StartPoint().X||x.StartPoint().Y!=y.StartPoint().Y||x.IsClosed()!=y.IsClosed()||x.IsFilled()!=y.IsFilled()||x.Segments().Size()!=1||y.Segments().Size()!=1)return false;
            auto p=x.Segments().GetAt(0).as<PolyLineSegment>().Points(),q=y.Segments().GetAt(0).as<PolyLineSegment>().Points();if(p.Size()!=q.Size()||!p.Size())return false;
            for(uint32_t i=0;i<p.Size();++i)if(p.GetAt(i).X!=q.GetAt(i).X||p.GetAt(i).Y!=q.GetAt(i).Y)return false;return true;
        };
        auto verify=[&]{
            require(strip->root.IsLoaded()&&strip->root.IsEnabled()&&strip->root.IsHitTestVisible()&&strip->root.Opacity()==1&&strip->root.ActualWidth()>0&&strip->root.ActualHeight()>0,L"Color picker strip did not remain enabled and arranged");
            require(equivalent(strip->surface.Data(),strip->feedbackSurface.Data()),L"Color picker strip paths did not retain equivalent separately owned geometry");
            require(strip->hex.Text()==str(content,L"hex")&&strip->intensity.Text()==str(content,L"intensity")&&strip->space.Text()==str(content,L"label")&&strip->values.Text()==str(content,L"values"),L"Color picker strip changed its shared presentation text");
            require(strip->original.Background().as<SolidColorBrush>().Color()==Windows::UI::Color{255,0,0,0}&&strip->sample.Background().as<SolidColorBrush>().Color()==Windows::UI::Color{255,244,241,232},L"Color picker strip changed its original or sampled paint");
            require(object(source->state,L"colors").Stringify()==savedColors&&source->colorPreview.Stringify()==savedPicker&&!dispatches,L"Color picker strip presentation changed paint or picker policy");
        };
        std::exception_ptr failure;
        try{
            co_await wait([&]{return surface.IsLoaded()&&surface.ActualWidth()==380&&surface.ActualHeight()==160;},L"Color picker strip host did not arrange");camera();
            phase=L"color-strip-presentation";strip->Show(content);
            co_await wait([&]{return strip->root.IsLoaded()&&strip->root.ActualWidth()>0&&strip->root.ActualHeight()>0;},L"Color picker strip did not arrange after Show");verify();
            strip->present();verify();strip->present();verify();
            strip->hover=Point{float(Canvas::GetLeft(strip->root)+20),float(Canvas::GetTop(strip->root)+strip->root.ActualHeight()/2)};strip->present();verify();
            surface.Width(360);surface.Height(170);co_await wait([&]{return surface.ActualWidth()==360&&surface.ActualHeight()==170;},L"Color picker strip host did not acknowledge resize");camera();strip->present();verify();
            strip->hover.reset();strip->present();verify();++checks;
        }catch(...){failure=std::current_exception();}
        cleanup();if(failure)std::rethrow_exception(failure);
        }catch(hresult_error const& error){
            wchar_t code[16]{};swprintf_s(code,L" [0x%08X]",uint32_t(error.code().value));
            throw hresult_error(error.code(),hstring(std::wstring(L"Color strip component: ")+error.message().c_str()+code));
        }
    }
    winrt::Windows::Foundation::IAsyncAction headerContextOrdering(){
        auto source=std::make_shared<WorkspaceData>();context.apply(source);
        A choices,section;section.Append(O({{L"label",S(L"Fixture workspace")},{L"action",O({{L"type",S(L"workspace_manager")},{L"command",O({{L"type",S(L"show_in_switcher")},{L"id",S(L"fixture-header")}})}})}}));choices.Append(section);
        A zones,items;for(uint32_t id:{1u,2u})items.Append(O({{L"id",N(id)},{L"item",O({{L"kind",S(L"workspaces")}})}}));zones.Append(items);
        auto header=O({{L"editing",B(true)},{L"zones",zones}});
        source->state=O({{L"theme",S(root.RequestedTheme()==ElementTheme::Light?L"light":L"dark")}});
        source->model=O({{L"header",header},{L"state",source->state},{L"windows_workspace",O({{L"switcher_options",O({{L"sections",choices}})}})}});
        unsigned queries=0,nativeOpened=0,nativeClosed=0;source->popupChanged=[&](bool open){if(open)++nativeOpened;else ++nativeClosed;};source->query=[&](CanvasQueryKind,std::string,PreviewReply){++queries;return false;};source->send=[](std::string){};
        Canvas surface;surface.Width(380);surface.Height(130);surface.Background(clear());Button sources[2],editors[2];
        for(uint32_t index=0;index<2;++index){
            sources[index].Content(box_value(L"Workspace source "+to_hstring(index+1)));editors[index].Content(box_value(L"Workspace editor "+to_hstring(index+1)));
            for(auto const& control:{sources[index],editors[index]}){control.Width(160);control.Height(34);Canvas::SetLeft(control,float(10+index*190));surface.Children().Append(control);}
            Canvas::SetTop(sources[index],8);Canvas::SetTop(editors[index],70);
        }
        uint32_t focusedItem=0;std::unique_ptr<HeaderInput> input;A history;
        auto record=[&](hstring const& step){unsigned popups=0;for(auto const& popup:VisualTreeHelper::GetOpenPopupsForXamlRoot(root.XamlRoot()))if(popup.IsOpen())++popups;history.Append(O({{L"step",S(step)},{L"selected",N(input?input->Selected():0)},{L"focused_item",N(focusedItem)},{L"popup_count",N(source->popupCount)},{L"registrations",N(double(source->transients.size()))},{L"native_popups",N(popups)},{L"native_opened",N(nativeOpened)},{L"native_closed",N(nativeClosed)}}));};
        input=std::make_unique<HeaderInput>(source,surface,[&]{
            if(input&&input->Selected()!=focusedItem){record(L"before-editor-focus");focusedItem=input->Selected();if(focusedItem>=1&&focusedItem<=2)editors[focusedItem-1].Focus(FocusState::Programmatic);record(L"after-editor-focus");}
        });
        for(uint32_t index=0;index<2;++index)input->Source(sources[index],O({{L"kind",S(L"item")},{L"value",N(index+1)}}),L"Workspace source");
        input->Configure(header,true,J{});root.Children().InsertAt(0,surface);
        auto cleanup=[&]{if(input)input->Cancel();uint32_t at;if(root.Children().IndexOf(surface,at))root.Children().RemoveAt(at);input.reset();};
        auto shown=[&]{auto command=workspaceElement(L"workspace-switcher-show-fixture-header");return source->popupCount==1&&visiblePopup()&&command&&command.IsLoaded()&&command.ActualWidth()>0;};
        std::exception_ptr failure;
        try{
            co_await wait([&]{return sources[0].IsLoaded()&&sources[1].ActualWidth()==160&&editors[0].IsLoaded()&&editors[1].ActualWidth()==160;},L"Header context sources and editors did not arrange");
            input->Select(1);co_await wait([&]{return focusedItem==1&&FocusManager::GetFocusedElement(root.XamlRoot())==editors[0];},L"Header context initial editor did not take native focus");privacy();
            input->Context(2,sources[1]);record(L"direct-return");
            co_await wait(shown,L"Header direct context closed before native Opened after editor selection");co_await settle();
            require(shown()&&input->Selected()==2&&focusedItem==2&&queries==0,L"Header direct context lost its native menu or selected editor");
            input->Cancel();co_await wait([&]{return source->popupCount==0&&source->transients.empty()&&!visiblePopup();},L"Header direct context did not acknowledge native Closed");
            input->Select(1);co_await wait([&]{return focusedItem==1&&FocusManager::GetFocusedElement(root.XamlRoot())==editors[0];},L"Header right-click initial editor did not take native focus");
            nativePress(sources[1],true);
            co_await wait(shown,L"Header ContextRequested closed before native Opened after editor selection");co_await settle();
            require(shown()&&input->Selected()==2&&focusedItem==2&&queries==0,L"Header ContextRequested lost its native menu or selected editor");
            input->Cancel();co_await wait([&]{return source->popupCount==0&&source->transients.empty()&&!visiblePopup();},L"Header ContextRequested did not acknowledge native Closed");
            ++checks;
        }catch(...){failure=std::current_exception();}
        record(L"before-cleanup");cleanup();auto evidence=to_string(history.Stringify());auto path=output.parent_path()/L"header-context-ordering.json";
        co_await resume_background();{std::ofstream file(path);file<<evidence;if(!file&&!failure)failure=std::make_exception_ptr(hresult_error(E_FAIL,L"Header context native evidence was not saved"));}co_await apartment;
        if(failure)std::rethrow_exception(failure);
    }
    winrt::Windows::Foundation::IAsyncAction ordinaryOpening(){
        struct Observation{unsigned opening=0,opened=0,closed=0;bool cancelOpening=true;hstring error;};
        auto observed=std::make_shared<Observation>();MenuFlyout ordinary;MenuFlyoutItem item;item.Text(L"Ordinary popup action");ordinary.Items().Append(item);
        TrackPopup(ordinary,data);auto weak=weak_from_this();
        ordinary.Opening([weak,observed](auto&&,auto&&){if(auto self=weak.lock())try{
            ++observed->opening;
            require(self->data->transients.size()==1&&self->data->popupCount==0,L"Ordinary popup is not registered before native Opened");
            if(observed->cancelOpening){
                require(self->data->dismissTransients(),L"Ordinary popup ignored cancellation during native Opening");
                require(!self->data->dismissTransients(),L"Repeated opening cancellation retained a registration");
            }
        }catch(hresult_error const& e){observed->error=e.message();}catch(...){observed->error=L"Ordinary popup Opening callback failed";}});
        ordinary.Opened([observed](auto&&,auto&&){++observed->opened;});
        ordinary.Closed([observed](auto&&,auto&&){++observed->closed;});
        require(data->transients.empty()&&data->popupCount==0,L"Ordinary popup opening inherited another popup lifetime");
        ordinary.ShowAt(anchor);co_await settle();require(observed->error.empty(),observed->error.c_str());
        require(observed->opening==1&&!ordinary.IsOpen()&&!visiblePopup(),L"Canceled ordinary Opening left a visible or late native popup");
        require(!data->dismissTransients(),L"Late ordinary Opened resurrected cancellation registration");empty();
        observed->cancelOpening=false;auto beforeOpened=observed->opened;auto beforeClosed=observed->closed;
        ordinary.ShowAt(anchor);
        co_await wait([this,observed,beforeOpened]{return observed->opened==beforeOpened+1&&data->popupCount==1&&visiblePopup();},L"Same ordinary flyout did not reopen with fresh lifetime");
        require(observed->error.empty(),observed->error.c_str());
        require(observed->opening==2&&data->transients.size()==1,L"Reopened ordinary flyout lacks exactly one fresh registration");
        require(data->dismissTransients(),L"Reopened ordinary flyout ignored cancellation");
        require(!data->dismissTransients(),L"Repeated visible ordinary cancellation retained a registration");
        co_await wait([this,observed,beforeClosed]{return observed->closed==beforeClosed+1&&!visiblePopup();},L"Reopened ordinary flyout did not acknowledge native Closed");
        co_await settle();require(!ordinary.IsOpen(),L"Ordinary flyout reopened after cancellation");empty();
    }
    winrt::Windows::Foundation::IAsyncAction ordinaryClosing(){
        struct Observation{bool cancel=true,seen=false;int count=-1;};
        auto observed=std::make_shared<Observation>();auto blocker=std::make_shared<WorkspaceData>();context.apply(blocker);
        blocker->popupChanged=[data=data](bool open){data->externalPopup=open;};
        MenuFlyout ordinary;MenuFlyoutItem item;item.Text(L"Ordinary popup action");ordinary.Items().Append(item);TrackPopup(ordinary,blocker);
        ordinary.Closing([weak=weak_from_this(),observed,blocker](auto const& sender,auto const& args){
            args.Cancel(observed->cancel);
            sender.DispatcherQueue().TryEnqueue([weak,observed,blocker]{if(weak.lock()){observed->count=blocker->popupCount;observed->seen=true;}});
        });
        ordinary.ShowAt(anchor);co_await wait([this,blocker]{return blocker->popupCount==1&&data->externalPopup;},L"Ordinary popup did not block foreign admission");
        ordinary.Hide();co_await wait([observed]{return observed->seen;},L"Canceled Closing did not finish event dispatch");
        require(observed->count==1&&data->externalPopup&&ordinary.IsOpen(),L"Canceled Closing retired popup admission");
        observed->cancel=false;observed->seen=false;ordinary.Hide();
        co_await wait([observed]{return observed->seen;},L"Accepted Closing did not finish event dispatch");
        require(observed->count==0&&!data->externalPopup,L"Accepted Closing blocked the next native menu until Closed");
        auto next=owner.Begin(data,anchor,menu());require(next.Current(),L"Closing popup rejected the next source request");next.Cancel();empty();
        co_await wait([this,&ordinary]{return !ordinary.IsOpen()&&!visiblePopup();},L"Ordinary popup did not finish closing");
        co_await wait([blocker]{return blocker->transients.empty();},L"Closed ordinary popup retained cancellation registration");
    }
    static FrameworkElement findElement(DependencyObject const& parent,hstring const& id){
        if(!parent)return nullptr;
        if(auto node=parent.try_as<FrameworkElement>();node&&(AutomationProperties::GetAutomationId(node)==id||node.Name()==id))return node;
        for(int i=0;i<VisualTreeHelper::GetChildrenCount(parent);++i)if(auto node=findElement(VisualTreeHelper::GetChild(parent,i),id))return node;
        return nullptr;
    }
    FrameworkElement workspaceElement(hstring const& id){
        for(auto const& popup:VisualTreeHelper::GetOpenPopupsForXamlRoot(root.XamlRoot()))if(auto node=findElement(popup.Child(),id))return node;
        return findElement(root,id);
    }
    winrt::Windows::Foundation::IAsyncAction tiltRows(J const& model,J const& catalog){
        auto source=std::make_shared<WorkspaceData>();context.apply(source,catalog);source->model=model;source->state=object(model,L"state");source->refreshPalette();source->send=[](std::string){};
        require(object(source->state,L"tilt_calibration").Size()!=0,L"Tilt row fixture needs shared calibration presentation");
        Canvas host;host.Width(1000);host.Height(700);auto previousSize=window.AppWindow().Size();auto width=root.Width(),height=root.Height();
        auto calibration=std::make_shared<ResponseCalibration>();calibration->data=source;calibration->init(host);
        root.Width(1000);root.Height(780);root.Children().InsertAt(0,host);window.AppWindow().Resize({1100,900});
        auto cleanup=[&]{uint32_t at;if(root.Children().IndexOf(host,at))root.Children().RemoveAt(at);calibration.reset();root.Width(width);root.Height(height);window.AppWindow().Resize(previousSize);};
        try{
            co_await wait([host]{return host.IsLoaded()&&host.ActualWidth()==1000&&host.ActualHeight()==700;},L"Tilt native row host did not arrange");
            auto font=calibration->sensitivityTitle.FontSize();
            auto bounds=[host](FrameworkElement const& item){return item.TransformToVisual(host).TransformBounds({0,0,float(item.ActualWidth()),float(item.ActualHeight())});};
            for(bool wrapped:{false,true}){
                for(auto title:{calibration->sensitivityTitle,calibration->directionTitle})title.FontSize(font*(wrapped?1.5:1.));
                calibration->Publish();host.UpdateLayout();
                co_await wait([calibration,wrapped]{
                    if(!calibration->frame.IsLoaded()||calibration->firmer.ActualWidth()<=0)return false;
                    for(auto title:{calibration->sensitivityTitle,calibration->directionTitle})if(wrapped?title.ActualHeight()<=36:title.ActualHeight()<=0||title.ActualHeight()>36)return false;
                    return true;
                },L"Tilt native captions did not arrange in their natural or wrapped rows");
                auto first=bounds(calibration->firmer),last=bounds(calibration->lighter);auto plot=findElement(host,L"pen-tilt-curve");require(bool(plot),L"Tilt native row fixture lost its actual response graph");auto graph=bounds(plot);
                require(std::abs(calibration->frame.ActualWidth()-384)<=1,L"Tilt row fixture lost its minimum-width utility");
                require(std::abs(first.Y-last.Y)<=1&&first.Y>=graph.Y+graph.Height,L"Tilt sensitivity controls left their row below the graph");
                for(auto item:{calibration->firmer,calibration->lighter}){auto box=bounds(item);require(std::abs(box.Width-36)<=1&&std::abs(box.Height-36)<=1,L"Tilt sensitivity row lost square tile targets");}
                for(auto axis:calibration->axes){auto box=bounds(axis);require(std::abs(box.Width-36)<=1&&std::abs(box.Height-36)<=1,L"Tilt direction row lost square tile targets");
                    require(std::abs(box.Y-first.Y)<=1&&box.X>last.X+last.Width,(L"Tilt "+hstring(wrapped?L"wrapped":L"natural")+L" direction row is not aligned with sensitivity: delta="+to_hstring(box.Y-first.Y)+L" sensitivity-caption-height="+to_hstring(calibration->sensitivityTitle.ActualHeight())+L" direction-caption-height="+to_hstring(calibration->directionTitle.ActualHeight())+L" sensitivity-y/height="+to_hstring(first.Y)+L"/"+to_hstring(first.Height)+L" axis-y/height="+to_hstring(box.Y)+L"/"+to_hstring(box.Height)+L" controls-y/height="+to_hstring(bounds(calibration->controls).Y)+L"/"+to_hstring(bounds(calibration->controls).Height)).c_str());
                }
            }
            cleanup();
        }catch(...){cleanup();throw;}
    }
    winrt::Windows::Foundation::IAsyncAction gradientContacts(J const& model,J const& catalog,J const& control){
        auto source=std::make_shared<WorkspaceData>();context.apply(source,catalog);source->model=J::Parse(model.Stringify());source->state=object(source->model,L"state");source->refreshPalette();
        auto edits=std::make_shared<std::vector<J>>();Bindings fields;
        auto editor=CapyEffects::GradientEditor(source,{[control]{return control;},[edits](J action,hstring gesturePhase){edits->push_back(O({{L"phase",S(gesturePhase)},{L"action",action}}));},[]{return true;},L"fixture-gradient"},fields);
        editor.Width(300);root.Children().InsertAt(0,editor);for(auto const& bind:fields)bind();
        auto focus=findElement(editor,L"fixture-gradient-gradient");
        co_await wait([focus]{return focus&&focus.IsLoaded()&&focus.ActualWidth()>12&&focus.ActualHeight()>=44;},L"Production gradient strip did not arrange");
        for(bool hostBackground:{true,false}){
            focus.as<ContentControl>().Background(hostBackground?clear():nullptr);
            for(int index=0;index<2;++index){
                auto begin=edits->size();nativePress(focus,false,Point{float(index?focus.ActualWidth()-6:6),39});
                co_await wait([edits,begin]{return edits->size()>=begin+3&&str(edits->back(),L"phase")==L"up";},L"Gradient endpoint contact did not complete its native gesture");
                for(size_t i=begin;i<edits->size();++i){
                    auto event=edits->at(i);auto gesturePhase=str(event,L"phase");
                    require(num(object(object(event,L"action"),L"edit"),L"index",-1)==index,L"Gradient endpoint contact selected the wrong stop");
                    require(i==begin?gesturePhase==L"down":i+1==edits->size()?gesturePhase==L"up":gesturePhase==L"move",L"Gradient endpoint contact did not preserve its native down/move/up sequence");
                }
            }
        }
        anchor.Focus(FocusState::Programmatic);root.Children().RemoveAt(0);fields.clear();
    }
    winrt::Windows::Foundation::IAsyncAction canvasBarLanguage(J const& model,J const& locale){
        auto source=std::make_shared<WorkspaceData>();context.apply(source,object(object(model,L"localization"),L"catalog"));
        source->model=J::Parse(model.Stringify());source->state=J::Parse(object(model,L"state").Stringify());source->refreshPalette();
        auto authored=source->state.Stringify();auto measurements=std::make_shared<std::vector<J>>();
        source->send=[measurements](std::string value){auto action=J::Parse(to_hstring(value));require(str(action,L"type")==L"measure_canvas_bar",L"Canvas-bar localization dispatched a non-measurement action");measurements->push_back(object(action,L"measure"));};
        source->query=[](CanvasQueryKind,std::string,PreviewReply){throw hresult_error(E_FAIL,L"Canvas-bar localization queried a renderer worker");return false;};
        auto state=[&]{
            auto apply=source->common(L"apply"),cancel=source->common(L"cancel");
            auto command=O({{L"id",S(L"fixture_action")},{L"label",S(apply)},{L"tooltip",S(cancel)},{L"enabled",B(true)}});
            auto action=O({{L"label",S(apply)},{L"option",O({{L"Action",O({{L"state",command}})}})}});
            auto menu=O({{L"menu",S(L"fixture_menu")},{L"label",S(cancel)}});
            A choices;choices.Append(O({{L"label",S(apply)},{L"selected",B(false)}}));choices.Append(O({{L"label",S(cancel)},{L"selected",B(true)}}));
            auto choice=[&](hstring const& id,bool segmented){return O({{L"option",O({{L"Choice",O({{L"id",S(id)},{L"label",S(apply)},{L"segmented",B(segmented)},{L"items",choices}})}})}});};
            A items;items.Append(action);items.Append(menu);items.Append(choice(L"fixture_segments",true));items.Append(choice(L"fixture_dropdown",false));
            return O({{L"canvas_bar",O({{L"context",O({{L"kind",S(L"fixture")},{L"generation",N(1)}})},{L"label",S(apply)},{L"items",items}})}});
        };
        Canvas surface;surface.Width(1000);surface.Height(180);root.Width(1000);root.Height(240);root.Children().InsertAt(0,surface);
        auto bar=std::make_shared<CanvasActionBar>();bar->data=source;bar->init(surface);auto english=state();bar->Apply(english);
        auto placement=O({{L"bounds",O({{L"x",N(0)},{L"y",N(0)},{L"width",N(1000)},{L"height",N(96)}})},{L"items",N(4)}});
        require(bar->pending&&measurements->size()==1,L"Initial native canvas-bar measurement missing");
        auto publication=[&](J const& state){bar->Published(O({{L"context",object(object(state,L"canvas_bar"),L"context")},{L"hold",N(0)},{L"layout",placement}}));};
        publication(english);
        co_await wait([&]{return bar->more.IsLoaded()&&bar->more.ActualWidth()>0&&bar->row.IsLoaded();},L"Canvas-bar native row did not arrange");
        auto row=bar->row;auto more=bar->more;auto caption=bar->caption;auto fields=bar->fields;
        std::vector<FrameworkElement> peers;for(auto id:{L"canvas-bar-fixture_action",L"canvas-bar-menu-fixture_menu",L"canvas-bar-choice-fixture_segments",L"canvas-bar-choice-fixture_segments-0",L"canvas-bar-choice-fixture_segments-1",L"canvas-bar-choice-fixture_dropdown"}){auto peer=findElement(row,id);require(bool(peer),L"Canvas-bar native field peer missing");peers.push_back(peer);}
        MenuFlyout popup;MenuFlyoutItem focused;focused.Text(L"Native keyboard focus");popup.Items().Append(focused);auto lifetime=bar->menuOwner.Begin(source,more,popup);
        require(lifetime.Show(more,{}),L"Canvas-bar More did not open its owned native menu");co_await wait([&]{return bar->menuOwner.Open()&&focused.IsLoaded()&&focused.ActualWidth()>0;},L"Canvas-bar menu did not arrange");
        require(focused.Focus(FocusState::Keyboard),L"Canvas-bar menu native focus prerequisite failed");co_await wait([&]{return FocusManager::GetFocusedElement(root.XamlRoot())==focused;},L"Canvas-bar menu did not actually own native focus");
        auto next=J::Parse(model.Stringify());next.Insert(L"localization",locale);require(source->adoptLocalization(next),L"Canvas-bar Japanese localization was not adopted");source->model=next;
        auto japanese=state();require(str(object(english,L"canvas_bar"),L"label")!=str(object(japanese,L"canvas_bar"),L"label"),L"Canvas-bar fixture has no actual translated label");
        bar->Apply(japanese);require(measurements->size()==2,L"Localized canvas-bar labels did not admit new native measurement");publication(japanese);
        require(bar->menuOwner.Open()&&lifetime.Current()&&popup.IsOpen()&&FocusManager::GetFocusedElement(root.XamlRoot())==focused,L"Language publication canceled the canvas-bar menu or lost native focus");
        require(bar->row==row&&bar->more==more&&bar->caption==caption,L"Language publication replaced the canvas action bar row or More");
        require(bar->fields.size()==fields.size(),L"Language publication changed canvas-bar field shape");for(size_t i=0;i<fields.size();++i)require(bar->fields[i].slot==fields[i].slot&&bar->fields[i].element==fields[i].element,L"Language publication replaced a canvas-bar native field");
        for(auto const& peer:peers)require(findElement(row,AutomationProperties::GetAutomationId(peer))==peer,L"Language publication replaced a canvas-bar native automation peer");
        auto apply=source->common(L"apply"),cancel=source->common(L"cancel");
        require(caption.Text()==apply&&AutomationProperties::GetName(more)==source->common(L"more")&&AutomationProperties::GetHelpText(more)==source->common(L"more"),L"Retained canvas-bar caption or More copy stayed English");
        require(AutomationProperties::GetName(peers[0])==apply&&AutomationProperties::GetHelpText(fields[0].element)==cancel&&AutomationProperties::GetName(peers[1])==cancel&&AutomationProperties::GetHelpText(fields[1].element)==cancel,L"Retained canvas-bar action/menu names or tooltips stayed English");
        require(AutomationProperties::GetName(peers[2])==apply&&AutomationProperties::GetName(peers[3])==apply&&AutomationProperties::GetHelpText(peers[3])==apply&&AutomationProperties::GetName(peers[4])==cancel&&AutomationProperties::GetHelpText(peers[4])==cancel,L"Retained canvas-bar segmented copy stayed English");
        require(AutomationProperties::GetName(peers[5])==apply&&AutomationProperties::GetHelpText(peers[5])==apply,L"Retained canvas-bar dropdown copy stayed English");
        std::function<bool(DependencyObject const&,hstring const&)> text;text=[&](DependencyObject const& node,hstring const& value){if(auto label=node.try_as<TextBlock>();label&&label.Text()==value)return true;for(int i=0;i<VisualTreeHelper::GetChildrenCount(node);++i)if(text(VisualTreeHelper::GetChild(node,i),value))return true;return false;};
        require(text(peers[0],apply)&&text(peers[1],cancel)&&text(peers[3],apply)&&text(peers[4],cancel)&&text(peers[5],cancel),L"Retained canvas-bar visible action/menu/choice text stayed English");
        auto measured=bar->measure();measured.Insert(L"context",bar->context());require(measurements->back().Stringify()==measured.Stringify(),L"Localized canvas-bar layout used stale native text measurements");publication(japanese);
        require(measurements->size()==2&&source->state.Stringify()==authored,L"Canvas-bar localization repeated measurement or changed authored state");
        auto narrowed=J::Parse(japanese.Stringify());object(object(narrowed,L"canvas_bar"),L"context").Insert(L"generation",N(2));
        bar->Apply(narrowed);
        require(bar->pending&&measurements->size()==3,L"Changed canvas-bar context did not prepare its replacement row");
        require(num(measurements->back(),L"more")==num(measured,L"more"),L"Prepared canvas-bar row changed the retained More extent");
        require(bar->row==row&&bar->more==more&&VisualTreeHelper::GetParent(more)==row,L"Canvas-bar preparation detached the retained More before publication");
        publication(narrowed);
        require(bar->more==more,L"Canvas-bar context publication replaced the retained More control");
        require(AutomationProperties::GetName(more)==source->common(L"more")&&AutomationProperties::GetHelpText(more)==source->common(L"more"),L"Retained More copy changed after canvas-bar context publication");
        require(!bar->menuOwner.Open(),L"Changed canvas-bar context retained its old owned menu");
        bar->menuOwner.Cancel();co_await wait([&]{return !popup.IsOpen()&&source->transients.empty()&&source->popupCount==0;},L"Canvas-bar menu did not retire after localization");
        root.Children().RemoveAt(0);bar.reset();root.Width(400);root.Height(240);++checks;
    }
    winrt::Windows::Foundation::IAsyncAction settingsResize(){
        auto previous=window.AppWindow().Size();auto scale=root.XamlRoot().RasterizationScale();
        Windows::Graphics::SizeInt32 wide{int32_t(1280*scale),int32_t(840*scale)},narrow{int32_t(720*scale),int32_t(576*scale)};window.AppWindow().Resize(wide);
        co_await wait([this]{return root.XamlRoot().Size().Width>1000;},L"Preferences wide native window did not arrange");
        auto model=J::Parse(LR"({"state":{"theme":"dark"},"titlebar_insets":[0,0,0],"preferences":{"page":"appearance","query":"","searching":true,"search_focus":0,"pages":[{"id":"appearance","title":"Appearance","icon":"settings","groups":[{"title":"Interface","rows":[{"id":"language","title":"Language","description":"Choose the interface language","enabled":true,"visible":true,"kind":{"type":"choice","selected":0,"options":["System language","English","Japanese"]}}]}]}]}})");
        object(model,L"state").Insert(L"theme",S(root.RequestedTheme()==ElementTheme::Light?L"light":L"dark"));
        auto bootstrap=J::Parse(to_hstring(context.bootstrap)),catalog=O({{L"bootstrap",bootstrap}});
        model.Insert(L"localization",O({{L"generation",N(0)},{L"catalog",J{}},{L"bootstrap",bootstrap}}));
        auto actions=std::make_shared<std::vector<std::string>>(),errors=std::make_shared<std::vector<std::string>>();
        auto send=[actions](std::string value){actions->push_back(std::move(value));};
        auto settings=std::make_unique<SettingsView>(send,catalog,context.localization,root.XamlRoot(),[](auto const&,bool){},[errors](std::string value){errors->push_back(std::move(value));},[]{},send);
        settings->Apply(model);
        co_await wait([this,&settings]{return settings->IsOpen()&&workspaceElement(L"preference-choice-language")&&workspaceElement(L"CloseButton")&&workspaceElement(L"settings-search");},L"Real Preferences controls did not open");
        auto language=workspaceElement(L"preference-choice-language").as<ComboBox>();
        auto close=workspaceElement(L"CloseButton").as<Button>();auto edit=workspaceElement(L"settings-search").as<TextBox>();
        co_await wait([&]{return language.IsLoaded()&&language.ActualWidth()>0&&close.ActualWidth()>0&&edit.IsLoaded();},L"Preferences native controls did not arrange");
        edit.Text(L"retained resize draft");edit.Focus(FocusState::Programmatic);edit.Select(2,5);
        co_await wait([&]{return FocusManager::GetFocusedElement(root.XamlRoot())==edit&&edit.SelectionStart()==2&&edit.SelectionLength()==5;},L"Preferences native draft/focus prerequisite failed");
        co_await wait([&]{for(auto const& value:*actions){auto sent=J::Parse(to_hstring(value));if(str(sent,L"type")==L"preferences"){auto action=object(sent,L"action");if(str(action,L"type")==L"search"&&str(action,L"query")==L"retained resize draft")return true;}}return false;},L"Preferences initial native draft action did not acknowledge");
        auto selected=language.SelectedIndex();auto languageFocus=language.FocusState(),closeFocus=close.FocusState();
        auto bounds=[](FrameworkElement const& node){auto point=node.TransformToVisual(nullptr).TransformPoint({0,0});return Windows::Foundation::Rect{point.X,point.Y,float(node.ActualWidth()),float(node.ActualHeight())};};
        auto inside=[&](FrameworkElement const& node){auto box=bounds(node);auto size=root.XamlRoot().Size();return node.IsLoaded()&&node.Visibility()==Visibility::Visible&&box.Width>0&&box.Height>0&&box.X>=-1&&box.Y>=-1&&box.X+box.Width<=size.Width+1&&box.Y+box.Height<=size.Height+1;};
        auto retained=[&]{require(workspaceElement(L"preference-choice-language")==language&&workspaceElement(L"CloseButton")==close&&workspaceElement(L"settings-search")==edit,L"Preferences resize replaced a native control");require(language.SelectedIndex()==selected&&language.FocusState()==languageFocus&&close.FocusState()==closeFocus,L"Preferences resize changed native choice or button focus");require(FocusManager::GetFocusedElement(root.XamlRoot())==edit&&edit.Text()==L"retained resize draft"&&edit.SelectionStart()==2&&edit.SelectionLength()==5,L"Preferences resize changed native edit text, focus or selection");require(actions->empty()&&errors->empty(),L"Preferences resize dispatched a settings action or error");};
        co_await wait([&]{return inside(language)&&inside(close);},L"Wide Preferences language or Close is outside XamlRoot");
        require(errors->empty(),L"Preferences opening reported an error");auto languageWide=bounds(language),closeWide=bounds(close);auto sizeWide=root.XamlRoot().Size();actions->clear();
        window.AppWindow().Resize(narrow);
        co_await wait([&]{auto size=root.XamlRoot().Size();return size.Width<sizeWide.Width&&size.Height<sizeWide.Height;},L"Preferences narrow native resize did not acknowledge");
        co_await wait([&]{return inside(language)&&inside(close);},L"Resized Preferences language or Close escaped current XamlRoot bounds");retained();
        window.AppWindow().Resize(wide);
        auto stable=[](Windows::Foundation::Rect const& a,Windows::Foundation::Rect const& b){return a.X>=b.X-1&&a.X<=b.X+1&&a.Y>=b.Y-1&&a.Y<=b.Y+1&&a.Width>=b.Width-1&&a.Width<=b.Width+1&&a.Height>=b.Height-1&&a.Height<=b.Height+1;};
        co_await wait([&]{auto size=root.XamlRoot().Size();return size.Width==sizeWide.Width&&size.Height==sizeWide.Height&&inside(language)&&inside(close)&&stable(bounds(language),languageWide)&&stable(bounds(close),closeWide);},L"Preferences growth did not restore native bounds");retained();
        object(model,L"preferences").Insert(L"query",S(L"retained resize draft"));
        auto closedModel=J::Parse(model.Stringify());closedModel.Remove(L"preferences");
        for(auto closedSize:{narrow,wide}){
            bool closedNarrow=closedSize.Width==narrow.Width;
            auto peer=winrt::Microsoft::UI::Xaml::Automation::Peers::FrameworkElementAutomationPeer::CreatePeerForElement(close);
            peer.GetPattern(winrt::Microsoft::UI::Xaml::Automation::Peers::PatternInterface::Invoke).as<winrt::Microsoft::UI::Xaml::Automation::Provider::IInvokeProvider>().Invoke();
            co_await wait([actions]{for(auto const& value:*actions)if(str(J::Parse(to_hstring(value)),L"type")==L"close_settings")return true;return false;},L"Preferences Close did not dispatch its shared request");
            settings->Apply(closedModel);
            co_await wait([&]{return !settings->IsOpen()&&!workspaceElement(L"preference-choice-language");},L"Preferences Close did not finish");
            auto ownerRoot=root.XamlRoot();bool resizedClosed=false;
            auto closedChange=ownerRoot.Changed(auto_revoke,[&](auto&&,auto&&){if(!settings->IsOpen())resizedClosed=true;});
            window.AppWindow().Resize(closedSize);
            co_await wait([&]{auto size=ownerRoot.Size();return resizedClosed&&!settings->IsOpen()&&(closedNarrow?size.Width<sizeWide.Width-40:size.Width==sizeWide.Width);},L"Closed Preferences did not receive its owner resize");
            settings->Apply(model);
            co_await wait([&]{return settings->IsOpen()&&inside(language)&&inside(close);},L"Reopened Preferences language or Close is outside its owner");
            edit.Focus(FocusState::Programmatic);edit.Select(2,5);actions->clear();retained();
            if(closedSize.Width==wide.Width){
                window.AppWindow().Resize(narrow);
                co_await wait([&]{return ownerRoot.Size().Width<sizeWide.Width-40&&inside(language)&&inside(close);},L"Reopened Preferences kept its closed-window bounds");retained();
            }
        }
        settings->Hide();co_await wait([&]{return !settings->IsOpen()&&!workspaceElement(L"preference-choice-language");},L"Preferences Hide did not finish native dialog close");
        settings.reset();window.AppWindow().Resize(previous);++checks;
    }
    winrt::Windows::Foundation::IAsyncAction backgroundAfterPartial(J original,J catalog,NativeLayoutInput layout){
        auto model=J::Parse(original.Stringify());std::unique_ptr<WorkspaceView> view;Canvas surface{nullptr};bool attached=false;
        try{
            {
                struct TraceSetting {
                    std::wstring previous;DWORD length=GetEnvironmentVariableW(L"CAPY_TRACE_UI",nullptr,0);
                    TraceSetting(){if(length){previous.resize(length);previous.resize(GetEnvironmentVariableW(L"CAPY_TRACE_UI",previous.data(),length));}check_bool(SetEnvironmentVariableW(L"CAPY_TRACE_UI",nullptr));}
                    ~TraceSetting(){SetEnvironmentVariableW(L"CAPY_TRACE_UI",length?previous.c_str():nullptr);}
                } tracing;
                auto send=[](std::string){};
                view=std::make_unique<WorkspaceView>(send,catalog,context.localization,[](CanvasQueryKind,std::string,PreviewReply){return false;},[](bool){},send,send,[](std::string,std::function<void(int32_t)>){ });
                surface=view->Root();surface.Width(1000);surface.Height(700);root.Children().InsertAt(0,surface);attached=true;
                require(view->Apply(model,layout),L"Background fixture initial model was rejected");view->Resize(1000,700);
            }
            auto group=find(array(object(model,L"layout"),L"groups"),L"active",L"layers");
            require(group.Size()!=0,L"Background fixture floating Layers group is missing");
            auto groupId=L"workspace-group-"+to_hstring(uint32_t(num(group,L"id")));
            co_await wait([&]{auto frame=findElement(surface,groupId);return frame&&frame.IsLoaded()&&frame.ActualWidth()>0;},L"Background fixture group did not arrange");
            require(view->Apply(model,layout),L"Background fixture measured model was rejected");
            group=find(array(object(model,L"layout"),L"groups"),L"active",L"layers");
            auto frame=findElement(surface,groupId).as<Border>();
            A connections,arcs,colors,panels;
            view->Glass(surface,connections,arcs,colors,panels);auto document=view->ChromeDocument();
            auto revision=uint64_t(std::max(0.,num(object(model,L"windows_glass"),L"chrome_revision")))+1;
            auto ack=[&](uint64_t value){A extent;extent.Append(N(num(document,L"width")));extent.Append(N(num(document,L"height")));return O({{L"chrome_revision",N(double(value))},{L"document",extent}});};
            view->ChromeSent(revision);require(view->Apply(O({{L"windows_glass",ack(revision)}})),L"Initial background fixture ACK was rejected");
            auto before=frame.CornerRadius(),expected=before;bool right=before.TopRight>0||before.BottomRight>0;
            require(right||before.TopLeft>0||before.BottomLeft>0,L"Background fixture lacks a rounded source corner");
            if(right)expected.TopRight=expected.BottomRight=0;else expected.TopLeft=expected.BottomLeft=0;
            auto sources=view->DrawerSources();auto saved=A::Parse(sources.Stringify());
            sources.Append(O({{L"anchor",O({{L"kind",S(L"tile")},{L"panel",S(str(group,L"active"))}})},{L"direction",S(right?L"right":L"left")},{L"bounds",object(group,L"bounds")}}));
            CanvasModelUpdate pending;pending.Read(model);view->ChromeSent(revision+1);
            A path,pair,updates;path.Append(S(L"windows_glass"));pair.Append(path);pair.Append(ack(revision+1));updates.Append(pair);
            pending.Read(O({{L"model_update",updates}}));
            auto applyTracked=[&]<class Update>(Update const& packet){
                if constexpr(requires{packet.changes;})return view->Apply(packet.Model(),layout,packet.changes);
                else return view->Apply(packet.Model(),layout);
            };
            require(applyTracked(pending),L"Equal-frame background fixture full model was rejected");
            require(frame.CornerRadius()==expected,L"Equal-frame ACK skipped backgrounds after an out-of-band drawer-source change");
            sources.Clear();for(auto source:saved)sources.Append(source);
            require(view->Apply(model,layout),L"Background fixture restoration was rejected");
            require(frame.CornerRadius()==before,L"Background fixture did not restore its source corners");
            root.Children().RemoveAt(0);attached=false;view.reset();
        }catch(...){if(attached)root.Children().RemoveAt(0);view.reset();throw;}
    }
    winrt::fire_and_forget measureFixtureHeader(std::string snapshot,std::string catalog,double id,std::shared_ptr<std::promise<std::string>> result){
        auto self=shared_from_this();std::shared_ptr<HeaderView> header;bool attached=false;auto previousSize=window.AppWindow().Size();auto previousWidth=root.Width();
        try{
            auto measured=std::make_shared<std::string>();auto model=J::Parse(to_hstring(snapshot)),copy=J::Parse(to_hstring(catalog));copy.Insert(L"bootstrap",object(object(model,L"localization"),L"bootstrap"));
            auto send=[measured,id](std::string value){auto action=J::Parse(to_hstring(value));if(str(action,L"type")==L"measure_header")for(auto item:array(action,L"items"))if(num(item.GetObject(),L"id")==id&&num(object(item.GetObject(),L"bounds"),L"width")>0)*measured=value;};
            header=std::make_shared<HeaderView>(send,copy,context.localization,[](bool){},[]{},[]{},[]{},[](CanvasQueryKind,std::string,PreviewReply){return false;},[](std::string){},[](std::string){});
            auto surface=header->Root();surface.Width(1000);root.Width(1000);root.Children().InsertAt(0,surface);attached=true;window.AppWindow().Resize({1100,900});header->Apply(model);
            co_await wait([measured]{return !measured->empty();},L"Inspector origin did not publish native header bounds");
            root.Children().RemoveAt(0);attached=false;header.reset();root.Width(previousWidth);window.AppWindow().Resize(previousSize);result->set_value(*measured);
        }catch(...){if(attached)root.Children().RemoveAt(0);header.reset();root.Width(previousWidth);window.AppWindow().Resize(previousSize);result->set_exception(std::current_exception());}
    }
    winrt::Windows::Foundation::IAsyncAction layerLanguageDraft(){
        auto theme=root.RequestedTheme();std::string snapshot,catalog,translated,paletteChanged,paletteRemoved,paletteSwitched,gradientSpec,inspectorSnapshot,tiltSnapshot,paperSnapshot;NativeLayoutInput layout,inspectorLayout,paperLayout;
        auto queue=root.DispatcherQueue();auto weak=weak_from_this();
        co_await resume_background();
        {
            struct Apartment{Apartment(){init_apartment(apartment_type::multi_threaded);}~Apartment(){uninit_apartment();}} apartment;
            NativeLaunch launch(capy_launch("[\"en\"]",nullptr));require(bool(launch),L"Layer language fixture launch failed");
            NativeHandle<capy_destroy> host(capy_create(1000,700,1,launch.get()));require(bool(host),L"Layer language fixture host failed");
            J presentation;auto waitModel=[&](auto ready,wchar_t const* message){
                for(int attempt=0;attempt<300;++attempt){
                    require(capy_poll_services(host.get())>=0,L"Layer fixture service poll failed");CapySnapshotInfo info{};NativeString value(capy_snapshot(host.get(),&info));
                    if(value){auto model=J::Parse(to_hstring(value.bytes));if(model.HasKey(L"localization"))presentation=object(model,L"localization");if(ready(model))return model;}
                    std::this_thread::sleep_for(std::chrono::milliseconds(50));
                }
                require(false,message);return J{};
            };
            require(capy_start_services(host.get(),nullptr,nullptr)>=0,L"Layer fixture services failed");
            waitModel([](J const& model){auto workspace=object(model,L"windows_workspace");return flag(workspace,L"ready")&&!flag(workspace,L"busy");},L"Layer fixture workspace did not become ready");
            auto action=[&](std::string const& value){if(capy_action(host.get(),value.c_str())!=0)throw hresult_error(E_FAIL,L"Layer language fixture action failed: "+to_hstring(value)+L": "+to_hstring(NativeString(capy_error()).get()));};
            action(theme==ElementTheme::Light?R"({"type":"set_theme","theme":"light"})":R"({"type":"set_theme","theme":"dark"})");
            action(R"({"type":"customize","action":{"type":"set_panel_visible","panel":"layers","visible":true}})");
            action(R"({"type":"move_panel","panel":"layers","target":{"kind":"float","position":[20,20]},"viewport":[1000,700]})");
            action(R"({"type":"layer","action":{"op":"new","group":false,"clipped":false}})");
            auto paletteQuery=[&](J request){NativeString value(capy_query(host.get(),to_string(request.Stringify()).c_str()));require(bool(value),L"Palette fixture query failed");auto reply=J::Parse(to_hstring(value.bytes));require(str(reply,L"error").empty(),L"Palette fixture action failed");return reply;};
            auto palette=paletteQuery(O({{L"type",S(L"palette_panel")},{L"query",S(L"")}}));
            auto swatch=num(array(palette,L"swatches").GetObjectAt(0),L"id"),other=num(array(palette,L"palettes").GetObjectAt(1),L"id");
            paletteQuery(O({{L"type",S(L"palette_action")},{L"action",O({{L"op",S(L"use")},{L"id",N(swatch)}})}}));
            CapySnapshotInfo info{};NativeString initial(capy_snapshot(host.get(),&info));require(bool(initial)&&info.full,L"Layer fixture full model missing");
            action(R"({"type":"invoke","command":"pen_tilt"})");
            NativeString tiltModel(capy_snapshot(host.get(),&info));require(bool(tiltModel)&&info.full,L"Tilt native fixture shared model missing");tiltSnapshot=std::move(tiltModel.bytes);
            action(R"({"type":"tilt_calibration","action":{"kind":"panel","action":{"kind":"cancel"}}})");
            auto model=J::Parse(to_hstring(initial.bytes));auto id=num(object(object(object(model,L"state"),L"layer_tools"),L"editing_layer"),L"id");
            action(to_string(O({{L"type",S(L"layer")},{L"action",O({{L"op",S(L"begin_rename")},{L"id",N(id)}})}}).Stringify()));
            NativeString current(capy_snapshot(host.get(),&info));require(bool(current)&&info.full,L"Layer rename full model missing");
            auto baseline=J::Parse(to_hstring(current.bytes));baseline.Insert(L"localization",presentation);snapshot=to_string(baseline.Stringify());
            NativeString copy(capy_query(host.get(),R"({"type":"catalog"})"));require(bool(copy),L"Layer fixture catalog missing");catalog=std::move(copy.bytes);
            auto input=to_string(object(baseline,L"layout_input").Stringify());layout=NativeLayoutInput(capy_layout_input_prepare(input.c_str()));require(bool(layout),L"Layer fixture layout input missing");
            auto tags=array(object(presentation,L"bootstrap"),L"shipped_tags");uint32_t japanese=0;for(uint32_t i=0;i<tags.Size();++i)if(tags.GetStringAt(i)==L"ja")japanese=i+1;
            require(japanese>0,L"Japanese fixture language choice missing");action(to_string(O({{L"type",S(L"preferences")},{L"action",O({{L"type",S(L"edit")},{L"id",S(L"language")},{L"value",N(japanese)}})}}).Stringify()));
            waitModel([&](J const&){return str(object(presentation,L"bootstrap"),L"active_tag")==L"ja";},L"Japanese fixture presentation did not publish");translated=to_string(presentation.Stringify());
            paletteQuery(O({{L"type",S(L"palette_action")},{L"action",O({{L"op",S(L"rename_palette")},{L"id",N(other)},{L"name",S(L"Remote palette metadata")}})}}));
            NativeString refreshed(capy_snapshot(host.get(),&info));require(bool(refreshed)&&info.full,L"Remote palette full model missing");paletteChanged=std::move(refreshed.bytes);
            paletteQuery(O({{L"type",S(L"palette_action")},{L"action",O({{L"op",S(L"remove")},{L"id",N(swatch)}})}}));
            NativeString removed(capy_snapshot(host.get(),&info));require(bool(removed)&&info.full,L"Removed swatch full model missing");paletteRemoved=std::move(removed.bytes);
            paletteQuery(O({{L"type",S(L"palette_action")},{L"action",O({{L"op",S(L"select_palette")},{L"id",N(other)}})}}));
            NativeString switched(capy_snapshot(host.get(),&info));require(bool(switched)&&info.full,L"Changed palette full model missing");paletteSwitched=std::move(switched.bytes);
            action(R"({"type":"invoke","command":"gradient"})");
            NativeString gradient(capy_snapshot(host.get(),&info));require(bool(gradient),L"Gradient fixture model missing");
            for(auto value:array(object(J::Parse(to_hstring(gradient.bytes)),L"state"),L"tool_extra"))if(auto control=object(value.GetObject(),L"Gradient");control.Size())gradientSpec=to_string(control.Stringify());
            require(!gradientSpec.empty(),L"Shared gradient tool control missing");
            action(R"({"type":"customize","action":{"type":"header","action":{"type":"edit","editing":true}}})");
            action(R"({"type":"customize","action":{"type":"header","action":{"type":"add","zone":"right","before":null,"item":{"kind":"tool","control":{"kind":"panel","panel":"layers"}}}}})");
            action(R"({"type":"customize","action":{"type":"header","action":{"type":"edit","editing":false}}})");
            NativeString header(capy_snapshot(host.get(),&info));require(bool(header),L"Inspector fixture header missing");
            double layersEntry=0;for(auto zone:array(object(object(J::Parse(to_hstring(header.bytes)),L"header"),L"model"),L"zones"))for(auto entry:zone.GetArray()){
                auto item=entry.GetObject();if(str(object(object(item,L"item"),L"control"),L"panel")==L"layers")layersEntry=num(item,L"id");
            }
            require(layersEntry>0,L"Inspector fixture Layers header missing");
            auto measurement=std::make_shared<std::promise<std::string>>();auto measured=measurement->get_future();auto headerSnapshot=J::Parse(to_hstring(header.bytes));headerSnapshot.Insert(L"localization",presentation);auto bytes=to_string(headerSnapshot.Stringify());
            require(queue.TryEnqueue([weak,bytes,catalog,layersEntry,measurement]{if(auto self=weak.lock())self->measureFixtureHeader(bytes,catalog,layersEntry,measurement);}),L"Inspector header measurement dispatch failed");
            require(measured.wait_for(std::chrono::seconds(6))==std::future_status::ready,L"Inspector header measurement did not return");action(measured.get());
            action(to_string(O({{L"type",S(L"customize")},{L"action",O({{L"type",S(L"toggle_header_drawer")},{L"id",N(layersEntry)}})}}).Stringify()));
            NativeString inspector(capy_snapshot(host.get(),&info));require(bool(inspector),L"Inspector fixture snapshot missing");auto inspectorModel=J::Parse(to_hstring(inspector.bytes));inspectorModel.Insert(L"localization",presentation);inspectorSnapshot=to_string(inspectorModel.Stringify());
            inspectorLayout=NativeLayoutInput(capy_layout_input_prepare(to_string(object(inspectorModel,L"layout_input").Stringify()).c_str()));require(bool(inspectorLayout),L"Inspector fixture layout missing");
            action(R"({"type":"customize","action":{"type":"close_expanded"}})");
            double paper=0;for(auto item:array(object(inspectorModel,L"state"),L"layers"))if(object(item.GetObject(),L"paper").Size())paper=num(item.GetObject(),L"id");
            require(paper>0,L"Paper fixture has no shared Paper layer");
            action(to_string(O({{L"type",S(L"layer")},{L"action",O({{L"op",S(L"select_row")},{L"id",N(paper)},{L"extend",B(false)},{L"toggle",B(false)}})}}).Stringify()));action(R"({"type":"customize","action":{"type":"set_panel_visible","panel":"properties","visible":true}})");
            action(R"({"type":"move_panel","panel":"properties","target":{"kind":"float","position":[20,20]},"viewport":[1000,700]})");
            CapySnapshotInfo paperInfo{};NativeString paperState(capy_snapshot(host.get(),&paperInfo));require(bool(paperState)&&paperInfo.full,L"Paper fixture selected-layer snapshot missing");
            auto paperModel=J::Parse(to_hstring(paperState.bytes));paperModel.Insert(L"localization",presentation);paperSnapshot=to_string(paperModel.Stringify());
            auto paperProperties=object(object(paperModel,L"state"),L"layer_properties");bool editable=false,projected=false;uint32_t tileCount=0;
            for(auto item:array(object(paperModel,L"state"),L"layers"))if(num(item.GetObject(),L"id")==paper)editable=flag(object(item.GetObject(),L"paper"),L"editable");
            for(auto item:array(paperProperties,L"controls")){auto control=item.GetObject();if(str(object(control,L"kind"),L"kind")==L"choice"&&object(control,L"paper").Size()){tileCount=array(object(control,L"paper"),L"tiles").Size();projected=tileCount>0;}}
            if(num(paperProperties,L"layer")!=paper||!editable||!flag(paperProperties,L"enabled")||!projected)throw hresult_error(E_FAIL,L"Paper shared setup: layer="+to_hstring(paper)+L" selected="+to_hstring(num(paperProperties,L"layer"))+L" editable="+to_hstring(editable)+L" enabled="+to_hstring(flag(paperProperties,L"enabled"))+L" tiles="+to_hstring(tileCount));
            paperLayout=NativeLayoutInput(capy_layout_input_prepare(to_string(object(paperModel,L"layout_input").Stringify()).c_str()));require(bool(paperLayout),L"Paper fixture actual shared layout missing");
            require(capy_finish_services(host.get())>=0,L"Layer fixture service shutdown failed");
        }
        co_await apartment;
        auto model=J::Parse(to_hstring(snapshot)),next=J::Parse(to_hstring(snapshot)),copy=J::Parse(to_hstring(catalog));
        co_await paperRegistryClosed(J::Parse(to_hstring(paperSnapshot)),copy,paperLayout);
        phase=L"cold-layer-inspector";
        {
            auto inspectorModel=J::Parse(to_hstring(inspectorSnapshot));auto inspectorCopy=J::Parse(to_hstring(catalog));inspectorCopy.Insert(L"bootstrap",object(object(inspectorModel,L"localization"),L"bootstrap"));
            auto send=[](std::string){};
            auto view=std::make_unique<WorkspaceView>(send,inspectorCopy,context.localization,[](CanvasQueryKind,std::string,PreviewReply){return false;},[](bool){},send,send,[](std::string,std::function<void(int32_t)>){ });
            auto previousSize=window.AppWindow().Size();auto previousWidth=root.Width();bool attached=false;
            try{
                auto surface=view->Root();surface.Width(1000);surface.Height(700);root.Width(1000);window.AppWindow().Resize({1100,900});
                require(!surface.IsLoaded(),L"Cold inspector fixture was already loaded");
                require(view->Apply(inspectorModel,inspectorLayout),L"Cold inspector model was rejected");view->Resize(1000,700);
                auto inspector=findElement(surface,L"layer-inspector");require(bool(inspector),L"Cold inspector root was not attached");auto pendingHeight=inspector.Height();
                root.Children().InsertAt(0,surface);attached=true;
                co_await wait([&]{return inspector.IsLoaded()&&inspector.ActualWidth()>0&&inspector.Height()>pendingHeight;},L"Cold inspector did not resolve after native measurement");
                require(view->Apply(inspectorModel,inspectorLayout),L"Measured inspector model was rejected");
                root.Children().RemoveAt(0);attached=false;view.reset();root.Width(previousWidth);window.AppWindow().Resize(previousSize);
            }catch(...){if(attached)root.Children().RemoveAt(0);view.reset();root.Width(previousWidth);window.AppWindow().Resize(previousSize);throw;}
        }
        phase=L"tilt-control-rows";co_await tiltRows(J::Parse(to_hstring(tiltSnapshot)),copy);
        phase=L"gradient-input";co_await gradientContacts(model,copy,J::Parse(to_hstring(gradientSpec)));phase=L"layer-language-draft";
        auto locale=J::Parse(to_hstring(translated));
        require(str(object(locale,L"bootstrap"),L"active_tag")==L"ja",L"Japanese fixture presentation is not Japanese");
        require(num(locale,L"generation")>num(object(model,L"localization"),L"generation"),L"Japanese fixture publication did not advance localization");next.Insert(L"localization",locale);
        auto state=object(model,L"state");auto id=num(object(state,L"layer_tools"),L"rename_layer");
        require(id>0&&object(next,L"state").Stringify()==state.Stringify(),L"Localized fixture changed authored state");
        require(str(state,L"theme")== (theme==ElementTheme::Light?L"light":L"dark"),L"Layer snapshot theme differs from the requested fixture theme");
        require(str(object(object(model,L"localization"),L"bootstrap"),L"active_tag")==L"en",L"Layer fixture presentation must start in English");
        auto localizedName=str(object(object(object(locale,L"catalog"),L"native_copy"),L"layers"),L"name");
        copy.Insert(L"bootstrap",object(object(model,L"localization"),L"bootstrap"));
        auto actions=std::make_shared<std::vector<J>>();auto send=[actions](std::string bytes){actions->push_back(J::Parse(to_hstring(bytes)));};
        auto view=std::make_unique<WorkspaceView>(send,copy,context.localization,[](CanvasQueryKind,std::string,PreviewReply){return false;},[](bool){},send,send,[](std::string,std::function<void(int32_t)>){ });
        auto surface=view->Root();surface.Width(1000);surface.Height(700);root.Width(1000);root.Height(780);root.Children().InsertAt(0,surface);window.AppWindow().Resize({1100,900});
        require(view->Apply(model,layout),L"Initial WorkspaceView model was rejected");view->Resize(1000,700);
        J group;for(auto value:array(object(model,L"layout"),L"groups")){auto candidate=value.GetObject();if(str(candidate,L"active")==L"layers"){group=candidate;break;}}
        require(group.Size()!=0,L"Real layer panel group missing");
        auto groupId=L"workspace-group-"+to_hstring(uint32_t(num(group,L"id"))),entryId=L"layer-"+to_hstring(uint64_t(id))+L"-rename";
        auto entry=[&]{return findElement(findElement(surface,groupId),entryId).try_as<TextBox>();};
        co_await wait([&]{auto field=entry();return field&&field.IsLoaded()&&field.ActualWidth()>0&&field.Visibility()==Visibility::Visible;},L"Real WorkspaceView layer editor did not become arranged");
        auto resizeHandles=array(group,L"resize_handles");require(resizeHandles.Size()>0,L"Floating layer fixture lacks a resize handle");
        auto handleId=L"floating-"+to_hstring(uint32_t(num(group,L"id")))+L"-"+str(resizeHandles.GetObjectAt(0),L"edge");
        auto resizeControl=findElement(surface,handleId).as<FrameworkElement>();auto tag=resizeControl.Tag();
        auto cursor=resizeControl.as<IUIElementProtected>().ProtectedCursor();require(bool(cursor),L"Floating resize cursor is absent");
        require(view->Apply(model,layout),L"Repeated full workspace model was rejected");
        require(resizeControl.Tag()==tag&&resizeControl.as<IUIElementProtected>().ProtectedCursor()==cursor,L"Unchanged resize handle replaced its action or cursor");
        co_await backgroundAfterPartial(model,copy,layout);
        auto field=entry();auto originalName=AutomationProperties::GetName(field);require(originalName==L"Layer name"&&localizedName==L"\u30ec\u30a4\u30e4\u30fc\u540d",L"Layer fixture needs English and Japanese accessibility text");captureTextComposition(field);require(field.Focus(FocusState::Keyboard),L"Layer draft focus failed");
        co_await wait([&]{return FocusManager::GetFocusedElement(root.XamlRoot())==field;},L"Real layer editor did not own focus");
        field.Text(L"\u65e5\u672c");field.Select(2,0);require(field.Text()==L"\u65e5\u672c"&&field.SelectionStart()==2&&field.SelectionLength()==0&&FocusManager::GetFocusedElement(root.XamlRoot())==field&&!textComposing(field),L"Layer draft must be focused, exact and unmarked before language publication");actions->clear();
        require(view->Apply(next,layout),L"Localized WorkspaceView model was rejected");
        require(entry()==field,L"Language publication replaced the active layer TextBox");
        co_await settle();
        require(field.IsLoaded()&&entry()==field&&FocusManager::GetFocusedElement(root.XamlRoot())==field,L"Language publication lost the retained layer editor or focus");
        require(field.Text()==L"\u65e5\u672c"&&field.SelectionStart()==2&&field.SelectionLength()==0,L"Language publication replaced the native layer draft or caret");
        require(AutomationProperties::GetName(field)==localizedName,L"Retained layer editor did not adopt Japanese accessibility text");
        for(auto const& command:*actions){auto action=command.HasKey(L"windows_epoch")?object(command,L"action"):command;if(str(action,L"type")==L"layer"){
            auto operation=str(object(action,L"action"),L"op");require(operation!=L"rename"&&operation!=L"cancel_rename",L"Language publication dispatched an authored rename or cancellation");
        }}
        phase=L"workspace-surface-eligibility";
        {
            auto counters=[&]{view->MeasureChrome();return object(J::Parse(AutomationProperties::GetItemStatus(surface)),L"surface_frames");};
            co_await wait([&]{return num(counters(),L"groups")>0;},L"Surface eligibility fixture did not observe visible native groups");
            auto host=surface.Parent().as<UIElement>();auto opacity=host.Opacity();auto visibility=host.Visibility();
            auto frames=std::make_shared<uint32_t>(0);
            auto rendered=CompositionTarget::Rendering(auto_revoke,[frames](auto&&,auto&&){++*frames;});
            for(bool collapsed:{false,true}){
                if(collapsed)host.Visibility(Visibility::Collapsed);else host.Opacity(0);
                auto before=counters();auto frame=*frames;
                co_await wait([frames,frame]{return *frames>frame;},L"Surface eligibility fixture did not receive native Rendering");
                auto after=counters();
                require(num(after,L"frames")>num(before,L"frames"),L"Hidden surface eligibility did not sample an actual Rendering tick");
                require(num(after,L"groups")==num(before,L"groups"),L"Native panel fill counter admitted an effectively hidden ancestor");
                require(num(after,L"mismatched_frames")==num(before,L"mismatched_frames")&&num(after,L"maximum_offset_px")==num(before,L"maximum_offset_px")&&num(after,L"tab_frames")==num(before,L"tab_frames")&&num(after,L"escaped_label_frames")==num(before,L"escaped_label_frames"),L"Hidden native panels changed visible fill or tab evidence");
                host.Visibility(visibility);host.Opacity(opacity);
            }
            auto before=counters();auto frame=*frames;
            co_await wait([frames,frame]{return *frames>frame;},L"Revealed surface eligibility fixture did not receive native Rendering");
            require(num(counters(),L"groups")>num(before,L"groups"),L"Native panel fill counter omitted revealed groups");
            rendered.revoke();++checks;
        }
        root.Children().RemoveAt(0);view.reset();root.Width(400);root.Height(240);++imeChecks;
        phase=L"canvas-bar-language";co_await canvasBarLanguage(model,locale);
        phase=L"palette-name-draft";co_await paletteNameDraft(model,locale,J::Parse(to_hstring(paletteChanged)),J::Parse(to_hstring(paletteRemoved)),J::Parse(to_hstring(paletteSwitched)));
    }
    winrt::Windows::Foundation::IAsyncAction paletteNameDraft(J const& model,J const& locale,J const& refreshed,J const& removed,J const& switched){
        auto source=std::make_shared<WorkspaceData>();context.apply(source);source->model=J::Parse(model.Stringify());source->state=object(source->model,L"state");
        auto requests=std::make_shared<std::vector<J>>();source->query=[requests](CanvasQueryKind kind,std::string value,PreviewReply){require(kind==CanvasQueryKind::Workspace,L"Palette draft queried a renderer");requests->push_back(J::Parse(to_hstring(value)));return false;};
        auto before=object(model,L"palette_panel"),after=object(refreshed,L"palette_panel");double target=0;for(auto value:array(before,L"swatches"))if(flag(value.GetObject(),L"current"))target=num(value.GetObject(),L"id");
        require(target>0&&num(before,L"palette")==num(after,L"palette"),L"Palette fixture needs the same edited swatch and palette");
        require(array(before,L"swatches").Stringify()==array(after,L"swatches").Stringify()&&str(before,L"color_detail")==str(after,L"color_detail"),L"Remote palette rename changed the edited color");
        require(array(before,L"palettes").Stringify()!=array(after,L"palettes").Stringify(),L"Remote palette publication did not change choices metadata");
        Bindings bindings;auto surface=PalettesPanel(source,bindings);surface.Width(500);surface.Height(400);root.Width(600);root.Height(500);root.Children().InsertAt(0,surface);
        co_await wait([&]{auto name=findElement(surface,L"palette-name");return name&&name.IsLoaded()&&name.ActualWidth()>0;},L"Palette name did not arrange");
        auto name=findElement(surface,L"palette-name").as<Button>();auto peer=Automation::Peers::FrameworkElementAutomationPeer::CreatePeerForElement(name);
        peer.GetPattern(Automation::Peers::PatternInterface::Invoke).as<Automation::Provider::IInvokeProvider>().Invoke();
        auto entry=findElement(surface,L"palette-name-editor").as<TextBox>();
        co_await wait([this,entry]{return entry.Visibility()==Visibility::Visible&&FocusManager::GetFocusedElement(root.XamlRoot())==entry;},L"Native swatch editor did not own focus");
        entry.Text(L"\u65e5\u672c draft");entry.Select(1,2);requests->clear();
        auto retained=[&]{require(findElement(surface,L"palette-name-editor")==entry&&entry.Visibility()==Visibility::Visible&&FocusManager::GetFocusedElement(root.XamlRoot())==entry,L"Palette metadata publication retired the native swatch editor");require(entry.Text()==L"\u65e5\u672c draft"&&entry.SelectionStart()==1&&entry.SelectionLength()==2,L"Palette publication changed exact draft or selection");for(auto const& request:*requests)require(str(request,L"type")!=L"palette_action",L"Palette publication committed an unsubmitted draft");};
        auto apply=[&](J snapshot){source->model=std::move(snapshot);source->state=object(source->model,L"state");source->adoptLocalization(source->model);for(auto const& binding:bindings)binding();};
        auto translated=J::Parse(model.Stringify());translated.Insert(L"localization",locale);apply(translated);retained();
        auto remote=J::Parse(refreshed.Stringify());remote.Insert(L"localization",locale);apply(remote);retained();
        nativeKey(VK_RETURN,false);nativeKey(VK_RETURN,true);
        co_await wait([requests]{return std::any_of(requests->begin(),requests->end(),[](J const& request){return str(request,L"type")==L"palette_action";});},L"Retained swatch draft did not submit");
        unsigned renames=0;for(auto const& request:*requests)if(str(request,L"type")==L"palette_action"){auto action=object(request,L"action");require(str(action,L"op")==L"rename"&&num(action,L"id")==target&&str(action,L"name")==L"\u65e5\u672c draft",L"Retained draft renamed a different swatch or lost text");++renames;}
        require(renames==1&&entry.Visibility()==Visibility::Collapsed,L"Swatch draft did not commit exactly once");
        for(auto invalid:{removed,switched}){
            apply(translated);peer.GetPattern(Automation::Peers::PatternInterface::Invoke).as<Automation::Provider::IInvokeProvider>().Invoke();
            co_await wait([this,entry]{return entry.Visibility()==Visibility::Visible&&FocusManager::GetFocusedElement(root.XamlRoot())==entry;},L"Swatch editor did not reopen for target invalidation");
            entry.Text(L"Unsubmitted");requests->clear();invalid.Insert(L"localization",locale);apply(invalid);
            require(entry.Visibility()==Visibility::Collapsed,L"Removing the edited swatch or changing palette retained an invalid editor");
            for(auto const& request:*requests)require(str(request,L"type")!=L"palette_action",L"Target invalidation committed an unsubmitted name");
        }
        anchor.Focus(FocusState::Programmatic);root.Children().RemoveAt(0);bindings.clear();root.Width(400);root.Height(240);++imeChecks;
    }
    void nativeKey(WORD key,bool released,DWORD time=0){
        privacy();INPUT event{};event.type=INPUT_KEYBOARD;event.ki.wVk=key;event.ki.dwFlags=released?KEYEVENTF_KEYUP:0;event.ki.time=time;
        require(SendInput(1,&event,sizeof(INPUT))==1,L"Workspace IME key injection failed");
    }
    struct CompletionRoute{
        UIElement const& entry;std::array<Windows::Foundation::IInspectable*,2> handlers;std::shared_ptr<std::function<void()>> callback;
        ~CompletionRoute(){*callback={};for(auto handler:handlers)if(*handler){entry.RemoveHandler(UIElement::PreviewKeyDownEvent(),*handler);*handler=nullptr;}}
    };
    winrt::Windows::Foundation::IAsyncAction defaultIme(){
        enum class DefaultIme{Active,Retired,Stale};
        for(auto mode:{DefaultIme::Active,DefaultIme::Retired,DefaultIme::Stale}){
        bool stale=mode==DefaultIme::Stale,retired=mode==DefaultIme::Retired;
        hstring label=stale?L"native-default-stale-enter":retired?L"native-default-retired-ordinary-enter":L"native-default-enter";
        auto busy=std::make_shared<bool>(false);auto pending=std::make_shared<bool>(false),published=std::make_shared<bool>(false);auto trace=imeTrace;
        auto complete=std::make_shared<std::function<void()>>();Windows::Foundation::IInspectable completion{nullptr};
        ContentDialog dialog;TextBox entry;entry.Text(L"Draft");
        if(retired){
            completion=box_value(KeyEventHandler([complete](auto&&,KeyRoutedEventArgs const& e){if(e.Key()==Windows::System::VirtualKey::Enter&&*complete)(*complete)();}));
            entry.AddHandler(UIElement::PreviewKeyDownEvent(),completion,true);
        }
        captureTextComposition(entry,[busy,pending,published](bool value){*busy=value;if(*pending&&!value)*published=true;});
        dialog.XamlRoot(root.XamlRoot());dialog.RequestedTheme(root.RequestedTheme());dialog.Title(box_value(L"Native default action"));dialog.Content(entry);
        dialog.PrimaryButtonText(L"Save");dialog.CloseButtonText(L"Cancel");dialog.DefaultButton(ContentDialogButton::Primary);
        auto primary=std::make_shared<unsigned>(0),events=std::make_shared<unsigned>(0);auto closed=std::make_shared<bool>(false);
        dialog.PrimaryButtonClick([this,primary,dialog,mode](auto&&,ContentDialogButtonClickEventArgs const& e){++*primary;e.Cancel(true);if(mode!=DefaultIme::Active){dialog.Hide();anchor.Focus(FocusState::Keyboard);}});
        dialog.Closed([closed](auto&&,auto&&){*closed=true;});
        dialog.AddHandler(UIElement::KeyDownEvent(),box_value(KeyEventHandler([events](auto&&,KeyRoutedEventArgs const& e){if(e.Key()==Windows::System::VirtualKey::Enter)++*events;})),true);
        dialog.AddHandler(UIElement::KeyUpEvent(),box_value(KeyEventHandler([events](auto&&,KeyRoutedEventArgs const& e){if(e.Key()==Windows::System::VirtualKey::Enter)++*events;})),true);
        auto shown=dialog.ShowAsync();bool heldKey=false,heldTrigger=false;Windows::Foundation::IInspectable starter{nullptr};
        try{
            co_await wait([entry]{return entry.IsLoaded()&&entry.ActualWidth()>0;},L"Native default TextBox did not become arranged");
            entry.Focus(FocusState::Keyboard);entry.Select(0,0);privacy();
            ScopedComposition composition(*textServices,[trace,label](hstring api,HRESULT result,uintptr_t pointer){
                trace->push_back(O({{L"case",S(label)},{L"api",S(api)},{L"code",N(int32_t(result))},{L"pointer",N(double(pointer))}}));
            });
            CompletionRoute completionRoute{entry,{&completion,&starter},complete};
            std::exception_ptr completionFailure;
            if(retired)*complete=[this,entry,&composition,&completionFailure]{
                try{
                    require(FocusManager::GetFocusedElement(root.XamlRoot())==entry&&textComposing(entry),L"Confirming Return lost its focused marked TextBox");
                    composition.end();
                    require(composition.observations.at(L"ITfContext::RequestEditSession/end").first==S_OK&&composition.observations.at(L"ITfEditSession::DoEditSession/end").first==S_OK,
                        L"Confirming Return did not complete its synchronous TSF edit session");
                    require(!textComposing(entry)&&entry.Text()==L"\u5019\u88dcDraft",L"Confirming Return did not retain exact completed native text");
                }catch(...){completionFailure=std::current_exception();}
            };
            co_await wait([this,entry,&composition]{return composition.focused(entry,root.XamlRoot());},L"Native default TextBox did not acquire its TSF context");
            composition.begin();co_await wait([entry]{return textComposing(entry)&&entry.Text()==L"\u5019\u88dcDraft";},L"Native default TextBox did not acquire exact marked text");
            require(*busy,L"Marked native input did not notify localization ownership");
            *pending=true;
            if(stale){
                auto triggered=std::make_shared<bool>(false);std::exception_ptr callbackFailure;auto oldTime=GetTickCount()-1000;
                starter=box_value(KeyEventHandler([this,entry,&composition,&heldKey,&callbackFailure,triggered,trace,label,oldTime](auto&&,KeyRoutedEventArgs const& e){
                    if(e.Key()!=Windows::System::VirtualKey::F24)return;
                    try{
                        auto message=uint32_t(GetMessageTime());auto queuedAt=GetTickCount();
                        require(message==oldTime&&int32_t(queuedAt-message)>0,L"Native stale-message precondition was not established");
                        require(FocusManager::GetFocusedElement(root.XamlRoot())==entry&&textComposing(entry),L"Stale-message trigger lost its focused marked TextBox");
                        nativeKey(VK_RETURN,false);heldKey=true;composition.end();auto completedAt=GetTickCount();
                        trace->push_back(O({{L"case",S(label)},{L"message",N(message)},{L"queued_at",N(queuedAt)},{L"completed_at",N(completedAt)},{L"message_after_end",N(uint32_t(GetMessageTime()))}}));
                        require(composition.observations.at(L"ITfContext::RequestEditSession/end").first==S_OK&&composition.observations.at(L"ITfEditSession::DoEditSession/end").first==S_OK,
                            L"Native completion did not run its synchronous TSF edit session");
                        require(int32_t(completedAt-queuedAt)>=0&&!textComposing(entry)&&entry.Text()==L"\u5019\u88dcDraft",L"Queued native Return did not follow exact synchronous TSF completion");
                    }catch(...){callbackFailure=std::current_exception();}
                    *triggered=true;
                }));
                entry.AddHandler(UIElement::PreviewKeyDownEvent(),starter,true);
                nativeKey(VK_F24,false,oldTime);heldTrigger=true;nativeKey(VK_F24,true,oldTime);heldTrigger=false;
                co_await wait([triggered]{return *triggered;},L"Native stale-message trigger did not reach its TextBox route");
                entry.RemoveHandler(UIElement::PreviewKeyDownEvent(),starter);starter=nullptr;
                if(callbackFailure)std::rethrow_exception(callbackFailure);
            }else{nativeKey(VK_RETURN,false);heldKey=true;}
            co_await wait([events]{return *events>0;},L"Candidate Enter did not reach the native dialog route");
            *complete={};if(completion){entry.RemoveHandler(UIElement::PreviewKeyDownEvent(),completion);completion=nullptr;}
            if(completionFailure)std::rethrow_exception(completionFailure);
            if(mode==DefaultIme::Active){
                require(textComposing(entry)&&*primary==0,L"Active composition invoked the native default button");
                require(FocusManager::GetFocusedElement(root.XamlRoot())==entry&&entry.Text()==L"\u5019\u88dcDraft",L"Candidate Enter changed the native default editor or its text");
                require(!*published,L"Localization published before physical candidate confirmation release");
                composition.end();co_await wait([entry]{return !textComposing(entry);},L"Native default composition did not finish");
                require(!*published,L"Completed composition published while confirming Enter was held");
            }else{
                co_await wait([closed,entry]{return *closed&&!entry.IsLoaded();},L"Passed Enter did not submit and unload its native form");
                require(*primary==1&&entry.Text()==L"\u5019\u88dcDraft",L"Passed Enter did not submit exact completed text once");
                require(!*busy,L"Retired ordinary Enter acquired language ownership without an IME press");
            }
            auto release=*events;nativeKey(VK_RETURN,true);heldKey=false;
            if(mode==DefaultIme::Active)co_await wait([events,release]{return *events>release;},L"Candidate Enter release did not reach the native dialog route");
            co_await wait([published]{return *published;},L"Completed native input did not admit publication");
            require(*primary==(mode==DefaultIme::Active?0u:1u),L"Native Enter release invoked an extra default action");
            nativeKey(VK_RETURN,false);nativeKey(VK_RETURN,true);
            if(mode==DefaultIme::Active)co_await wait([primary]{return *primary>0;},L"Ordinary Enter did not recover the native default button");
            co_await settle();require(*primary==1,L"Native Enter invoked duplicate default actions");composition.finish();
            trace->push_back(O({{L"case",S(label)},{L"passed",B(true)}}));++imeChecks;
            if(!*closed)dialog.Hide();co_await wait([closed]{return *closed;},L"Native default dialog did not close");co_await shown;
        }catch(...){if(starter)entry.RemoveHandler(UIElement::PreviewKeyDownEvent(),starter);if(heldKey)try{nativeKey(VK_RETURN,true);}catch(...){}if(heldTrigger)try{nativeKey(VK_F24,true);}catch(...){}dialog.Hide();throw;}
        }
    }
    winrt::Windows::Foundation::IAsyncAction paperRegistryClosed(J model,J catalog,NativeLayoutInput layout){
        phase=L"paper-registry-setup";
        struct CloseFacts{winrt::weak_ref<Canvas> surface;bool armed=false,notified=false,applyAbsent=false,busy=false;hstring identity;std::exception_ptr failure;};
        auto facts=std::make_shared<CloseFacts>();auto copy=J::Parse(catalog.Stringify());copy.Insert(L"bootstrap",object(object(model,L"localization"),L"bootstrap"));
        auto send=[](std::string){};
        auto view=std::make_unique<WorkspaceView>(send,copy,context.localization,[](CanvasQueryKind,std::string,PreviewReply){return false;},[facts](bool open){
            if(open||!facts->armed)return;facts->notified=true;
            try{auto surface=facts->surface.get();require(bool(surface),L"Paper close observer lost its native root");
                auto status=J::Parse(AutomationProperties::GetItemStatus(surface));auto registered=object(status,L"texture_dialog");
                require(str(registered,L"id")==facts->identity,L"Paper close observer lost its owned registry identity");facts->busy=registered.Size()!=0;facts->applyAbsent=true;
                for(auto const& popup:VisualTreeHelper::GetOpenPopupsForXamlRoot(surface.XamlRoot()))if(findElement(popup.Child(),L"texture-apply"))facts->applyAbsent=false;
            }catch(...){facts->failure=std::current_exception();}
        },send,send,[](std::string,std::function<void(int32_t)>){ });
        auto surface=view->Root();facts->surface=make_weak(surface);auto previous=window.AppWindow().Size();auto width=root.Width(),height=root.Height();
        auto properties=object(object(model,L"state"),L"layer_properties");double layer=num(properties,L"layer"),epoch=num(object(object(model,L"state"),L"document_file"),L"epoch");
        Rect bounds{};bool propertyReady=false,tileReady=false;ContentDialog picker{nullptr};std::exception_ptr failure;
        try{
            require(flag(properties,L"enabled"),L"Paper selected-layer Properties are disabled");
            RECT client{};check_bool(GetClientRect(hwnd,&client));auto scale=root.XamlRoot().RasterizationScale();
            root.Width(1000);root.Height(780);surface.Width(1000);surface.Height(700);root.Children().InsertAt(0,surface);
            window.AppWindow().Resize({int32_t(std::ceil(1000*scale))+previous.Width-(client.right-client.left),int32_t(std::ceil(780*scale))+previous.Height-(client.bottom-client.top)});
            require(view->Apply(model,layout),L"Paper real shared model was rejected");view->Resize(1000,700);
            co_await wait([&]{auto control=findElement(surface,L"property-texture").try_as<Button>();propertyReady=control&&control.Visibility()==Visibility::Visible&&control.IsLoaded()&&control.IsEnabled()&&control.ActualWidth()>0&&control.ActualHeight()>0;return propertyReady;},L"Paper native property did not become ready");
            auto property=findElement(surface,L"property-texture");bounds=property.TransformToVisual(surface).TransformBounds({0,0,float(property.ActualWidth()),float(property.ActualHeight())});
            require(bounds.X>=0&&bounds.Y>=0&&bounds.X+bounds.Width<=surface.ActualWidth()&&bounds.Y+bounds.Height<=surface.ActualHeight(),L"Paper native property lies outside its actual workspace");
            require(!workspaceElement(L"texture-dialog"),L"Paper test began with another picker");privacy();
            auto invoke=[](FrameworkElement const& node){auto peer=Automation::Peers::FrameworkElementAutomationPeer::CreatePeerForElement(node);peer.GetPattern(Automation::Peers::PatternInterface::Invoke).as<Automation::Provider::IInvokeProvider>().Invoke();};
            invoke(property);
            co_await wait([&]{picker=workspaceElement(L"texture-dialog").try_as<ContentDialog>();auto tile=workspaceElement(L"texture-tile-0");tileReady=tile&&tile.IsLoaded()&&tile.ActualWidth()>0;return picker&&picker.IsLoaded()&&tileReady;},L"Paper real chooser and tile did not open");
            phase=L"paper-registry-open";auto status=J::Parse(AutomationProperties::GetItemStatus(surface));
            auto value=status.GetNamedValue(L"texture_dialog",JsonValue::CreateNullValue());
            require(value.ValueType()==JsonValueType::Object,L"Paper provider did not publish its owned native registry acknowledgement");auto registered=value.GetObject();facts->identity=str(registered,L"id");
            require(!facts->identity.empty()&&num(registered,L"epoch",-1)==epoch&&num(registered,L"layer",-1)==layer,L"Paper provider did not identify its admitted picker");auto admitted=num(status,L"snapshot_qpc");require(admitted>0,L"Paper native trace has no snapshot QPC");
            auto foreign=std::make_unique<WorkspaceView>(send,copy,context.localization,[](CanvasQueryKind,std::string,PreviewReply){return false;},[](bool){},send,send,[](std::string,std::function<void(int32_t)>){ });
            require(foreign->Apply(model,layout),L"Paper foreign workspace model was rejected");foreign->Resize(1000,700);
            auto foreignValue=J::Parse(AutomationProperties::GetItemStatus(foreign->Root())).GetNamedValue(L"texture_dialog",JsonValue::CreateNullValue());
            require(foreignValue.ValueType()==JsonValueType::Object&&foreignValue.GetObject().Size()==0,L"Paper registry publication leaked a different workspace owner's picker");
            auto closed=std::make_shared<bool>(false);auto revoker=picker.Closed(auto_revoke,[closed](auto&&,auto&&){*closed=true;});facts->armed=true;
            invoke(workspaceElement(L"texture-cancel"));co_await wait([closed]{return *closed;},L"Paper native Closed event did not arrive");if(facts->failure)std::rethrow_exception(facts->failure);
            require(facts->notified&&facts->applyAbsent&&facts->busy,L"Paper pre-finish boundary did not retain registry busy after its popup Apply disappeared");
            status=J::Parse(AutomationProperties::GetItemStatus(surface));value=status.GetNamedValue(L"texture_dialog",JsonValue::CreateNullValue());
            require(value.ValueType()==JsonValueType::Object&&value.GetObject().Size()==0&&num(status,L"snapshot_qpc")>admitted,L"Paper native Closed did not publish fresh owned registry-empty acknowledgement");++checks;
        }catch(hresult_error const& error){
            wchar_t hex[16]{};swprintf_s(hex,L" [0x%08X]",uint32_t(error.code().value));
            auto detail=L" [layer="+to_hstring(layer)+L" epoch="+to_hstring(epoch)+L" enabled="+to_hstring(flag(properties,L"enabled"))+L" propertyReady="+to_hstring(propertyReady)+L" bounds="+to_hstring(bounds.X)+L","+to_hstring(bounds.Y)+L","+to_hstring(bounds.Width)+L","+to_hstring(bounds.Height)+L" tileReady="+to_hstring(tileReady)+L" closeNotified="+to_hstring(facts->notified)+L" applyAbsent="+to_hstring(facts->applyAbsent)+L" busy="+to_hstring(facts->busy)+L"]";
            failure=std::make_exception_ptr(hresult_error(error.code(),hstring(std::wstring(error.message().c_str())+detail.c_str()+hex)));
        }catch(...){failure=std::current_exception();}
        facts->armed=false;if(picker)picker.Hide();
        try{co_await wait([this]{return !workspaceElement(L"texture-dialog");},L"Paper fixture cleanup retained its native chooser");}catch(...){if(!failure)failure=std::current_exception();}
        uint32_t at;if(root.Children().IndexOf(surface,at))root.Children().RemoveAt(at);view.reset();root.Width(width);root.Height(height);window.AppWindow().Resize(previous);
        if(failure)std::rethrow_exception(failure);
    }
    winrt::Windows::Foundation::IAsyncAction paperNameIme(){
        auto source=std::make_shared<WorkspaceData>();context.apply(source);
        A actions,tiles,controls;
        actions.Append(O({{L"label",S(L"Rename")},{L"action",O({{L"op",S(L"rename_paper")},{L"id",S(L"fixture-paper")},{L"name",S(L"Fixture scan")}})}}));
        tiles.Append(O({{L"id",S(L"fixture-paper")},{L"label",S(L"Fixture scan")},{L"actions",actions}}));
        controls.Append(O({{L"kind",O({{L"kind",S(L"choice")}})},{L"key",S(L"paper")},{L"value",O({{L"value",N(0)}})},{L"paper",O({{L"tiles",tiles}})}}));
        source->state=O({{L"theme",S(root.RequestedTheme()==ElementTheme::Light?L"light":L"dark")},{L"document_file",O({{L"epoch",N(1)}})},
            {L"layer_properties",O({{L"layer",N(1)},{L"enabled",B(true)},{L"controls",controls}})}});
        auto sent=std::make_shared<std::vector<J>>();source->send=[sent](std::string json){sent->push_back(J::Parse(to_hstring(json)));};
        auto invoke=[](FrameworkElement const& element){auto peer=Automation::Peers::FrameworkElementAutomationPeer::CreatePeerForElement(element);
            peer.GetPattern(Automation::Peers::PatternInterface::Invoke).as<Automation::Provider::IInvokeProvider>().Invoke();};
        ChooseTexture(source,anchor,1);
        co_await wait([source]{return flag(TextureDialogState(source),L"ready");},L"Paper native chooser did not acknowledge Opened and initial focus");
        co_await wait([this]{auto tile=workspaceElement(L"texture-tile-0").try_as<Button>();return tile&&tile.IsLoaded()&&tile.ActualWidth()>0;},L"Paper fixture tile did not arrange");
        auto tile=workspaceElement(L"texture-tile-0").as<Button>();auto menu=tile.ContextFlyout().as<MenuFlyout>();auto command=menu.Items().GetAt(0).as<MenuFlyoutItem>();
        auto opened=std::make_shared<bool>(false);menu.Opened([opened](auto&&,auto&&){*opened=true;});menu.ShowAt(tile);
        co_await wait([command,opened]{return *opened&&command.IsLoaded()&&command.ActualWidth()>0;},L"Paper rename context command did not open");
        invoke(command);
        co_await wait([this]{auto entry=workspaceElement(L"paper-rename-name").try_as<TextBox>();return entry&&entry.IsLoaded()&&entry.ActualWidth()>0;},L"Real paper name prompt did not arrange");
        auto entry=workspaceElement(L"paper-rename-name").as<TextBox>();auto prompt=workspaceElement(L"paper-rename-dialog").as<ContentDialog>();
        auto complete=std::make_shared<std::function<void()>>();Windows::Foundation::IInspectable completion{nullptr},starter{nullptr};
        CompletionRoute route{prompt,{&completion,&starter},complete};bool heldKey=false;
        completion=box_value(KeyEventHandler([complete](auto&&,KeyRoutedEventArgs const& e){if(e.Key()==Windows::System::VirtualKey::Enter&&*complete)(*complete)();}));
        prompt.AddHandler(UIElement::PreviewKeyDownEvent(),completion,true);
        try{
            captureTextComposition(entry);entry.Focus(FocusState::Keyboard);entry.Text(L"");
            co_await wait([entry,prompt]{return entry.Text().empty()&&!prompt.IsPrimaryButtonEnabled();},L"Empty paper name did not disable native Primary");
            require(sent->empty(),L"Paper rename dispatched before confirmation");
            ScopedComposition composition(*textServices,[trace=imeTrace](hstring api,HRESULT result,uintptr_t pointer){trace->push_back(O({{L"case",S(L"paper-name-ime")},{L"api",S(api)},{L"code",N(int32_t(result))},{L"pointer",N(double(pointer))}}));});
            co_await wait([this,entry,&composition]{return composition.focused(entry,root.XamlRoot());},L"Paper name did not acquire its native TSF context");
            composition.begin(L" ");
            co_await wait([entry,prompt]{return textComposing(entry)&&entry.Text()==L" "&&!prompt.IsPrimaryButtonEnabled();},L"Blank marked paper name did not retain disabled Primary");
            auto triggered=std::make_shared<bool>(false);std::exception_ptr failure;
            *complete=[this,entry,prompt,&composition,&failure,triggered]{
                try{
                    require(FocusManager::GetFocusedElement(root.XamlRoot())==entry&&!prompt.IsPrimaryButtonEnabled(),L"Paper name lost its disabled native default precondition");
                    composition.edit(L"paper-name",[&composition](TfEditCookie cookie){com_ptr<ITfRange> range;check_hresult(composition.composition->GetRange(range.put()));check_hresult(range->SetText(cookie,0,L"\ud55c\uae00",2));});composition.end();
                    require(!textComposing(entry)&&entry.Text()==L"\ud55c\uae00",L"Paper Return did not follow full synchronous Korean TSF completion");
                }catch(...){failure=std::current_exception();}
                *triggered=true;
            };
            nativeKey(VK_RETURN,false);heldKey=true;
            co_await wait([triggered]{return *triggered;},L"Paper TSF completion trigger did not reach native input");
            *complete={};if(failure)std::rethrow_exception(failure);
            co_await wait([sent,entry]{return sent->size()==1&&!entry.IsLoaded();},L"Passed Korean Enter did not submit the real paper name prompt");
            auto request=sent->front();auto effect=object(request,L"action");auto rename=object(effect,L"action");
            require(str(request,L"windows_epoch")==L"1"&&str(effect,L"type")==L"effect"&&str(rename,L"op")==L"rename_paper"&&str(rename,L"id")==L"fixture-paper"&&str(rename,L"name")==L"\ud55c\uae00",
                L"Paper default action did not preserve exact full name and original identity");
            nativeKey(VK_RETURN,true);heldKey=false;co_await settle();require(sent->size()==1,L"Paper Return release submitted a duplicate rename");composition.finish();
            co_await wait([this]{auto cancel=workspaceElement(L"texture-cancel").try_as<Button>();return cancel&&cancel.IsLoaded();},L"Paper chooser did not return after rename");
            invoke(workspaceElement(L"texture-cancel").as<FrameworkElement>());
            co_await wait([this]{return !workspaceElement(L"texture-dialog");},L"Paper chooser did not close");++imeChecks;
        }catch(...){if(heldKey)try{nativeKey(VK_RETURN,true);}catch(...){}prompt.Hide();throw;}
    }
    winrt::Windows::Foundation::IAsyncAction shortcutSearchIme(){
        auto source=std::make_shared<WorkspaceData>();context.apply(source);source->state=O({{L"theme",S(root.RequestedTheme()==ElementTheme::Light?L"light":L"dark")}});
        auto actions=std::make_shared<std::vector<J>>();source->send=[actions](std::string value){actions->push_back(J::Parse(to_hstring(value)));};
        auto chords=[actions]{return std::count_if(actions->begin(),actions->end(),[](J const& action){return str(object(action,L"action"),L"type")==L"search_shortcut_key";});};
        auto previous=window.AppWindow().Size();Grid overlay;StackPanel content;ShortcutPage page(source,overlay);page.Container(L"shortcuts",content);
        ScrollViewer scroll;scroll.Content(content);scroll.Height(600);root.Width(800);root.Height(760);root.Children().InsertAt(0,scroll);window.AppWindow().Resize({900,900});
        co_await wait([&]{auto field=findElement(content,L"shortcuts-search").try_as<TextBox>();return field&&field.IsLoaded()&&field.ActualWidth()>0;},L"Shortcut search did not arrange");
        auto entry=findElement(content,L"shortcuts-search").as<TextBox>();captureTextComposition(entry);entry.Text(L"Draft");entry.SelectAll();entry.Focus(FocusState::Keyboard);
        ScopedComposition composition(*textServices,[trace=imeTrace](hstring api,HRESULT result,uintptr_t pointer){trace->push_back(O({{L"case",S(L"shortcut-search-ime")},{L"api",S(api)},{L"hresult",N(double(result))},{L"pointer",S(to_hstring(uint64_t(pointer)))}}));});
        co_await wait([this,entry,&composition]{return composition.focused(entry,root.XamlRoot());},L"Shortcut search has no native TSF context");composition.begin();
        co_await wait([entry]{return textComposing(entry)&&entry.Text()==L"\u5019\u88dc";},L"Shortcut search did not retain exact marked text");
        auto text=entry.Text();auto start=entry.SelectionStart(),length=entry.SelectionLength();actions->clear();
        auto presses=std::make_shared<unsigned>(0);entry.AddHandler(UIElement::PreviewKeyDownEvent(),box_value(KeyEventHandler([presses](auto&&,KeyRoutedEventArgs const& e){if(e.Key()==Windows::System::VirtualKey::F7)++*presses;})),true);
        nativeKey(VK_F7,false);nativeKey(VK_F7,true);co_await wait([presses]{return *presses==1;},L"Native conversion F7 did not reach shortcut search");
        require(chords()==0,L"Composition-owned F7 dispatched a shortcut search");
        require(entry.Text()==text&&entry.SelectionStart()==start&&entry.SelectionLength()==length,L"Composition-owned F7 changed the native draft or selection");
        require(textComposing(entry)&&FocusManager::GetFocusedElement(root.XamlRoot())==entry,L"Composition-owned F7 retired composition or focus");
        composition.finish();co_await wait([entry]{return !textComposing(entry);},L"Shortcut search composition did not retire");actions->clear();
        nativeKey(VK_F7,false);nativeKey(VK_F7,true);co_await wait([chords]{return chords()==1;},L"Ordinary F7 did not search shortcuts");
        require(chords()==1,L"Ordinary F7 did not dispatch exactly one shortcut search");
        require(entry.Text()==text&&entry.SelectionStart()==0&&entry.SelectionLength()==int32_t(text.size()),L"Ordinary F7 did not preserve and select the native search text");
        anchor.Focus(FocusState::Programmatic);root.Children().RemoveAt(0);root.Width(400);root.Height(240);window.AppWindow().Resize(previous);++imeChecks;
    }
    winrt::Windows::Foundation::IAsyncAction customizationDraft(){
        auto actions=std::make_shared<std::vector<J>>();auto errors=std::make_shared<std::vector<std::string>>();
        auto prompt=O({{L"panel",S(L"fixture-toolbar")},{L"operation",S(L"rename")},{L"title",S(L"Rename Toolbar")},{L"name",S(L"Acknowledged A")},
            {L"name_label",S(L"Toolbar name")},{L"confirm_label",S(L"Rename")},{L"cancel_label",S(L"Cancel")},{L"can_confirm",B(true)}});
        auto snapshot=O({{L"state",O({{L"theme",S(root.RequestedTheme()==ElementTheme::Light?L"light":L"dark")}})},{L"toolbar_prompt",prompt}});
        WorkspaceDialogs dialogs([actions](std::string value){actions->push_back(J::Parse(to_hstring(value)));},data->catalog,context.localization,root.XamlRoot(),[]{},[errors](std::string value){errors->push_back(std::move(value));});
        auto apply=[&]{dialogs.Apply(J::Parse(snapshot.Stringify()),false);};bool heldKey=false;
        try{
            apply();co_await wait([this]{auto entry=workspaceElement(L"toolbar-name").try_as<TextBox>();return entry&&entry.IsLoaded()&&entry.ActualWidth()>0;},L"Customization name did not become arranged");
            auto entry=workspaceElement(L"toolbar-name").as<TextBox>();auto dialog=workspaceElement(L"toolbar-prompt").as<ContentDialog>();
            auto primary=workspaceElement(L"PrimaryButton").as<Button>();entry.Focus(FocusState::Keyboard);
            co_await wait([this,entry]{return FocusManager::GetFocusedElement(root.XamlRoot())==entry;},L"Customization name did not own native focus");privacy();
            entry.Text(L"Pending B");entry.Text(L"Acknowledged A");
            require(actions->size()==2&&str(object(actions->at(0),L"action"),L"name")==L"Pending B"&&str(object(actions->at(1),L"action"),L"name")==L"Acknowledged A",L"Returning to an older shared name lost a newer native edit");
            prompt.Insert(L"name",S(L"Pending B"));apply();
            require(entry.Text()==L"Acknowledged A"&&primary.IsEnabled(),L"Delayed acknowledgement replaced or blocked a newer native edit");
            prompt.Insert(L"name",S(L"Acknowledged A"));apply();require(primary.IsEnabled(),L"Latest name acknowledgement did not enable confirmation");actions->clear();entry.SelectAll();
            ScopedComposition composition(*textServices,[trace=imeTrace](hstring api,HRESULT result,uintptr_t pointer){trace->push_back(O({{L"case",S(L"customization-name-ack")},{L"api",S(api)},{L"hresult",N(double(result))},{L"pointer",S(to_hstring(uint64_t(pointer)))}}));});
            co_await wait([this,entry,&composition]{return composition.focused(entry,root.XamlRoot());},L"Customization name has no native TSF context");composition.begin();
            co_await wait([this,entry]{return entry.Text()==L"\u5019\u88dc"&&textComposing(entry)&&FocusManager::GetFocusedElement(root.XamlRoot())==entry;},L"Native customization composition did not publish its exact marked name");
            auto acknowledged=actions->size();require(acknowledged>0&&str(object(actions->back(),L"action"),L"name")==L"\u5019\u88dc",L"Native composition did not dispatch its exact authored name");
            prompt.Insert(L"name",S(entry.Text()));apply();require(primary.IsEnabled()&&textComposing(entry),L"Marked name acknowledgement did not retain enabled native confirmation");
            auto notifications=std::make_shared<unsigned>(0);entry.TextChanging([notifications](TextBox const& sender,auto&&){if(sender.Text()==L"\u5019\u88dc")++*notifications;});
            composition.edit(L"same-name",[&composition](TfEditCookie cookie){com_ptr<ITfRange> range;composition.checked(L"ITfComposition::GetRange",composition.composition->GetRange(range.put()));composition.checked(L"ITfRange::SetText/same-name",range->SetText(cookie,0,L"\u5019\u88dc",2));composition.checked(L"ITfRange::Collapse/same-name",range->Collapse(cookie,TF_ANCHOR_END));TF_SELECTION selection{};selection.range=range.get();selection.style.ase=TF_AE_END;composition.checked(L"ITfContext::SetSelection/same-name",composition.context->SetSelection(cookie,1,&selection));});
            composition.end();co_await settle();co_await wait([entry]{return !textComposing(entry);},L"Same-name native composition did not retire");
            require(*notifications>0,L"Same-value native TSF edit did not produce TextChanging");
            require(primary.IsEnabled(),L"Acknowledged native name completion disabled confirmation");
            require(actions->size()==acknowledged,L"Same acknowledged native name dispatched a phantom edit");
            require(dialogs.IsOpen(),L"Same native name completion closed its dialog");
            require(workspaceElement(L"toolbar-name")==entry,L"Same native name completion replaced its editor");
            require(FocusManager::GetFocusedElement(root.XamlRoot())==entry,L"Same native name completion moved native focus");
            require(entry.Text()==L"\u5019\u88dc",(L"Same native name completion changed exact text: "+entry.Text()).c_str());
            require(entry.SelectionStart()==2&&entry.SelectionLength()==0,(L"Same native name completion changed caret: "+to_hstring(entry.SelectionStart())+L","+to_hstring(entry.SelectionLength())).c_str());
            prompt.Insert(L"name",S(L""));prompt.Insert(L"can_confirm",B(false));apply();actions->clear();entry.Text(L"\ud55c\uae00");
            require(primary.IsEnabled()&&actions->size()==1&&str(object(actions->front(),L"action"),L"name")==L"\ud55c\uae00",L"Unacknowledged native Korean name was not ordered before submission");
            nativeKey(VK_RETURN,false);heldKey=true;nativeKey(VK_RETURN,true);heldKey=false;co_await wait([actions]{return actions->size()==2;},L"Single Enter dropped an unacknowledged Korean name");
            require(str(object(actions->back(),L"action"),L"type")==L"confirm_toolbar"&&errors->empty(),L"Korean name produced an incorrect or duplicate confirmation");composition.finish();++imeChecks;
            dialogs.Hide();co_await wait([&dialogs]{return !dialogs.IsOpen();},L"Customization name fixture did not close");
        }catch(...){if(heldKey)try{nativeKey(VK_RETURN,true);}catch(...){}dialogs.Hide();throw;}
    }
    winrt::Windows::Foundation::IAsyncOperation<hstring> workspaceFixtureCatalog(){
        std::string catalog;
        co_await resume_background();
        {
            struct Apartment{Apartment(){init_apartment(apartment_type::multi_threaded);}~Apartment(){uninit_apartment();}} apartment;
            NativeLaunch launch(capy_launch("[\"en\"]",nullptr));require(bool(launch),L"Workspace fixture launch failed");
            NativeHandle<capy_destroy> host(capy_create(1000,700,1,launch.get()));require(bool(host),L"Workspace fixture host failed");
            NativeString copy(capy_query(host.get(),R"({"type":"catalog"})"));require(bool(copy),L"Workspace fixture production catalog missing");catalog=std::move(copy.bytes);
        }
        co_await apartment;
        auto value=J::Parse(to_hstring(catalog));value.Insert(L"bootstrap",object(data->catalog,L"bootstrap"));co_return value.Stringify();
    }
    winrt::Windows::Foundation::IAsyncAction workspaceIme(){
        auto dark=root.RequestedTheme()!=ElementTheme::Light;
        auto header=O({{L"this_workspace",S(L"This workspace")},{L"saved_toolbars",S(L"Saved toolbars")},{L"toolbar_actions",S(L"Actions")},
            {L"new_workspace",S(L"New Workspace")},{L"new_toolbar",S(L"New Toolbar")},{L"search",S(L"Search")},{L"description",S(L"Description")},
            {L"retry",S(L"Retry")},{L"move_up",S(L"Move Up")},{L"move_down",S(L"Move Down")},{L"current_workspace",S(L"Current workspace")},{L"drag_to_reorder",S(L"Drag to reorder")}});
        auto catalog=J::Parse(co_await workspaceFixtureCatalog());catalog.Insert(L"native_copy",O({{L"header",header}}));
        require(catalog.HasKey(L"panel_metrics"),L"Workspace fixture production panel metrics missing");
        require(num(object(catalog,L"panel_metrics"),L"layer_swipe_action_width")>0,L"Workspace fixture production swipe width must be non-zero");
        auto threshold=num(object(catalog,L"panel_metrics"),L"layer_swipe_threshold");require(threshold>0&&threshold<1,L"Workspace fixture production swipe threshold is invalid");
        auto palette=O({{L"text",S(dark?L"#eeeeee":L"#111111")},{L"card",S(dark?L"#343434":L"#eeeeee")},{L"accent",S(L"#4488cc")},{L"accent_foreground",S(L"#ffffff")}});
        auto state=O({{L"theme",S(dark?L"dark":L"light")},{L"palette",palette}});
        A options;for(auto type:{L"rename",L"delete"})options.Append(O({{L"label",S(type)},{L"enabled",B(true)},{L"action",O({{L"type",S(type)},{L"value",S(L"fixture-custom")}})}}));
        A rows;rows.Append(O({{L"id",S(L"fixture-custom")},{L"title",S(L"Fixture workspace")},{L"subtitle",S(L"")},{L"current",B(true)},
            {L"pin",O({{L"selected",B(true)},{L"enabled",B(true)},{L"label",S(L"Hide from top bar")},{L"icon",S(L"pin")},{L"edit",O({{L"type",S(L"toggle")},{L"id",S(L"fixture-custom")}})}})},
            {L"options",options}}));
        A order;order.Append(S(L"fixture-custom"));
        auto model=O({{L"id",S(L"fixture-custom")},{L"name",S(L"Fixture workspace")},{L"ready",B(true)},{L"busy",B(false)},{L"loading",B(false)},
            {L"page",S(L"workspaces")},{L"selected",S(L"fixture-custom")},{L"rows",rows},{L"order",order},{L"title",S(L"Workspaces")},{L"primary",S(L"Switch to Workspace")},{L"enabled",B(false)}});
        auto trace=imeTrace;
        auto close=[this](WorkspaceManagerView& manager)->winrt::Windows::Foundation::IAsyncAction{
            manager.Hide();co_await wait([&manager]{return !manager.IsOpen();},L"Workspace IME dialog did not close");
        };
        {
            phase=L"workspace-grip-route";
            WorkspaceManagerView manager([](std::string,std::function<void()> complete){if(complete)complete();},catalog,context.localization,root.XamlRoot(),[]{},
                [](CanvasQueryKind,std::string,PreviewReply reply){reply(PreviewPacket{});return true;});
            try{
                manager.Apply(O({{L"state",state},{L"windows_workspace",model}}),false);
                co_await wait([this]{auto grip=workspaceElement(L"workspace-manager-grip-fixture-custom").try_as<Button>();return grip&&grip.IsLoaded()&&grip.ActualWidth()>0;},L"Workspace reorder grip did not arrange");
                auto grip=workspaceElement(L"workspace-manager-grip-fixture-custom").as<Button>();
                auto swipe=workspaceElement(L"workspace-manager-swipe-fixture-custom").as<SwipeControl>();
                auto row=workspaceElement(L"workspace-manager-row-fixture-custom").as<ListViewItem>();
                require(!descendantOf(grip,swipe),L"Workspace reorder grip is inside the SwipeControl touch redirect route");
                require(descendantOf(grip,row)&&descendantOf(swipe,row)&&VisualTreeHelper::GetParent(grip)==VisualTreeHelper::GetParent(swipe),L"Workspace grip and swipe body do not share their row layout");
                auto slot=workspaceElement(L"workspace-name-slot-fixture-custom");auto pin=workspaceElement(L"workspace-manager-pin-fixture-custom");
                require(descendantOf(slot,swipe)&&descendantOf(pin,swipe),L"Workspace body controls left their swipe route");
                auto actions=swipe.RightItems();require(actions&&actions.Mode()==SwipeMode::Reveal&&actions.Size()==1&&AutomationProperties::GetAutomationId(actions.GetAt(0))==L"workspace-manager-swipe-delete-fixture-custom",L"Workspace body lost its native Reveal Delete action");
                ++checks;co_await close(manager);
            }catch(...){manager.Hide();throw;}
            phase=L"workspace-ime";
        }
        for(auto [key,label]:{std::pair<WORD,wchar_t const*>{WORD(VK_RETURN),L"inline-enter"},{WORD(VK_ESCAPE),L"inline-escape"},{WORD(VK_F2),L"inline-f2"},{WORD(VK_ESCAPE),L"form-escape"},{WORD(0),L"form-primary"},{WORD(VK_RETURN),L"retired-ordinary-korean-enter"},{WORD(VK_ESCAPE),L"retired-ordinary-inline-escape"},{WORD(VK_ESCAPE),L"retired-ordinary-form-escape"}}){
            bool retired=label[0]==L'r',heldKey=false,inlineName=label[0]==L'i'||hstring(label)==L"retired-ordinary-inline-escape";auto inputs=std::make_shared<std::vector<J>>();auto events=std::make_shared<unsigned>(0);
            auto activeModel=J::Parse(model.Stringify());
            activeModel.Insert(L"prompt_action",inlineName?O({{L"type",S(L"rename")},{L"value",S(L"fixture-custom")}}):O({{L"type",S(L"new")}}));
            activeModel.Insert(L"prompt",O({{L"title",S(L"Workspace name")},{L"name",S(L"Fixture draft")},{L"confirm",S(L"Create")},{L"message",S(L"")},{L"destructive",B(false)}}));
            WorkspaceManagerView manager([inputs,trace,label](std::string json,std::function<void()> complete){auto request=J::Parse(to_hstring(json));inputs->push_back(request);trace->push_back(O({{L"case",S(label)},{L"dispatch",request}}));if(complete)complete();},catalog,context.localization,root.XamlRoot(),[]{},
                [](CanvasQueryKind,std::string,PreviewReply reply){reply(PreviewPacket{});return true;});
            try{
                manager.Apply(O({{L"state",state},{L"windows_workspace",activeModel}}),false);
                auto id=inlineName?L"workspace-manager-rename-fixture-custom":L"workspace-manager-name";
                co_await wait([this,id]{auto entry=workspaceElement(id).try_as<TextBox>();return entry&&entry.IsLoaded()&&entry.ActualWidth()>0;},L"Workspace IME TextBox did not become arranged");
                auto entry=workspaceElement(id).as<TextBox>();auto busy=std::make_shared<bool>(false);captureTextComposition(entry,[busy](bool value){*busy=value;});entry.Focus(FocusState::Keyboard);
                co_await wait([this,entry]{return FocusManager::GetFocusedElement(root.XamlRoot())==entry;},L"Workspace IME TextBox did not own focus");privacy();
                entry.AddHandler(UIElement::KeyDownEvent(),box_value(KeyEventHandler([trace,events,label,editor=make_weak(entry)](auto&&,KeyRoutedEventArgs const& e){auto entry=editor.get();
                    ++*events;trace->push_back(O({{L"case",S(label)},{L"key",N(uint32_t(e.Key()))},{L"composing",B(textComposing(entry))},{L"handled",B(e.Handled())},{L"time",N(uint32_t(GetMessageTime()))}}));
                })),true);
                trace->push_back(O({{L"case",S(label)},{L"composition_requested",B(true)}}));
                auto dialog=workspaceElement(L"workspace-manager").as<ContentDialog>();
                std::vector<std::pair<UIElement,hstring>> routes{{dialog,L"dialog"}};
                if(inlineName)routes.emplace_back(workspaceElement(L"workspace-manager-items").as<UIElement>(),L"list");
                for(auto [control,route]:routes){
                    control.AddHandler(UIElement::KeyDownEvent(),box_value(KeyEventHandler([trace,label,route,editor=make_weak(entry)](auto&&,KeyRoutedEventArgs const& e){
                        trace->push_back(O({{L"case",S(label)},{L"route",S(route)},{L"key",N(uint32_t(e.Key()))},{L"handled",B(e.Handled())},{L"composing",B(textComposing(editor.get()))}}));
                    })),true);
                }
                dialog.Closing([trace,label,editor=make_weak(entry)](auto&&,ContentDialogClosingEventArgs const& e){trace->push_back(O({{L"case",S(label)},{L"native_event",S(L"Closing")},{L"canceled",B(e.Cancel())},{L"composing",B(textComposing(editor.get()))}}));});
                dialog.CloseButtonClick([trace,label,editor=make_weak(entry)](auto&&,ContentDialogButtonClickEventArgs const& e){trace->push_back(O({{L"case",S(label)},{L"native_event",S(L"CloseButtonClick")},{L"canceled",B(e.Cancel())},{L"composing",B(textComposing(editor.get()))}}));});
                dialog.PrimaryButtonClick([trace,label,editor=make_weak(entry)](auto&&,ContentDialogButtonClickEventArgs const& e){trace->push_back(O({{L"case",S(label)},{L"native_event",S(L"PrimaryButtonClick")},{L"canceled",B(e.Cancel())},{L"composing",B(textComposing(editor.get()))}}));});
                trace->push_back(O({{L"case",S(label)},{L"module_path",S(hstring(textServices->path))},{L"module_worker",N(textServices->worker)},{L"ui_thread",N(GetCurrentThreadId())}}));
                auto complete=std::make_shared<std::function<void()>>();Windows::Foundation::IInspectable completion{nullptr},starter{nullptr};
                if(retired){completion=box_value(KeyEventHandler([complete,key](auto&&,KeyRoutedEventArgs const& e){if(uint32_t(e.Key())==key&&*complete)(*complete)();}));dialog.AddHandler(UIElement::PreviewKeyDownEvent(),completion,true);}
                ScopedComposition composition(*textServices,[trace,label](hstring api,HRESULT result,uintptr_t pointer){
                    wchar_t hex[11]{};swprintf_s(hex,L"0x%08X",static_cast<unsigned>(result));
                    trace->push_back(O({{L"case",S(label)},{L"api",S(api)},{L"hresult",S(hex)},{L"code",N(int32_t(result))},{L"pointer",N(double(pointer))},{L"thread",N(GetCurrentThreadId())},{L"time",N(double(GetTickCount64()))}}));
                });
                CompletionRoute completionRoute{dialog,{&completion,&starter},complete};std::exception_ptr completionFailure;
                if(retired)*complete=[entry,&composition,&completionFailure]{try{composition.end();require(!textComposing(entry)&&entry.Text()==L"\ud55c\uae00",L"Passed key did not retain the full committed Korean name");}catch(...){completionFailure=std::current_exception();}};
                if(retired)entry.SelectAll();
                co_await wait([this,entry,&composition]{return composition.focused(entry,root.XamlRoot());},L"Owned TextBox real TSF document/context did not become focused within the existing five-second prerequisite");
                trace->push_back(O({{L"case",S(label)},{L"focus_probes",N(composition.probes)},{L"context_ready",B(true)}}));composition.begin(retired?L"\ud55c\uae00":L"\u5019\u88dc");
                co_await wait([entry]{return textComposing(entry);},L"Actual TSF composition did not reach the production TextBox observer");
                auto draft=entry.Text();auto selection=entry.SelectionStart();auto length=entry.SelectionLength();
                updateText(entry,L"Stale shared snapshot");
                require(entry.Text()==draft&&entry.SelectionStart()==selection&&entry.SelectionLength()==length,
                    L"Shared snapshot replaced a native composition or its selection");
                require(focusedTextComposing(root.XamlRoot()),L"Workspace TSF composition is not on the focused owned TextBox");
                trace->push_back(O({{L"case",S(label)},{L"composition_context",N(double(reinterpret_cast<uintptr_t>(composition.context.get())))},{L"preedit",S(entry.Text())},{L"started",B(true)}}));
                if(key){
                    nativeKey(key,false);heldKey=true;co_await wait([events]{return *events>0;},L"Workspace composition key did not reach the actual TextBox route");co_await settle();
                }else{
                    co_await wait([this]{auto button=workspaceElement(L"PrimaryButton").try_as<Button>();return button&&button.IsEnabled();},L"Workspace native primary button was not enabled");
                    auto peer=winrt::Microsoft::UI::Xaml::Automation::Peers::ButtonAutomationPeer(workspaceElement(L"PrimaryButton").as<Button>());
                    peer.GetPattern(winrt::Microsoft::UI::Xaml::Automation::Peers::PatternInterface::Invoke).as<winrt::Microsoft::UI::Xaml::Automation::Provider::IInvokeProvider>().Invoke();co_await settle();
                    require(focusedTextComposing(root.XamlRoot()),L"Native primary invocation lost its composing TextBox prerequisite");
                }
                if(retired){
                    if(completionFailure)std::rethrow_exception(completionFailure);
                    co_await wait([inputs]{return inputs->size()==1;},L"Passed Korean key did not perform its workspace action");
                    auto action=object(inputs->front(),L"input");
                    require(str(action,L"type")== (key==VK_ESCAPE?L"cancel":L"submit"),L"Passed Korean key performed an incorrect workspace action");
                    require(entry.Text()==L"\ud55c\uae00",L"Passed workspace key lost committed Korean text");
                    if(key==VK_RETURN)require(str(action,L"name")==L"\ud55c\uae00",L"Korean workspace Enter lost the final syllable");
                    if(key==VK_ESCAPE)manager.Apply(O({{L"state",state},{L"windows_workspace",model}}),false);
                    co_await close(manager);anchor.Focus(FocusState::Keyboard);
                    require(!*busy,L"Retired ordinary workspace key acquired language ownership without an IME press");
                    nativeKey(key,true);heldKey=false;co_await wait([busy]{return !*busy;},L"Ordinary workspace key left language publication busy");
                    nativeKey(key,false);heldKey=true;nativeKey(key,true);heldKey=false;co_await settle();
                    require(inputs->size()==1,L"A second key dispatched from a retired workspace form");composition.finish();++imeChecks;continue;
                }
                require(inputs->empty(),L"Composition-owned input dispatched a workspace submit, cancel, select, form or dismissal");
                require(manager.IsOpen()&&workspaceElement(id)==entry&&entry.IsLoaded(),L"Composition-owned input closed or replaced its native manager control");
                composition.end();co_await wait([entry]{return !textComposing(entry);},L"Workspace TSF composition did not finish");
                if(key){nativeKey(key,true);heldKey=false;co_await settle();require(inputs->empty(),L"Composition key release dispatched a workspace action");nativeKey(key,false);heldKey=true;nativeKey(key,true);heldKey=false;}
                else{
                    auto peer=winrt::Microsoft::UI::Xaml::Automation::Peers::ButtonAutomationPeer(workspaceElement(L"PrimaryButton").as<Button>());
                    peer.GetPattern(winrt::Microsoft::UI::Xaml::Automation::Peers::PatternInterface::Invoke).as<winrt::Microsoft::UI::Xaml::Automation::Provider::IInvokeProvider>().Invoke();
                }
                co_await wait([inputs]{return !inputs->empty();},L"Fresh ordinary workspace input did not recover after composition");co_await settle();
                std::vector<hstring> actual;for(auto const& request:*inputs){require(str(request,L"operation")==L"input",L"Workspace IME fixture received a host failure");actual.push_back(str(object(request,L"input"),L"type"));}
                std::vector<hstring> expected=key==VK_F2?std::vector<hstring>{L"submit",L"form"}:std::vector<hstring>{key==VK_ESCAPE?L"cancel":L"submit"};
                require(actual==expected,L"Ordinary workspace input produced duplicate or incorrect follow-up actions");
                if(key!=VK_ESCAPE)require(str(object(inputs->front(),L"input"),L"name")==entry.Text(),L"Workspace submitted an older TextBox draft");
                composition.finish();trace->push_back(O({{L"case",S(label)},{L"passed",B(true)}}));++imeChecks;co_await close(manager);
            }catch(...){if(heldKey)try{nativeKey(key,true);}catch(...){}manager.Hide();throw;}
        }
    }

    winrt::Windows::Foundation::IAsyncAction bubblePressTimestamps(){
        auto source=std::make_shared<WorkspaceData>();context.apply(source);
        auto actions=std::make_shared<std::vector<J>>();
        source->send=[actions](std::string json){actions->push_back(J::Parse(to_hstring(json)));};
        auto bubbles=std::make_shared<WorkspaceBubbles::Impl>();bubbles->data=source;bubbles->workspace=Canvas{};
        auto record=std::make_shared<CapyBubbles::Record>();record->id=17;
        using Device=winrt::Microsoft::UI::Input::PointerDeviceType;
        auto press=[&](hstring hit,uint64_t timestamp,Point at=Point{},Device device=Device::Pen){
            auto contact=std::make_shared<CapyBubbles::Contact>();contact->record=record;contact->hit=S(hit);contact->device=device;
            contact->started=CapyBubbles::milliseconds();bubbles->begin(contact,at,timestamp);return contact;
        };
        auto threshold=uint64_t(GetDoubleClickTime())*1000;
        for(auto hit:{hstring(L"ring"),hstring(L"intensity")}){
            auto downMode=hit==L"ring"?L"ring":L"intensity";
            auto resetType=hit==L"ring"?L"reset_ring":L"set_intensity";
            bubbles->lastRing.reset();actions->clear();
            press(hit,1000000);
            co_await winrt::resume_after(std::chrono::milliseconds(GetDoubleClickTime()+50));co_await apartment;
            auto repeated=press(hit,1000000+threshold/2);
            require(actions->size()==2,L"Delayed bubble presses dispatched extra actions");
            auto action=object(actions->back(),L"action");
            require(repeated->mode==L"reset",L"Delayed handling lost the bubble event double tap");
            require(str(action,L"type")==resetType,L"Bubble event double tap dispatched the wrong reset");
            if(hit==L"intensity")require(num(action,L"stops",-1)==0,L"Intensity double tap did not reset stops");
            bubbles->lastRing.reset();actions->clear();
            press(hit,2000000);auto separate=press(hit,2000000+threshold+1000);
            action=object(actions->back(),L"action");
            require(actions->size()==2&&separate->mode==downMode&&str(action,L"type")==hit&&str(action,L"phase")==L"down",L"Compressed handling invented a bubble event double tap");
            bubbles->lastRing.reset();actions->clear();
            press(hit,3000000);record->id=18;auto another=press(hit,3000000+threshold/2);record->id=17;
            action=object(actions->back(),L"action");
            require(another->mode==downMode&&str(action,L"type")==hit&&str(action,L"phase")==L"down",L"Bubble double tap crossed record identity");
            bubbles->lastRing.reset();actions->clear();
            auto slop=std::max(8.,double(std::max(GetSystemMetrics(SM_CXDOUBLECLK),GetSystemMetrics(SM_CYDOUBLECLK))));
            press(hit,4000000);auto distant=press(hit,4000000+threshold/2,{float(slop+1),0});
            action=object(actions->back(),L"action");
            require(distant->mode==downMode&&str(action,L"type")==hit&&str(action,L"phase")==L"down",L"Bubble double tap crossed its existing slop");
            bubbles->lastRing.reset();actions->clear();
            press(hit,5000000,{},Device::Mouse);auto pen=press(hit,5000000+threshold/2,{},Device::Pen);
            require(pen->mode==L"reset"&&str(object(actions->back(),L"action"),L"type")==resetType,L"Bubble event timing changed cross-device pairing");
            bubbles->lastRing.reset();actions->clear();
            auto resumed=press(hit,6000000);bubbles->restore(resumed);
            bubbles->begin(resumed,{},6000000+threshold+1000);
            action=object(actions->back(),L"action");
            require(resumed->mode==downMode&&str(action,L"type")==hit&&str(action,L"phase")==L"down",L"Resumed bubble reused its original press event time");
        }
    }

    winrt::fire_and_forget run(){
        auto self=shared_from_this();hstring error;
        try {
            co_await wait([this]{return anchor.IsLoaded()&&untagged.ActualWidth()>0&&GetForegroundWindow()==hwnd;},L"Fixture did not become arranged and foreground");privacy();
            phase=L"color-strip-presentation";co_await colorStripPresentation();
            phase=L"bubble-press-timestamps";co_await bubblePressTimestamps();
            phase=L"tooltip-help";
            Border semantic,owned;TranslateTransform metadata;
            AutomationProperties::SetHelpText(semantic,L"Selected texture");tooltip(semantic,L"Texture");tooltip(semantic,L"Texture updated");
            require(AutomationProperties::GetHelpText(semantic)==L"Selected texture",L"Tooltip replaced pre-existing semantic help");
            tooltip(semantic,L"");require(AutomationProperties::GetHelpText(semantic)==L"Selected texture",L"Clearing a tooltip cleared semantic help");
            tooltip(owned,L"First tooltip");tooltip(owned,L"Second tooltip");
            require(AutomationProperties::GetHelpText(owned)==L"Second tooltip",L"Tooltip-owned help did not update");
            AutomationProperties::SetHelpText(owned,L"Graph counts");tooltip(owned,L"Third tooltip");tooltip(owned,L"Fourth tooltip");
            require(AutomationProperties::GetHelpText(owned)==L"Graph counts",L"Repeated tooltip updates replaced explicit caller help");
            AutomationProperties::SetHelpText(owned,L"Fourth tooltip");tooltip(owned,L"Fifth tooltip");
            require(AutomationProperties::GetHelpText(owned)==L"Fourth tooltip",L"Tooltip claimed a caption it had never written as help");
            AutomationProperties::SetHelpText(owned,L"");tooltip(owned,L"Reclaimed tooltip");
            require(AutomationProperties::GetHelpText(owned)==L"Reclaimed tooltip",L"Tooltip did not acquire empty help");
            tooltip(owned,L"");require(AutomationProperties::GetHelpText(owned).empty(),L"Clearing tooltip-owned help retained its caption");
            tooltip(metadata,L"First metadata tooltip");tooltip(metadata,L"Second metadata tooltip");tooltip(metadata,L"");
            require(AutomationProperties::GetHelpText(metadata).empty(),L"Nonvisual tooltip-owned help did not update and clear");
            phase=L"primitives";
            {
                Button tab;tab.Content(box_value(L"Retained tab"));tab.Width(100);tab.Height(30);
                auto shell=panelTabShell(tab,nullptr);root.Children().Append(shell);
                co_await wait([tab]{return tab.IsLoaded()&&tab.ActualWidth()>0;},L"Retained tab did not arrange");
                selectPanelTabShell(shell,true);
                auto path=shell.Children().GetAt(0).as<Canvas>().Children().GetAt(0).as<Microsoft::UI::Xaml::Shapes::Path>();
                auto selectedGeometry=path.Data();selectPanelTabShell(shell,true);
                require(path.Data()==selectedGeometry,L"Unchanged tab presentation replaced its path geometry");
                selectPanelTabShell(shell,false);auto unselectedGeometry=path.Data();
                require(unselectedGeometry!=selectedGeometry,L"Changed tab corners retained the old path geometry");
                selectPanelTabShell(shell,false);
                require(path.Data()==unselectedGeometry,L"Unchanged unselected tab replaced its path geometry");
                tab.Width(120);co_await wait([tab]{return tab.ActualWidth()==120;},L"Retained tab did not resize");
                require(path.Data()!=unselectedGeometry,L"Resized tab retained the old path geometry");
                root.Children().RemoveAtEnd();++checks;
            }
            {
                Border element,extra;auto id=ChromeFrames::Id(element);ChromeFrames frames;
                auto frame=[id]{ChromeFrames::Frame next;next.groups.insert(1);next.columns.insert(2);next.elements.insert(id);return next;};
                auto document=O({{L"width",N(100)},{L"height",N(80)}});bool retired=false;
                frames.current=frame();frames.Sent(1);
                require(frames.Present(1,document),L"Initial chrome frame was unchanged");
                frames.retiring.emplace(&retired,[&]{retired=true;});frames.current=frame();frames.Sent(2);
                require(!frames.Present(2,document)&&frames.revision==2&&retired,L"Equal chrome did not acknowledge, retire and preserve presentation");
                frames.Sent(5);document=O({{L"width",N(120)},{L"height",N(80)}});
                require(frames.Present(5,document),L"Changed chrome document was ignored");
                frames.current.groups.insert(3);frames.Sent(6);require(frames.Present(6,document),L"Changed chrome group was ignored");
                frames.current.columns.insert(4);frames.Sent(7);require(frames.Present(7,document),L"Changed chrome column was ignored");
                frames.current.elements.insert(ChromeFrames::Id(extra));frames.Sent(8);require(frames.Present(8,document),L"Changed chrome element was ignored");
                require(!frames.Present(1,document)&&frames.revision==8,L"Stale chrome frame changed the acknowledgement");
                frames.Close();++checks;
            }
            ComboBox choice;comboOption(choice,L"One");auto selected=comboOption(choice,L"Two");choiceColors(data,choice,ControlSurface::Settings,ChoiceFeedbackMode::Field,Windows::UI::Text::FontWeights::Bold());
            require(choice.Background().as<SolidColorBrush>().Color()!=data->brush(L"settings").Color(),L"Settings choice has no visible face");
            selectIndex(choice,1);require(choice.SelectedIndex()==1,L"Valid choice was rejected");
            selectIndex(choice,99);require(choice.SelectedIndex()==-1,L"Out-of-range choice was accepted");
            require(selected.Content().as<Windows::Foundation::IPropertyValue>().Type()==Windows::Foundation::PropertyType::String&&selected.ContentTemplate(),L"Native choice retained a visual as item data");
            selectIndex(choice,1);root.Children().Append(choice);co_await wait([choice]{return choice.IsLoaded()&&choice.ActualWidth()>0;},L"Native choice did not load");
            std::function<TextBlock(DependencyObject const&)> caption;caption=[&](DependencyObject const& node)->TextBlock{if(auto text=node.try_as<TextBlock>();text&&text.Text()==L"Two")return text;for(int i=0;i<VisualTreeHelper::GetChildrenCount(node);++i)if(auto text=caption(VisualTreeHelper::GetChild(node,i)))return text;return nullptr;};
            co_await wait([&]{return bool(caption(choice));},L"Selected native choice caption did not arrange");auto face=caption(choice);
            require(face.FontWeight().Weight==700&&face.FontFamily().Source()==L"Segoe UI"&&face.TextTrimming()==TextTrimming::CharacterEllipsis&&face.FontSize()==choice.FontSize()&&std::abs(face.RenderTransform().as<TranslateTransform>().Y()+TextBlock().FontSize()*262/4096)<.001,L"Selected choice lost its native font or optical metrics");
            choice.IsDropDownOpen(true);co_await wait([this,&selected,&caption]{return data->popupCount==1&&bool(caption(selected));},L"Native choice did not block workspace popup routing");
            auto popupCaption=caption(selected);require(popupCaption!=face&&popupCaption.FontWeight().Weight==400,L"Popup choice reused the selected visual or bold caption");
            choice.IsDropDownOpen(false);co_await wait([this]{return data->popupCount==0;},L"Native choice did not release workspace popup routing");
            require(caption(choice)&&caption(choice).FontWeight().Weight==700,L"Closing popup changed the selected choice caption weight");
            root.Children().RemoveAtEnd();++checks;
            phase=L"chrome-replacement";auto fade=std::make_shared<ChromeVisibility>();Border previous,replacement,decoration;
            fade->apply(previous,true,true);fade->apply(replacement,true,true);
            require(replacement.Visibility()==Visibility::Collapsed&&replacement.Opacity()==0&&!replacement.IsHitTestVisible(),L"Replacement chrome became visible while Zen remained hidden");
            fade->apply(replacement,true,false);
            require(replacement.Visibility()==Visibility::Visible&&replacement.Opacity()==1&&replacement.IsHitTestVisible(),L"Replacement chrome did not return from Zen");
            decoration.IsHitTestVisible(false);fade->apply(decoration,true,false);
            require(!decoration.IsHitTestVisible(),L"Replacement decorative chrome accepted input");++checks;
            phase=L"registry";auto popup=menu();auto ticket=owner.Begin(data,anchor,popup);pending();
            require(data->dismissTransients(),L"Pending popup ignored window cancellation registry");
            require(!ticket.Current()&&!ticket.Show(anchor,{}),L"Registry-canceled reply reopened popup");empty();++checks;
            phase=L"replacement";auto reused=menu();auto old=owner.Begin(data,anchor,reused);auto current=owner.Begin(data,anchor,reused);pending();
            require(!old.Cancel()&&!old.Current()&&!old.Show(anchor,{})&&current.Current(),L"Superseded query canceled or replaced current popup lifetime");
            require(current.Cancel(),L"Current query cancellation was refused");empty();++checks;
            phase=L"unload";auto unloaded=owner.Begin(data,anchor,menu());root.Children().RemoveAt(0);co_await settle();
            require(!unloaded.Current()&&!unloaded.Show(untagged,{}),L"Unloaded source query opened on replacement anchor");empty();
            root.Children().InsertAt(0,anchor);co_await settle();++checks;
            phase=L"escape";auto escaped=owner.Begin(data,anchor,menu());pending();anchor.Focus(FocusState::Programmatic);privacy();
            INPUT keyboard[2]{};keyboard[0].type=keyboard[1].type=INPUT_KEYBOARD;keyboard[0].ki.wVk=keyboard[1].ki.wVk=VK_ESCAPE;keyboard[1].ki.dwFlags=KEYEVENTF_KEYUP;
            require(SendInput(2,keyboard,sizeof(INPUT))==2,L"Native Escape injection failed");co_await wait([this]{return keys==1&&!owner.Active();},L"Native Escape cancellation timed out");
            require(keys==1&&!escaped.Current()&&!escaped.Show(anchor,{}),L"Native Escape did not cancel delayed popup");empty();++checks;
            phase=L"originating-press";nativePress(anchor,true);
            co_await wait([this]{return originPresses==1;},L"Originating native press did not reach source");co_await settle();
            require(origin&&origin->Current(),L"Popup canceled itself on its originating routed press");pending();
            nativePress(untagged);co_await wait([this]{return presses==1&&!owner.Active();},L"Next native press did not cancel originating popup request");
            require(!origin->Current()&&!origin->Show(anchor,{}),L"Canceled originating request reopened popup");empty();++checks;
            phase=L"untagged-press";auto beforePress=presses;auto pressed=owner.Begin(data,anchor,menu());pending();co_await settle();co_await wait([this]{return anchor.IsLoaded()&&untagged.ActualWidth()>0&&GetForegroundWindow()==hwnd;},L"Fixture did not become arranged and foreground");privacy();
            nativePress(untagged);co_await wait([this,beforePress]{return presses==beforePress+1&&!owner.Active();},L"Native untagged press cancellation timed out");
            require(presses==beforePress+1&&!pressed.Current()&&!pressed.Show(anchor,{}),L"Handled native untagged press did not cancel delayed popup");empty();++checks;
            phase=L"blur";auto beforeBlur=blurs;auto blurred=owner.Begin(data,anchor,menu());pending();other.Activate();SetForegroundWindow(handle(other));co_await wait([this,beforeBlur]{return blurs>beforeBlur&&!owner.Active();},L"Native deactivation cancellation timed out");
            require(blurs>beforeBlur&&!blurred.Current()&&!blurred.Show(anchor,{}),L"Native window deactivation did not cancel delayed popup");empty();auto wrongOwner=owner.Begin(data,anchor,menu());
            require(!wrongOwner.Current()&&!wrongOwner.Show(anchor,{}),L"Popup accepted source owned by background window");empty();++checks;
            window.Activate();SetForegroundWindow(hwnd);co_await wait([this]{return anchor.IsLoaded()&&untagged.ActualWidth()>0&&GetForegroundWindow()==hwnd;},L"Fixture did not become arranged and foreground");privacy();
            phase=L"opening-gap";auto beforeGap=opened;auto opening=owner.Begin(data,anchor,menu());pending();require(opening.Show(anchor,{}),L"Current opening-gap popup did not show");
            require(owner.Pending()&&opened==beforeGap,L"Opening-gap prerequisite reached Opened synchronously");
            require(data->dismissTransients(),L"Opening popup ignored window cancellation registry");co_await settle();
            require(opened==beforeGap&&!visiblePopup(),L"Opening-gap cancellation left a late or visible native popup");
            require(!opening.Current()&&!opening.Show(anchor,{}),L"Opening-gap canceled reply reopened popup");empty();
            phase=L"ordinary-opening";co_await ordinaryOpening();phase=L"ordinary-closing";co_await ordinaryClosing();++checks;
            phase=L"show-close";auto beforeOpen=opened;auto beforeClosed=closedCount;auto shown=owner.Begin(data,anchor,menu());pending();require(shown.Show(anchor,{}),L"Current popup did not show");co_await wait([this,beforeOpen]{return opened==beforeOpen+1&&owner.Open();},L"Popup Opened timed out");
            require(opened==beforeOpen+1&&owner.Open()&&data->transients.size()==1,L"Popup registration did not survive ShowAt through Opened");
            require(data->dismissTransients(),L"Visible popup ignored cancellation registry");co_await wait([this,beforeClosed]{return closedCount==beforeClosed+1&&!visiblePopup();},L"Native popup Closed acknowledgement or visible-popup retirement timed out");
            require(!shown.Current()&&!shown.Show(anchor,{}),L"Reply reopened closed popup");empty();++checks;
            phase=L"header-context-ordering";co_await headerContextOrdering();
            phase=L"workspace-ime";co_await workspaceIme();
            phase=L"shortcut-search-ime";co_await shortcutSearchIme();
            phase=L"customization-name-ack";co_await customizationDraft();
            phase=L"native-default-ime";co_await defaultIme();
            phase=L"layer-language-draft";co_await layerLanguageDraft();
            phase=L"paper-name-ime";co_await paperNameIme();
            phase=L"settings-resize";co_await settingsResize();
            phase=L"window-close";auto closed=owner.Begin(data,anchor,menu());window.Close();co_await settle();
            require(!closed.Current()&&!closed.Show(anchor,{}),L"Reply reopened canceled owner");empty();++checks;
        } catch(hresult_error const& e){resultCode=1;wchar_t hex[11]{};swprintf_s(hex,L"0x%08X",static_cast<unsigned>(e.code().value));error=hstring(hex)+L": "+e.message();}catch(...){resultCode=1;error=L"Unhandled popup fixture failure";}
        owner.Cancel();auto count=checks;auto imeCount=imeChecks;auto step=to_string(phase);auto message=to_string(error);auto path=output;
        A imeEvents;for(auto const& event:*imeTrace)imeEvents.Append(event);auto imeJson=to_string(imeEvents.Stringify());
        co_await resume_background();
        {std::ofstream file(path.parent_path()/L"workspace-ime-events.json");file<<imeJson;if(!file)resultCode=1;}
        {std::ofstream file(path);file<<"{\"passed\":"<<count<<",\"ime_passed\":"<<imeCount<<",\"exit_code\":"<<resultCode<<",\"phase\":\""<<step<<"\"}\n";if(!file)resultCode=1;}
        if(!message.empty()){std::ofstream file(path.parent_path()/L"popup-fixture-error.log");file<<message;}
        co_await apartment;other.Close();if(!windowClosed)window.Close();Application::Current().Exit();
    }
    void init(std::filesystem::path path){
        output=path.parent_path()/L"popup-fixture-result.json";context.apply(data);
        wchar_t theme[16]{};GetEnvironmentVariableW(L"CAPY_POPUP_THEME",theme,16);root.RequestedTheme(std::wstring(theme)==L"light"?ElementTheme::Light:ElementTheme::Dark);
        anchor.Content(box_value(L"Delayed popup source"));untagged.Content(box_value(L"Untagged input target"));
        root.Children().Append(anchor);root.Children().Append(untagged);root.Width(400);root.Height(240);
        root.HorizontalAlignment(HorizontalAlignment::Left);root.VerticalAlignment(VerticalAlignment::Top);
        root.KeyDown([weak=weak_from_this()](auto&&,Input::KeyRoutedEventArgs const& e){if(e.Key()==Windows::System::VirtualKey::Escape)if(auto self=weak.lock()){++self->keys;e.Handled(self->data->dismissTransients());}});
        anchor.AddHandler(UIElement::PointerPressedEvent(),box_value(Input::PointerEventHandler([weak=weak_from_this()](auto&&,Input::PointerRoutedEventArgs const& e){
            if(auto self=weak.lock();self&&self->phase==L"originating-press"){
                ++self->originPresses;self->origin=self->owner.Begin(self->data,self->anchor,self->menu());e.Handled(true);
            }
        })),true);
        untagged.AddHandler(UIElement::PointerPressedEvent(),box_value(Input::PointerEventHandler([weak=weak_from_this()](auto&&,Input::PointerRoutedEventArgs const& e){if(auto self=weak.lock())++self->presses;e.Handled(true);})),true);
        window.Closed([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock()){self->windowClosed=true;self->owner.Cancel();}});
        window.Activated([weak=weak_from_this()](auto&&,WindowActivatedEventArgs const& e){if(e.WindowActivationState()==WindowActivationState::Deactivated)if(auto self=weak.lock()){++self->blurs;self->owner.Cancel();}});
        TextBlock text;text.Text(L"Owned blur target");other.Content(text);other.Title(L"Popup fixture blur target");other.AppWindow().Move({600,80});
        root.Loaded([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock())self->run();});
        window.Content(root);window.Title(L"Capy Canvas popup review");hwnd=handle(window);window.AppWindow().Move({40,40});CapyUi::RegisterFixtureWindow(other);CapyUi::RegisterFixtureWindow(window);window.Activate();
    }
};
struct App:ApplicationT<App,Markup::IXamlMetadataProvider>{
    Microsoft::UI::Xaml::XamlTypeInfo::XamlControlsXamlMetaDataProvider metadata;
    std::shared_ptr<Fixture> fixture;ControlContext context;std::shared_ptr<TextServicesLibrary> textServices;
    App(ControlContext source,std::shared_ptr<TextServicesLibrary> services):context(std::move(source)),textServices(std::move(services)){UnhandledException([this](auto&&,UnhandledExceptionEventArgs const& e){resultCode=1;e.Handled(true);Exit();});}
    Markup::IXamlType GetXamlType(Windows::UI::Xaml::Interop::TypeName const& type){return metadata.GetXamlType(type);}
    Markup::IXamlType GetXamlType(hstring const& name){return metadata.GetXamlType(name);}
    com_array<Markup::XmlnsDefinition> GetXmlnsDefinitions(){return metadata.GetXmlnsDefinitions();}
    void OnLaunched(LaunchActivatedEventArgs const&){Resources().MergedDictionaries().Append(XamlControlsResources());Resources().MergedDictionaries().Append(CapyUi::paletteResources());wchar_t path[32768];auto size=GetEnvironmentVariableW(L"CAPY_POPUP_FIXTURE",path,32768);if(!size||size>=32768||!std::filesystem::path(path).is_absolute())throw hresult_invalid_argument();fixture=std::make_shared<Fixture>();fixture->context=context;fixture->textServices=textServices;fixture->init(path);}
};
}
int WINAPI wWinMain(HINSTANCE,HINSTANCE,PWSTR,int){auto context=fixtureContext(L"CAPY_POPUP_FIXTURE");auto textServices=std::make_shared<TextServicesLibrary>();init_apartment(apartment_type::single_threaded);Application::Start([context,textServices](auto&&){make<App>(context,textServices);});return resultCode;}
