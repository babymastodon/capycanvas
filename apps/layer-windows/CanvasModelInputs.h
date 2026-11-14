#pragma once
#include <algorithm>
#include <initializer_list>
#include <map>
#include <functional>
#include <optional>
#include <set>
#include <string>
#include <utility>
#include <vector>
#include <winrt/Windows.Foundation.Collections.h>
#include <winrt/Windows.Data.Json.h>

class CanvasModelPaths {
public:
    using Path=std::vector<std::wstring>;
private:
    std::set<Path> paths;
    static bool contains(Path const& prefix,Path const& path){return prefix.size()<=path.size()&&std::equal(prefix.begin(),prefix.end(),path.begin());}
public:
    bool Empty()const{return paths.empty();}
    CanvasModelPaths(std::initializer_list<Path> initial={}){for(auto const& path:initial)Add(path);}
    void Add(Path const& path){
        auto at=paths.lower_bound(path);
        if(at!=paths.end()&&*at==path)return;
        if(at!=paths.begin()&&contains(*std::prev(at),path))return;
        auto end=at;while(end!=paths.end()&&contains(path,*end))++end;
        paths.emplace_hint(paths.erase(at,end),path);
    }
    void Merge(CanvasModelPaths const& other){for(auto const& path:other.paths)Add(path);}
    bool Touches(Path const& path)const{
        auto at=paths.lower_bound(path);
        return (at!=paths.end()&&contains(path,*at))||(at!=paths.begin()&&contains(*std::prev(at),path));
    }
    bool Intersects(CanvasModelPaths const& other)const{
        if(paths.size()>other.paths.size())return other.Intersects(*this);
        return std::any_of(paths.begin(),paths.end(),[&](auto const& path){return other.Touches(path);});
    }
};

struct CanvasModelChanges {
    using Path=CanvasModelPaths::Path;
    bool all=false;
    CanvasModelPaths paths;
    void Merge(CanvasModelChanges const& other){all|=other.all;paths.Merge(other.paths);}
    bool Touches(Path const& input)const{return all||paths.Touches(input);}
};

class CanvasModelInputs {
    using J=winrt::Windows::Data::Json::JsonObject;
    using Path=CanvasModelChanges::Path;
    CanvasModelPaths keys;
    bool captured=false;
public:
    class Capture {
        CanvasModelInputs& inputs;
        std::vector<std::pair<J,Path>> roots;
        Capture* previous;
        CanvasModelChanges const* changes;
        bool force;
        static inline thread_local Capture* current=nullptr;
        Capture(CanvasModelInputs& readSet,std::vector<std::pair<J,Path>> sources,CanvasModelChanges const* changed,bool refresh):
            inputs(readSet),roots(std::move(sources)),previous(std::exchange(current,this)),changes(changed),force(refresh){inputs.captured=true;}
    public:
        Capture(CanvasModelInputs& readSet,std::initializer_list<std::pair<J,Path>> sources,CanvasModelChanges const* changed=nullptr,bool refresh=false):
            Capture(readSet,std::vector<std::pair<J,Path>>(sources),changed,refresh){}
        Capture(Capture const&)=delete;
        ~Capture(){current=previous;}
        static bool Active(){return current!=nullptr;}
        static CanvasModelChanges const* Changes(){return current?current->changes:nullptr;}
        static void Merge(CanvasModelInputs const& readSet){if(current)current->inputs.keys.Merge(readSet.keys);}
        static void ReadPath(J const& object,Path const& suffix){
            if(!current)return;
            for(auto const& [root,prefix]:current->roots)if(object==root){
                auto path=prefix;path.insert(path.end(),suffix.begin(),suffix.end());
                current->inputs.keys.Add(path);
                break;
            }
        }
        static void Read(J const& object,wchar_t const* field){if(current)ReadPath(object,{field});}
        static void Alias(J const& object,J const& owner,Path const& suffix){
            if(!current||std::any_of(current->roots.begin(),current->roots.end(),[&](auto const& entry){return entry.first==object;}))return;
            for(auto const& [root,prefix]:current->roots)if(owner==root){auto path=prefix;path.insert(path.end(),suffix.begin(),suffix.end());current->roots.emplace_back(object,std::move(path));return;}
        }
        static bool Apply(CanvasModelInputs& readSet,std::function<void()> const& update,bool refresh=false){
            auto parent=current;
            if(!parent){update();return true;}
            bool changed=refresh||parent->force||!parent->changes||readSet.Changed(*parent->changes);
            if(changed){
                Capture capture(readSet,parent->roots,parent->changes,refresh||parent->force);update();
            }
            parent->inputs.keys.Merge(readSet.keys);return changed;
        }
    };
    class Presentation {
        CanvasModelInputs& inputs;
        std::optional<Capture> scope;
    public:
        Presentation(CanvasModelInputs& reads,std::initializer_list<std::pair<J,Path>> roots,CanvasModelChanges const* changes,bool force):inputs(reads){
            if(force||!changes||reads.Changed(*changes))scope.emplace(reads,roots,changes,force||!changes);
        }
        ~Presentation(){scope.reset();Capture::Merge(inputs);}
        explicit operator bool()const{return scope.has_value();}
    };
    void Add(Path path){keys.Add(path);}
    bool Changed(CanvasModelChanges const& changes)const{
        if(!captured||changes.all)return true;
        return keys.Intersects(changes.paths);
    }
};

