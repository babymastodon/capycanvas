#include "PocWindow.h"

namespace Poc {
PocWindow::PocWindow(JsonObject const& value,std::function<void(int)> done):options(value),closed(std::move(done)){}
void PocWindow::Open(){
    model=poc_model_create(uint32_t(options.GetNamedNumber(L"rows")));
    if(!model)throw hresult_invalid_argument();
    dispatcher=DispatcherQueue::GetForCurrentThread();window=Window();window.Title(spec.value.GetNamedString(L"title"));
    check_hresult(window.as<IWindowNative>()->get_WindowHandle(&handle));
    scale=float(GetDpiForWindow(handle))/96;inputScale=scale;Build();window.Content(root);window.ExtendsContentIntoTitleBar(true);window.SetTitleBar(title);auto caption=window.AppWindow().TitleBar();caption.ButtonForegroundColor(spec.Probe(L"zen").Color());caption.ButtonInactiveForegroundColor(spec.Probe(L"zen").Color());caption.ButtonBackgroundColor(Windows::UI::Color{});caption.ButtonInactiveBackgroundColor(Windows::UI::Color{});
    SetWindowSubclass(handle,WindowProc,1,reinterpret_cast<DWORD_PTR>(this));
    window.AppWindow().Closing([weak=weak_from_this()](auto&&,Microsoft::UI::Windowing::AppWindowClosingEventArgs const& args){if(auto self=weak.lock();self&&!self->finished){args.Cancel(true);self->RequestClose();}});
    root.Loaded([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock();self&&!self->loaded){
        self->loaded=true;self->scale=float(self->root.XamlRoot().RasterizationScale());self->inputScale=self->scale;
        if(!self->FitClient())return;self->root.UpdateLayout();
        auto path=to_string(self->options.GetNamedString(L"trace"));
        self->canvas=poc_canvas_create_for_panel(uint32_t(std::lround(self->root.ActualWidth()*self->scale)),uint32_t(std::lround(self->root.ActualHeight()*self->scale)),self->scale,path.c_str(),&self->surfaceHandle,[](void* context,bool ready){auto self=static_cast<PocWindow*>(context);self->dispatcher.TryEnqueue([weak=self->weak_from_this(),ready]{if(auto self=weak.lock();self&&!self->closing)self->CanvasReady(ready);});},[](void* context,bool ready){auto self=static_cast<PocWindow*>(context);self->dispatcher.TryEnqueue([weak=self->weak_from_this(),ready]{if(auto self=weak.lock();self&&!self->closing)self->CanvasPresented(ready);});},self.get());
        if(!self->canvas||!self->surfaceHandle){self->Fail(E_FAIL);return;}
        if(!poc_canvas_set_load(self->canvas,self->options.GetNamedString(L"load")==L"slow")||!poc_canvas_set_thumbnails(self->canvas,self->options.GetNamedBoolean(L"thumbnails"))){self->Fail(E_FAIL);return;}
        poc_canvas_set_motion(self->canvas,self->options.GetNamedString(L"load")==L"slow");
        self->Resize();self->PublishGlass();
    }});
    root.SizeChanged([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock())self->Resize();});
    panel.CompositionScaleChanged([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock())self->Resize();});
    root.LayoutUpdated([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock();self&&!self->closing){self->PublishGlass();self->ObserveList();self->PublishTitleInput();}});
    auto client=spec.value.GetNamedArray(L"client");window.AppWindow().ResizeClient({int32_t(std::lround(client.GetNumberAt(0)*GetDpiForWindow(handle)/96.0)),int32_t(std::lround(client.GetNumberAt(1)*GetDpiForWindow(handle)/96.0))});
    window.Activate();
    cpuTimer=dispatcher.CreateTimer();cpuTimer.Interval(std::chrono::seconds(1));cpuTimer.Tick([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock();self&&!self->closing)self->Trace("thread_cpu",Fields({{L"thread",JsonValue::CreateStringValue(L"ui")},{L"cpu_ms",JsonValue::CreateNumberValue(CpuMs())}}));});cpuTimer.Start();
    auto stall=options.GetNamedNumber(L"ui_stall_ms");
    if(stall>0){stallTimer=dispatcher.CreateTimer();stallTimer.Interval(std::chrono::milliseconds(int64_t(options.GetNamedNumber(L"ui_stall_every_ms"))));stallTimer.Tick([weak=weak_from_this(),stall](auto&&,auto&&){if(auto self=weak.lock();self&&!self->closing){auto end=std::chrono::steady_clock::now()+std::chrono::duration<double,std::milli>(stall);while(std::chrono::steady_clock::now()<end)YieldProcessor();self->Trace("ui_stall",Fields({{L"duration_ms",JsonValue::CreateNumberValue(stall)}}));}});stallTimer.Start();}
}
void PocWindow::CanvasReady(bool ready){
    if(!ready){Fail(E_FAIL);return;}
    try{check_hresult(panel.as<ISwapChainPanelNative2>()->SetSwapChainHandle(surfaceHandle));Trace("startup",Fields({{L"stage",JsonValue::CreateStringValue(L"surface_attached")}}));StartInput();}catch(hresult_error const& error){Fail(error.code());}
}
void PocWindow::CanvasPresented(bool ready){
    if(!ready){Fail(E_FAIL);return;}
    try{StartThumbnails();ObserveList();Trace("startup",Fields({{L"stage",JsonValue::CreateStringValue(L"ui_ready")},{L"scale",JsonValue::CreateNumberValue(scale)}}));Trace("startup",Fields({{L"stage",JsonValue::CreateStringValue(L"independent_input_ready")}}));auto event=options.GetNamedValue(L"ready_event",JsonValue::CreateNullValue());if(event.ValueType()==JsonValueType::String){auto name=to_string(event.GetString());if(!poc_signal_ready(name.c_str()))Fail(E_FAIL);}}catch(hresult_error const& error){Fail(error.code());}
}
bool PocWindow::FitClient(){
    auto bridge=FindWindowExW(handle,nullptr,L"Microsoft.UI.Content.DesktopChildSiteBridge",nullptr);RECT client{},content{};POINT origin{};
    if(!bridge||FindWindowExW(handle,bridge,L"Microsoft.UI.Content.DesktopChildSiteBridge",nullptr)||!GetClientRect(handle,&client)||!GetWindowRect(bridge,&content)||!ClientToScreen(handle,&origin)){Fail(E_UNEXPECTED);return false;}
    Thickness desired{double(origin.x-content.left)/scale,double(origin.y-content.top)/scale,double(content.right-origin.x-client.right)/scale,double(content.bottom-origin.y-client.bottom)/scale};auto current=root.Margin();
    if(current.Left!=desired.Left||current.Top!=desired.Top||current.Right!=desired.Right||current.Bottom!=desired.Bottom)root.Margin(desired);
    return true;
}
void PocWindow::Build(){
    root=Grid();root.RequestedTheme(ElementTheme::Dark);root.UseLayoutRounding(true);panel=SwapChainPanel();root.Children().Append(Accessible(panel,spec.Automation(L"drawing_canvas"),spec.String(L"canvas")));
    chrome=Grid();chrome.IsHitTestVisible(true);root.Children().Append(chrome);
    BuildTitle();
    column=Grid();column.Width(spec.Metric(L"column_left")-spec.Metric(L"column_right"));column.HorizontalAlignment(HorizontalAlignment::Right);column.Margin({0,spec.Metric(L"column_top"),spec.Metric(L"column_right"),spec.Metric(L"column_bottom")});
    upper=Row(1,GridUnitType::Star);column.RowDefinitions().Append(upper);column.RowDefinitions().Append(Row(spec.Metric(L"divider")));column.RowDefinitions().Append(Row(1,GridUnitType::Star));chrome.Children().Append(column);
    BuildLayers();BuildProperties();
    divider=Border();divider.Background(Brush(0,0,0,0));auto dividerPeer=Accessible(divider,spec.Automation(L"divider"),spec.String(L"divider"));Grid::SetRow(dividerPeer,1);column.Children().Append(dividerPeer);
    divider.PointerPressed([weak=weak_from_this()](auto&&,Input::PointerRoutedEventArgs const& args){if(auto self=weak.lock();self&&!self->closing){
        auto receipt=Now();auto point=args.GetCurrentPoint(self->column);self->dividerContact=self->divider.CapturePointer(args.Pointer());self->dividerPointer=point.PointerId();self->dividerOffset=point.Position().Y-self->upper.Height().Value;args.Handled(true);self->Trace("native_input",Fields({{L"input",JsonValue::CreateStringValue(L"divider_down")},{L"receipt_qpc",JsonValue::CreateNumberValue(double(receipt))}}));
    }});
    divider.PointerMoved([weak=weak_from_this()](auto&&,Input::PointerRoutedEventArgs const& args){auto receipt=Now();if(auto self=weak.lock();self&&self->dividerContact&&!self->closing){auto point=args.GetCurrentPoint(self->column);if(point.PointerId()!=self->dividerPointer)return;
        auto changed=poc_model_set_divider(self->model,float(point.Position().Y-self->dividerOffset),float(self->column.ActualHeight()));self->Sync();if(changed)self->Changed(uint16_t(std::lround((self->spec.Metric(L"column_top")+self->column.ActualHeight()*poc_model_state(self->model).divider_fraction)*self->scale)));args.Handled(true);self->Trace("ui_event",Fields({{L"event",JsonValue::CreateStringValue(L"divider")},{L"receipt_qpc",JsonValue::CreateNumberValue(double(receipt))}}));
    }});
    auto release=[weak=weak_from_this()](auto&&,Input::PointerRoutedEventArgs const& args){if(auto self=weak.lock();self&&self->dividerContact){self->dividerContact=false;self->divider.ReleasePointerCapture(args.Pointer());args.Handled(true);}};
    divider.PointerReleased(release);divider.PointerCanceled(release);divider.PointerCaptureLost([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock())self->dividerContact=false;});
    BuildDrawer();BuildMarker();Sync();RebuildRows();
}
void PocWindow::BuildTitle(){
    title=Grid();title.Height(spec.Metric(L"title_height"));title.VerticalAlignment(VerticalAlignment::Top);title.Margin({0,0,spec.Metric(L"title_height")*3.5,0});auto titlePeer=Accessible(title,spec.Automation(L"title_bar"),spec.String(L"title_bar"));titlePeer.Height(spec.Metric(L"title_height"));titlePeer.VerticalAlignment(VerticalAlignment::Top);chrome.Children().Append(titlePeer);
    StackPanel controls;controls.Orientation(Orientation::Horizontal);controls.Spacing(spec.Metric(L"divider"));controls.Margin({spec.Metric(L"divider"),0,0,0});title.Children().Append(controls);
    auto label=Text(spec.value.GetNamedString(L"title"));label.Width(spec.Metric(L"title_height")*6);controls.Children().Append(label);
    for(uint32_t i=0;i<2;++i){auto button=ToggleButton();button.Content(box_value(spec.String(i?L"tab_b":L"tab_a")));Id(button,spec.Automation(i?L"tab_b":L"tab_a"));button.Click([weak=weak_from_this(),i](auto&&,auto&&){if(auto self=weak.lock())self->Activate(0,i);});tabs[i]=button;controls.Children().Append(button);titleInputs.push_back(button);}
    for(auto entry:std::array<std::pair<wchar_t const*,uint32_t>,2>{{{L"drawer",2},{L"zen",3}}}){Button button;button.Content(box_value(spec.String(entry.first)));Id(button,spec.Automation(entry.second==2?L"drawer_toggle":L"zen_toggle"));button.Click([weak=weak_from_this(),action=entry.second](auto&&,auto&&){if(auto self=weak.lock())self->Activate(action);});controls.Children().Append(button);titleInputs.push_back(button);}
    widthEdge=Border();widthEdge.Background(spec.Probe(L"width"));widthEdge.Width(2);widthEdge.HorizontalAlignment(HorizontalAlignment::Right);widthEdge.IsHitTestVisible(false);title.Children().Append(widthEdge);
    zenPatch=Border();zenPatch.Background(spec.Probe(L"zen"));zenPatch.HorizontalAlignment(HorizontalAlignment::Left);zenPatch.VerticalAlignment(VerticalAlignment::Top);zenPatch.IsHitTestVisible(false);chrome.Children().Append(zenPatch);
}
void PocWindow::BuildLayers(){
    layers=Border();layers.CornerRadius(CornerRadius(spec.Metric(L"radius")));layers.Background(spec.Tint());column.Children().Append(Accessible(layers,spec.Automation(L"layers"),spec.String(L"layers")));
    layersContent=Grid();layersContent.RowDefinitions().Append(Row(spec.Metric(L"header_height")));layersContent.RowDefinitions().Append(Row(1,GridUnitType::Star));layers.Child(layersContent);
    StackPanel header;header.Orientation(Orientation::Horizontal);header.Spacing(spec.Metric(L"divider"));header.Margin({spec.Metric(L"divider"),0,0,0});
    for(uint32_t i=0;i<2;++i){ToggleButton button;button.Content(box_value(spec.String(i?L"brushes":L"layers")));Id(button,spec.Automation(i?L"brushes_tab":L"layers_tab"));button.Click([weak=weak_from_this(),i](auto&&,auto&&){if(auto self=weak.lock())self->Activate(1,i);});groupTabs[i]=button;header.Children().Append(button);}layersContent.Children().Append(header);
    list=ListView();Id(list,spec.Automation(L"layers_list"));list.SelectionMode(ListViewSelectionMode::None);list.ItemsSource(items);list.Background(Brush(0,0,0,0));list.Padding(Thickness{0});list.BorderThickness(Thickness{0});list.HorizontalContentAlignment(HorizontalAlignment::Stretch);list.VerticalContentAlignment(VerticalAlignment::Stretch);Grid::SetRow(list,1);layersContent.Children().Append(list);
    ScrollViewer::SetVerticalScrollBarVisibility(list,ScrollBarVisibility::Auto);ScrollViewer::SetHorizontalScrollBarVisibility(list,ScrollBarVisibility::Disabled);
    list.ItemsPanel(Markup::XamlReader::Load(L"<ItemsPanelTemplate xmlns='http://schemas.microsoft.com/winfx/2006/xaml/presentation'><ItemsStackPanel Orientation='Vertical'/></ItemsPanelTemplate>").as<ItemsPanelTemplate>());
    auto style=L"<Style xmlns='http://schemas.microsoft.com/winfx/2006/xaml/presentation' TargetType='ListViewItem'><Setter Property='HorizontalContentAlignment' Value='Stretch'/><Setter Property='Padding' Value='0'/><Setter Property='Margin' Value='0'/><Setter Property='MinHeight' Value='0'/><Setter Property='AutomationProperties.AutomationId' Value='{Binding [id]}'/><Setter Property='AutomationProperties.Name' Value='{Binding [label]}'/></Style>";
    list.ItemContainerStyle(Markup::XamlReader::Load(style).as<Style>());
    list.ItemTemplate(Markup::XamlReader::Load(L"<DataTemplate xmlns='http://schemas.microsoft.com/winfx/2006/xaml/presentation'><Grid/></DataTemplate>").as<DataTemplate>());
    list.ContainerContentChanging([weak=weak_from_this()](auto&&,ContainerContentChangingEventArgs const& args){if(auto self=weak.lock();self&&!self->closing)self->Realize(args);});
    magenta=Border();magenta.Background(spec.Probe(L"divider"));magenta.HorizontalAlignment(HorizontalAlignment::Left);magenta.VerticalAlignment(VerticalAlignment::Bottom);magenta.IsHitTestVisible(false);layersContent.Children().Append(magenta);Grid::SetRowSpan(magenta,2);
    orange=Border();orange.Background(Brush(0,0,0,0));orange.BorderBrush(spec.Probe(L"fill"));orange.HorizontalAlignment(HorizontalAlignment::Right);orange.VerticalAlignment(VerticalAlignment::Stretch);orange.IsHitTestVisible(false);layersContent.Children().Append(orange);Grid::SetRowSpan(orange,2);
}
void PocWindow::BuildProperties(){
    properties=Border();properties.CornerRadius(CornerRadius(spec.Metric(L"radius")));properties.Background(spec.Tint());auto propertiesPeer=Accessible(properties,spec.Automation(L"properties"),spec.String(L"properties"));Grid::SetRow(propertiesPeer,2);column.Children().Append(propertiesPeer);
    ScrollViewer scroll;scroll.VerticalScrollBarVisibility(ScrollBarVisibility::Auto);scroll.HorizontalScrollBarVisibility(ScrollBarVisibility::Disabled);properties.Child(scroll);
    StackPanel body;body.Spacing(spec.Metric(L"divider"));body.Margin(Thickness{spec.Metric(L"divider")});scroll.Content(body);
    DropDownButton menu;menu.Content(box_value(spec.String(L"menu")));Id(menu,spec.Automation(L"menu_button"));menu.HorizontalAlignment(HorizontalAlignment::Stretch);MenuFlyout flyout;flyout.ShouldConstrainToRootBounds(false);
    auto menuItems=spec.value.GetNamedArray(L"menu_items");
    for(uint32_t i=0;i<menuItems.Size();++i){MenuFlyoutItem item;item.Text(menuItems.GetStringAt(i));Id(item,spec.value.GetNamedArray(L"menu_ids").GetStringAt(i));item.Click([weak=weak_from_this(),i](auto&&,auto&&){if(auto self=weak.lock())self->Activate(5,i);});flyout.Items().Append(item);}menu.Flyout(flyout);body.Children().Append(menu);
    flyout.Opened([weak=weak_from_this()](auto&& sender,auto&&){if(auto self=weak.lock()){auto flyout=sender.template as<MenuFlyout>();self->Changed();JsonArray rectangles;for(auto popup:VisualTreeHelper::GetOpenPopupsForXamlRoot(self->root.XamlRoot()))if(popup.IsOpen())if(auto child=popup.Child().try_as<FrameworkElement>()){auto at=child.TransformToVisual(self->root).TransformPoint({0,0});JsonArray rect;for(auto value:{at.X*self->scale,at.Y*self->scale,float(child.ActualWidth())*self->scale,float(child.ActualHeight())*self->scale})rect.Append(JsonValue::CreateNumberValue(value));rectangles.Append(rect);}self->Trace("popup_open",Fields({{L"constrained_to_root",JsonValue::CreateBooleanValue(flyout.IsConstrainedToRootBounds())},{L"rects",rectangles}}));}});flyout.Closed([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock())self->Changed();});
    auto sliderSpec=spec.value.GetNamedArray(L"sliders");
    std::array<wchar_t const*,4> names{L"size",L"opacity",L"flow",L"spacing"};
    for(uint32_t i=0;i<4;++i){
        Grid row;row.ColumnDefinitions().Append(Col(1,GridUnitType::Star));row.ColumnDefinitions().Append(Col(spec.Metric(L"header_height")));auto label=Text(spec.String(names[i]));row.Children().Append(label);values[i]=Text(L"");Grid::SetColumn(values[i],1);row.Children().Append(values[i]);body.Children().Append(row);
        Slider slider;auto limits=sliderSpec.GetObjectAt(i);slider.Minimum(limits.GetNamedNumber(L"min"));slider.Maximum(limits.GetNamedNumber(L"max"));slider.StepFrequency(limits.GetNamedNumber(L"step"));Id(slider,limits.GetNamedString(L"id"));Name(slider,spec.String(names[i]));
        if(!i)for(auto key:{L"SliderThumbBackground",L"SliderThumbBackgroundPointerOver",L"SliderThumbBackgroundPressed",L"SliderThumbBackgroundDisabled"})slider.Resources().Insert(box_value(key),spec.Probe(L"slider"));
        slider.ValueChanged([weak=weak_from_this(),i](auto&&,RangeBaseValueChangedEventArgs const& args){auto receipt=Now();if(auto self=weak.lock();self&&!self->synchronizing&&!self->closing){auto before=poc_model_state(self->model);poc_model_set_slider(self->model,i,args.NewValue());auto after=poc_model_state(self->model);self->Sync();if(after.revision!=before.revision){self->Changed(uint16_t(std::lround(after.sliders[i]*10)));self->Trace("ui_event",Fields({{L"event",JsonValue::CreateStringValue(L"slider")},{L"index",JsonValue::CreateNumberValue(i)},{L"receipt_qpc",JsonValue::CreateNumberValue(double(receipt))}}));}}});sliders[i]=slider;body.Children().Append(slider);
    }
    number=NumberBox();Id(number,spec.Automation(L"number_field"));number.Header(box_value(spec.String(L"number")));auto numberSpec=spec.value.GetNamedObject(L"number");number.Minimum(numberSpec.GetNamedNumber(L"min"));number.Maximum(numberSpec.GetNamedNumber(L"max"));body.Children().Append(number);
    number.ValueChanged([weak=weak_from_this()](auto&&,NumberBoxValueChangedEventArgs const& args){if(auto self=weak.lock();self&&!self->synchronizing&&!self->closing){auto before=poc_model_state(self->model);poc_model_set_number(self->model,args.NewValue());self->Sync();if(poc_model_state(self->model).revision!=before.revision)self->Changed();}});
    filter=TextBox();Id(filter,spec.Automation(L"text_field"));filter.Header(box_value(spec.String(L"text")));filter.AcceptsReturn(false);body.Children().Append(filter);
    filter.TextChanged([weak=weak_from_this()](auto&&,auto&&){auto receipt=Now();if(auto self=weak.lock();self&&!self->synchronizing&&!self->closing){auto text=to_string(self->filter.Text());auto before=poc_model_state(self->model);poc_model_set_text(self->model,text.c_str(),text.size());auto after=poc_model_state(self->model);if(after.revision!=before.revision){self->RebuildRows();self->PublishCaret();self->Changed(uint16_t(self->filter.Text().size()));self->Trace("ui_event",Fields({{L"event",JsonValue::CreateStringValue(L"text")},{L"receipt_qpc",JsonValue::CreateNumberValue(double(receipt))},{L"text",JsonValue::CreateStringValue(self->filter.Text())}}));}}});
    filter.SelectionChanged([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock())self->PublishCaret();});filter.GotFocus([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock())self->PublishCaret();});filter.SizeChanged([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock())self->PublishCaret();});
    auto paragraph=Text(spec.value.GetNamedString(L"paragraph"));paragraph.TextWrapping(TextWrapping::Wrap);paragraph.Width(spec.Metric(L"wrap_width"));paragraph.HorizontalAlignment(HorizontalAlignment::Left);Id(paragraph,spec.Automation(L"wrap_text"));body.Children().Append(paragraph);
}
void PocWindow::BuildDrawer(){
    drawerHost=Grid();drawerHost.Width(spec.Metric(L"drawer_width")+spec.value.GetNamedObject(L"probe_metrics").GetNamedNumber(L"bar")/scale);drawerHost.HorizontalAlignment(HorizontalAlignment::Left);drawerHost.Margin({0,spec.Metric(L"column_top"),0,spec.Metric(L"column_bottom")});drawerTransform=TranslateTransform();drawerTransform.X(-spec.Metric(L"drawer_width"));drawerHost.RenderTransform(drawerTransform);chrome.Children().Append(drawerHost);
    drawer=Border();drawer.Background(spec.Tint());drawer.CornerRadius(CornerRadius(spec.Metric(L"radius")));drawer.Width(spec.Metric(L"drawer_width"));drawer.HorizontalAlignment(HorizontalAlignment::Left);auto drawerPeer=Accessible(drawer,spec.Automation(L"drawer"),spec.String(L"drawer"));drawerPeer.Width(spec.Metric(L"drawer_width"));drawerPeer.HorizontalAlignment(HorizontalAlignment::Left);drawerHost.Children().Append(drawerPeer);
    auto text=Text(spec.String(L"drawer"));text.Margin(Thickness{spec.Metric(L"divider")});text.VerticalAlignment(VerticalAlignment::Top);drawer.Child(text);drawerEdge=Border();drawerEdge.Background(spec.Probe(L"drawer"));drawerEdge.HorizontalAlignment(HorizontalAlignment::Left);drawerEdge.IsHitTestVisible(false);drawerHost.Children().Append(drawerEdge);
}
void PocWindow::BuildMarker(){
    marker=Grid();marker.HorizontalAlignment(HorizontalAlignment::Left);marker.VerticalAlignment(VerticalAlignment::Top);marker.IsHitTestVisible(false);auto guard=spec.value.GetNamedObject(L"marker").GetNamedArray(L"guard");marker.Background(Brush(uint8_t(guard.GetNumberAt(0)),uint8_t(guard.GetNumberAt(1)),uint8_t(guard.GetNumberAt(2))));root.Children().Append(marker);
    for(auto& bit:bits){bit=Shapes::Rectangle();bit.HorizontalAlignment(HorizontalAlignment::Left);bit.VerticalAlignment(VerticalAlignment::Top);bit.Fill(Brush(0,0,0));marker.Children().Append(bit);}
    Changed();
}
void PocWindow::Sync(){
    if(!model)return;auto state=poc_model_state(model);synchronizing=true;
    for(uint32_t i=0;i<4;++i){sliders[i].Value(state.sliders[i]);auto length=poc_model_slider_text(model,i,nullptr,0);std::string text(length,'\0');poc_model_slider_text(model,i,text.data(),text.size());values[i].Text(to_hstring(text.c_str()));}
    number.Value(state.number);for(uint32_t i=0;i<2;++i){tabs[i].IsChecked(i==state.tab);groupTabs[i].IsChecked(i==state.group_tab);}
    if(column.ActualHeight()>0)upper.Height(GridLength{column.ActualHeight()*state.divider_fraction,GridUnitType::Pixel});
    synchronizing=false;
}
void PocWindow::RebuildRows(){
    auto started=Now();auto state=poc_model_state(model);std::vector<IInspectable> replacement;replacement.reserve(state.visible_rows);tiles.clear();
    for(uint32_t ordinal=0;ordinal<state.visible_rows;++ordinal){auto row=poc_model_visible_row(model,ordinal);auto data=single_threaded_observable_map<hstring,IInspectable>();data.Insert(L"index",box_value(row));data.Insert(L"id",box_value(spec.RowId(row)));data.Insert(L"label",box_value(spec.Label(row)));replacement.push_back(data);}
    items.ReplaceAll(replacement);Trace("ui_layout",Fields({{L"start_qpc",JsonValue::CreateNumberValue(double(started))},{L"rows",JsonValue::CreateNumberValue(state.rows)},{L"visible_rows",JsonValue::CreateNumberValue(state.visible_rows)}}));
}
void PocWindow::Realize(ContainerContentChangingEventArgs const& args){
    auto container=args.ItemContainer().as<ListViewItem>();
    if(args.InRecycleQueue()){for(auto it=tiles.begin();it!=tiles.end();)if(it->second->container==container)it=tiles.erase(it);else++it;return;}
    auto started=Now();auto data=args.Item().as<IObservableMap<hstring,IInspectable>>();auto row=unbox_value<uint32_t>(data.Lookup(L"index"));
    for(auto it=tiles.begin();it!=tiles.end();)if(it->second->container==container)it=tiles.erase(it);else++it;
    auto tile=std::make_shared<Tile>();tile->row=row;tile->container=container;tile->content=container.ContentTemplateRoot().try_as<Grid>();if(!tile->content){args.RegisterUpdateCallback([weak=weak_from_this()](auto&&,ContainerContentChangingEventArgs const& next){if(auto self=weak.lock();self&&!self->closing)self->Realize(next);});return;}tile->content.Children().Clear();tile->content.ColumnDefinitions().Clear();tile->content.Height(spec.Metric(L"row_height"));tile->content.ColumnDefinitions().Append(Col(spec.Metric(L"thumbnail_size")+spec.Metric(L"divider")*2));tile->content.ColumnDefinitions().Append(Col(1,GridUnitType::Star));tile->content.ColumnDefinitions().Append(Col(spec.Metric(L"header_height")));tile->content.ColumnDefinitions().Append(Col(spec.Metric(L"header_height")));
    Id(container,spec.RowId(row));Name(container,spec.Label(row));container.MinHeight(0);container.Padding(Thickness{0});container.HorizontalContentAlignment(HorizontalAlignment::Stretch);
    tile->image=Image();tile->image.Width(spec.Metric(L"thumbnail_size"));tile->image.Height(spec.Metric(L"thumbnail_size"));tile->content.Children().Append(tile->image);if(auto found=thumbnailSources.find(row);found!=thumbnailSources.end()){tile->sources[0]=found->second;tile->image.Source(found->second);}
    auto label=Text(spec.Label(row));label.TextTrimming(TextTrimming::CharacterEllipsis);Grid::SetColumn(label,1);tile->content.Children().Append(label);
    ToggleButton eye;eye.Width(spec.Metric(L"header_height"));eye.Padding(Thickness{0});eye.IsChecked(poc_model_row_visible(model,row));Id(eye,spec.RowId(row,true));Name(eye,spec.Label(row));
    Image icon;SvgImageSource source;source.UriSource(Uri(L"ms-appx:///Assets/layer-eye-symbolic.svg"));source.RasterizePixelWidth(spec.Metric(L"column_right")*scale);source.RasterizePixelHeight(spec.Metric(L"column_right")*scale);icon.Source(source);icon.Width(spec.Metric(L"column_right"));icon.Height(spec.Metric(L"column_right"));eye.Content(icon);Grid::SetColumn(eye,2);tile->content.Children().Append(eye);eye.Click([weak=weak_from_this(),row](auto&&,auto&&){if(auto self=weak.lock())self->Activate(4,row);});
    auto percent=Text(spec.String(L"percent"));Grid::SetColumn(percent,3);tile->content.Children().Append(percent);
    auto rulerColor=poc_spec_ruler_color(row);Border ruler;ruler.Background(Brush(uint8_t(rulerColor>>16),uint8_t(rulerColor>>8),uint8_t(rulerColor)));ruler.Width(spec.value.GetNamedObject(L"probe_metrics").GetNamedNumber(L"bar")/scale);ruler.HorizontalAlignment(HorizontalAlignment::Left);ruler.Margin({spec.value.GetNamedObject(L"probe_metrics").GetNamedNumber(L"ruler_x")/scale,0,0,0});ruler.IsHitTestVisible(false);tile->content.Children().Append(ruler);Grid::SetColumnSpan(ruler,4);
    tiles[row]=tile;args.Handled(true);Changed();Trace("ui_layout",Fields({{L"start_qpc",JsonValue::CreateNumberValue(double(started))},{L"row",JsonValue::CreateNumberValue(row)},{L"rows",JsonValue::CreateNumberValue(poc_model_state(model).rows)}}));
}
void PocWindow::ObserveList(){
    if(!loaded||closing)return;
    if(!itemsPanel)itemsPanel=list.ItemsPanelRoot().try_as<ItemsStackPanel>();
    if(!listScroll){std::vector<DependencyObject> pending{list};while(!pending.empty()&&!listScroll){auto at=pending.back();pending.pop_back();if(auto found=at.try_as<ScrollViewer>())listScroll=found;else for(int i=0;i<VisualTreeHelper::GetChildrenCount(at);++i)pending.push_back(VisualTreeHelper::GetChild(at,i));}
        if(listScroll)listScroll.ViewChanged([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock();self&&!self->closing){self->Changed();if(self->thumbnailTimer&&!self->options.GetNamedBoolean(L"thumbnails"))self->ReadThumbnails();self->Trace("scroll",Fields({{L"offset",JsonValue::CreateNumberValue(self->listScroll.VerticalOffset())}}));}});
    }
    if(thumbnailTimer&&!options.GetNamedBoolean(L"thumbnails"))ReadThumbnails();
}
void PocWindow::Resize(){
    if(closing||!loaded)return;
    scale=float(root.XamlRoot().RasterizationScale());inputScale=scale;if(!FitClient())return;auto width=uint32_t(std::lround(root.ActualWidth()*scale)),height=uint32_t(std::lround(root.ActualHeight()*scale));
    if(canvasSize==std::array<uint32_t,2>{width,height}&&canvasScale==scale)return;auto oldScale=canvasScale;canvasSize={width,height};canvasScale=scale;
    if(canvas&&!poc_canvas_resize(canvas,width,height,scale)){Fail(E_FAIL);return;}
    if(oldScale>0&&oldScale!=scale)RebuildRows();Sync();UpdateProbes();Changed(uint16_t(width));
    Trace("resize",Fields({{L"width",JsonValue::CreateNumberValue(width)},{L"height",JsonValue::CreateNumberValue(height)},{L"scale",JsonValue::CreateNumberValue(scale)}}));
}
void PocWindow::UpdateProbes(){
    if(!loaded)return;auto pixel=1.0/scale;
    auto markerSpec=spec.value.GetNamedObject(L"marker"),probes=spec.value.GetNamedObject(L"probe_metrics");auto cell=markerSpec.GetNamedNumber(L"cell");auto origin=markerSpec.GetNamedArray(L"ui_origin"),patch=probes.GetNamedArray(L"zen_patch");
    marker.Width((markerSpec.GetNamedNumber(L"width")+2)*pixel);marker.Height((cell+2)*pixel);marker.Margin({(origin.GetNumberAt(0)-1)*pixel,(origin.GetNumberAt(1)-1)*pixel,0,0});
    for(uint32_t i=0;i<bits.size();++i){bits[i].Width(cell*pixel);bits[i].Height(cell*pixel);bits[i].Margin({(1+i*cell)*pixel,pixel,0,0});}
    auto bar=probes.GetNamedNumber(L"bar")*pixel;magenta.Width(layers.ActualWidth()/3);magenta.Height(bar);orange.Width(layers.ActualWidth()/3);orange.BorderThickness({0,0,0,probes.GetNamedNumber(L"fill")*pixel});drawerHost.Width(spec.Metric(L"drawer_width")+bar);drawerEdge.Width(bar);drawerEdge.Margin({spec.Metric(L"drawer_width"),0,0,0});widthEdge.Width(bar);zenPatch.Width(patch.GetNumberAt(2)*pixel);zenPatch.Height(patch.GetNumberAt(3)*pixel);zenPatch.Margin({patch.GetNumberAt(0)*pixel,patch.GetNumberAt(1)*pixel,0,0});
}
void PocWindow::PublishCaret(){
    if(!loaded||closing||!canvas||filter.ActualWidth()<=0||filter.ActualHeight()<=0)return;
    auto text=filter.Text();if(text.empty())return;auto end=uint32_t(filter.SelectionStart()+filter.SelectionLength());if(end>text.size())return;auto caret=filter.GetRectFromCharacterIndex(end==text.size()?end-1:end,end==text.size());if(caret.Height<=0)return;auto at=filter.TransformToVisual(root).TransformPoint({caret.X,caret.Y});JsonArray rect;for(auto value:{at.X*scale,at.Y*scale,std::max(caret.Width,1.0f/scale)*scale,caret.Height*scale})rect.Append(JsonValue::CreateNumberValue(value));
    Trace("caret",Fields({{L"rect",rect},{L"line_height",JsonValue::CreateNumberValue(caret.Height*scale)}}));
}
void PocWindow::PublishTitleInput(){
    if(!loaded||closing)return;std::vector<Windows::Graphics::RectInt32> rectangles;
    for(auto const& item:titleInputs){auto point=item.TransformToVisual(root).TransformPoint({0,0});rectangles.push_back({int32_t(std::lround(point.X*scale)),int32_t(std::lround(point.Y*scale)),int32_t(std::lround(item.ActualWidth()*scale)),int32_t(std::lround(item.ActualHeight()*scale))});}
    bool changed=rectangles.size()!=titleInputRects.size();if(!changed)for(size_t i=0;i<rectangles.size();++i){auto a=rectangles[i],b=titleInputRects[i];if(a.X!=b.X||a.Y!=b.Y||a.Width!=b.Width||a.Height!=b.Height){changed=true;break;}}
    if(changed){Microsoft::UI::Input::InputNonClientPointerSource::GetForWindowId(window.AppWindow().Id()).SetRegionRects(Microsoft::UI::Input::NonClientRegionKind::Passthrough,rectangles);titleInputRects=std::move(rectangles);}
}
void PocWindow::PublishGlass(){
    if(!loaded||!canvas||closing)return;UpdateProbes();std::vector<PocGlassRect> rectangles;auto revision=++glassRevision;
    for(auto const& group:std::array<FrameworkElement,3>{layers,properties,drawer}){if(group.ActualWidth()<=0||group.ActualHeight()<=0)return;auto position=group.TransformToVisual(root).TransformPoint({0,0});rectangles.push_back({position.X,position.Y,float(group.ActualWidth()),float(group.ActualHeight()),revision});}
    if(!poc_canvas_set_glass(canvas,rectangles.data(),rectangles.size()))Fail(E_FAIL);
}
void PocWindow::Changed(std::optional<uint16_t> state){
    if(closing)return;if(state)state16=*state;++uiFrame;auto value=(uint32_t(state16)<<8)|uiFrame;
    for(uint32_t i=0;i<bits.size();++i)bits[i].Fill(value&(1u<<i)?white:black);
    Trace("ui_change",Fields({{L"ui_frame",JsonValue::CreateNumberValue(uiFrame)},{L"state",JsonValue::CreateNumberValue(state16)}}));
}
void PocWindow::Activate(uint32_t action,uint32_t index){
    if(closing)return;auto before=poc_model_state(model);poc_model_activate(model,action,index);auto after=poc_model_state(model);Sync();if(before.revision==after.revision)return;
    if(action==2)AnimateDrawer(after.drawer_open);if(action==3)AnimateZen(after.zen_visible);
    if(action==4){auto found=tiles.find(index);if(found!=tiles.end())for(auto child:found->second->content.Children())if(auto toggle=child.try_as<ToggleButton>())toggle.IsChecked(poc_model_row_visible(model,index));}
    Changed();Trace("ui_action",Fields({{L"action",JsonValue::CreateNumberValue(action)},{L"index",JsonValue::CreateNumberValue(index)},{L"revision",JsonValue::CreateNumberValue(double(after.revision))}}));
}
void PocWindow::AnimateDrawer(bool open){
    auto from=drawerTransform.X();auto at=drawer.TransformToVisual(root).TransformPoint({0,0});if(drawerAnimation)drawerAnimation.Stop();drawerAnimation=Storyboard();DoubleAnimation animation;animation.From(from);animation.To(open?0:-spec.Metric(L"drawer_width"));animation.Duration(DurationHelper::FromTimeSpan(std::chrono::milliseconds(int64_t(spec.Metric(L"drawer_ms")))));CubicEase ease;ease.EasingMode(EasingMode::EaseOut);animation.EasingFunction(ease);Storyboard::SetTarget(animation,drawerTransform);Storyboard::SetTargetProperty(animation,L"X");drawerAnimation.Children().Append(animation);drawerAnimation.Completed([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock();self&&!self->closing){self->Changed();self->PublishGlass();}});PocGlassRect origin{at.X,at.Y,float(drawer.ActualWidth()),float(drawer.ActualHeight()),glassRevision};auto target=origin;target.x=open?0:-float(spec.Metric(L"drawer_width"));PocGlassAnimation glassAnimation{origin,target,1,1,2,Now(),uint32_t(spec.Metric(L"drawer_ms")),POC_GLASS_CUBIC_OUT};if(!poc_canvas_animate_glass(canvas,&glassAnimation)){Fail(E_FAIL);return;}drawerAnimation.Begin();
}
void PocWindow::AnimateZen(bool visible){
    auto from=chrome.Opacity();if(zenAnimation)zenAnimation.Stop();zenAnimation=Storyboard();DoubleAnimation animation;animation.From(from);animation.To(visible?1:0);animation.Duration(DurationHelper::FromTimeSpan(std::chrono::milliseconds(int64_t(spec.Metric(L"zen_ms")))));Storyboard::SetTarget(animation,chrome);Storyboard::SetTargetProperty(animation,L"Opacity");zenAnimation.Children().Append(animation);zenAnimation.Completed([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock();self&&!self->closing)self->Changed();});PocGlassAnimation glassAnimation{{},{},float(from),visible?1.0f:0.0f,POC_GLASS_OPACITY,Now(),uint32_t(spec.Metric(L"zen_ms")),POC_GLASS_LINEAR};if(!poc_canvas_animate_glass(canvas,&glassAnimation)){Fail(E_FAIL);return;}zenAnimation.Begin();
}
void PocWindow::Trace(char const* kind,JsonObject const& payload){if(!canvas)return;payload.Insert(L"kind",JsonValue::CreateStringValue(to_hstring(kind)));payload.Insert(L"qpc",JsonValue::CreateNumberValue(double(Now())));auto text=to_string(payload.Stringify());if(!poc_canvas_trace_record(canvas,text.c_str(),text.size())){exitCode=1;dispatcher.TryEnqueue([weak=weak_from_this()]{if(auto self=weak.lock())self->RequestClose();});}}
void PocWindow::Fail(hresult error){if(closing)return;exitCode=1;Trace("host_error",Fields({{L"hresult",JsonValue::CreateNumberValue(int32_t(error))}}));RequestClose();}
void PocWindow::RequestClose(){if(closing.exchange(true))return;if(cpuTimer)cpuTimer.Stop();if(stallTimer)stallTimer.Stop();if(thumbnailTimer)thumbnailTimer.Stop();if(drawerAnimation)drawerAnimation.Stop();if(zenAnimation)zenAnimation.Stop();FinishClose();}
fire_and_forget PocWindow::FinishClose(){
    auto lifetime=shared_from_this();auto ui=dispatcher;
    if(inputController)co_await inputController.ShutdownQueueAsync();
    if(thumbnailController)co_await thumbnailController.ShutdownQueueAsync();
    co_await resume_background();auto retired=canvas;if(retired){if(!poc_canvas_destroy(retired))exitCode=1;}
    ui.TryEnqueue([lifetime]{auto& self=*lifetime;self.canvas=nullptr;self.panel.as<ISwapChainPanelNative>()->SetSwapChain(nullptr);RemoveWindowSubclass(self.handle,WindowProc,1);poc_model_destroy(self.model);self.model=nullptr;self.tiles.clear();self.thumbnailSources.clear();self.items.Clear();self.window.Content(nullptr);self.root.Children().Clear();self.finished=true;self.window.Close();self.closed(self.exitCode.load());});
}
LRESULT CALLBACK PocWindow::WindowProc(HWND window,UINT message,WPARAM wparam,LPARAM lparam,UINT_PTR,DWORD_PTR reference){
    auto self=reinterpret_cast<PocWindow*>(reference);
    if(message==WM_CLOSE&&!self->finished){self->RequestClose();return 0;}
    return DefSubclassProc(window,message,wparam,lparam);
}
}
