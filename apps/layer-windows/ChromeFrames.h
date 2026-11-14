#pragma once
#include "pch.h"
#include <map>
#include <set>
#include <functional>
#include <vector>
#include <utility>

namespace CapyUi {
struct ChromeFrames {
    using Object=winrt::Windows::Data::Json::JsonObject;
    struct Frame {
        std::set<uint32_t> groups,columns;
        std::set<void*> elements;
        Object document;
    };
    Frame current,presented;
    std::map<uint64_t,Frame> sent;
    std::map<void*,std::function<void()>> retiring;
    winrt::weak_ref<winrt::Microsoft::UI::Xaml::FrameworkElement> root;
    uint64_t revision=0,sentRevision=0;
    bool closed=false;
    static void* Id(winrt::Windows::Foundation::IInspectable const& element){return element.as<::IUnknown>().get();}
    void Track(winrt::Windows::Foundation::IInspectable const& element){current.elements.insert(Id(element));}
    bool RetainsId(void* id)const{
        if(closed)return false;
        if(presented.elements.contains(id))return true;
        for(auto const& [revision,frame]:sent)if(frame.elements.contains(id))return true;
        return false;
    }
    bool Retains(winrt::Windows::Foundation::IInspectable const& element)const{return RetainsId(Id(element));}
    bool Group(uint32_t id)const{return current.groups.contains(id)&&presented.groups.contains(id);}
    bool Column(uint32_t id)const{return current.columns.contains(id)&&presented.columns.contains(id);}
    static bool SameFrame(Frame const& first,Frame const& second,Object const& document){
        if(first.groups!=second.groups||first.columns!=second.columns||first.elements!=second.elements)return false;
        return first.document.GetNamedNumber(L"width",0)==document.GetNamedNumber(L"width",0)
            &&first.document.GetNamedNumber(L"height",0)==document.GetNamedNumber(L"height",0);
    }
    bool Sent(uint64_t next){
        if(closed)return false;
        sentRevision=next;sent.insert_or_assign(next,current);
        while(sent.size()>16&&sent.begin()->first<next)sent.erase(sent.begin());
        if(next!=revision||(current.elements==presented.elements&&current.groups==presented.groups&&current.columns==presented.columns))return false;
        return Present(next,presented.document);
    }
    bool Present(uint64_t next,Object document){
        if(closed)return false;
        auto found=sent.find(next);if(found==sent.end()||next<revision)return false;
        bool changed=!SameFrame(presented,found->second,document);
        presented=found->second;presented.document=document;revision=next;
        sent.erase(sent.begin(),found);
        std::vector<std::function<void()>> release;
        for(auto it=retiring.begin();it!=retiring.end();)if(!RetainsId(it->first)){
            release.push_back(std::move(it->second));it=retiring.erase(it);
        }else ++it;
        for(auto& ready:release)ready();
        return changed;
    }
    void Reset(){presented=Frame{};revision=0;}
    void Close(){if(closed)return;closed=true;current=Frame{};presented=Frame{};sent.clear();auto release=std::exchange(retiring,{});for(auto& [id,ready]:release)ready();}
};
}
