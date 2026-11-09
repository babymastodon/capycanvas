#pragma once
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct Canvas Canvas;
typedef struct PocPenSample {
    float x,y,pressure,tilt_x,tilt_y,twist;
    uint64_t timestamp;
    uint32_t flags;
} PocPenSample;
typedef struct PocGlassRect {
    float x,y,width,height;
    uint64_t revision;
} PocGlassRect;
__declspec(dllimport) Canvas* poc_canvas_create_for_panel(uint32_t width,uint32_t height,float scale,const char* trace_path,void** surface_handle);
__declspec(dllimport) bool poc_canvas_resize(Canvas*,uint32_t width,uint32_t height,float scale);
__declspec(dllimport) bool poc_canvas_pen(Canvas*,const PocPenSample*,size_t count,int64_t receipt_qpc);
__declspec(dllimport) bool poc_canvas_set_glass(Canvas*,const PocGlassRect*,size_t count);
__declspec(dllimport) bool poc_canvas_set_thumbnail_rows(Canvas*,uint32_t first,uint32_t count);
__declspec(dllimport) bool poc_canvas_set_load(Canvas*,bool slow);
__declspec(dllimport) bool poc_canvas_set_motion(Canvas*,bool motion);
__declspec(dllimport) bool poc_canvas_thumbnails_readback(Canvas*,uint32_t* first,uint32_t* count,uint8_t* out,size_t capacity);
__declspec(dllimport) const char* poc_spec_json(void);
__declspec(dllimport) size_t poc_spec_label(uint32_t index,char* out,size_t capacity);
__declspec(dllimport) bool poc_canvas_set_thumbnails(Canvas*,bool on);
__declspec(dllimport) bool poc_canvas_shutdown(Canvas*);
__declspec(dllimport) uint32_t poc_canvas_status(Canvas*);
__declspec(dllimport) bool poc_canvas_trace_open(Canvas*,const char* path);
__declspec(dllimport) void poc_canvas_destroy(Canvas*);
#ifdef __cplusplus
}
#endif
