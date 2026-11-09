pub fn client_at_origin(outer:[i32;4],client_size:[i32;2],client_origin:[i32;2],target:[i32;2])->[i32;4] {
    [outer[0]-client_origin[0],outer[1]-client_origin[1],target[0]+outer[2]-outer[0]-client_size[0],target[1]+outer[3]-outer[1]-client_size[1]]
}
pub fn application_window(expected_pid:u32,pid:u32,title:&str,visible:bool,ownerless:bool)->bool {
    pid==expected_pid && title==poc_common::TITLE && visible && ownerless
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn placement_uses_actual_client_insets_for_standard_and_extended_frames() {
        assert_eq!(client_at_origin([100,100,1716,1147],[1600,1000],[108,139],[2400,1500]),[-8,-39,2416,1547]);
        assert_eq!(client_at_origin([100,100,1716,1116],[1600,1000],[108,108],[2400,1500]),[-8,-8,2416,1516]);
        assert_eq!(client_at_origin([100,100,1700,1100],[1600,1000],[100,100],[2400,1500]),[0,0,2400,1500]);
        assert_eq!(client_at_origin([100,100,1700,1101],[1600,1000],[100,101],[2400,1500]),[0,-1,2400,1501]);
    }
    #[test]
    fn an_owned_input_indicator_before_the_app_is_not_the_capture_or_input_target() {
        let windows=[(7444,"Input Indicator overlay window",true,true),(7444,poc_common::TITLE,true,true)];
        let selected=windows.iter().position(|(pid,title,visible,ownerless)|application_window(7444,*pid,title,*visible,*ownerless));
        assert_eq!(selected,Some(1));
        assert!(!application_window(7444,7445,poc_common::TITLE,true,true));
        assert!(!application_window(7444,7444,poc_common::TITLE,true,false));
        assert!(!application_window(7444,7444,poc_common::TITLE,false,true));
    }
}
