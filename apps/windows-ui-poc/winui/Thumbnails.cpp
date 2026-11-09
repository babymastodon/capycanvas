#include "PocWindow.h"
#include <robuffer.h>

namespace Poc {
using winrt::Windows::Graphics::Imaging::SoftwareBitmap;
using winrt::Windows::Graphics::Imaging::BitmapPixelFormat;
using winrt::Windows::Graphics::Imaging::BitmapAlphaMode;
using winrt::Windows::Storage::Streams::Buffer;
namespace {
fire_and_forget ShowTile(std::weak_ptr<PocWindow> weak,std::shared_ptr<PocWindow::Tile> tile,SoftwareBitmap bitmap){
    auto self=weak.lock();if(!self||self->closing||tile->uploading)co_return;
    auto found=self->tiles.find(tile->row);if(found==self->tiles.end()||found->second!=tile)co_return;
    auto back=1-tile->front;if(!tile->sources[back])tile->sources[back]=SoftwareBitmapSource();tile->uploading=true;self->thumbnailSources[tile->row]=tile->sources[back];
    auto capacity=size_t(self->spec.Metric(L"thumbnail_capacity"));for(auto it=self->thumbnailSources.begin();self->thumbnailSources.size()>capacity&&it!=self->thumbnailSources.end();)if(!self->tiles.contains(it->first))it=self->thumbnailSources.erase(it);else ++it;
    try{co_await tile->sources[back].SetBitmapAsync(bitmap);
        if(!self->closing){found=self->tiles.find(tile->row);if(found!=self->tiles.end()&&self->thumbnailSources[tile->row]==tile->sources[back]){if(found->second==tile)tile->front=back;else found->second->sources[found->second->front]=tile->sources[back];found->second->image.Source(tile->sources[back]);self->Changed();}}
    }catch(hresult_error const& error){self->Fail(error.code());}
    tile->uploading=false;
}
}
void PocWindow::StartThumbnails(){
    thumbnailController=DispatcherQueueController::CreateOnDedicatedThread();thumbnailController.DispatcherQueue().TryEnqueue([weak=weak_from_this()]{if(auto self=weak.lock();self&&!self->closing){self->thumbnailCpuTimer=self->thumbnailController.DispatcherQueue().CreateTimer();self->thumbnailCpuTimer.Interval(std::chrono::seconds(1));self->thumbnailCpuTimer.Tick([weak](auto&&,auto&&){if(auto self=weak.lock();self&&!self->closing)self->Trace("thread_cpu",Fields({{L"thread",JsonValue::CreateStringValue(L"thumbnails")},{L"cpu_ms",JsonValue::CreateNumberValue(CpuMs())}}));});self->thumbnailCpuTimer.Start();}});thumbnailTimer=dispatcher.CreateTimer();thumbnailTimer.Interval(std::chrono::microseconds(int64_t(1'000'000/spec.Metric(L"thumbnail_hz"))));thumbnailTimer.Tick([weak=weak_from_this()](auto&&,auto&&){if(auto self=weak.lock();self&&!self->closing)self->ReadThumbnails();});if(options.GetNamedBoolean(L"thumbnails"))thumbnailTimer.Start();else ReadThumbnails();
}
void PocWindow::ReadThumbnails(){
    if(closing||!canvas||!itemsPanel)return;
    auto first=itemsPanel.FirstVisibleIndex(),last=itemsPanel.LastVisibleIndex();auto count=poc_model_state(model).visible_rows;
    if(!count||first<0||uint32_t(first)>=count||last<first){poc_canvas_set_thumbnail_rows(canvas,nullptr,0);visibleThumbnailRows.clear();if(!options.GetNamedBoolean(L"thumbnails"))thumbnailTimer.Stop();return;}
    last=std::min(last,int32_t(count-1));
    std::vector<uint32_t> rowIds;rowIds.reserve(size_t(last-first+1));for(auto ordinal=first;ordinal<=last;++ordinal)rowIds.push_back(poc_model_visible_row(model,uint32_t(ordinal)));
    visibleThumbnailRows=rowIds;if(!options.GetNamedBoolean(L"thumbnails")&&rowIds==deliveredThumbnailRows){thumbnailTimer.Stop();return;}
    if(!options.GetNamedBoolean(L"thumbnails"))thumbnailTimer.Start();if(readbackPending.exchange(true))return;
    if(!poc_canvas_set_thumbnail_rows(canvas,rowIds.data(),uint32_t(rowIds.size()))){readbackPending=false;Fail(E_FAIL);return;}
    thumbnailController.DispatcherQueue().TryEnqueue([weak=weak_from_this()]{if(auto self=weak.lock();self&&!self->closing){
        auto capacity=uint32_t(self->spec.Metric(L"thumbnail_capacity"));auto size=uint32_t(self->spec.Metric(L"thumbnail_size"));std::vector<uint8_t> pixels(size_t(capacity)*size*size*4);std::vector<uint32_t> rowIds(capacity);uint32_t count{};
        if(poc_canvas_thumbnails_readback(self->canvas,rowIds.data(),capacity,&count,pixels.data(),pixels.size())){pixels.resize(size_t(count)*size*size*4);rowIds.resize(count);self->Upload(std::move(pixels),std::move(rowIds));}else self->readbackPending=false;
    }});
}
void PocWindow::Upload(std::vector<uint8_t> pixels,std::vector<uint32_t> rowIds){
    try{
        auto size=uint32_t(spec.Metric(L"thumbnail_size"));auto area=size*size;std::vector<SoftwareBitmap> bitmaps;bitmaps.reserve(rowIds.size());
        for(uint32_t row=0;row<rowIds.size();++row){auto offset=size_t(row)*area*4;Buffer buffer(area*4);buffer.Length(area*4);uint8_t* destination{};check_hresult(buffer.as<::Windows::Storage::Streams::IBufferByteAccess>()->Buffer(&destination));
            for(size_t pixel=0;pixel<area;++pixel){destination[pixel*4]=pixels[offset+pixel*4+2];destination[pixel*4+1]=pixels[offset+pixel*4+1];destination[pixel*4+2]=pixels[offset+pixel*4];destination[pixel*4+3]=pixels[offset+pixel*4+3];}
            bitmaps.push_back(SoftwareBitmap::CreateCopyFromBuffer(buffer,BitmapPixelFormat::Bgra8,int32_t(size),int32_t(size),BitmapAlphaMode::Premultiplied));
        }
        dispatcher.TryEnqueue([weak=weak_from_this(),rowIds=std::move(rowIds),bitmaps=std::move(bitmaps)]{if(auto self=weak.lock();self&&!self->closing){for(uint32_t i=0;i<bitmaps.size();++i){auto found=self->tiles.find(rowIds[i]);if(found!=self->tiles.end())ShowTile(weak,found->second,bitmaps[i]);}self->deliveredThumbnailRows=rowIds;if(!self->options.GetNamedBoolean(L"thumbnails")&&self->visibleThumbnailRows==rowIds)self->thumbnailTimer.Stop();self->readbackPending=false;}});
    }catch(hresult_error const& error){dispatcher.TryEnqueue([weak=weak_from_this(),code=error.code()]{if(auto self=weak.lock()){self->readbackPending=false;self->Fail(code);}});}
}
}
