#pragma once
#include <winrt/Microsoft.UI.Xaml.Media.Animation.h>
#include <winrt/Windows.UI.ViewManagement.h>

namespace CapyUi {
struct WorkflowPages : std::enable_shared_from_this<WorkflowPages> {
    Grid root;
    std::vector<FrameworkElement> pages;
    winrt::Microsoft::UI::Xaml::Media::Animation::Storyboard motion{nullptr};
    int selected=-1;
    uint64_t serial=0;
    void append(FrameworkElement const& page){page.Visibility(Visibility::Collapsed);pages.push_back(page);root.Children().Append(page);}
    void show(int index,bool slide=false,bool vertical=false,std::function<void()> settled={}){
        if(index==selected)return;
        auto previous=selected;selected=index;auto generation=++serial;auto from=root.ActualHeight();
        auto enteringOpacity=pages[index].Visibility()==Visibility::Visible?pages[index].Opacity():0.;
        auto leavingOpacity=previous>=0?pages[previous].Opacity():1.;
        auto enterTransform=pages[index].RenderTransform().try_as<TranslateTransform>();
        auto leaveTransform=previous>=0?pages[previous].RenderTransform().try_as<TranslateTransform>():TranslateTransform{nullptr};
        std::optional<double> enteringX=enterTransform?std::optional<double>((vertical?enterTransform.Y():enterTransform.X())):std::nullopt;
        auto leavingX=leaveTransform?(vertical?leaveTransform.Y():leaveTransform.X()):0.;
        if(motion){motion.Stop();motion=nullptr;}
        root.ClearValue(FrameworkElement::HeightProperty());
        for(int i=0;i<int(pages.size());++i){auto page=pages[i];page.Visibility(i==previous||i==index?Visibility::Visible:Visibility::Collapsed);page.Opacity(1);page.RenderTransform(nullptr);page.IsHitTestVisible(i==index);}
        auto settle=[self=shared_from_this(),generation,settled=std::move(settled)]{
            if(self->serial!=generation)return;
            for(int i=0;i<int(self->pages.size());++i){auto page=self->pages[i];page.Visibility(i==self->selected?Visibility::Visible:Visibility::Collapsed);page.Opacity(1);page.RenderTransform(nullptr);}
            self->root.ClearValue(FrameworkElement::HeightProperty());self->root.Clip(nullptr);self->motion=nullptr;
            if(settled)settled();
        };
        if(previous<0||!SystemUiSettings().AnimationsEnabled()){settle();return;}
        auto outgoing=pages[previous],incoming=pages[index];
        incoming.Visibility(Visibility::Visible);incoming.Measure({float(std::max(1.,root.ActualWidth())),INFINITY});
        auto to=incoming.DesiredSize().Height;
        namespace Motion=winrt::Microsoft::UI::Xaml::Media::Animation;
        motion=Motion::Storyboard();
        auto animate=[&](winrt::Windows::Foundation::IInspectable const& target,hstring const& property,double start,double end){
            Motion::DoubleAnimation animation;animation.From(start);animation.To(end);animation.Duration(DurationHelper::FromTimeSpan(std::chrono::milliseconds(200)));animation.EnableDependentAnimation(true);
            Motion::CubicEase ease;ease.EasingMode(Motion::EasingMode::EaseOut);animation.EasingFunction(ease);
            Motion::Storyboard::SetTarget(animation,target.as<DependencyObject>());Motion::Storyboard::SetTargetProperty(animation,property);motion.Children().Append(animation);
        };
        RectangleGeometry clip;clip.Rect({0,0,float(root.ActualWidth()),float(std::max(from,double(to)))});root.Clip(clip);
        root.Height(from);animate(root,L"Height",from,to);
        if(slide){
            auto distance=(vertical?-1:1)*(vertical?std::max(1.,double(to)):std::max(1.,root.ActualWidth()))*(index>previous?1:-1);
            TranslateTransform entering,leaving;incoming.RenderTransform(entering);outgoing.RenderTransform(leaving);
            auto axis=vertical?L"Y":L"X";animate(entering,axis,enteringX.value_or(distance),0);animate(leaving,axis,leavingX,-distance);
        }else{incoming.Opacity(enteringOpacity);animate(incoming,L"Opacity",enteringOpacity,1);animate(outgoing,L"Opacity",leavingOpacity,0);}
        motion.Completed([settle](auto&&,auto&&){settle();});motion.Begin();
    }
};
}
