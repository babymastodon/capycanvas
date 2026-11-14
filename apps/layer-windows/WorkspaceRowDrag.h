#pragma once
#include "pch.h"
#include "RowSwipePresenter.h"
#include "SelectableRows.h"
#include <functional>
#include <memory>
#include <optional>
#include <vector>

class WorkspaceRowDrag {
public:
    using Id=winrt::hstring;
    using Move=std::function<void(Id,std::optional<Id>,std::function<void()>)>;
    struct Swipe {
        std::shared_ptr<CapyUi::RowSwipePresenter> presenter;
        winrt::Microsoft::UI::Xaml::Controls::Button remove;
        double actionWidth,threshold;
    };
    WorkspaceRowDrag(winrt::Microsoft::UI::Xaml::Controls::ItemsView list,
        winrt::Microsoft::UI::Xaml::Controls::Grid surface,std::shared_ptr<CapyUi::WorkspaceData> const& data,
        std::function<bool()> available,Move move,std::function<void()> ended);
    ~WorkspaceRowDrag();
    void Attach(winrt::Microsoft::UI::Xaml::Controls::ItemContainer row,
        winrt::Microsoft::UI::Xaml::Controls::Button grip,
        winrt::Microsoft::UI::Xaml::Controls::Button more,
        winrt::Microsoft::UI::Xaml::UIElement pin,
        winrt::Microsoft::UI::Xaml::Controls::MenuFlyout menu,Id id,
        winrt::Microsoft::UI::Xaml::Controls::MenuFlyout context = nullptr,std::optional<Swipe> swipe = {});
    void Refresh(std::vector<Id> const& order);
    bool FenceSelection()const;
    bool SuppressClick()const;
    bool Pending()const;
    bool Cancel();
    bool Escape();
private:
    struct Impl;
    std::shared_ptr<Impl> impl;
};
