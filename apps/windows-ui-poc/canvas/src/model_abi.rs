use poc_common::{AppOptions,model::{Model,UiState}};
use std::ffi::{CStr,c_char};

#[cfg(windows)]
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_signal_ready(name:*const c_char)->bool {
    if name.is_null(){return false;}
    let Ok(name)=unsafe{CStr::from_ptr(name)}.to_str() else{return false;};
    poc_common::ready::signal(name).is_ok()
}

unsafe fn write_text(text:&str,out:*mut c_char,capacity:usize)->usize {
    let needed=text.len()+1;
    if !out.is_null() && capacity>=needed {unsafe{std::ptr::copy_nonoverlapping(text.as_ptr(),out.cast(),text.len());*out.add(text.len())=0;}}
    needed
}
#[unsafe(no_mangle)]
extern "C" fn poc_model_create(rows:u32)->*mut Model {Model::new(rows).map(|model|Box::into_raw(Box::new(model))).unwrap_or(std::ptr::null_mut())}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_model_destroy(model:*mut Model) {if !model.is_null() {drop(unsafe{Box::from_raw(model)});}}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_model_state(model:*const Model)->UiState {unsafe{model.as_ref()}.map(Model::state).unwrap_or_default()}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_model_set_slider(model:*mut Model,index:u32,value:f64)->bool {unsafe{model.as_mut()}.is_some_and(|model|model.set_slider(index,value))}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_model_set_number(model:*mut Model,value:f64)->bool {unsafe{model.as_mut()}.is_some_and(|model|model.set_number(value))}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_model_set_text(model:*mut Model,text:*const c_char,length:usize)->bool {
    if length>1_048_576 || (text.is_null() && length!=0) {return false;}
    let text=if length==0 {""} else {let Ok(text)=std::str::from_utf8(unsafe{std::slice::from_raw_parts(text.cast(),length)}) else{return false;};text};
    unsafe{model.as_mut()}.is_some_and(|model|model.set_text(text))
}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_model_text(model:*const Model,out:*mut c_char,capacity:usize)->usize {unsafe{model.as_ref()}.map_or(0,|model|unsafe{write_text(model.text(),out,capacity)})}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_model_slider_text(model:*const Model,index:u32,out:*mut c_char,capacity:usize)->usize {unsafe{model.as_ref()}.map_or(0,|model|unsafe{write_text(&model.slider_text(index),out,capacity)})}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_model_set_divider(model:*mut Model,top:f32,height:f32)->bool {unsafe{model.as_mut()}.is_some_and(|model|model.set_divider(top,height))}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_model_activate(model:*mut Model,action:u32,index:u32)->bool {unsafe{model.as_mut()}.is_some_and(|model|model.activate(action,index))}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_model_visible_row(model:*const Model,ordinal:u32)->u32 {unsafe{model.as_ref()}.and_then(|model|model.visible_row(ordinal)).unwrap_or(u32::MAX)}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_model_row_visible(model:*const Model,row:u32)->bool {unsafe{model.as_ref()}.is_some_and(|model|model.row_visible(row))}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_spec_row_id(row:u32,visibility:bool,out:*mut c_char,capacity:usize)->usize {
    if row>65_535 {return 0;}
    unsafe{write_text(&if visibility {poc_common::visibility_id(row as usize)}else{poc_common::row_id(row as usize)},out,capacity)}
}
#[unsafe(no_mangle)]
extern "C" fn poc_spec_ruler_color(row:u32)->u32 {
    let [r,g,b]=poc_common::ruler_rgb(row as u16);((r as u32)<<16)|((g as u32)<<8)|b as u32
}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_app_options_json(args_json:*const c_char,out:*mut c_char,capacity:usize)->usize {
    let result=(||->Result<AppOptions,String>{
        let args=if args_json.is_null(){"[]"}else{unsafe{CStr::from_ptr(args_json)}.to_str().map_err(|error|error.to_string())?};
        let args:Vec<String>=serde_json::from_str(args).map_err(|error|error.to_string())?;
        AppOptions::parse(&args)
    })();
    let json=match result {Ok(options)=>serde_json::json!({"ok":true,"options":options}),Err(error)=>serde_json::json!({"ok":false,"error":error})};
    unsafe{write_text(&json.to_string(),out,capacity)}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_text_bridge_preserves_utf8_and_respects_buffer_capacity() {
        unsafe {
            let model=poc_model_create(1000);assert!(!model.is_null());
            let text="日本語";assert!(poc_model_set_text(model,text.as_ptr().cast(),text.len()));
            let mut small=[23i8;2];assert_eq!(poc_model_text(model,small.as_mut_ptr(),small.len()),10);assert_eq!(small,[23;2]);
            let mut out=[0i8;10];assert_eq!(poc_model_text(model,out.as_mut_ptr(),out.len()),10);
            assert_eq!(CStr::from_ptr(out.as_ptr()).to_str().unwrap(),text);
            assert!(!poc_model_set_text(model,[255u8].as_ptr().cast(),1));assert_eq!(poc_model_state(model).visible_rows,15);
            poc_model_destroy(model);
        }
    }
    #[test]
    fn native_options_reject_invalid_row_counts_in_shared_validation() {
        unsafe {
            let args=c"[\"--rows\",\"0\"]";let mut out=vec![0i8;poc_app_options_json(args.as_ptr(),std::ptr::null_mut(),0)];
            poc_app_options_json(args.as_ptr(),out.as_mut_ptr(),out.len());
            let result:serde_json::Value=serde_json::from_str(CStr::from_ptr(out.as_ptr()).to_str().unwrap()).unwrap();
            assert_eq!(result["ok"],false);assert!(poc_model_create(0).is_null());
        }
    }
}
