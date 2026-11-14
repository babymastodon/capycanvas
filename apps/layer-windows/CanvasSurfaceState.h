#pragma once
#include <cstdint>
#include <chrono>
#include <cmath>
#include <memory>

class CanvasSurfaceState {
    uint64_t minimum=0;
    bool attached=false;
public:
    void Reset(uint64_t generation){minimum=generation+1;attached=false;}
    bool CanAttach(uint64_t generation,bool ready)const{return !attached&&ready&&generation>=minimum;}
    void DidAttach(){attached=true;}
    bool Attached()const{return attached;}
    uint64_t MinimumGeneration()const{return minimum;}
};

class CanvasLaunchPaint {
    struct Frame {void const* surface;uint64_t generation;bool rendered=false;};
public:
    using Receipt=std::shared_ptr<Frame>;
    Receipt Request(void const* surface,uint64_t generation){
        if(pending&&pending->surface==surface&&pending->generation==generation)return {};
        pending=std::make_shared<Frame>(Frame{surface,generation});return pending;
    }
    void Cancel(){pending.reset();}
    bool Rendered(Receipt const& receipt){
        if(!receipt||receipt!=pending||receipt->rendered)return false;
        receipt->rendered=true;return true;
    }
    bool Presented(Receipt const& receipt,void const* surface,uint64_t generation){
        if(!receipt||receipt!=pending||!receipt->rendered||receipt->surface!=surface||receipt->generation!=generation)return false;
        pending.reset();return true;
    }
private:
    Receipt pending;
};

class CanvasLaunchTiming {
public:
    using Clock=std::chrono::steady_clock;
    static constexpr auto CaptionFadeDuration=std::chrono::milliseconds(180);
    explicit CanvasLaunchTiming(Clock::time_point start=Clock::now()):started(start){}
    std::chrono::milliseconds CaptionRemaining(Clock::time_point now,std::chrono::milliseconds delay)const {
        auto elapsed=std::chrono::duration_cast<std::chrono::milliseconds>(now-started);
        return elapsed<delay?delay-elapsed:std::chrono::milliseconds(0);
    }
private:
    Clock::time_point started;
};

struct CanvasLaunchPlacement {
    double left,top,size;
    CanvasLaunchPlacement(double width,double height,double markSize):left((width-markSize)/2),top((height-markSize)/2),size(markSize){}
    double CaptionTop(double gap,double inkBottom,double rasterHeight,double scale)const{
        auto bottom=top+size*(rasterHeight>0?std::ceil(inkBottom*rasterHeight)/rasterHeight:inkBottom);
        return (std::ceil(bottom*scale)+std::ceil(gap*scale))/scale;
    }
};
