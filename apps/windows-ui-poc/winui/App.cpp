#include "PocWindow.h"

namespace Poc {
struct App : ApplicationT<App,Markup::IXamlMetadataProvider> {
    XamlTypeInfo::XamlControlsXamlMetaDataProvider metadata;
    std::shared_ptr<PocWindow> owner;
    JsonObject options{nullptr};
    int* exitCode{};
    App(JsonObject const& value,int* result):options(value),exitCode(result){}
    Markup::IXamlType GetXamlType(Windows::UI::Xaml::Interop::TypeName const& type){return metadata.GetXamlType(type);}
    Markup::IXamlType GetXamlType(hstring const& name){return metadata.GetXamlType(name);}
    com_array<Markup::XmlnsDefinition> GetXmlnsDefinitions(){return metadata.GetXmlnsDefinitions();}
    void OnLaunched(LaunchActivatedEventArgs const&){
        Resources().MergedDictionaries().Append(XamlControlsResources());
        owner=std::make_shared<PocWindow>(options,[weak=get_weak()](int code){if(auto self=weak.get()){*self->exitCode=code;self->owner.reset();self->Exit();}});
        owner->Open();
    }
};
}
int WINAPI wWinMain(HINSTANCE,HINSTANCE,PWSTR,int){
    winrt::init_apartment(winrt::apartment_type::single_threaded);
    int exitCode=0;
    try{
        int count{};auto args=CommandLineToArgvW(GetCommandLineW(),&count);
        Poc::JsonArray arguments;
        for(int i=1;i<count;++i)arguments.Append(Poc::JsonValue::CreateStringValue(args[i]));
        LocalFree(args);
        auto text=winrt::to_string(arguments.Stringify());
        auto length=poc_app_options_json(text.c_str(),nullptr,0);
        std::string result(length,'\0');poc_app_options_json(text.c_str(),result.data(),result.size());
        auto options=Poc::JsonObject::Parse(winrt::to_hstring(result.c_str()));
        if(options.HasKey(L"error")){OutputDebugStringW(options.GetNamedString(L"error").c_str());return 1;}
        winrt::Microsoft::UI::Xaml::Application::Start([&](auto&&){winrt::make<Poc::App>(options.GetNamedObject(L"options"),&exitCode);});
    }catch(winrt::hresult_error const& error){OutputDebugStringW(error.message().c_str());return 1;}
    return exitCode;
}
