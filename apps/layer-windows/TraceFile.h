#pragma once
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include <winrt/base.h>
#include <filesystem>
#include <fstream>
#include <cstring>
#include <memory>
#include <mutex>
#include <string>
#include <vector>

inline void WriteTraceFile(std::filesystem::path const& name,std::string const& value){
    auto pending=name.wstring()+L".pending";
    {std::ofstream stream(pending);stream<<value;if(!stream)return;}
    auto target=std::filesystem::absolute(name).wstring();
    auto size=sizeof(FILE_RENAME_INFO)+target.size()*sizeof(wchar_t);
    std::vector<FILE_RENAME_INFO> storage((size+sizeof(FILE_RENAME_INFO)-1)/sizeof(FILE_RENAME_INFO));
    auto info=storage.data();
    info->Flags=FILE_RENAME_FLAG_REPLACE_IF_EXISTS|FILE_RENAME_FLAG_POSIX_SEMANTICS;
    info->FileNameLength=static_cast<DWORD>(target.size()*sizeof(wchar_t));
    std::memcpy(info->FileName,target.c_str(),(target.size()+1)*sizeof(wchar_t));
    auto source=CreateFileW(pending.c_str(),DELETE,FILE_SHARE_READ|FILE_SHARE_WRITE|FILE_SHARE_DELETE,nullptr,OPEN_EXISTING,FILE_ATTRIBUTE_NORMAL,nullptr);
    auto published=source!=INVALID_HANDLE_VALUE&&SetFileInformationByHandle(source,FileRenameInfoEx,info,static_cast<DWORD>(size));
    auto error=GetLastError();
    if(source!=INVALID_HANDLE_VALUE)CloseHandle(source);
    if(!published){
        DeleteFileW(pending.c_str());
        OutputDebugStringW((std::wstring(L"Capy trace replacement failed: ")+std::to_wstring(error)+L"\n").c_str());
    }
}
struct OrderedTraceFile {std::mutex mutex;std::filesystem::path path;std::string pending;bool writing=false;};
inline void PublishTraceFile(std::shared_ptr<OrderedTraceFile> const& file,std::filesystem::path path,std::string payload){
    {std::lock_guard lock(file->mutex);file->path=std::move(path);file->pending=std::move(payload);if(file->writing)return;file->writing=true;}
    [](std::shared_ptr<OrderedTraceFile> file)->winrt::fire_and_forget{
        co_await winrt::resume_background();
        for(;;){
            std::filesystem::path path;std::string payload;
            {std::lock_guard lock(file->mutex);if(file->pending.empty()){file->writing=false;co_return;}path=file->path;payload=std::move(file->pending);file->pending.clear();}
            try{WriteTraceFile(path,payload);}catch(...){OutputDebugStringW((L"Capy trace publication failed: "+path.wstring()+L"\n").c_str());}
        }
    }(file);
}
