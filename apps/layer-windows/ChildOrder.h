#pragma once
#include <cstdint>

template<typename Items,typename Item,typename Pinned>
void PlaceChild(Items& items,uint32_t index,Item const& item,Pinned const& pinned){
    uint32_t at=0;
    if(index<items.Size()&&items.GetAt(index)==item)return;
    if(!items.IndexOf(item,at)){items.InsertAt(index,item);return;}
    if(!pinned(item)){items.RemoveAt(at);items.InsertAt(index,item);return;}
    for(;at>index;--at){auto other=items.GetAt(index);items.RemoveAt(index);items.InsertAt(at,other);}
}