inline bool JsonEqual(winrt::Windows::Data::Json::IJsonValue const& left,winrt::Windows::Data::Json::IJsonValue const& right){
    using Type=winrt::Windows::Data::Json::JsonValueType;
    if(!left||!right)return !left&&!right;
    if(left.ValueType()!=right.ValueType())return false;
    switch(left.ValueType()){
    case Type::Object:{
        auto a=left.GetObject(),b=right.GetObject();if(a.Size()!=b.Size())return false;
        for(auto const& entry:a)if(!b.HasKey(entry.Key())||!JsonEqual(entry.Value(),b.Lookup(entry.Key())))return false;
        return true;
    }
    case Type::Array:{
        auto a=left.GetArray(),b=right.GetArray();if(a.Size()!=b.Size())return false;
        for(uint32_t i=0;i<a.Size();++i)if(!JsonEqual(a.GetAt(i),b.GetAt(i)))return false;
        return true;
    }
    case Type::String:return left.GetString()==right.GetString();
    case Type::Number:return left.GetNumber()==right.GetNumber();
    case Type::Boolean:return left.GetBoolean()==right.GetBoolean();
    case Type::Null:return true;
    default:return false;
    }
}

struct CanvasModelSource {
    using J=winrt::Windows::Data::Json::JsonObject;
    using V=winrt::Windows::Data::Json::IJsonValue;
    using Path=CanvasModelPaths::Path;
private:
    CanvasModelChanges pending;
    J presentedBeforeAdoption{nullptr};
    static V valueAt(J const& root,Path const& path){
        V value=root;
        for(auto const& name:path){
            if(value.ValueType()!=winrt::Windows::Data::Json::JsonValueType::Object)return winrt::Windows::Data::Json::JsonValue::CreateNullValue();
            value=value.GetObject().GetNamedValue(name,winrt::Windows::Data::Json::JsonValue::CreateNullValue());
        }
        return value;
    }
    struct Change {std::optional<V> value;std::map<std::wstring,Change> children;};
    static V replace(V const& previous,Change const& change){
        auto value=change.value.value_or(previous);
        if(change.children.empty())return value;
        auto object=value.ValueType()==winrt::Windows::Data::Json::JsonValueType::Object?value.GetObject():J{};
        std::optional<J> result;
        for(auto const& [name,child]:change.children){
            auto before=object.GetNamedValue(name,winrt::Windows::Data::Json::JsonValue::CreateNullValue());
            auto after=replace(before,child);
            if(after==before&&object.HasKey(name))continue;
            if(!result){J copy;for(auto const& item:object)copy.Insert(item.Key(),item.Value());result=std::move(copy);}
            result->Insert(name,after);
        }
        return result?V(*result):value;
    }
    void stage(Change& tree,Path const& path,V const& value,bool independent){
        if(independent||valueAt(presentedBeforeAdoption?presentedBeforeAdoption:model,path)!=value){pending.paths.Add(path);changes.paths.Add(path);}
        auto target=&tree;for(auto const& name:path)target=&target->children[name];target->value=value;
    }
    void patch(Change const& tree){auto next=replace(model,tree);if(next!=model){model=next.GetObject();roots();}}
    void roots(){
        state=model.GetNamedObject(L"state",J{});
        auto value=model.GetNamedValue(L"color_preview",winrt::Windows::Data::Json::JsonValue::CreateNullValue());
        colorPreview=value.ValueType()==winrt::Windows::Data::Json::JsonValueType::Object?value.GetObject():J{};
    }
public:
    J state,catalog,model,colorPreview;
    CanvasModelChanges changes{true};
    CanvasModelSource()=default;
    CanvasModelSource(CanvasModelSource const& other):state(other.state),catalog(other.catalog),model(other.model),colorPreview(other.colorPreview),changes(other.changes){}
    class Adoption {
        CanvasModelSource& source;
    public:
        Adoption(CanvasModelSource& owner,J const& snapshot,CanvasModelChanges const& changes):source(owner){
            source.presentedBeforeAdoption=source.model;source.model=snapshot;source.roots();
            source.changes=changes;source.changes.Merge(std::exchange(source.pending,{}));
        }
        Adoption(Adoption const&)=delete;
        ~Adoption(){source.presentedBeforeAdoption=nullptr;}
    };
    bool HasPendingChanges()const{return pending.all||!pending.paths.Empty();}
    void Patch(Path const& path,V const& value,bool independent=false){Change tree;stage(tree,path,value,independent);patch(tree);}
    void Patch(J const& packet){
        Change tree;
        for(auto const& item:packet){
            auto name=std::wstring(item.Key());
            if(name==L"camera"||name==L"command_search")stage(tree,{L"state",name},item.Value(),true);
            else if(name==L"workspace_layout")for(auto const& field:item.Value().GetObject())stage(tree,{L"state",L"workspace",L"layout",std::wstring(field.Key())},field.Value(),true);
            else if(!name.starts_with(L"windows_")||!JsonEqual(model.GetNamedValue(name,nullptr),item.Value()))stage(tree,{name},item.Value(),true);
        }
        patch(tree);
    }
    CanvasModelInputs::Capture inputs(CanvasModelInputs& reads,bool force=false)const{
        return CanvasModelInputs::Capture(reads,{{model,{}},{state,{L"state"}},{catalog,{L"localization",L"catalog"}},{colorPreview,{L"color_preview"}}},&changes,force);
    }
    CanvasModelInputs::Presentation presentation(CanvasModelInputs& reads,bool force=false,CanvasModelChanges const* changed=nullptr)const{
        return CanvasModelInputs::Presentation(reads,{{model,{}},{state,{L"state"}},{catalog,{L"localization",L"catalog"}},{colorPreview,{L"color_preview"}}},changed?changed:CanvasModelInputs::Capture::Changes(),force);
    }
    template<class Update> bool apply(CanvasModelInputs& inputs,Update update,bool force=false){
        auto scope=presentation(inputs,force,&changes);if(scope)update();return bool(scope);
    }
};

class CanvasModelBinding {
    std::function<void()> update;
    mutable CanvasModelInputs inputs;
public:
    CanvasModelBinding()=default;
    template<class Update> CanvasModelBinding(Update callback):update(std::move(callback)){}
    explicit operator bool()const{return bool(update);}
    void operator()()const{update();}
    void Apply(bool force=false)const{CanvasModelInputs::Capture::Apply(inputs,update,force);}
};
