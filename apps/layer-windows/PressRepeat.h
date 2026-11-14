#pragma once
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <optional>

namespace CapyUi {
class PressRepeat {
    struct Press {uint64_t timestamp;double x,y;uint32_t target;};
    std::optional<Press> last;
public:
    struct Limits {uint64_t interval;double slop;};
    static Limits System(double scale){
        return {uint64_t(GetDoubleClickTime())*1000,std::max(8.,double(std::max(GetSystemMetrics(SM_CXDOUBLECLK),GetSystemMetrics(SM_CYDOUBLECLK)))/scale)};
    }
    bool Next(Limits limits,uint64_t timestamp,double x,double y,uint32_t target=0){
        bool repeated=last&&last->target==target&&timestamp-last->timestamp<=limits.interval&&std::hypot(x-last->x,y-last->y)<=limits.slop;
        if(repeated)last.reset();else last=Press{timestamp,x,y,target};
        return repeated;
    }
    void Reset(){last.reset();}
};
}
