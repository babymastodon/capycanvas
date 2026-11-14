#pragma once
#include <algorithm>
#include <cmath>

namespace CapyBubbles {
struct Tween {
    double value=0,target=0,from=0,start=-1,duration=0,delay=0;
    bool smooth=false;
    explicit Tween(double initial=0,bool smooth=false):value(initial),target(initial),from(initial),smooth(smooth){}
    void snap(double next){value=target=from=next;start=-1;}
    void to(double next,double ms,double wait=0){if(target==next)return;from=value;target=next;duration=ms;delay=wait;start=-1;}
    bool step(double now,bool reduced){if(value==target)return false;if(start<0)start=now;
        auto t=reduced||duration==0?1.:std::clamp((now-start-delay)/duration,0.,1.);value=t<1?from+(target-from)*(smooth?t*t*(3-2*t):1-std::pow(1-t,3)):target;return true;}
};
struct MotionTicks {
    bool polling=false,busy=false;
    template<class Advance,class Request> bool tick(double now,Advance&& advance,Request&& request){
        bool animating=advance(now);
        if(polling&&!busy)busy=request();
        return animating||polling||busy;
    }
    void replied(bool moving){busy=false;polling=moving;}
};
}
