#pragma once
#include "PanelScroll.h"

namespace CapyUi {
inline Windows::Foundation::Collections::IVector<Windows::Foundation::IInspectable> rowItems(ItemsView const& list){
    return list.ItemsSource().as<Windows::Foundation::Collections::IVector<Windows::Foundation::IInspectable>>();
}
inline ItemsView selectableRows(std::shared_ptr<WorkspaceData> const& data){
    ItemsView list;
    list.ItemsSource(single_threaded_observable_vector<Windows::Foundation::IInspectable>());
    list.SelectionMode(ItemsViewSelectionMode::Single);list.IsItemInvokedEnabled(true);list.ItemTransitionProvider(nullptr);
    list.TabNavigation(KeyboardNavigationMode::Local);
    StackLayout layout;layout.Spacing(0);list.Layout(layout);
    auto observed=std::make_shared<weak_ref<ScrollView>>();
    auto attach=[weak=make_weak(list),context=std::weak_ptr(data),observed]{
        auto host=weak.get();auto data=context.lock();if(!host||!data)return;
        auto scroll=host.ScrollView();if(!scroll||scroll==observed->get())return;
        *observed=make_weak(scroll);scroll.HorizontalScrollMode(ScrollingScrollMode::Disabled);
        scroll.HorizontalScrollBarVisibility(ScrollingScrollBarVisibility::Hidden);scroll.VerticalScrollBarVisibility(ScrollingScrollBarVisibility::Auto);
        scroll.ContentOrientation(ScrollingContentOrientation::Vertical);mouseScrollbars(scroll);
        auto content=scroll.Content().as<UIElement>();content.TabFocusNavigation(KeyboardNavigationMode::Local);panelScrolling(scroll,content,data);
    };
    list.RegisterPropertyChangedCallback(ItemsView::ScrollViewProperty(),[attach](auto&&,auto&&){attach();});
    list.Loaded([attach](auto&&,auto&&){attach();});return list;
}
inline ItemContainer selectableRow(){
    ItemContainer row;row.CornerRadius({0});
    row.RegisterPropertyChangedCallback(ItemContainer::ChildProperty(),[](DependencyObject const& owner,auto&&){
        if(auto child=owner.as<ItemContainer>().Child())Canvas::SetZIndex(child,1);
    });
    row.GettingFocus([weak=make_weak(row)](auto&&,GettingFocusEventArgs const& e){
        auto owner=weak.get();auto target=e.NewFocusedElement().try_as<DependencyObject>();
        if(owner&&target!=owner&&descendantOf(target,owner)&&e.FocusState()==FocusState::Keyboard)e.Handled(true);
    });
    row.Resources().Insert(box_value(L"ItemContainerSelectedInnerThickness"),box_value(0.));row.Resources().Insert(box_value(L"ItemContainerSelectedInnerMargin"),box_value(Thickness{}));
    for(auto key:{L"ItemContainerSelectionVisualBackground",L"ItemContainerSelectionVisualPointerOverBackground",L"ItemContainerSelectionVisualPressedBackground",L"ItemContainerSelectedInnerBorderBrush"})row.Resources().Insert(box_value(hstring(key)),clear());
    return row;
}
inline void rowColors(ItemContainer const& row,std::shared_ptr<WorkspaceData> const& data){
    for(auto state:{L"Selected",L"SelectedPointerOver",L"SelectedPressed"})row.Resources().Insert(box_value(hstring(L"ItemContainer")+state+L"Background"),selected(data));
    row.Resources().Insert(box_value(L"ItemContainerPointerOverBackground"),data->tint(L"text",8));row.Resources().Insert(box_value(L"ItemContainerPressedBackground"),data->tint(L"text",20));
}
inline void rowTap(ItemContainer const& row,std::function<void()> invoke){
    row.AddHandler(UIElement::TappedEvent(),box_value(TappedEventHandler([weak=make_weak(row),invoke=std::move(invoke)](auto&&,TappedRoutedEventArgs const& e){
        for(auto node=e.OriginalSource().try_as<DependencyObject>();node&&node!=weak.get();node=VisualTreeHelper::GetParent(node))
            if(node.try_as<Primitives::ButtonBase>()||node.try_as<TextBox>())return;
        invoke();
    })),true);
}
inline void selectRow(ItemsView const& list,Windows::Foundation::IInspectable const& item){
    uint32_t index=0;if(item&&rowItems(list).IndexOf(item,index))list.Select(int32_t(index));else list.DeselectAll();
}
}
