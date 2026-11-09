#include "PocWindow.h"

namespace Poc {
void PocWindow::StartInput(){
    inputController=DispatcherQueueController::CreateOnDedicatedThread();
    inputController.DispatcherQueue().TryEnqueue([weak=weak_from_this()]{if(auto self=weak.lock();self&&!self->closing){
        try{
            using namespace Microsoft::UI::Input;
            self->input=self->panel.CreateCoreIndependentInputSource(InputPointerSourceDeviceKinds::Pen|InputPointerSourceDeviceKinds::Touch|InputPointerSourceDeviceKinds::Mouse);
            self->input.PointerPressed([weak](auto&&,PointerEventArgs const& args){if(auto self=weak.lock())self->Pointer(args,1);});
            self->input.PointerMoved([weak](auto&&,PointerEventArgs const& args){if(auto self=weak.lock())self->Pointer(args,2);});
            self->input.PointerReleased([weak](auto&&,PointerEventArgs const& args){if(auto self=weak.lock())self->Pointer(args,3);});
            self->input.PointerCaptureLost([weak](auto&&,PointerEventArgs const& args){if(auto self=weak.lock())self->Pointer(args,4);});
            self->input.PointerRoutedReleased([weak](auto&&,PointerEventArgs const& args){if(auto self=weak.lock())self->Pointer(args,3);});
            self->inputCpuTimer=self->inputController.DispatcherQueue().CreateTimer();self->inputCpuTimer.Interval(std::chrono::seconds(1));self->inputCpuTimer.Tick([weak](auto&&,auto&&){if(auto self=weak.lock();self&&!self->closing)self->Trace("thread_cpu",Fields({{L"thread",JsonValue::CreateStringValue(L"input")},{L"cpu_ms",JsonValue::CreateNumberValue(CpuMs())}}));});self->inputCpuTimer.Start();
            self->Trace("startup",Fields({{L"stage",JsonValue::CreateStringValue(L"independent_input_registered")}}));
            if(!poc_canvas_start_panel(self->canvas))self->dispatcher.TryEnqueue([weak]{if(auto self=weak.lock())self->Fail(E_FAIL);});
        }catch(hresult_error const& error){self->dispatcher.TryEnqueue([weak,code=error.code()]{if(auto self=weak.lock())self->Fail(code);});}
    }});
}
void PocWindow::Pointer(Microsoft::UI::Input::PointerEventArgs const& args,uint32_t phase){
    auto receipt=Now();if(closing||!canvas)return;
    auto current=args.CurrentPoint();auto id=current.PointerId();if(current.Properties().IsCanceled())phase=4;
    if(phase==1)contacts.insert_or_assign(id,Contact{current.PointerDeviceType()==Microsoft::UI::Input::PointerDeviceType::Pen});
    auto target=contacts.find(id);if(target==contacts.end())return;
    if(!target->second.canvas){if(phase>=3)contacts.erase(target);return;}
    auto history=args.GetIntermediatePoints();std::vector<PocPenSample> samples;samples.reserve(std::max(1u,history.Size()));auto factor=inputScale.load();
    auto append=[&](Microsoft::UI::Input::PointerPoint const& point,bool last){auto properties=point.Properties();auto position=point.Position();uint32_t flags=point.IsInContact()?0x4u:0u;if(last&&phase==1)flags|=0x10000u;if(last&&phase==3)flags|=0x40000u;if(last&&phase==4)flags|=0x8000u;samples.push_back({position.X*factor,position.Y*factor,properties.Pressure(),float(properties.XTilt()),float(properties.YTilt()),float(properties.Twist()),point.Timestamp(),flags});};
    if(history.Size()==0)append(current,true);else for(uint32_t remaining=history.Size();remaining>0;--remaining)append(history.GetAt(remaining-1),remaining==1);
    POINT origin{};auto originAvailable=ClientToScreen(handle,&origin)!=FALSE;
    auto same=[&](PocPenSample const& sample,POINTER_PEN_INFO const& point){return originAvailable&&std::lround(sample.x)==point.pointerInfo.ptPixelLocation.x-origin.x&&std::lround(sample.y)==point.pointerInfo.ptPixelLocation.y-origin.y&&std::abs(sample.pressure-float(point.pressure)/1024)<=1.0f/1024&&sample.tilt_x==float(point.tiltX)&&sample.tilt_y==float(point.tiltY)&&sample.twist==float(point.rotation);};
    UINT nativeCount{};std::vector<POINTER_PEN_INFO> nativeHistory;
    GetPointerPenInfoHistory(id,&nativeCount,nullptr);
    if(nativeCount>0&&nativeCount==samples.size()){nativeHistory.resize(nativeCount);if(!GetPointerPenInfoHistory(id,&nativeCount,nativeHistory.data())||nativeCount!=samples.size())nativeHistory.clear();}
    bool matched=!nativeHistory.empty();for(size_t i=0;matched&&i<samples.size();++i)matched=same(samples[i],nativeHistory[samples.size()-1-i]);
    if(matched)for(size_t i=0;i<samples.size();++i)samples[i].timestamp=nativeHistory[samples.size()-1-i].pointerInfo.PerformanceCount;
    POINTER_PEN_INFO latest{};auto latestAvailable=GetPointerPenInfo(id,&latest)!=FALSE;auto latestMatched=latestAvailable&&same(samples.back(),latest);
    Trace("host_pen_message",Fields({{L"receipt_qpc",JsonValue::CreateNumberValue(double(receipt))},{L"pointer_id",JsonValue::CreateNumberValue(id)},{L"phase",JsonValue::CreateNumberValue(phase)},{L"history_count",JsonValue::CreateNumberValue(history.Size())},{L"samples",JsonValue::CreateNumberValue(double(samples.size()))},{L"timestamp_basis",JsonValue::CreateStringValue(matched?L"native_PerformanceCount_matched_ordered_history":L"PointerPoint.Timestamp_microseconds")},{L"native_history_count",JsonValue::CreateNumberValue(nativeCount)},{L"native_history_matched",JsonValue::CreateBooleanValue(matched)},{L"native_latest_tuple_matched",JsonValue::CreateBooleanValue(latestMatched)},{L"native_performance_count",JsonValue::CreateNumberValue(latestMatched?double(latest.pointerInfo.PerformanceCount):0)},{L"native_latest_x",JsonValue::CreateNumberValue(latestAvailable?latest.pointerInfo.ptPixelLocation.x-origin.x:0)},{L"native_latest_y",JsonValue::CreateNumberValue(latestAvailable?latest.pointerInfo.ptPixelLocation.y-origin.y:0)},{L"native_latest_pressure",JsonValue::CreateNumberValue(latestAvailable?double(latest.pressure)/1024:0)},{L"native_latest_tilt_x",JsonValue::CreateNumberValue(latestAvailable?latest.tiltX:0)},{L"native_latest_tilt_y",JsonValue::CreateNumberValue(latestAvailable?latest.tiltY:0)},{L"native_latest_twist",JsonValue::CreateNumberValue(latestAvailable?latest.rotation:0)}}));
    if(!poc_canvas_pen(canvas,samples.data(),samples.size(),receipt))dispatcher.TryEnqueue([weak=weak_from_this()]{if(auto self=weak.lock())self->Fail(E_FAIL);});
    if(phase>=3)contacts.erase(id);args.Handled(true);
}
}
