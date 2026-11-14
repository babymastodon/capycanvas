use super::*;
pub(super) use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_dmabuf_feedback_v1 as dmabuf_feedback, zwp_linux_dmabuf_v1 as dmabuf,
};

impl Child {
    pub fn compositor_gpu(&mut self) -> Result<Option<String>, String> {
        let Some(dmabuf) = self.dmabuf.take() else { return Ok(None) };
        let feedback = dmabuf.get_default_feedback(&self.events.handle(), ());
        dmabuf.destroy();
        let received = self.events.roundtrip(&mut self.state).map_err(error);
        feedback.destroy();
        received?;
        Ok(self.state.compositor_device.take().and_then(pci_slot))
    }
}

fn pci_slot(device: Vec<u8>) -> Option<String> {
    let device = libc::dev_t::from_ne_bytes(device.try_into().ok()?);
    std::fs::read_to_string(format!("/sys/dev/char/{}:{}/device/uevent", libc::major(device), libc::minor(device)))
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("PCI_SLOT_NAME="))
        .filter(|slot| !slot.is_empty())
        .map(str::to_owned)
}

impl Dispatch<dmabuf_feedback::ZwpLinuxDmabufFeedbackV1, ()> for Events {
    fn event(state: &mut Self, _: &dmabuf_feedback::ZwpLinuxDmabufFeedbackV1, event: dmabuf_feedback::Event,
        _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let dmabuf_feedback::Event::MainDevice { device } = event {
            state.compositor_device = Some(device);
        }
    }
}
wayland_client::delegate_noop!(Events: ignore dmabuf::ZwpLinuxDmabufV1);
