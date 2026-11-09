#pragma once
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct Canvas Canvas;
typedef struct PocModel PocModel;
typedef struct PocUiState {
    double sliders[4];
    double number;
    float divider_fraction;
    uint32_t rows,visible_rows,tab,group_tab,menu_item;
    uint64_t revision;
    bool drawer_open,zen_visible;
} PocUiState;
enum PocAction { POC_TAB=0,POC_GROUP_TAB=1,POC_DRAWER=2,POC_ZEN=3,POC_ROW_VISIBILITY=4,POC_MENU=5 };
typedef struct PocPenSample {
    float x,y,pressure,tilt_x,tilt_y,twist;
    uint64_t timestamp;
    uint32_t flags;
} PocPenSample;
typedef struct PocGlassRect {
    float x,y,width,height;
    uint64_t revision;
} PocGlassRect;
typedef struct PocGlassAnimation {
    PocGlassRect from_rect,to_rect;
    float from_opacity,to_opacity;
    uint32_t rect_index;
    int64_t start_qpc;
    uint32_t duration_ms,easing;
} PocGlassAnimation;
#define POC_GLASS_OPACITY UINT32_MAX
enum PocGlassEasing { POC_GLASS_LINEAR=0, POC_GLASS_CUBIC_OUT=1 };
typedef void (*PocCanvasReady)(void* context,bool ready);
__declspec(dllimport) bool poc_signal_ready(const char* name);
__declspec(dllimport) Canvas* poc_canvas_create_for_panel(uint32_t width,uint32_t height,float scale,const char* trace_path,void** surface_handle,PocCanvasReady configured,PocCanvasReady presented,void* context);
__declspec(dllimport) bool poc_canvas_start_panel(Canvas* canvas);
__declspec(dllimport) bool poc_canvas_resize(Canvas*,uint32_t width,uint32_t height,float scale);
__declspec(dllimport) bool poc_canvas_pen(Canvas*,const PocPenSample*,size_t count,int64_t receipt_qpc);
__declspec(dllimport) bool poc_canvas_set_glass(Canvas*,const PocGlassRect*,size_t count);
__declspec(dllimport) bool poc_canvas_animate_glass(Canvas*,const PocGlassAnimation*);
__declspec(dllimport) bool poc_canvas_set_thumbnail_rows(Canvas*,const uint32_t* row_ids,uint32_t count);
__declspec(dllimport) bool poc_canvas_set_load(Canvas*,bool slow);
__declspec(dllimport) bool poc_canvas_set_motion(Canvas*,bool motion);
__declspec(dllimport) bool poc_canvas_thumbnails_readback(Canvas*,uint32_t* row_ids,uint32_t row_capacity,uint32_t* count,uint8_t* out,size_t capacity);
__declspec(dllimport) const char* poc_spec_json(void);
__declspec(dllimport) size_t poc_spec_label(uint32_t index,char* out,size_t capacity);
__declspec(dllimport) bool poc_canvas_set_thumbnails(Canvas*,bool on);
__declspec(dllimport) bool poc_canvas_shutdown(Canvas*);
__declspec(dllimport) uint32_t poc_canvas_status(Canvas*);
__declspec(dllimport) bool poc_canvas_trace_open(Canvas*,const char* path);
__declspec(dllimport) bool poc_canvas_trace_record(Canvas*,const char* utf8_json,size_t length);
__declspec(dllimport) bool poc_canvas_destroy(Canvas*);
__declspec(dllimport) PocModel* poc_model_create(uint32_t rows);
__declspec(dllimport) void poc_model_destroy(PocModel*);
__declspec(dllimport) PocUiState poc_model_state(const PocModel*);
__declspec(dllimport) bool poc_model_set_slider(PocModel*,uint32_t index,double value);
__declspec(dllimport) bool poc_model_set_number(PocModel*,double value);
__declspec(dllimport) bool poc_model_set_text(PocModel*,const char* utf8,size_t length);
__declspec(dllimport) size_t poc_model_text(const PocModel*,char* out,size_t capacity);
__declspec(dllimport) size_t poc_model_slider_text(const PocModel*,uint32_t index,char* out,size_t capacity);
__declspec(dllimport) bool poc_model_set_divider(PocModel*,float top_logical,float column_height_logical);
__declspec(dllimport) bool poc_model_activate(PocModel*,uint32_t action,uint32_t index);
__declspec(dllimport) uint32_t poc_model_visible_row(const PocModel*,uint32_t ordinal);
__declspec(dllimport) bool poc_model_row_visible(const PocModel*,uint32_t row);
__declspec(dllimport) size_t poc_spec_row_id(uint32_t row,bool visibility,char* out,size_t capacity);
__declspec(dllimport) uint32_t poc_spec_ruler_color(uint32_t row);
__declspec(dllimport) size_t poc_app_options_json(const char* args_json,char* out,size_t capacity);
#ifdef __cplusplus
}
#endif
