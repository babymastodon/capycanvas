#pragma once
#include "pch.h"
#include <winrt/Microsoft.UI.Composition.h>
#include <winrt/Microsoft.UI.Xaml.Hosting.h>
#include <algorithm>
#include <array>
#include <chrono>
#include <cmath>
#include <memory>

namespace CapyUi {
class RowSwipePresenter {
    using Element=winrt::Microsoft::UI::Xaml::FrameworkElement;
    using View=winrt::Microsoft::UI::Xaml::UIElement;
    using Grid=winrt::Microsoft::UI::Xaml::Controls::Grid;
    using Animation=winrt::Microsoft::UI::Composition::ExpressionAnimation;
    struct State:std::enable_shared_from_this<State> {
        Grid frame,viewport,actions;
        Element body{nullptr};View grip{nullptr};
        winrt::Microsoft::UI::Xaml::Media::RectangleGeometry frameClip,viewportClip;
        winrt::Microsoft::UI::Composition::Vector3KeyFrameAnimation animation{nullptr};
        winrt::Microsoft::UI::Composition::CompositionScopedBatch batch{nullptr};
        std::array<Animation,3> expressions{nullptr,nullptr,nullptr};
        std::array<double,3> bound{};
        winrt::Microsoft::UI::Composition::Visual opacityVisual{nullptr};
        winrt::Microsoft::UI::Composition::CompositionAnimationGroup hideAnimation{nullptr};
        double minimum=0,maximum=0,unit=1,opacity=1,target=0;
        uint64_t generation=0;
        bool settling=false;
        double width()const{return std::min(maximum,std::max(1.,frame.ActualWidth()));}
        double offset()const{
            return body&&body.IsLoaded()&&frame.IsLoaded()
                ?-double(body.TransformToVisual(frame).TransformPoint({0,0}).X):target;
        }
        void actionsVisible(bool visible,bool animate){
            using winrt::Microsoft::UI::Xaml::Hosting::ElementCompositionPreview;
            if(!opacityVisual&&!visible&&animate&&viewport.Visibility()==winrt::Microsoft::UI::Xaml::Visibility::Visible)opacityVisual=ElementCompositionPreview::GetElementVisual(viewport);
            if(visible||!animate){
                ElementCompositionPreview::SetImplicitHideAnimation(viewport,nullptr);
                if(opacityVisual){if(hideAnimation)opacityVisual.StopAnimationGroup(hideAnimation);opacityVisual.Opacity(1);}
            }else if(opacityVisual){
                if(!hideAnimation){
                    auto compositor=opacityVisual.Compositor();auto fade=compositor.CreateScalarKeyFrameAnimation();fade.Target(L"Opacity");
                    fade.InsertExpressionKeyFrame(0,L"this.StartingValue");
                    fade.InsertKeyFrame(1,0,compositor.CreateCubicBezierEasingFunction({.333333f,1.f},{.666667f,1.f}));
                    fade.Duration(std::chrono::milliseconds(150));hideAnimation=compositor.CreateAnimationGroup();hideAnimation.Add(fade);
                }
                ElementCompositionPreview::SetImplicitHideAnimation(viewport,hideAnimation);
            }
            viewport.IsHitTestVisible(visible);
            viewport.Visibility(visible?winrt::Microsoft::UI::Xaml::Visibility::Visible:winrt::Microsoft::UI::Xaml::Visibility::Collapsed);
        }
        void stopExpressions(){
            if(expressions[0])viewport.StopAnimation(expressions[0]);
            if(expressions[1])actions.StopAnimation(expressions[1]);
            if(expressions[2]&&grip)grip.StopAnimation(expressions[2]);
            expressions={nullptr,nullptr,nullptr};
        }
        void bind(){
            if(!body||!frame.IsLoaded())return;
            std::array<double,3> inputs{width(),unit,opacity};
            if(expressions[0]&&bound==inputs)return;
            stopExpressions();bound=inputs;
            auto compositor=winrt::Microsoft::UI::Xaml::Media::CompositionTarget::GetCompositorForCurrentThread();
            auto expression=[&](wchar_t const* text,wchar_t const* property){
                auto value=compositor.CreateExpressionAnimation(text);value.Target(property);
                value.SetExpressionReferenceParameter(L"source",body);
                value.SetScalarParameter(L"width",float(width()));value.SetScalarParameter(L"unit",float(unit));
                value.SetScalarParameter(L"baseOpacity",float(opacity));return value;
            };
            expressions[0]=expression(L"Vector3(width - Max(0, -source.Translation.X), 0, 0)",L"Translation");
            expressions[1]=expression(L"Vector3(Max(0, -source.Translation.X) - width, 0, 0)",L"Translation");
            viewport.StartAnimation(expressions[0]);actions.StartAnimation(expressions[1]);
            if(grip){
                expressions[2]=expression(L"baseOpacity * (1 - Clamp(Abs(source.Translation.X) / (source.Translation.X < 0 ? Max(1, width) : unit), 0, 1))",L"Opacity");
                grip.StartAnimation(expressions[2]);
            }
        }
        void layout(){
            using winrt::Windows::Foundation::Rect;
            frameClip.Rect(Rect{0,0,float(frame.ActualWidth()),float(frame.ActualHeight())});
            viewport.Width(width());actions.Width(width());
            viewportClip.Rect(Rect{0,0,float(width()),float(frame.ActualHeight())});
            bind();
            auto actual=offset();
            if(actual<minimum||actual>width())set(std::clamp(actual,minimum,width()),false);
            else if(target<minimum||target>width())set(std::clamp(target,minimum,width()),settling);
        }
        void set(double value,bool animate){
            auto from=offset();value=std::clamp(value,minimum,width());
            animate=animate&&frame.IsLoaded()&&SystemUiSettings().AnimationsEnabled();
            if(target==value&&((animate&&settling)||(!settling&&from==value))){if(!animate&&value<=0)actionsVisible(false,false);return;}
            auto epoch=++generation;
            if(animation){body.StopAnimation(animation);animation=nullptr;}
            batch=nullptr;settling=false;target=value;
            if(!body)return;
            actionsVisible(value>0,animate&&from!=value);
            if(!animate||from==value){body.Translation({float(-value),0,0});return;}
            body.Translation({float(-from),0,0});settling=true;
            auto compositor=winrt::Microsoft::UI::Xaml::Media::CompositionTarget::GetCompositorForCurrentThread();
            animation=compositor.CreateVector3KeyFrameAnimation();animation.Target(L"Translation");
            animation.InsertExpressionKeyFrame(0,L"this.StartingValue");
            animation.InsertKeyFrame(1,{float(-value),0,0},compositor.CreateCubicBezierEasingFunction({.333333f,1.f},{.666667f,1.f}));
            animation.Duration(std::chrono::milliseconds(150));
            animation.StopBehavior(winrt::Microsoft::UI::Composition::AnimationStopBehavior::LeaveCurrentValue);
            batch=compositor.CreateScopedBatch(winrt::Microsoft::UI::Composition::CompositionBatchTypes::Animation);
            batch.Completed([weak=weak_from_this(),epoch,value](auto&&,auto&&){
                if(auto self=weak.lock();self&&self->generation==epoch){
                    self->settling=false;self->animation=nullptr;self->batch=nullptr;
                    self->body.Translation({float(-value),0,0});
                }
            });
            body.StartAnimation(animation);batch.End();
        }
        void attach(Element const& content,View const& handle,double baseOpacity){
            using namespace winrt::Microsoft::UI::Xaml;
            body=content;grip=handle;opacity=baseOpacity;
            viewport.HorizontalAlignment(HorizontalAlignment::Right);viewport.VerticalAlignment(VerticalAlignment::Stretch);
            actions.HorizontalAlignment(HorizontalAlignment::Left);actions.VerticalAlignment(VerticalAlignment::Stretch);
            viewport.Clip(viewportClip);frame.Clip(frameClip);viewport.Children().Append(actions);
            frame.Children().Append(viewport);frame.Children().Append(body);viewport.Visibility(Visibility::Collapsed);
            frame.SizeChanged([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock())self->layout();});
            frame.Loaded([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock())self->layout();});
            frame.Unloaded([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock()){self->set(0,false);self->actionsVisible(false,false);self->stopExpressions();if(self->grip)self->grip.Opacity(self->opacity);}});
        }
    };
    std::shared_ptr<State> state=std::make_shared<State>();
public:
    Grid Frame()const{return state->frame;}
    Grid Actions()const{return state->actions;}
    void Attach(Element const& body,View const& grip=nullptr,double opacity=1){state->attach(body,grip,opacity);}
    void Configure(double minimum,double maximum,double unit){
        if(state->minimum==minimum&&state->maximum==maximum&&state->unit==unit)return;
        state->minimum=minimum;state->maximum=maximum;state->unit=unit;state->layout();
    }
    void SetOffset(double value,bool animate=true){state->set(value,animate);}
    double Offset()const{return state->offset();}
    double RevealWidth()const{return state->width();}
    bool AtRest()const{return state->target==0&&Offset()==0;}
};
}
