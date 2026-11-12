#include "pch.h"
#include "NavigatorView.h"
#include "NavigationControls.h"
#include "PointerGesture.h"
#include <microsoft.ui.xaml.media.dxinterop.h>
#include <atomic>
#include <optional>

using namespace CapyUi;
using Windows::Foundation::Point;
using Windows::Foundation::Rect;
namespace {
A coordinates(std::initializer_list<double> values){A result;for(auto v:values)result.Append(N(v));return result;}
struct NavigatorLayout:PanelT<NavigatorLayout>{
    ContentControl overview;
    Grid actions;
    double inset,spacing,controlHeight,aspect=1;
    std::function<void()> arranged;
    NavigatorLayout(ContentControl const& image,Grid const& buttons,double padding,double gap,double height):
        overview(image),actions(buttons),inset(padding),spacing(gap),controlHeight(height){
        Children().Append(overview);Children().Append(actions);
    }
    double fixedHeight()const{return 2*inset+spacing+controlHeight;}
    double naturalImage(double width)const{return std::max(0.,width-2*inset)*aspect;}
    double imageHeight(Windows::Foundation::Size available)const{
        double natural=naturalImage(available.Width);
        return std::isfinite(available.Height)?std::clamp(double(available.Height)-fixedHeight(),0.,natural):natural;
    }
    Windows::Foundation::Size MeasureOverride(Windows::Foundation::Size available){
        actions.Measure({std::max(0.f,available.Width-float(2*inset)),float(controlHeight)});
        float width=std::isfinite(available.Width)?available.Width:actions.DesiredSize().Width+float(2*inset);
        double height=imageHeight({width,available.Height});
        overview.Measure({std::max(0.f,width-float(2*inset)),float(height)});
        return {width,float(height+fixedHeight())};
    }
    Windows::Foundation::Size ArrangeOverride(Windows::Foundation::Size finalSize){
        float width=std::max(0.f,finalSize.Width-float(2*inset));double height=imageHeight(finalSize);
        overview.Arrange({float(inset),float(inset),width,float(height)});
        actions.Arrange({float(inset),float(inset+height+spacing),width,float(controlHeight)});
        if(arranged)arranged();
        return finalSize;
    }
    void Aspect(double value){if(aspect!=value){aspect=value;InvalidateMeasure();}}
};
}
struct NavigatorView::Impl : std::enable_shared_from_this<Impl> {
    std::shared_ptr<WorkspaceData> data;
    std::function<void()> layoutChanged;
    Grid root,actions;
    com_ptr<NavigatorLayout> content;
    ContentControl overview;
    Grid inputSurface;Canvas surfaceLayer{nullptr};SwapChainPanel surface{nullptr};ScaleTransform surfaceScale{nullptr};
    uint64_t id=0,generation=0,minimumGeneration=0;uintptr_t handle=0;
    std::shared_ptr<void> ownedSurface;
    double maximumWidth=capy_navigator_maximum_width();
    Bindings bindings;
    std::shared_ptr<ControlCapture> capture;
    std::optional<std::array<double,10>> geometry;
    std::optional<std::array<double,4>> contactBounds;
    std::optional<CapyNavigatorViewport> requested;
    Impl(std::shared_ptr<WorkspaceData> state,std::function<void()> changed):
        data(std::move(state)),layoutChanged(std::move(changed)){}
    void send(wchar_t const* phase,Point at){
        data->dispatch(O({{L"type",S(L"navigator")},{L"phase",S(phase)},
            {L"position",coordinates({at.X,at.Y})},
            {L"viewport",coordinates({overview.ActualWidth(),overview.ActualHeight()})}}));
    }
    void cancel(){
        if(capture)capture->Cancel();
    }
    std::array<double,4> inputBounds()const{
        auto at=overview.TransformToVisual(nullptr).TransformPoint({});
        return {at.X,at.Y,overview.ActualWidth(),overview.ActualHeight()};
    }
    void init(){
        auto weak=weak_from_this();
        root.Background(clear());
        copyName(data,root,data->copyCaption(L"color",L"navigator_panel"));
        inputSurface.Background(data->brush(L"panel"));overview.Content(inputSurface);
        if(data->navigatorSurface){
            surfaceLayer=Canvas();surface=SwapChainPanel();surfaceScale=ScaleTransform();
            surface.Width(256);surface.Height(256);surface.HorizontalAlignment(HorizontalAlignment::Left);surface.VerticalAlignment(VerticalAlignment::Top);
            surface.IsHitTestVisible(false);surface.Visibility(Visibility::Collapsed);surface.RenderTransform(surfaceScale);
            surfaceLayer.Children().Append(surface);inputSurface.Children().Append(surfaceLayer);
            static std::atomic<uint64_t> nextId=0;id=++nextId;
            data->navigatorViews->emplace(id,std::pair{[weak]{if(auto self=weak.lock())self->refreshGeometry();},[weak]{if(auto self=weak.lock())self->resetSurface();}});
        }
        overview.HorizontalContentAlignment(HorizontalAlignment::Stretch);overview.VerticalContentAlignment(VerticalAlignment::Stretch);edgeTag(overview,L"edge_contact",S(L"input"));
        overview.IsTabStop(true);overview.UseSystemFocusVisuals(false);
        overview.HorizontalAlignment(HorizontalAlignment::Stretch);
        copyName(data,overview,data->copyCaption(L"color",L"navigator_overview"));
        AutomationProperties::SetAutomationId(overview,L"navigator-overview");
        actions=NavigationButtons(data,L"navigator",[data=data](hstring const& id){return find(data->state,L"commands",L"id",id);},bindings);
        for(auto const& bind:bindings)bind();
        content=make_self<NavigatorLayout>(overview,actions,data->panelMetric(L"inset"),data->panelMetric(L"control_spacing"),data->panelMetric(L"control_height"));
        content->arranged=[weak]{if(auto self=weak.lock())self->refreshGeometry();};
        root.Children().Append(content.as<Panel>());updateAspect();
        capture=captureControl(data,overview,[weak](uint32_t,bool cancel,PointerRoutedEventArgs const* event){if(auto self=weak.lock()){
            self->contactBounds.reset();self->send(cancel?L"cancel":L"up",event&&!cancel?event->GetCurrentPoint(self->overview).Position():Point{});
            if(event)event->Handled(true);
        }});
        overview.PointerPressed([weak](auto&&,PointerRoutedEventArgs const& e){if(auto self=weak.lock()){
            auto p=e.GetCurrentPoint(self->overview);
            if(self->capture->Active())return;
            if(self->overview.ActualWidth()<=8||self->overview.ActualHeight()<=8)return;
            if(self->capture->Begin(e)){self->contactBounds=self->inputBounds();self->overview.Focus(FocusState::Pointer);self->send(L"down",p.Position());e.Handled(true);}
        }});
        overview.PointerMoved([weak](auto&&,PointerRoutedEventArgs const& e){if(auto self=weak.lock()){
            if(self->capture->Owns(e.Pointer().PointerId())){self->send(L"move",e.GetCurrentPoint(self->overview).Position());e.Handled(true);}
        }});
        root.Unloaded([weak](auto&&,auto&&){if(auto self=weak.lock()){self->cancel();self->geometry.reset();self->refreshGeometry();self->layoutChanged();}});
        root.Loaded([weak](auto&&,auto&&){if(auto self=weak.lock())self->refreshGeometry();});
        root.LayoutUpdated([weak](auto&&,auto&&){if(auto self=weak.lock())self->refreshGeometry();});
    }
    void resetSurface(){
        check_hresult(surface.as<ISwapChainPanelNative>()->SetSwapChain(nullptr));
        minimumGeneration=std::max(minimumGeneration,generation+1);handle=0;ownedSurface.reset();geometry.reset();
        surface.Visibility(Visibility::Collapsed);inputSurface.Background(data->brush(L"panel"));
    }
    void receive(NativeSurfaceAttachment attachment){
        auto info=attachment.info;
        if(info.generation<minimumGeneration)return;
        if(info.generation!=generation&&handle){
            check_hresult(surface.as<ISwapChainPanelNative>()->SetSwapChain(nullptr));handle=0;ownedSurface.reset();
        }
        generation=info.generation;bool ready=info.ready!=0;
        if(ready&&info.handle&&handle!=info.handle){
            check_hresult(surface.as<ISwapChainPanelNative2>()->SetSwapChainHandle(attachment.handle.get()));ownedSurface=std::move(attachment.handle);handle=info.handle;
        }
        surface.Visibility(ready?Visibility::Visible:Visibility::Collapsed);
        inputSurface.Background(data->brush(ready?L"bg":L"panel"));
    }
    void publish(){
        if(id&&requested&&data->navigatorSurface)data->navigatorSurface(id,&*requested,[weak=weak_from_this()](NativeSurfaceAttachment attachment){if(auto self=weak.lock())self->receive(std::move(attachment));});
    }
    void refreshGeometry(){
        if(!surface)return;
        bool moving=data->navigatorMoving&&data->navigatorMoving();
        if(!root.IsLoaded()){
            if(requested&&requested->moving!=uint32_t(moving)){requested->moving=uint32_t(moving);geometry.reset();publish();}
            return;
        }
        if(capture->Active()&&contactBounds!=inputBounds())cancel();
        if(overview.ActualWidth()<=8||overview.ActualHeight()<=8)return;
        auto tabs=array(data->state,L"tabs");auto document=tabs.Size()?tabs.GetObjectAt(0):J{};
        float image[4]{};
        if(!capy_navigator_image(float(overview.ActualWidth()),float(overview.ActualHeight()),uint32_t(num(document,L"width")),uint32_t(num(document,L"height")),image))return;
        auto workspace=data->chromeFrames->root.get();
        auto maximumHeight=std::max(overview.ActualHeight(),workspace?workspace.ActualHeight()-fixedHeight()-data->panelMetric(L"tab_bar_height"):overview.ActualHeight());
        float maximum[4]{};capy_navigator_image(float(std::max(maximumWidth,overview.ActualWidth())),float(maximumHeight),uint32_t(num(document,L"width")),uint32_t(num(document,L"height")),maximum);
        double scale=root.XamlRoot()?root.XamlRoot().RasterizationScale():1.;
        std::array<double,10> next{overview.ActualWidth(),overview.ActualHeight(),image[0],image[1],image[2],image[3],maximum[2],maximum[3],scale,double(moving)};
        if(geometry==next)return;geometry=next;
        Canvas::SetLeft(surface,image[0]);Canvas::SetTop(surface,image[1]);surfaceScale.ScaleX(image[2]/256.);surfaceScale.ScaleY(image[3]/256.);
        RectangleGeometry clip;clip.Rect({0,0,float(overview.ActualWidth()),float(overview.ActualHeight())});surfaceLayer.Clip(clip);
        if(id&&data->navigatorSurface){
            requested=CapyNavigatorViewport{{float(image[2]*scale),float(image[3]*scale)},
                {float(std::max(image[2],maximum[2])*scale),float(std::max(image[3],maximum[3])*scale)},{256,256},float(scale),uint32_t(moving)};
            publish();
        }
    }
    void updateAspect(){
        auto tabs=array(data->state,L"tabs");auto document=tabs.Size()?tabs.GetObjectAt(0):J{};
        content->Aspect(capy_navigator_aspect(uint32_t(num(document,L"width")),uint32_t(num(document,L"height"))));
    }
    double naturalImage(double width)const{return content->naturalImage(width);}
    double fixedHeight()const{return content->fixedHeight();}
    void apply(bool show){
        if(!show)cancel();
        for(auto const& bind:bindings)bind.Apply();
        updateAspect();refreshGeometry();
    }

};
NavigatorView::NavigatorView(std::shared_ptr<WorkspaceData> data,std::function<void()> changed):
    impl(std::make_shared<Impl>(std::move(data),std::move(changed))){impl->init();}
NavigatorView::~NavigatorView(){
    impl->cancel();if(impl->id){
        impl->data->navigatorViews->erase(impl->id);
        impl->surface.as<ISwapChainPanelNative>()->SetSwapChain(nullptr);
        if(impl->data->navigatorSurface)impl->data->navigatorSurface(impl->id,nullptr,{});
    }
}
FrameworkElement NavigatorView::Root()const{return impl->root;}
double NavigatorView::NaturalHeight(double width)const{return impl->naturalImage(width)+impl->fixedHeight();}
double NavigatorView::FixedHeight()const{return impl->fixedHeight();}
void NavigatorView::Apply(bool visible){impl->apply(visible);}
void NavigatorView::MaximumWidth(double width){if(impl->maximumWidth!=width){impl->maximumWidth=width;impl->refreshGeometry();}}
