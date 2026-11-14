#pragma once
#include "UiControls.h"
#include <winrt/Microsoft.UI.Xaml.Documents.h>
#include <winrt/Microsoft.UI.Xaml.Shapes.h>

namespace CapyUi {
inline void meterColumns(Grid const& grid,std::initializer_list<GridLength> widths){
    for(auto width:widths){ColumnDefinition column;column.Width(width);grid.ColumnDefinitions().Append(column);}
}
struct MeterChip {
    Border root;
    TextBlock caption{nullptr};
    explicit MeterChip(std::shared_ptr<WorkspaceData> const& data){
        root.Padding({7,0,7,0});root.CornerRadius({999});root.VerticalAlignment(VerticalAlignment::Center);
        caption=label(data,L"");caption.FontSize(data->textSize()*.85);opticalText(caption,data->textSize()*.85);
        caption.FontWeight(Windows::UI::Text::FontWeights::SemiBold());root.Child(caption);
    }
    void update(std::shared_ptr<WorkspaceData> const& data,J const& chip){
        root.Visibility(chip.Size()?Visibility::Visible:Visibility::Collapsed);
        if(!chip.Size())return;
        auto text=str(chip,L"label");if(caption.Text()!=text)caption.Text(text);
        bool warning=flag(chip,L"warning");caption.Foreground(data->brush(warning?L"error":L"text"));
        root.Background(data->tint(warning?L"error":L"text",warning?41:26));
    }
};
struct MeterBar {
    Grid root;
    Border track;
    Microsoft::UI::Xaml::Shapes::Rectangle notch;
    double limit=-1;
    hstring shown,palette;
    MeterBar(){
        root.Height(12);meterColumns(root,{{1,GridUnitType::Star},{1,GridUnitType::Star}});
        track.Height(6);track.VerticalAlignment(VerticalAlignment::Center);track.CornerRadius({3});Grid::SetColumnSpan(track,2);
        root.Children().Append(track);notch.Width(1);notch.Height(12);notch.HorizontalAlignment(HorizontalAlignment::Right);root.Children().Append(notch);
    }
    static Windows::UI::Color overlay(Windows::UI::Color front,double opacity,Windows::UI::Color back){
        double a=front.A/255.*opacity,b=back.A/255.*(1-a),sum=a+b;
        auto channel=[&](uint8_t x,uint8_t y){return uint8_t(std::lround(sum?(x*a+y*b)/sum:0));};
        return {uint8_t(std::lround(sum*255)),channel(front.R,back.R),channel(front.G,back.G),channel(front.B,back.B)};
    }
    void update(std::shared_ptr<WorkspaceData> const& data,J const& meter){
        root.Visibility(meter.Size()?Visibility::Visible:Visibility::Collapsed);if(!meter.Size())return;
        auto key=meter.Stringify();if(shown==key&&palette==data->paletteKey)return;shown=key;palette=data->paletteKey;
        auto next=num(meter,L"limit");auto segments=array(meter,L"segments");
        auto ink=data->paletteColor(L"text"),accent=data->paletteColor(L"accent"),error=data->paletteColor(L"error");
        std::vector<double> boundaries{0,1,next};
        for(auto value:segments){auto segment=value.GetObject();boundaries.push_back(num(segment,L"from"));boundaries.push_back(num(segment,L"to"));}
        std::sort(boundaries.begin(),boundaries.end());boundaries.erase(std::unique(boundaries.begin(),boundaries.end()),boundaries.end());
        LinearGradientBrush brush;brush.StartPoint({0,0});brush.EndPoint({1,0});
        for(size_t i=1;i<boundaries.size();++i){
            double from=boundaries[i-1],to=boundaries[i],at=(from+to)/2;
            auto color=ink;color.A=uint8_t(std::lround(255*(at<next?.12:.06)));
            for(auto value:segments){
                auto segment=value.GetObject();if(num(segment,L"from")>at||num(segment,L"to")<at)continue;
                auto tone=str(segment,L"tone");
                if(tone==L"fill")color=accent;else if(tone==L"over")color=error;
                else color=overlay(tone==L"soft"?accent:ink,tone==L"soft"?.4:.32,color);
            }
            for(auto offset:{from,to}){GradientStop stop;stop.Offset(offset);stop.Color(color);brush.GradientStops().Append(stop);}
        }
        track.Background(brush);notch.Fill(data->tint(L"text",153));notch.Visibility(next<.999?Visibility::Visible:Visibility::Collapsed);
        if(limit!=next){limit=next;root.ColumnDefinitions().GetAt(0).Width({limit,GridUnitType::Star});root.ColumnDefinitions().GetAt(1).Width({1-limit,GridUnitType::Star});}
    }
};
struct MeterRow {
    StackPanel root;
    Grid line,lead;
    TextBlock caption{nullptr},value{nullptr};
    MeterChip chip;
    MeterBar bar;
    explicit MeterRow(std::shared_ptr<WorkspaceData> const& data,hstring const& rowId={},hstring const& valueId={},hstring const& barId={}):chip(data){
        root.Spacing(3);root.Background(clear());meterColumns(line,{{1,GridUnitType::Star},{1,GridUnitType::Auto}});line.ColumnSpacing(6);
        meterColumns(lead,{{1,GridUnitType::Star},{1,GridUnitType::Auto}});lead.ColumnSpacing(6);lead.HorizontalAlignment(HorizontalAlignment::Left);
        caption=label(data,L"");caption.TextTrimming(TextTrimming::CharacterEllipsis);caption.MaxLines(1);
        lead.Children().Append(caption);Grid::SetColumn(chip.root,1);lead.Children().Append(chip.root);line.Children().Append(lead);
        value=label(data,L"");Documents::Typography::SetNumeralAlignment(value,FontNumeralAlignment::Tabular);value.TextAlignment(TextAlignment::Right);Grid::SetColumn(value,1);line.Children().Append(value);
        root.Children().Append(line);root.Children().Append(bar.root);
        if(!rowId.empty()){AutomationProperties::SetAutomationId(root,rowId);AutomationProperties::SetAccessibilityView(root,Automation::Peers::AccessibilityView::Control);}
        if(!valueId.empty())AutomationProperties::SetAutomationId(value,valueId);
        if(!barId.empty()){AutomationProperties::SetAutomationId(bar.root,barId);AutomationProperties::SetAccessibilityView(bar.root,Automation::Peers::AccessibilityView::Control);}
    }
    void update(std::shared_ptr<WorkspaceData> const& data,J const& row){
        auto name=str(row,L"label"),reading=str(row,L"value");if(caption.Text()!=name)caption.Text(name);if(value.Text()!=reading)value.Text(reading);
        value.Foreground(data->brush(flag(row,L"warning")?L"error":L"text"));chip.update(data,object(row,L"chip"));bar.update(data,object(row,L"meter"));
        auto tip=object(row,L"tip");tooltip(root,tip);AutomationProperties::SetHelpText(value,str(row,L"description",plainTip(tip)));
    }
};
struct MeterSection {
    StackPanel root,body;
    Grid heading;
    TextBlock title{nullptr},note{nullptr};
    MeterChip chip;
    std::vector<std::unique_ptr<MeterRow>> rows;
    explicit MeterSection(std::shared_ptr<WorkspaceData> const& data):chip(data){
        root.Spacing(8);body.Spacing(8);AutomationProperties::SetAutomationId(root,L"renderer-memory");
        meterColumns(heading,{{1,GridUnitType::Auto},{1,GridUnitType::Star},{1,GridUnitType::Auto}});heading.ColumnSpacing(6);heading.Background(clear());
        title=label(data,L"");title.FontWeight(Windows::UI::Text::FontWeights::SemiBold());heading.Children().Append(title);
        note=label(data,L"");note.Opacity(.55);note.TextTrimming(TextTrimming::CharacterEllipsis);note.MaxLines(1);Grid::SetColumn(note,1);heading.Children().Append(note);
        Grid::SetColumn(chip.root,2);heading.Children().Append(chip.root);AutomationProperties::SetAutomationId(heading,L"renderer-memory-heading");
        AutomationProperties::SetAccessibilityView(heading,Automation::Peers::AccessibilityView::Control);root.Children().Append(heading);root.Children().Append(body);
    }
    void update(std::shared_ptr<WorkspaceData> const& data,J const& section){
        auto name=str(section,L"title"),detail=str(section,L"note");if(title.Text()!=name)title.Text(name);if(note.Text()!=detail)note.Text(detail);
        chip.update(data,object(section,L"chip"));tooltip(heading,object(section,L"tip"));auto items=array(section,L"rows");
        while(rows.size()>items.Size()){body.Children().RemoveAtEnd();rows.pop_back();}
        while(rows.size()<items.Size()){
            auto index=to_hstring(rows.size());auto row=std::make_unique<MeterRow>(data,L"renderer-memory-row-"+index,L"renderer-memory-value-"+index,L"renderer-memory-bar-"+index);
            body.Children().Append(row->root);rows.push_back(std::move(row));
        }
        for(uint32_t i=0;i<items.Size();++i)rows[i]->update(data,items.GetObjectAt(i));
    }
};
}
