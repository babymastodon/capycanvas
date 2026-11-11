#pragma once
#include "UiControls.h"

class NavigatorView {
public:
    NavigatorView(std::shared_ptr<CapyUi::WorkspaceData> data, std::function<void()> layoutChanged);
    ~NavigatorView();
    winrt::Microsoft::UI::Xaml::FrameworkElement Root() const;
    void Apply(bool visible);
    double NaturalHeight(double width) const;
    double FixedHeight() const;
    void MaximumWidth(double width);
private:
    struct Impl;
    std::shared_ptr<Impl> impl;
};
