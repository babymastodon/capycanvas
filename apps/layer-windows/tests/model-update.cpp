#include "../KeyNames.h"
#include "../CanvasModelUpdate.h"
#include <cassert>
#include "../NativeJson.h"

int main(){
    winrt::init_apartment();
    using winrt::Windows::System::VirtualKey;
    assert(CapyUi::LatinKey(VirtualKey::Z,false,L"\u044f")==L"z");
    assert(CapyUi::LatinKey(VirtualKey::Z,true,L"\u042f")==L"Z");
    assert(CapyUi::LatinKey(VirtualKey::Q,false,L"a")==L"q");
    assert(CapyUi::LatinKey(VirtualKey::Z,false,L"z").empty());
    assert(CapyUi::LatinKey(VirtualKey::Number2,true,L"\u00e9")==L"@");
    assert(CapyUi::LatinKey(VirtualKey(VK_OEM_4),true,L"\u0445")==L"{");
    assert(CapyUi::LatinKey(VirtualKey::Escape,false,L"escape").empty());
    using J=winrt::Windows::Data::Json::JsonObject;
    CanvasModelSource previews;
    {CanvasModelSource::Adoption adoption(previews,J::Parse(LR"({"state":{},"color_preview":{"picker":{"editor":true,"preview":{"rgba":[1,0,0,1]}}}})"),{true});}
    auto retainedPreview=previews.colorPreview;
    auto camera=J::Parse(LR"({"revision":52,"camera":{"revision":10}})");
    previews.Patch(camera);assert(previews.colorPreview==retainedPreview);
    auto placement=J::Parse(LR"({"workspace_update":{"revision":52,"model_revision":39,"canvas_bar":{"layout":null}}})");
    previews.Patch(placement);assert(previews.colorPreview==retainedPreview);
    auto ended=J::Parse(LR"({"color_preview":{"picker":{"editor":false,"preview":null}}})");
    previews.Patch(ended);
    assert(!previews.colorPreview.GetNamedObject(L"picker").GetNamedBoolean(L"editor"));
    assert(previews.colorPreview.GetNamedObject(L"picker").GetNamedValue(L"preview").ValueType()==winrt::Windows::Data::Json::JsonValueType::Null);
    CanvasModelUpdate models;
    CanvasModelInputs body;
    auto record=[&](J const& model){
        auto state=model.GetNamedObject(L"state");
        CanvasModelInputs::Capture capture(body,{{model,{}},{state,{L"state"}}});
        assert(CapyUi::array(state,L"layers").Size()==1);
    };
    auto inputModel=J::Parse(LR"({"state":{"brush":{"diameter":9},"layers":[{"opacity":1}]}})");
    assert(body.Changed({}));
    record(inputModel);
    CanvasModelUpdate inputs;inputs.Read(inputModel);
    inputs.Read(J::Parse(LR"({"model_update":[[["state","brush","diameter"],32]]})"));
    assert(!body.Changed(inputs.changes));
    auto pending=inputs.changes;
    inputs.Read(J::Parse(LR"({"model_update":[[["state","layers","0","opacity"],0.5]]})"));
    pending.Merge(inputs.changes);
    assert(body.Changed(pending));
    inputs.Read(J::Parse(LR"({"model_update":[[["state","layers"],[]]]})"));
    assert(body.Changed(inputs.changes));
    inputs.Read(J::Parse(LR"({"model_update":[[["state"],{"layers":[]}]]})"));
    assert(body.Changed(inputs.changes));
    inputs.Read(inputModel);
    assert(body.Changed(inputs.changes));
    CanvasModelInputs numeric;
    {
        CanvasModelInputs::Capture capture(numeric,{{inputModel.GetNamedObject(L"state"),{L"state"}}});
        assert(CapyUi::num(inputModel.GetNamedObject(L"state"),{L"brush",L"diameter"})==9);
    }
    assert(!numeric.Changed({false,{{L"state",L"layers",L"0",L"opacity"}}}));
    assert(numeric.Changed({false,{{L"state",L"brush",L"diameter"}}}));
    record(inputModel);
    assert(body.Changed({false,{{L"state",L"layers",L"1"}}}));
    assert(!body.Changed({false,{{L"state",L"layer_properties"}}}));
    CanvasModelInputs layout;
    {
        auto state=inputModel.GetNamedObject(L"state");
        CanvasModelInputs::Capture capture(layout,{{inputModel,{}},{state,{L"state"}}});
        auto adoptedState=CapyUi::branch(inputModel,L"state");assert(adoptedState.Size()==2);
        {
            CanvasModelInputs::Capture nested(numeric,{{state,{L"state"}}});
            assert(CapyUi::num(state,{L"brush",L"diameter"})==9);
        }
        assert(CapyUi::array(inputModel,L"panels").Size()==0);
    }
    assert(!layout.Changed({false,{{L"state",L"brush",L"diameter"}}}));
    assert(layout.Changed({false,{{L"panels",L"0",L"controls"}}}));
    assert(!layout.Changed({}));
    assert(layout.Changed({true}));
    CanvasModelInputs conditional;
    for(auto field:{L"layers",L"brush"}){
        CanvasModelInputs::Capture capture(conditional,{{inputModel.GetNamedObject(L"state"),{L"state"}}});
        auto value=CapyUi::object(inputModel.GetNamedObject(L"state"),field);assert(field==std::wstring(L"layers")||value.Size()==1);
    }
    assert(conditional.Changed({false,{{L"state",L"layers"}}}));
    assert(conditional.Changed({false,{{L"state",L"brush"}}}));
    auto selected=J::Parse(LR"({"commands":[{"id":"undo","label":"Undo"},{"id":"fit","label":"Fit"}]})");
    CanvasModelInputs command;
    {
        CanvasModelInputs::Capture capture(command,{{selected,{L"state"}}});
        auto row=CapyUi::find(selected,L"commands",L"id",L"fit");
        assert(CapyUi::str(row,L"label")==L"Fit");
    }
    assert(command.Changed({false,{{L"state",L"commands",L"1",L"label"}}}));
    assert(!command.Changed({false,{{L"state",L"commands",L"0",L"label"}}}));
    assert(command.Changed({false,{{L"state",L"commands",L"0",L"id"}}}));
    assert(command.Changed({false,{{L"state",L"commands"}}}));
    int refreshes=0;
    CanvasModelInputs parent;
    auto state=inputModel.GetNamedObject(L"state");
    CanvasModelBinding binding([&]{assert(CapyUi::num(state,{L"brush",L"preset"},-1)==-1);++refreshes;});
    auto applyBinding=[&](CanvasModelChanges const& changes,bool force=false){
        CanvasModelInputs::Capture capture(parent,{{state,{L"state"}}},&changes,force);binding.Apply();
    };
    applyBinding({});assert(refreshes==1);
    applyBinding({false,{{L"state",L"brush",L"color"}}});assert(refreshes==1);
    applyBinding({false,{{L"state",L"layers"}}});assert(refreshes==1);
    applyBinding({false,{{L"state",L"brush",L"preset"}}});assert(refreshes==2);
    applyBinding({},true);assert(refreshes==3);
    assert(parent.Changed({false,{{L"state",L"brush",L"preset"}}}));
    assert(!parent.Changed({false,{{L"state",L"brush",L"color"}}}));
    binding();assert(refreshes==4);
    std::vector<CanvasModelBinding> copied{binding};copied.front()();assert(refreshes==5);
    bool alternate=false;int conditionalRefreshes=0;
    CanvasModelBinding conditionalBinding([&]{if(alternate)CapyUi::array(state,L"layers");else CapyUi::object(state,L"brush");++conditionalRefreshes;});
    {
        CanvasModelChanges unchanged;
        CanvasModelInputs::Capture capture(parent,{{state,{L"state"}}},&unchanged);
        conditionalBinding.Apply();alternate=true;conditionalBinding.Apply(true);
    }
    assert(conditionalRefreshes==2);
    {
        CanvasModelChanges changed{false,{{L"state",L"layers"}}};
        CanvasModelInputs::Capture capture(parent,{{state,{L"state"}}},&changed);conditionalBinding.Apply();
    }
    assert(conditionalRefreshes==3);
    CanvasModelPaths paths{{L"state",L"layers",L"0",L"opacity"},{L"state",L"layers",L"1",L"opacity"}};
    paths.Add({L"state",L"layers"});paths.Add({L"state",L"layers",L"2"});
    assert(paths.Touches({L"state"})&&paths.Touches({L"state",L"layers",L"9",L"name"}));
    assert(!paths.Touches({L"state",L"layer_properties"}));
    CanvasModelPaths siblings{{L"state",L"brush",L"diameter"},{L"catalog",L"name"}};
    assert(!paths.Intersects(siblings)&&!siblings.Intersects(paths));
    siblings.Merge(paths);
    assert(siblings.Intersects(paths)&&paths.Intersects(siblings));
    assert(siblings.Touches({L"state",L"brush",L"diameter"}));
    CanvasModelPaths rootPaths;rootPaths.Add({});
    assert(rootPaths.Intersects(paths)&&paths.Intersects(rootPaths));
    CanvasModelInputs fallback;
    {
        CanvasModelInputs::Capture capture(fallback,{{state,{L"state"}}});
        assert(CapyUi::num(state,{L"missing",L"value"},17)==17);
    }
    assert(fallback.Changed({false,{{L"state",L"missing"}}}));
    assert(!fallback.Changed({false,{{L"state",L"brush"}}}));
    auto workspaceModel=J::Parse(LR"({"windows_workspace":{"name":"Studio","revision":1}})");
    CanvasModelInputs workspaceName;
    {
        CanvasModelInputs::Capture capture(workspaceName,{{workspaceModel,{}}});
        assert(CapyUi::str(CapyUi::branch(workspaceModel,L"windows_workspace"),L"name")==L"Studio");
    }
    assert(!workspaceName.Changed({false,{{L"windows_workspace",L"revision"}}}));
    assert(workspaceName.Changed({false,{{L"windows_workspace",L"name"}}}));
    assert(workspaceName.Changed({false,{{L"windows_workspace"}}}));
    CanvasModelInputs emptyBranch;
    {
        CanvasModelInputs::Capture capture(emptyBranch,{{workspaceModel,{}}});
        assert(CapyUi::str(CapyUi::branch(workspaceModel,L"missing"),L"name").empty());
    }
    assert(emptyBranch.Changed({false,{{L"missing",L"name"}}}));
    assert(!emptyBranch.Changed({false,{{L"windows_workspace",L"name"}}}));
    for(auto field:{L"command_search",L"canvas_bar",L"new_drawing"}){
        J readerModel;readerModel.Insert(field,winrt::Windows::Data::Json::JsonValue::CreateNullValue());
        CanvasModelInputs reads;
        {
            CanvasModelInputs::Capture capture(reads,{{readerModel,{}}});
            assert(CapyUi::named(readerModel,field).ValueType()==winrt::Windows::Data::Json::JsonValueType::Null);
        }
        assert(reads.Changed({false,{{field}}}));
        assert(!reads.Changed({false,{{L"unrelated"}}}));
        readerModel.Insert(field,J::Parse(LR"({"value":3})"));
        {
            CanvasModelInputs::Capture capture(reads,{{readerModel,{}}});
            auto value=CapyUi::named(readerModel,field);
            assert(value.ValueType()==winrt::Windows::Data::Json::JsonValueType::Object&&value.GetObject().GetNamedNumber(L"value")==3);
        }
        assert(reads.Changed({false,{{field,L"value"}}}));
        assert(!reads.Changed({false,{{L"unrelated",L"value"}}}));
    }
    CanvasModelInputs wholeState,preciseState;
    {
        CanvasModelInputs::Capture capture(wholeState,{{inputModel,{}},{state,{L"state"}}});
        assert(CapyUi::object(inputModel,L"state").GetNamedObject(L"brush").GetNamedNumber(L"diameter")==9);
    }
    {
        CanvasModelInputs::Capture capture(preciseState,{{inputModel,{}},{state,{L"state"}}});
        assert(CapyUi::num(CapyUi::branch(inputModel,L"state"),{L"brush",L"diameter"})==9);
    }
    assert(wholeState.Changed({false,{{L"state",L"layers"}}}));
    assert(!preciseState.Changed({false,{{L"state",L"layers"}}}));
    assert(preciseState.Changed({false,{{L"state",L"brush",L"diameter"}}}));
    auto aliasModel=J::Parse(LR"({"projection":{"value":1}})");
    int aliasRefreshes=0;double aliasValue=0;
    CanvasModelInputs aliasParent;
    CanvasModelBinding aliasChild([&]{aliasValue=CapyUi::object(aliasModel,L"projection").GetNamedNumber(L"value");++aliasRefreshes;});
    for(int value:{1,2}){
        aliasModel=J::Parse(value==1?LR"({"projection":{"value":1}})":LR"({"projection":{"value":2}})");
        CanvasModelChanges changed{value==1,{{L"projection",L"value"}}};
        CanvasModelInputs::Capture capture(aliasParent,{{aliasModel,{}}},&changed);
        auto inherited=CapyUi::object(aliasModel,L"projection");assert(inherited.GetNamedNumber(L"value")==value);
        aliasChild.Apply();
    }
    assert(aliasRefreshes==2&&aliasValue==2);
    CanvasModelInputs cardinality;
    {
        CanvasModelInputs::Capture capture(cardinality,{{selected,{L"state"}}});
        assert(CapyUi::count(selected,L"commands")==2);
        assert(CapyUi::count(selected,L"missing")==0);
    }
    assert(!cardinality.Changed({false,{{L"state",L"commands",L"0",L"label"}}}));
    assert(!cardinality.Changed({false,{{L"state",L"commands",L"1"}}}));
    assert(cardinality.Changed({false,{{L"state",L"commands"}}}));
    assert(cardinality.Changed({false,{{L"state"}}}));
    assert(cardinality.Changed({false,{{L"state",L"missing"}}}));
    CanvasModelInputs presentation,parentPresentation;
    int adopted=0,presented=0;
    auto present=[&](J const& latest,CanvasModelChanges const* changes,bool force){
        ++adopted;
        CanvasModelInputs::Presentation scope(presentation,{{latest,{L"state"}}},changes,force);
        if(scope){assert(CapyUi::num(latest,{L"brush",L"diameter"})==9);++presented;}
    };
    CanvasModelChanges noChanges,unrelated{false,{{L"state",L"layers"}}};
    present(state,&noChanges,false);present(state,&unrelated,false);
    assert(adopted==2&&presented==1);
    {
        CanvasModelInputs::Capture capture(parentPresentation,{{state,{L"state"}}},&unrelated);
        present(state,&unrelated,false);
    }
    assert(adopted==3&&presented==1);
    assert(parentPresentation.Changed({false,{{L"state",L"brush",L"diameter"}}}));
    assert(!parentPresentation.Changed(unrelated));
    present(state,&noChanges,true);assert(adopted==4&&presented==2);
    present(state,nullptr,false);assert(adopted==5&&presented==3);
    CanvasModelSource source;
    source.model=J::Parse(LR"({"state":{"selected":false,"other":0}})");
    source.state=source.model.GetNamedObject(L"state");
    CanvasModelInputs container,bodyInputs;
    int containerUpdates=0,bodyUpdates=0;
    bool selectedValue=false;
    auto updateBody=[&]{source.apply(bodyInputs,[&]{selectedValue=CapyUi::flag(source.state,L"selected");++bodyUpdates;});};
    updateBody();assert(bodyUpdates==1&&!selectedValue);
    source.changes={false,{{L"state",L"other"}}};
    source.apply(container,[&]{++containerUpdates;updateBody();});
    assert(containerUpdates==1&&bodyUpdates==1);
    source.model=J::Parse(LR"({"state":{"selected":true,"other":0}})");
    source.state=source.model.GetNamedObject(L"state");
    source.changes={false,{{L"state",L"selected"}}};
    source.apply(container,[&]{++containerUpdates;updateBody();});
    assert(containerUpdates==2&&bodyUpdates==2&&selectedValue);
    for(auto const& [packet,path]:std::vector<std::pair<wchar_t const*,CanvasModelPaths::Path>>{
        {LR"({"command_search":{"open":true}})",{L"state",L"command_search",L"open"}},
        {LR"({"camera":{"zoom":2}})",{L"state",L"camera",L"zoom"}},
        {LR"({"layout":{"width":200},"workspace_layout":{"corner":"right"}})",{L"layout",L"width"}},
        {LR"({"layout":{"width":200},"workspace_layout":{"corner":"right"}})",{L"state",L"workspace",L"layout",L"corner"}},
        {LR"({"workspace_update":{"drag":{"x":2}}})",{L"workspace_update",L"drag"}},
        {LR"({"color_preview":{"picker":{"editor":true}}})",{L"color_preview",L"picker",L"editor"}},
        {LR"({"windows_glass":{"chrome_revision":2}})",{L"windows_glass",L"chrome_revision"}},
        {LR"({"background_inputs":{"value":2}})",{L"background_inputs",L"value"}}}){
        CanvasModelUpdate transport;CanvasModelSource packetSource;CanvasModelInputs reads;int updates=0;winrt::hstring value;
        transport.Read(J::Parse(LR"({"state":{"revision":1,"command_search":{"open":false},"camera":{"zoom":1},"workspace":{"layout":{"corner":"left"}}},"layout":{"width":100},"workspace_update":{"drag":null},"color_preview":{"picker":{"editor":false}},"windows_glass":{"chrome_revision":1},"background_inputs":{"value":1}})"));
        auto refresh=[&]{packetSource.apply(reads,[&]{auto object=packetSource.model;for(size_t i=0;i+1<path.size();++i)object=CapyUi::branch(object,path[i].c_str());value=CapyUi::named(object,path.back().c_str()).Stringify();++updates;});};
        {CanvasModelSource::Adoption adoption(packetSource,transport.Model(),transport.changes);refresh();}
        auto initialValue=value;auto original=transport.Model().Stringify();
        packetSource.changes={};packetSource.Patch(J::Parse(packet));refresh();
        assert(updates==2&&value!=initialValue&&packetSource.HasPendingChanges());
        assert(transport.Model().Stringify()==original);
        transport.Read(J::Parse(LR"({"model_update":[[["state","revision"],2]]})"));
        assert(!transport.changes.Touches(path));
        {CanvasModelSource::Adoption adoption(packetSource,transport.Model(),transport.changes);refresh();}
        assert(updates==3&&value==initialValue&&!packetSource.HasPendingChanges());
        transport.Read(J::Parse(LR"({"model_update":[[["state","revision"],3]]})"));
        {CanvasModelSource::Adoption adoption(packetSource,transport.Model(),transport.changes);refresh();}
        assert(updates==3);
    }
    {
        CanvasModelSource batch;auto initial=J::Parse(LR"({"state":{"camera":{"zoom":1},"command_search":null,"workspace":{"layout":{"corner":"left","height":100}}},"layout":{"width":100},"windows_glass":{"chrome_revision":1}})");auto original=initial.Stringify();
        {CanvasModelSource::Adoption adoption(batch,initial,{true});}
        batch.Patch(J::Parse(LR"({"camera":{"zoom":2},"command_search":{"open":true},"workspace_layout":{"corner":"right","height":200},"layout":{"width":200},"windows_glass":{"chrome_revision":2},"unregistered_background":null})"));
        assert(initial.Stringify()==original&&CapyUi::num(CapyUi::object(batch.state,L"camera"),L"zoom")==2&&CapyUi::flag(CapyUi::object(batch.state,L"command_search"),L"open"));
        auto batchLayout=CapyUi::object(CapyUi::object(batch.state,L"workspace"),L"layout");
        assert(CapyUi::str(batchLayout,L"corner")==L"right"&&CapyUi::num(batchLayout,L"height")==200&&CapyUi::num(CapyUi::object(batch.model,L"layout"),L"width")==200);
        assert(CapyUi::num(CapyUi::object(batch.model,L"windows_glass"),L"chrome_revision")==2&&batch.model.HasKey(L"unregistered_background")&&batch.model.GetNamedValue(L"unregistered_background").ValueType()==winrt::Windows::Data::Json::JsonValueType::Null);
        {CanvasModelSource::Adoption adoption(batch,initial,{});assert(batch.changes.Touches({L"state",L"workspace",L"layout",L"height"})&&batch.changes.Touches({L"unregistered_background"}));}
        assert(batch.model.Stringify()==original&&!batch.HasPendingChanges());
        batch.Patch(J::Parse(LR"({"windows_glass":{"chrome_revision":1}})"));
        assert(!batch.HasPendingChanges());
        batch.Patch(J::Parse(LR"({"windows_glass":{"chrome_revision":3}})"));
        assert(batch.HasPendingChanges()&&CapyUi::num(CapyUi::object(batch.model,L"windows_glass"),L"chrome_revision")==3);
    }
    {
        auto snapshot=[](wchar_t const* panels){return J::Parse(std::wstring(LR"({"state":{"camera":{"zoom":1},"workspace":{"id":"paint","layout":{"phone":null,"header":{"items":[{"label":"Ink \"wet\" {1}, [2]"}]},"canvas_info":true,"canvas_bar":{"edge":"top"},"bands":[],"panels":)")+panels+LR"(,"floating":[],"collapsed":[],"column_stacks":[[1,2]],"fit_tab_groups":[],"fit_height_groups":[],"next_tile_id":4,"next_id":9}}},"layout":{"width":200},"windows_glass":{"chrome_revision":1}})");};
        CanvasModelSource traced;traced.model=snapshot(LR"(["layers"])");
        traced.Patch(J::Parse(LR"({"workspace_layout":{"panels":["layers","color"]},"windows_glass":{"chrome_revision":1}})"));
        auto canonical=CapyUi::CanonicalJson(winrt::to_string(traced.model.Stringify()));
        assert(canonical==CapyUi::CanonicalJson(winrt::to_string(snapshot(LR"(["layers","color"])").Stringify())));
        assert(CapyUi::JsonEqual(J::Parse(winrt::to_hstring(canonical)),traced.model));
    }
    {
        CanvasModelSource layoutSource;CanvasModelInputs layoutReads;int updates=0;
        auto full=J::Parse(LR"({"state":{},"layout":{"width":100}})"),resolved=J::Parse(LR"({"width":200})");
        auto refresh=[&]{layoutSource.apply(layoutReads,[&]{assert(CapyUi::num(CapyUi::object(layoutSource.model,L"layout"),L"width")==200);++updates;});};
        {CanvasModelSource::Adoption adoption(layoutSource,full,{true});layoutSource.Patch({L"layout"},resolved);refresh();}
        {CanvasModelSource::Adoption adoption(layoutSource,full,{});layoutSource.Patch({L"layout"},resolved);refresh();}
        assert(updates==2&&!layoutSource.HasPendingChanges());
        layoutSource.Patch({L"layout"},resolved);assert(!layoutSource.HasPendingChanges());
        {CanvasModelSource::Adoption adoption(layoutSource,full,{});layoutSource.Patch({L"layout"},resolved);refresh();}
        assert(updates==2&&!layoutSource.HasPendingChanges());
        J repeated;repeated.Insert(L"layout",resolved);layoutSource.Patch(repeated);assert(layoutSource.HasPendingChanges());
        {CanvasModelSource::Adoption adoption(layoutSource,full,{});layoutSource.Patch({L"layout"},resolved);refresh();}
        assert(updates==3&&!layoutSource.HasPendingChanges());
    }
    auto initial=J::Parse(LR"({"state":{"revision":1,"histogram":{"status":"old"},"workspace":{"layout":{"extent":240}}},"catalog":{"01":"literal","a/b":"key"},"rows":[{"value":1},{"value":2}]})");
    assert(models.Read(initial)==CanvasAllSections);
    initial.Insert(L"localization",J::Parse(LR"({"generation":1})"));
    models.Read(initial);
    assert(!models.Model().HasKey(L"localization"));
    auto old=models.Model();
    auto fields=J::Parse(LR"({"model_update":[[["state","revision"],2],[["state","histogram","status"],"ready"]]})");
    assert(models.Read(fields)==CanvasScopes);
    auto next=models.Model();
    assert(old.GetNamedObject(L"state").GetNamedObject(L"histogram").GetNamedString(L"status")==L"old");
    assert(next.GetNamedObject(L"state").GetNamedObject(L"histogram").GetNamedString(L"status")==L"ready");
    old.GetNamedObject(L"state").GetNamedObject(L"workspace").GetNamedObject(L"layout").Insert(L"extent",winrt::Windows::Data::Json::JsonValue::CreateNumberValue(999));
    assert(models.Model().GetNamedObject(L"state").GetNamedObject(L"workspace").GetNamedObject(L"layout").GetNamedNumber(L"extent")==240);
    models.Read(J::Parse(LR"({"model_update":[[["catalog","01"],"changed"],[["catalog","a/b"],"literal path"],[["rows","0","value"],3],[["rows","1","value"],4]]})"));
    next=models.Model();
    assert(next.GetNamedObject(L"catalog").GetNamedString(L"01")==L"changed");
    assert(next.GetNamedObject(L"catalog").GetNamedString(L"a/b")==L"literal path");
    assert(next.GetNamedArray(L"rows").GetObjectAt(0).GetNamedNumber(L"value")==3);
    assert(next.GetNamedArray(L"rows").GetObjectAt(1).GetNamedNumber(L"value")==4);
    assert(old.GetNamedArray(L"rows").GetObjectAt(0).GetNamedNumber(L"value")==1);
    models.Read(J::Parse(LR"({"model_update":[[["rows"],[7]],[["state","histogram"],null]]})"));
    next=models.Model();
    assert(next.GetNamedArray(L"rows").Size()==1&&next.GetNamedArray(L"rows").GetNumberAt(0)==7);
    assert(next.GetNamedObject(L"state").GetNamedValue(L"histogram").ValueType()==winrt::Windows::Data::Json::JsonValueType::Null);
    CanvasModelUpdate toolbar;
    toolbar.Read(J::Parse(LR"({"state":{},"component":{"options":[{"Range":{"bounds":[{"preview":false,"id":"lower","group_id":"","numeric":{"min":-149,"max":128},"value":-3.5},{"preview":false,"id":"upper","group_id":"","numeric":{"min":-149,"max":128},"value":-1.5}]}}]}})"));
    auto schema=[](J const& model){
        auto component=J::Parse(model.GetNamedObject(L"component").Stringify());
        for(auto bound:component.GetNamedArray(L"options").GetObjectAt(0).GetNamedObject(L"Range").GetNamedArray(L"bounds"))
            bound.GetObject().SetNamedValue(L"value",winrt::Windows::Data::Json::JsonValue::CreateNumberValue(0));
        return component;
    };
    auto before=schema(toolbar.Model());
    toolbar.Read(J::Parse(LR"({"model_update":[[["component","options","0","Range","bounds","1","value"],6]]})"));
    auto after=schema(toolbar.Model());
    assert(CapyUi::JsonEqual(before,after));
    auto changed=J::Parse(after.Stringify());
    changed.GetNamedArray(L"options").GetObjectAt(0).GetNamedObject(L"Range").GetNamedArray(L"bounds").GetObjectAt(1).SetNamedValue(L"id",winrt::Windows::Data::Json::JsonValue::CreateStringValue(L"other"));
    assert(!CapyUi::JsonEqual(before,changed));
    assert(!CapyUi::JsonEqual(J::Parse(LR"({"bounds":[1,2]})"),J::Parse(LR"({"bounds":[2,1]})")));
    assert(!CapyUi::JsonEqual(J::Parse(LR"({"enabled":true})"),J::Parse(LR"({"enabled":false})")));
    return 0;
}
