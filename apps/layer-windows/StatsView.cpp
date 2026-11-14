#include "pch.h"
#include <winrt/Microsoft.UI.Xaml.Documents.h>
#include "StatsView.h"
#include "MeterControls.h"
#include "TraceFile.h"
#include "WorkspaceQuery.h"
#include "StrokeRecording.h"
#include <chrono>
#include <winrt/Microsoft.UI.Xaml.Shapes.h>

using namespace CapyUi;
namespace {
struct StatsView:std::enable_shared_from_this<StatsView>{
    std::shared_ptr<WorkspaceData> data;
    StackPanel root;
    Canvas chart;
    Microsoft::UI::Xaml::Shapes::Polyline samples;
    Microsoft::UI::Xaml::Shapes::Line budget;
    Microsoft::UI::Dispatching::DispatcherQueueTimer timer{nullptr};
    std::vector<std::unique_ptr<MeterRow>> timings;
    std::unique_ptr<MeterSection> memory;
    std::shared_ptr<OrderedTraceFile> trace;
    hstring palette;
    Button record{nullptr};
    TextBlock recordError;
    uint64_t listener=0;
    J model;
    bool busy=false;
    uint64_t generation=0;
    ~StatsView(){if(timer)timer.Stop();if(listener&&data->strokes)data->strokes->forget(listener);}
    bool visible()const{
        // An empty stack has zero natural height until its first query arrives.
        if(!root.IsLoaded()||root.ActualWidth()<=0)return false;
        DependencyObject item=root;
        while(item){
            if(auto element=item.try_as<UIElement>();element&&element.Visibility()!=Visibility::Visible)return false;
            item=VisualTreeHelper::GetParent(item);
        }
        return true;
    }
    void init(){
        root.Spacing(6);
        memory=std::make_unique<MeterSection>(data);
        if(GetEnvironmentVariableW(L"CAPY_TRACE_UI",nullptr,0))trace=std::make_shared<OrderedTraceFile>();
        AutomationProperties::SetAutomationId(root,L"renderer-stats");
        chart.Height(46);chart.Background(clear());
        AutomationProperties::SetAutomationId(chart,L"renderer-stats-chart");
        samples.Stroke(data->brush(L"text"));samples.StrokeThickness(1.5);samples.Opacity(.9);
        budget.Stroke(data->brush(L"text"));budget.StrokeThickness(1);budget.Opacity(.3);DoubleCollection dash;dash.Append(3);dash.Append(3);budget.StrokeDashArray(dash);
        chart.Children().Append(budget);chart.Children().Append(samples);
        chart.SizeChanged([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock())self->draw();});
        record=raisedButton(data,L"",[weak=weak_from_this()]{if(auto self=weak.lock();self&&self->data->strokes)self->data->strokes->click();});
        record.HorizontalAlignment(HorizontalAlignment::Left);record.MinHeight(data->controlHeight());record.Padding({6,0,6,0});
        AutomationProperties::SetAutomationId(record,L"stroke-recording");
        recordError=label(data,L"");recordError.TextWrapping(TextWrapping::Wrap);recordError.Opacity(.75);AutomationProperties::SetAutomationId(recordError,L"stroke-recording-error");
        AutomationProperties::SetLiveSetting(recordError,Automation::Peers::AutomationLiveSetting::Polite);
        root.Children().Append(record);root.Children().Append(recordError);
        if(data->strokes)listener=data->strokes->listen([weak=weak_from_this()]{if(auto self=weak.lock())self->recording();});
        recording();data->copyView([weak=weak_from_this()]{if(auto self=weak.lock()){self->recording();return true;}return false;});
        timer=root.DispatcherQueue().CreateTimer();timer.Interval(std::chrono::milliseconds(200));
        timer.Tick([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock())self->refresh();});
        root.Loaded([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock()){self->timer.Start();self->refresh();}});
        root.Unloaded([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock()){self->timer.Stop();++self->generation;}});
    }
    void refresh(){
        if(busy||!visible())return;
        busy=true;auto requested=generation;
        if(!QueryWorkspace(data->query,O({{L"type",S(L"renderer_stats")}}),
            [weak=weak_from_this(),requested](J reply){
                if(auto self=weak.lock()){
                    self->busy=false;
                    if(requested!=self->generation||!self->visible())return;
                    auto model=object(reply,L"result");
                    if(model.Size())self->apply(model);
                }
            }))busy=false;
    }
    void apply(J const& next){
        if(model&&model.Stringify()==next.Stringify()&&palette==data->paletteKey)return;
        auto rows=array(next,L"rows");bool rebuild=timings.size()!=rows.Size();
        bool chartChanged=rebuild||array(model,L"samples").Stringify()!=array(next,L"samples").Stringify()
            ||num(model,L"budget_ms")!=num(next,L"budget_ms")||str(model,L"chart_label")!=str(next,L"chart_label");
        model=next;palette=data->paletteKey;
        if(trace){
            auto packet=O({{L"process_id",N(GetCurrentProcessId())},{L"window_id",N(double(data->windowId))},{L"result",next}});
            PublishTraceFile(trace,"renderer-stats-"+std::to_string(GetCurrentProcessId())+"-"+std::to_string(data->windowId)+".json",to_string(packet.Stringify()));
        }
        if(rebuild){
            root.Children().Clear();timings.clear();
            for(uint32_t i=0;i<rows.Size();++i){
                auto row=std::make_unique<MeterRow>(data,L"",L"renderer-stat-"+to_hstring(i));root.Children().Append(row->root);timings.push_back(std::move(row));
                if(i+1==uint32_t(num(model,L"chart_after_rows")))root.Children().Append(chart);
            }
            auto separator=[&]{Border line;line.Height(1);line.Background(data->line(L"panel",L"border"));root.Children().Append(line);};
            separator();root.Children().Append(memory->root);separator();root.Children().Append(record);root.Children().Append(recordError);
        }
        for(uint32_t i=0;i<rows.Size();++i)timings[i]->update(data,rows.GetObjectAt(i));
        memory->update(data,object(model,L"memory"));tooltip(chart,str(model,L"chart_label"));
        if(chartChanged){AutomationProperties::SetName(chart,str(model,L"chart_label"));draw();}
    }
    void recording(){
        if(!data->strokes)return;auto const& strokes=*data->strokes;auto status=strokes.status;auto text=str(status,L"label");
        buttonCaption(record,text);AutomationProperties::SetName(record,text);record.IsEnabled(!strokes.saving);
        hstring hint=data->caption(L"color",L"record_tablet");
        if(flag(status,L"recording"))hint=hint+L" \u00b7 "+to_hstring(int64_t(num(status,L"elapsed_seconds")))+L"s \u00b7 "+to_hstring(int64_t(num(status,L"raw_events")))+L" inputs";
        tooltip(record,hint);
        recordError.Text(strokes.error);recordError.Visibility(strokes.error.empty()?Visibility::Collapsed:Visibility::Visible);
    }
    void draw(){
        auto readings=array(model,L"samples");double limit=num(model,L"budget_ms",1000./120.);
        double maximum=limit;
        for(auto value:readings)maximum=std::max(maximum,value.GetNumber());
        maximum=std::max(0.001,maximum*1.1);double width=chart.ActualWidth(),height=chart.Height();
        auto points=PointCollection();
        for(uint32_t i=0;i<readings.Size();i++)
            points.Append({float(i*width/119.),float(height*(1-readings.GetNumberAt(i)/maximum))});
        samples.Points(points);
        budget.X1(0);budget.X2(width);budget.Y1(height*(1-limit/maximum));budget.Y2(budget.Y1());
    }
};
}
FrameworkElement StatsPanel(std::shared_ptr<WorkspaceData> const& data,Bindings& bindings){
    auto view=std::make_shared<StatsView>();view->data=data;view->init();
    bindings.emplace_back([view]{view->refresh();});return view->root;
}
