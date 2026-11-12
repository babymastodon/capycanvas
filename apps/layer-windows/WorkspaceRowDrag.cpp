#include "pch.h"
#include "WorkspaceRowDrag.h"
#include "UiControls.h"
#include <chrono>

using namespace CapyUi;
using Windows::Foundation::Point;
namespace NativeInput=Microsoft::UI::Input;
struct WorkspaceRowDrag::Impl:std::enable_shared_from_this<Impl>{
    ItemsView list{nullptr};
    ScrollView scroll{nullptr};
    std::weak_ptr<WorkspaceData> context;
    bool panning=false;
    Grid surface{nullptr};std::shared_ptr<RowDragPresentation> preview=std::make_shared<RowDragPresentation>();Point origin{};
    Border hint;
    MenuFlyout visibleMenu{nullptr};
    std::function<bool()> available;
    Move move;
    std::function<void()> ended;
    struct Row{
        weak_ref<ItemContainer> item;
        weak_ref<Button> grip,more;
        weak_ref<MenuFlyout> context;
        weak_ref<UIElement> pin;
        weak_ref<MenuFlyout> menu;
        hstring id;
        std::optional<Swipe> swipe;
    };
    std::vector<Row> rows;
    std::vector<hstring> order;
    std::optional<Row> source;
    Input::Pointer pointer{nullptr};
    PointerGesture recognizer;
    Microsoft::UI::Dispatching::DispatcherQueueTimer timer{nullptr};
    std::vector<weak_ref<UIElement>> path;
    struct ScrollClaim{ScrollingScrollMode horizontal,vertical;};
    std::optional<ScrollClaim> scrollInput;
    Point position{};
    NativeInput::PointerDeviceType device=NativeInput::PointerDeviceType::Mouse;
    HWND owner=nullptr;
    std::optional<hstring> before;
    enum class Mode {Idle,Pending,Held,Swiping,Dragging,Finishing};
    enum class Part {Body,Grip,Child};
    Mode mode=Mode::Idle;
    Part part=Part::Body;
    bool releasing=false,pickup=false;
    double initialSwipe=0,initialScroll=0;
    weak_ref<UIElement> outsideSurface;
    PointerEventHandler outsidePress{nullptr};
    bool ignoreClick=false,suppressContext=false,valid=false,recognizing=false;
    bool held()const{return mode==Mode::Held;}
    bool dragging()const{return mode==Mode::Dragging;}
    bool finishing()const{return mode==Mode::Finishing;}
    bool swiping()const{return mode==Mode::Swiping;}
    bool grip()const{return part==Part::Grip;}
    bool child()const{return part==Part::Child;}
    bool trace=GetEnvironmentVariableW(L"CAPY_TRACE_UI",nullptr,0)!=0;
    uint64_t generation=0;
    J lastRelease,lastCancel;
    std::chrono::steady_clock::time_point tickAt;
    void evidence(){
        if(!trace)return;
        hstring phase=finishing()?L"finishing":dragging()?L"dragging":swiping()?L"swiping":held()?L"held":pointer?L"pressed":L"idle";
        auto value=O({{L"phase",S(phase)},{L"source",source?S(source->id):JsonValue::CreateNullValue()},
            {L"device",S(device==NativeInput::PointerDeviceType::Mouse?L"mouse":device==NativeInput::PointerDeviceType::Pen?L"pen":L"touch")},
            {L"grip",B(grip())},{L"panning",B(panning)},{L"can_drop",B(valid)},{L"before",before?S(*before):JsonValue::CreateNullValue()}});
        if(lastRelease.Size())value.Insert(L"last_release",lastRelease);
        if(lastCancel.Size())value.Insert(L"last_cancel",lastCancel);
        AutomationProperties::SetItemStatus(list,value.Stringify());
    }
    void deferClick(){
        auto epoch=generation;
        surface.DispatcherQueue().TryEnqueue(Microsoft::UI::Dispatching::DispatcherQueuePriority::Low,
            [weak=weak_from_this(),epoch]{if(auto self=weak.lock();self&&self->generation==epoch&&!self->pointer)self->ignoreClick=false;});
    }
    void hideMenus(){
        if(visibleMenu){auto menu=std::exchange(visibleMenu,nullptr);menu.Hide();}
        if(source){
            if(auto menu=source->context.get())menu.Hide();
            if(auto menu=source->menu.get())menu.Hide();
        }
    }
    void release(){
        timer.Stop();hint.Visibility(Visibility::Collapsed);
        if(recognizing){recognizing=false;recognizer.CompleteGesture();}
        if(pointer)if(auto data=context.lock())data->pointerCancels.Forget(pointer.PointerId(),this);
        releasing=true;surface.ReleasePointerCaptures();releasing=false;
        if(scrollInput&&scroll){scroll.HorizontalScrollMode(scrollInput->horizontal);scroll.VerticalScrollMode(scrollInput->vertical);scrollInput.reset();}
        path.clear();pointer=nullptr;
    }
    void clear(bool hideMenu){
        preview->Clear();
        if(hideMenu)hideMenus();
        ++generation;source.reset();mode=Mode::Idle;valid=false;before.reset();release();
        ended();deferClick();evidence();
    }
    bool closeSwipes(DependencyObject const& keep=nullptr){
        bool closed=false;
        for(auto const& row:rows)if(row.swipe&&(!keep||!descendantOf(keep,row.item.get()))&&!row.swipe->presenter->AtRest()){
            row.swipe->presenter->SetOffset(0);closed=true;
        }
        return closed;
    }
    void detachOutside(){
        if(auto target=outsideSurface.get();target&&outsidePress)target.RemoveHandler(UIElement::PointerPressedEvent(),box_value(outsidePress));
        outsideSurface=weak_ref<UIElement>{};outsidePress=nullptr;
    }
    void attachOutside(){
        DependencyObject node=surface;
        while(node&&!node.try_as<ContentDialog>())node=VisualTreeHelper::GetParent(node);
        auto target=node.try_as<UIElement>();if(!target||target==outsideSurface.get())return;
        detachOutside();outsideSurface=make_weak(target);
        outsidePress=[weak=weak_from_this()](auto&&,PointerRoutedEventArgs const& event){if(auto self=weak.lock()){
            if(self->pointer&&self->pointer.PointerId()!=event.Pointer().PointerId())self->cancel(L"multiple_contacts");
            else self->closeSwipes(event.OriginalSource().try_as<DependencyObject>());
        }};
        target.AddHandler(UIElement::PointerPressedEvent(),box_value(outsidePress),true);
    }
    bool cancel(hstring reason=L"cancel"){
        auto closed=closeSwipes();
        if(!pointer&&!source&&!visibleMenu)return closed;
        if(trace)lastCancel=O({{L"generation",N(double(generation))},{L"reason",S(reason)}});
        ignoreClick=true;clear(true);return true;
    }
    bool ownsFocus()const{return GetAncestor(GetForegroundWindow(),GA_ROOTOWNER)==owner&&!IsIconic(owner);}
    bool current()const{
        if(!source||(!child()&&!available()))return false;
        auto row=source->item.get();uint32_t index=0;
        return row&&(dragging()||held()||row.IsLoaded())&&rowItems(list).IndexOf(row,index)
            &&std::find(order.begin(),order.end(),source->id)!=order.end();
    }
    bool owns(UIElement const& node)const{
        return pointer&&PointerGesture::Captured(node,pointer.PointerId());
    }
    bool claim(){
        if(!current()||!pointer||!scroll)return false;
        if(owns(surface))return true;
        auto data=context.lock();if(!data)return false;
        releasing=true;data->pointerCancels.Cancel(pointer.PointerId());
        scrollInput=ScrollClaim{scroll.HorizontalScrollMode(),scroll.VerticalScrollMode()};
        scroll.HorizontalScrollMode(ScrollingScrollMode::Disabled);scroll.VerticalScrollMode(ScrollingScrollMode::Disabled);
        for(auto const& weak:path)if(auto node=weak.get();node&&owns(node))node.ReleasePointerCapture(pointer);
        bool captured=surface.CapturePointer(pointer);
        releasing=false;
        if(captured)data->pointerCancels.Observe(pointer.PointerId(),this,[weak=weak_from_this()]{if(auto self=weak.lock())self->cancel(L"capture_handoff");});
        else cancel();
        return captured;
    }
    void menuAt(Row const& row,Point at,bool holding,bool reordering=false){
        if(auto menu=reordering||!row.context.get()?row.menu.get():row.context.get())if(auto item=row.item.get()){
            Primitives::FlyoutShowOptions options;options.Position(at);
            options.ShowMode(holding?Primitives::FlyoutShowMode::Transient:Primitives::FlyoutShowMode::Standard);
            menu.ShowAt(item,options);
        }
    }
    void hold(NativeInput::HoldingEventArgs const& e){
        if(!pointer||mode!=Mode::Pending||child()||device==NativeInput::PointerDeviceType::Mouse)return;
        if(e.HoldingState()!=NativeInput::HoldingState::Started)return;
        if(!claim())return;
        mode=Mode::Held;ignoreClick=true;suppressContext=true;if(source->swipe)source->swipe->presenter->SetOffset(0);
        auto item=source->item.get();item.Focus(FocusState::Pointer);
        auto at=surface.TransformToVisual(item).TransformPoint(position);
        menuAt(*source,at,true,grip());evidence();
    }
    void down(PointerRoutedEventArgs const& e){
        if(finishing()){e.Handled(true);return;}
        if(pointer){if(pointer.PointerId()!=e.Pointer().PointerId())cancel(L"multiple_contacts");return;}
        ignoreClick=false;suppressContext=false;++generation;
        auto original=e.OriginalSource().try_as<DependencyObject>();if(!original)return;
        for(auto const& row:rows)if(auto item=row.item.get();item&&descendantOf(original,item)){
            auto point=e.GetCurrentPoint(surface);
            if(!PointerGesture::PrimaryContact(point))return;
            auto inputPart=descendantOf(original,row.grip.get())?Part::Grip:Part::Body;
            if(descendantOf(original,row.more.get())||descendantOf(original,row.pin.get())||(row.swipe&&descendantOf(original,row.swipe->remove)))inputPart=Part::Child;
            auto node=original;
            while(node&&node!=surface){
                if(node.try_as<TextBox>())inputPart=Part::Child;
                if(auto element=node.try_as<UIElement>())path.emplace_back(make_weak(element));
                node=VisualTreeHelper::GetParent(node);
            }
            if(inputPart!=Part::Child&&!available()){path.clear();return;}
            closeSwipes(original);
            source=row;pointer=e.Pointer();device=point.PointerDeviceType();mode=Mode::Pending;part=inputPart;
            initialScroll=scroll?scroll.VerticalOffset():0;origin=position=point.Position();owner=GetAncestor(GetForegroundWindow(),GA_ROOTOWNER);
            ignoreClick=grip()||child();pickup=!row.swipe||row.swipe->presenter->AtRest();initialSwipe=row.swipe?std::max(0.,row.swipe->presenter->Offset()):0;
            if(!child()){
                if(row.swipe)row.swipe->presenter->SetOffset(initialSwipe,false);
                if(grip()&&!claim())return;
                recognizing=true;recognizer.Begin(item,point,device==NativeInput::PointerDeviceType::Mouse?NativeInput::GestureSettings::None:NativeInput::GestureSettings::Hold);
            }
            tickAt=std::chrono::steady_clock::now();timer.Start();evidence();return;
        }
    }
    bool crossed()const{return recognizer.Moved();}
    void target(){
        valid=false;before.reset();hint.Visibility(Visibility::Collapsed);
        if(!dragging()||!current()||!std::isfinite(position.X)||!std::isfinite(position.Y))return;
        if(position.X<0||position.Y<0||position.X>surface.ActualWidth()||position.Y>surface.ActualHeight())return;
        std::optional<hstring> last;double line=0;
        for(auto value:rowItems(list)){
            auto row=value.try_as<ItemContainer>();if(!row||!row.IsLoaded())continue;
            auto bounds=row.TransformToVisual(surface).TransformBounds({0,0,float(row.ActualWidth()),float(row.ActualHeight())});
            if(bounds.Height<=0||bounds.Y+bounds.Height<=0||bounds.Y>=surface.ActualHeight())continue;
            auto id=unbox_value<hstring>(row.Tag());
            if(capy_list_drop_edge(position.Y,bounds.Y,bounds.Y+bounds.Height*.5f)!=1){before=id;line=bounds.Y;last.reset();break;}
            last=id;line=bounds.Y+bounds.Height;
        }
        if(last){
            auto found=std::find(order.begin(),order.end(),*last);
            if(found!=order.end()&&++found!=order.end())before=*found;
        }else if(!before)return;
        auto found=std::find(order.begin(),order.end(),source->id);
        std::optional<hstring> successor;
        if(found!=order.end()&&++found!=order.end())successor=*found;
        // Both edges of the source are the same no-op. Shared Rust still
        // validates the final id/before pair against current persisted order.
        if(before==source->id||before==successor)return;
        valid=true;
        hint.Width(std::max(0.,surface.ActualWidth()));hint.Margin({0,std::clamp(line,0.,std::max(0.,surface.ActualHeight()-2)),0,0});
        hint.Visibility(Visibility::Visible);
    }
    void motion(PointerRoutedEventArgs const& e){
        if(!pointer||pointer.PointerId()!=e.Pointer().PointerId())return;
        if(!current()){cancel(L"source_invalid");return;}
        position=e.GetCurrentPoint(surface).Position();
        if(child())return;
        if(recognizing)recognizer.Move(surface,e);
        if(!dragging()&&!swiping()&&crossed()){
            if(!grip()&&device!=NativeInput::PointerDeviceType::Mouse&&!held()){
                auto action=source->swipe?source->swipe->remove:Button{nullptr};double dx=position.X-origin.X,dy=position.Y-origin.Y;
                if(source->swipe&&std::abs(dx)>std::abs(dy)&&((dx<0&&action&&action.IsEnabled())||(dx>0&&initialSwipe>0))){
                    if(!claim())return;
                    mode=Mode::Swiping;ignoreClick=true;suppressContext=true;hideMenus();
                    if(recognizing){recognizing=false;recognizer.CompleteGesture();}
                }else{clear(true);return;}
            }else{
                if(!pickup)return;
                if(!claim())return;
                mode=Mode::Dragging;ignoreClick=true;suppressContext=true;
                auto row=source->item.get();preview->Start(row,surface,origin,position,.45);hideMenus();
            }
        }
        if(swiping()){source->swipe->presenter->SetOffset(std::max(0.,initialSwipe-(position.X-origin.X)),false);e.Handled(true);evidence();}
        if(dragging()){preview->Move(surface,position);target();e.Handled(true);evidence();}
    }
    void up(PointerRoutedEventArgs const& e){
        if(!pointer){
            if(suppressContext){ignoreClick=true;deferClick();e.Handled(true);}
            return;
        }
        if(pointer.PointerId()!=e.Pointer().PointerId())return;
        if(!ownsFocus()){cancel(L"focus_lost");e.Handled(true);return;}
        if(e.GetCurrentPoint(surface).Properties().IsCanceled()){cancel(L"pointer_canceled");e.Handled(true);return;}
        if(recognizing){recognizing=false;recognizer.ProcessUpEvent(e.GetCurrentPoint(surface));}
        position=e.GetCurrentPoint(surface).Position();
        if(swiping()){
            auto threshold=source->swipe->actionWidth*source->swipe->threshold;
            if(initialSwipe>0)threshold=initialSwipe-threshold;
            source->swipe->presenter->SetOffset(source->swipe->presenter->Offset()>=threshold?source->swipe->presenter->RevealWidth():0);
            ignoreClick=true;clear(true);e.Handled(true);return;
        }
        if(initialSwipe>0&&part==Part::Body&&!held()){
            source->swipe->presenter->SetOffset(0);ignoreClick=true;clear(true);e.Handled(true);return;
        }
        if(dragging())target();
        auto id=source->id;auto destination=before;bool commit=dragging()&&valid&&current();
        if(trace)lastRelease=O({{L"generation",N(double(generation))},{L"source",S(id)},{L"commit",B(commit)},
            {L"canceled",B(e.GetCurrentPoint(surface).Properties().IsCanceled())},
            {L"before",before?S(*before):JsonValue::CreateNullValue()},
            {L"position",O({{L"x",N(position.X)},{L"y",N(position.Y)}})}});
        auto menuRow=*source;
        auto menuPoint=surface.TransformToVisual(source->item.get()).TransformPoint(position);
        bool open=grip()&&!held()&&!dragging()&&current();
        if(auto button=source->grip.get();open&&button){
            auto point=surface.TransformToVisual(button).TransformPoint(position);
            open=point.X>=0&&point.Y>=0&&point.X<button.ActualWidth()&&point.Y<button.ActualHeight();
        }
        bool handled=held()||dragging()||grip();
        ignoreClick=ignoreClick||handled;
        if(commit){
            mode=Mode::Finishing;hideMenus();release();evidence();
            preview->Commit([move=move,id,destination](std::function<void()> ready){move(id,destination,std::move(ready));},
                [weak=weak_from_this()]{if(auto self=weak.lock())self->clear(true);});
        }else clear(dragging());
        if(handled)e.Handled(true);
        if(open)menuAt(menuRow,menuPoint,false,true);
    }
    void tick(){
        if(!pointer)return;
        if(!ownsFocus()){cancel(L"focus_lost");return;}
        if(!current()){cancel();return;}
        auto now=std::chrono::steady_clock::now();
        double elapsed=std::min(.1,std::chrono::duration<double>(now-tickAt).count());tickAt=now;
        if(!dragging())return;
        if(!scroll)return;
        constexpr double edge=32;
        double direction=position.Y<edge?-std::clamp((edge-position.Y)/edge,0.,1.):
            position.Y>surface.ActualHeight()-edge?std::clamp((position.Y-surface.ActualHeight()+edge)/edge,0.,1.):0;
        if(direction&&position.X>=0&&position.X<=surface.ActualWidth()){
            scrollVertical(scroll,scroll.VerticalOffset()+direction*480*elapsed);
            target();evidence();
        }
    }
    void attach(ItemContainer const& item,Button const& gripButton,Button const& more,UIElement const& pin,MenuFlyout const& menu,hstring id,MenuFlyout const& context,std::optional<Swipe> swipe){
        Row row{make_weak(item),make_weak(gripButton),make_weak(more),make_weak(context),make_weak(pin),make_weak(menu),id,std::move(swipe)};rows.push_back(row);
        row.swipe.reset();
        menu.Opening([weak=weak_from_this()](auto&& sender,auto&&){if(auto self=weak.lock())self->visibleMenu=sender.template as<MenuFlyout>();});
        menu.Closed([weak=weak_from_this()](auto&& sender,auto&&){if(auto self=weak.lock();self&&self->visibleMenu==sender)self->visibleMenu=nullptr;});
        if(context)context.Opening([weak=weak_from_this()](auto&& sender,auto&&){if(auto self=weak.lock())self->visibleMenu=sender.template as<MenuFlyout>();});
        if(context)context.Closed([weak=weak_from_this()](auto&& sender,auto&&){if(auto self=weak.lock();self&&self->visibleMenu==sender)self->visibleMenu=nullptr;});
        PointerGesture::OwnHolding(item);
        item.RightTapped([weak=weak_from_this(),row](auto&&,RightTappedRoutedEventArgs const& e){
            if(auto self=weak.lock();self&&self->available()){
                e.Handled(true);
                if(self->suppressContext)return;
                if(!self->cancel()){self->ignoreClick=true;self->deferClick();}
                if(auto target=row.item.get())self->menuAt(row,e.GetPosition(target),false);
            }
        });
        item.KeyDown([weak=weak_from_this(),row](auto&&,KeyRoutedEventArgs const& e){
            bool context=e.Key()==Windows::System::VirtualKey::Application||
                e.Key()==Windows::System::VirtualKey::F10&&(GetKeyState(VK_SHIFT)&0x8000);
            if(context)if(auto self=weak.lock();self&&self->available()){
                self->cancel();self->menuAt(row,{24,24},false);e.Handled(true);
            }
        });
        // The accessible grip opens the same keyboard Move Up/Down commands.
        gripButton.Click([weak=weak_from_this(),row](auto&&,auto&&){
            if(auto self=weak.lock();self&&!self->ignoreClick&&self->available())self->menuAt(row,{24,24},false,true);
        });
        item.Unloaded([weak=weak_from_this(),id](auto&&,auto&&){
            if(auto self=weak.lock();self&&self->source&&self->source->id==id){
                auto epoch=self->generation;
                self->surface.DispatcherQueue().TryEnqueue([weak,id,epoch]{
                    // Retained rows can be removed/reinserted in one reconciliation.
                    if(auto owner=weak.lock();owner&&owner->generation==epoch&&owner->source&&owner->source->id==id&&!owner->finishing()&&!owner->current())owner->cancel();
                });
            }
        });
    }
    void init(){
        hint.Height(2);hint.Background(paletteResource(L"AccentFillColorDefaultBrush"));hint.IsHitTestVisible(false);
        hint.HorizontalAlignment(HorizontalAlignment::Left);hint.VerticalAlignment(VerticalAlignment::Top);
        hint.Visibility(Visibility::Collapsed);AutomationProperties::SetAutomationId(hint,L"workspace-manager-drop");
        surface.Children().Append(hint);
        surface.Loaded([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock()){self->hint.Background(paletteResource(L"AccentFillColorDefaultBrush"));self->attachOutside();}});
        surface.Unloaded([weak=weak_from_this()](auto&&,auto&&){
            if(auto self=weak.lock())self->surface.DispatcherQueue().TryEnqueue([weak]{
                if(auto self=weak.lock();self&&!self->surface.IsLoaded()){self->cancel(L"unloaded");self->detachOutside();}
            });
        });
        auto observe=[weak=weak_from_this()]{if(auto self=weak.lock()){
            auto next=self->list.ScrollView();if(!next||next==self->scroll)return;self->scroll=next;
            auto content=next.Content().as<UIElement>();
            content.ManipulationStarted([weak](auto&&,auto&&){if(auto self=weak.lock()){self->panning=true;self->evidence();}});
            content.ManipulationCompleted([weak](auto&&,auto&&){if(auto self=weak.lock()){self->panning=false;self->evidence();}});
            next.ViewChanged([weak](auto&&,auto&&){if(auto self=weak.lock();self&&self->pointer&&!self->held()&&!self->grip()&&!self->dragging()&&!self->swiping()&&self->device!=NativeInput::PointerDeviceType::Mouse&&std::abs(self->scroll.VerticalOffset()-self->initialScroll)>.5)self->cancel(L"native_scroll");});
        }};
        list.RegisterPropertyChangedCallback(ItemsView::ScrollViewProperty(),[observe](auto&&,auto&&){observe();});
        list.Loaded([observe](auto&&,auto&&){observe();});if(list.IsLoaded())observe();
        recognizer.Holding([weak=weak_from_this()](auto&&,NativeInput::HoldingEventArgs const& e){if(auto self=weak.lock())self->hold(e);});
        timer=surface.DispatcherQueue().CreateTimer();timer.Interval(std::chrono::milliseconds(16));
        timer.Tick([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock())self->tick();});
        surface.AddHandler(UIElement::PointerPressedEvent(),box_value(PointerEventHandler([weak=weak_from_this()](auto&&,auto&& e){if(auto self=weak.lock())self->down(e);})),true);
        surface.AddHandler(UIElement::PointerMovedEvent(),box_value(PointerEventHandler([weak=weak_from_this()](auto&&,auto&& e){if(auto self=weak.lock())self->motion(e);})),true);
        PointerGesture::OnLeaving(surface,[weak=weak_from_this()](PointerRoutedEventArgs const& e){
            if(auto self=weak.lock();self&&self->pointer&&self->pointer.PointerId()==e.Pointer().PointerId()&&!self->owns(self->surface)){
                self->clear(false);
            }
        });
        surface.AddHandler(UIElement::PointerReleasedEvent(),box_value(PointerEventHandler([weak=weak_from_this()](auto&&,auto&& e){if(auto self=weak.lock())self->up(e);})),true);
        auto canceled=PointerEventHandler([weak=weak_from_this()](auto&&,PointerRoutedEventArgs const& e){
            if(auto self=weak.lock();self&&!self->releasing&&self->pointer&&self->pointer.PointerId()==e.Pointer().PointerId())self->cancel(L"pointer_canceled");
        });
        surface.AddHandler(UIElement::PointerCanceledEvent(),box_value(canceled),true);
        surface.AddHandler(UIElement::PointerCaptureLostEvent(),box_value(PointerEventHandler([weak=weak_from_this()](auto&&,PointerRoutedEventArgs const& e){
            if(auto self=weak.lock();self&&!self->releasing&&self->pointer&&self->pointer.PointerId()==e.Pointer().PointerId()){
                auto point=e.GetCurrentPoint(self->surface);bool captured=self->owns(self->surface);
                if(e.OriginalSource()!=self->surface&&captured&&!point.Properties().IsCanceled())return;
                // Native buttons release their capture before Click/ItemClick.
                // Preserve that ordinary release and the More button's flyout.
                if(self->child())self->clear(false);
                else if(!self->held()&&!self->dragging()&&!self->swiping()&&!self->grip()&&!e.GetCurrentPoint(self->surface).IsInContact())self->up(e);
                else{
                    auto generation=self->generation;auto id=self->pointer.PointerId();
                    self->surface.DispatcherQueue().TryEnqueue([weak=std::weak_ptr<Impl>(self),generation,id]{
                        if(auto self=weak.lock()){
                            if(self->generation==generation&&self->pointer&&self->pointer.PointerId()==id&&!self->owns(self->surface))self->cancel(L"capture_lost");
                        }
                    });
                }
            }
        })),true);
        ShortcutKeys(surface,[weak=weak_from_this()](auto&&,KeyRoutedEventArgs const& e){
            if(auto self=weak.lock()){
                if(e.Key()==Windows::System::VirtualKey::Escape&&self->cancel(L"escape"))e.Handled(true);
                else if(!self->pointer&&!self->finishing())self->ignoreClick=false;
            }
        });
    }
};
WorkspaceRowDrag::WorkspaceRowDrag(ItemsView list,Grid surface,std::shared_ptr<WorkspaceData> const& data,std::function<bool()> available,Move move,std::function<void()> ended):impl(std::make_shared<Impl>()){
    impl->list=list;impl->surface=surface;impl->context=data;impl->available=std::move(available);impl->move=std::move(move);impl->ended=std::move(ended);impl->init();
}
WorkspaceRowDrag::~WorkspaceRowDrag(){impl->cancel();impl->detachOutside();impl->timer.Stop();}
void WorkspaceRowDrag::Attach(ItemContainer row,Button grip,Button more,UIElement pin,MenuFlyout menu,Id id,MenuFlyout context,std::optional<Swipe> swipe){impl->attach(row,grip,more,pin,menu,id,context,std::move(swipe));}
void WorkspaceRowDrag::Refresh(std::vector<Id> const& order){
    impl->order=order;
    std::erase_if(impl->rows,[](auto const& row){return !row.item.get();});
    if(impl->pointer&&!impl->current())impl->cancel();
}
bool WorkspaceRowDrag::FenceSelection()const{return bool(impl->pointer)||impl->ignoreClick;}
bool WorkspaceRowDrag::SuppressClick()const{return impl->ignoreClick;}
bool WorkspaceRowDrag::Pending()const{return impl->finishing();}
bool WorkspaceRowDrag::Cancel(){return impl->cancel();}
bool WorkspaceRowDrag::Escape(){
    if(impl->cancel(L"escape"))return true;
    if(impl->visibleMenu){impl->visibleMenu.Hide();return true;}
    return false;
}
