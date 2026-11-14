#pragma once
#include "UiControls.h"
#include <winrt/Windows.ApplicationModel.DataTransfer.h>

namespace CapyUi {
inline V colorUi(CapyLocalization  localization,J const& request) {
    auto text=to_string(request.Stringify());
    NativeString raw(capy_color_ui(localization,text.c_str()),capy_string_free);
    if(!raw)throw hresult_error(E_OUTOFMEMORY);
    return JsonValue::Parse(to_hstring(raw.get()));
}
inline Windows::UI::Color previewColor(A const& a){
    if(a.Size()!=4)return {};
    auto byte=[&](int i){return uint8_t(std::round(std::clamp(a.GetNumberAt(i),0.,1.)*255));};
    return {byte(3),byte(0),byte(1),byte(2)};
}
inline Windows::UI::Color displayColor(J const& value){return previewColor(array(value,L"rgba"));}
inline bool copyText(hstring const& text){
    Windows::ApplicationModel::DataTransfer::DataPackage package;package.SetText(text);
    try{Windows::ApplicationModel::DataTransfer::Clipboard::SetContent(package);return true;}catch(hresult_error const&){return false;}
}
using ColorAccepted=std::function<void(J,std::optional<double>)>;
void EditColor(std::shared_ptr<WorkspaceData> const& data,UIElement const& owner,J const& target,bool opaque,ColorAccepted accepted);
struct ColorForm {
    std::shared_ptr<void> owner;
    Grid body,head;
    StackPanel below;
    std::function<void(J const&)> choose;
    std::function<void(std::optional<hstring> const&)> transparent;
    std::function<void(bool)> arrange;
    std::function<bool()> escape,commit;
    std::function<void()> close;
};
std::optional<ColorForm> EmbedColor(std::shared_ptr<WorkspaceData> const& data,J const& color,std::function<void(J const&)> viewed,std::function<void(bool)> blocked);
}
