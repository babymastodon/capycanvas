#pragma once
#include <windows.h>
#undef GetCurrentTime
#include <algorithm>
#include <unknwn.h>
#include <shellapi.h>
#include <commctrl.h>
#include <microsoft.ui.xaml.window.h>
#include <microsoft.ui.xaml.media.dxinterop.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.Foundation.Collections.h>
#include <winrt/Windows.Data.Json.h>
#include <winrt/Windows.Storage.Streams.h>
#include <winrt/Windows.Graphics.Imaging.h>
#include <winrt/Windows.UI.h>
#include <winrt/Windows.UI.Xaml.Interop.h>
#include <winrt/Microsoft.UI.h>
#include <winrt/Microsoft.UI.Dispatching.h>
#include <winrt/Microsoft.UI.Input.h>
#include <winrt/Microsoft.UI.Windowing.h>
#include <winrt/Microsoft.UI.Xaml.h>
#include <winrt/Microsoft.UI.Xaml.Controls.h>
#include <winrt/Microsoft.UI.Xaml.Controls.Primitives.h>
#include <winrt/Microsoft.UI.Xaml.Automation.h>
#include <winrt/Microsoft.UI.Xaml.Automation.Peers.h>
#include <winrt/Microsoft.UI.Xaml.Input.h>
#include <winrt/Microsoft.UI.Xaml.Markup.h>
#include <winrt/Microsoft.UI.Xaml.Media.h>
#include <winrt/Microsoft.UI.Xaml.Media.Animation.h>
#include <winrt/Microsoft.UI.Xaml.Media.Imaging.h>
#include <winrt/Microsoft.UI.Xaml.Shapes.h>
#include <winrt/Microsoft.UI.Xaml.XamlTypeInfo.h>
#include <array>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <functional>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <thread>
#include <utility>
#include <vector>
#include "poc_canvas.h"

namespace Poc {
using namespace winrt;
using namespace Windows::Foundation;
using namespace Windows::Foundation::Collections;
using namespace Windows::Data::Json;
using namespace Microsoft::UI::Xaml;
using namespace Microsoft::UI::Xaml::Controls;
using namespace Microsoft::UI::Xaml::Controls::Primitives;
using namespace Microsoft::UI::Xaml::Media;
using namespace Microsoft::UI::Xaml::Media::Animation;
using namespace Microsoft::UI::Xaml::Media::Imaging;
using namespace Microsoft::UI::Dispatching;
inline JsonObject Fields(std::initializer_list<std::pair<hstring,IJsonValue>> fields){JsonObject object;for(auto const& [key,value]:fields)object.Insert(key,value);return object;}
inline int64_t Now(){LARGE_INTEGER value{};QueryPerformanceCounter(&value);return value.QuadPart;}
inline double CpuMs(){FILETIME start{},end{},kernel{},user{};if(!GetThreadTimes(GetCurrentThread(),&start,&end,&kernel,&user))return 0;ULARGE_INTEGER k{},u{};k.LowPart=kernel.dwLowDateTime;k.HighPart=kernel.dwHighDateTime;u.LowPart=user.dwLowDateTime;u.HighPart=user.dwHighDateTime;return double(k.QuadPart+u.QuadPart)/10000;}
inline SolidColorBrush Brush(uint8_t r,uint8_t g,uint8_t b,uint8_t a=255){return SolidColorBrush(Windows::UI::Color{a,r,g,b});}
inline void Id(DependencyObject const& item,hstring const& id){Automation::AutomationProperties::SetAutomationId(item,id);}
inline void Name(DependencyObject const& item,hstring const& name){Automation::AutomationProperties::SetName(item,name);}
inline ContentControl Accessible(UIElement const& child,hstring const& id,hstring const& label){ContentControl control;control.Padding(Thickness{0});control.BorderThickness(Thickness{0});control.HorizontalContentAlignment(HorizontalAlignment::Stretch);control.VerticalContentAlignment(VerticalAlignment::Stretch);control.IsTabStop(false);Automation::AutomationProperties::SetAccessibilityView(control,Automation::Peers::AccessibilityView::Control);Id(control,id);Name(control,label);control.Content(child);return control;}
inline TextBlock Text(hstring const& value){TextBlock text;text.Text(value);text.VerticalAlignment(VerticalAlignment::Center);return text;}
inline RowDefinition Row(double value,GridUnitType unit=GridUnitType::Pixel){RowDefinition row;row.Height(GridLength{value,unit});return row;}
inline ColumnDefinition Col(double value,GridUnitType unit=GridUnitType::Pixel){ColumnDefinition column;column.Width(GridLength{value,unit});return column;}
struct Spec {
    JsonObject value{JsonObject::Parse(to_hstring(poc_spec_json()))};
    double Metric(wchar_t const* key)const{return value.GetNamedObject(L"metrics").GetNamedNumber(key);}
    hstring Automation(wchar_t const* key)const{return value.GetNamedObject(L"automation").GetNamedString(key);}
    hstring String(wchar_t const* key)const{return value.GetNamedObject(L"strings").GetNamedString(key);}
    SolidColorBrush Probe(wchar_t const* key)const{auto a=value.GetNamedObject(L"probes").GetNamedArray(key);return Brush(uint8_t(a.GetNumberAt(0)),uint8_t(a.GetNumberAt(1)),uint8_t(a.GetNumberAt(2)));}
    SolidColorBrush Tint()const{auto a=value.GetNamedArray(L"glass_tint");return Brush(uint8_t(a.GetNumberAt(0)),uint8_t(a.GetNumberAt(1)),uint8_t(a.GetNumberAt(2)),uint8_t(a.GetNumberAt(3)));}
    hstring RowId(uint32_t row,bool visibility=false)const{auto size=poc_spec_row_id(row,visibility,nullptr,0);std::string text(size,'\0');poc_spec_row_id(row,visibility,text.data(),text.size());return to_hstring(text.c_str());}
    hstring Label(uint32_t index)const{auto size=poc_spec_label(index,nullptr,0);std::string text(size,'\0');poc_spec_label(index,text.data(),text.size());return to_hstring(text.c_str());}
};
}
