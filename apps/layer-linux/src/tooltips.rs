//! GTK 4.22 re-queries the seat's mouse position after its tooltip timeout,
//! even for tablet events. Use the pen's picked widget without moving the mouse.
use adw::prelude::*;
use gtk::{gdk, glib};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

type Content = Rc<dyn Fn(&gtk::Widget) -> Option<layer_ui::Tip>>;
#[derive(Default)]
pub(crate) struct PenTooltips {
    pending: RefCell<Option<glib::SourceId>>,
    source: RefCell<Option<(gtk::Widget, gtk::Widget)>>,
    popup: RefCell<Option<gtk::Popover>>,
    providers: RefCell<Vec<(glib::WeakRef<gtk::Widget>, Content)>>,
    suppressed: RefCell<Vec<glib::WeakRef<gtk::Widget>>>,
    pen_active: Cell<bool>,
    watchers: RefCell<Vec<(gtk::Widget, glib::SignalHandlerId)>>,
}

impl PenTooltips {
    pub fn bind(
        &self,
        widget: &impl IsA<gtk::Widget>,
        text: impl Fn(&gtk::Widget) -> Option<String> + 'static,
    ) {
        self.bind_tip(widget, move |widget| text(widget).map(layer_ui::Tip::text));
    }

    pub fn bind_tip(
        &self,
        widget: &impl IsA<gtk::Widget>,
        tip: impl Fn(&gtk::Widget) -> Option<layer_ui::Tip> + 'static,
    ) {
        let widget = widget.as_ref();
        let tip: Content = Rc::new(tip);
        widget.set_has_tooltip(true);
        widget.connect_query_tooltip({
            let tip = tip.clone();
            let shown = RefCell::new(None::<(layer_ui::Tip, gtk::Widget)>);
            move |widget, _, _, _, tooltip| {
                let Some(tip) = tip(widget).filter(|tip| !tip.is_empty()) else {
                    return false;
                };
                if tip.lines.is_empty() {
                    tooltip.set_text(Some(&tip.text));
                    return true;
                }
                let mut shown = shown.borrow_mut();
                if shown.as_ref().is_none_or(|(current, _)| *current != tip) {
                    let content = view(&tip);
                    *shown = Some((tip, content));
                }
                tooltip.set_custom(shown.as_ref().map(|(_, content)| content));
                true
            }
        });
        let mut providers = self.providers.borrow_mut();
        providers.retain(|(w, _)| w.upgrade().is_some_and(|w| w != *widget));
        providers.push((widget.downgrade(), tip));
    }

    pub fn install(self: &Rc<Self>, window: &impl IsA<gtk::Window>) {
        let window = window.as_ref();
        let events = gtk::EventControllerLegacy::new();
        events.set_name(Some("pen-tooltips"));
        events.set_propagation_phase(gtk::PropagationPhase::Capture);
        events.set_propagation_limit(gtk::PropagationLimit::None);
        events.connect_event(glib::clone!(
            #[weak(rename_to = tips)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, event| {
                // GTK synthesizes mouse motion after layout changes. It must
                // not retire the actual pen's hover (native tooltips skip it too).
                if event.event_type() == gdk::EventType::MotionNotify
                    && event.time() == gdk::CURRENT_TIME
                {
                    return glib::Propagation::Proceed;
                }
                let pen = event.device_tool().is_some()
                    || event
                        .device()
                        .is_some_and(|d| d.source() == gdk::InputSource::Pen);
                let pressed = event.modifier_state().intersects(
                    gdk::ModifierType::BUTTON1_MASK
                        | gdk::ModifierType::BUTTON2_MASK
                        | gdk::ModifierType::BUTTON3_MASK
                        | gdk::ModifierType::BUTTON4_MASK
                        | gdk::ModifierType::BUTTON5_MASK,
                );
                if pen
                    && !pressed
                    && matches!(
                        event.event_type(),
                        gdk::EventType::MotionNotify
                            | gdk::EventType::EnterNotify
                            | gdk::EventType::ProximityIn
                    )
                {
                    if let Some((native, point)) = event
                        .surface()
                        .and_then(|surface| gtk::Native::for_surface(&surface))
                        .and_then(|native| {
                            let (x, y) = event.position()?;
                            let (dx, dy) = native.surface_transform();
                            Some((native.dynamic_cast::<gtk::Widget>().ok()?, [x - dx, y - dy]))
                        })
                    {
                        tips.hover(&native, point);
                    }
                } else if !matches!(
                    event.event_type(),
                    gdk::EventType::MotionNotify | gdk::EventType::EnterNotify
                ) || !pen
                    || pressed
                {
                    tips.hide();
                    if !pen || event.event_type() == gdk::EventType::ProximityOut {
                        tips.restore_mouse();
                    }
                }
                glib::Propagation::Proceed
            }
        ));
        window.add_controller(events);
        window.connect_is_active_notify(glib::clone!(
            #[weak(rename_to = tips)]
            self,
            move |window| {
                if !window.is_active() {
                    tips.hide();
                    tips.restore_mouse();
                }
            }
        ));
        window.connect_unmap(glib::clone!(
            #[weak(rename_to = tips)]
            self,
            move |_| {
                tips.hide();
                tips.restore_mouse();
            }
        ));
    }

    fn content(&self, widget: &gtk::Widget) -> Option<layer_ui::Tip> {
        let provider = self
            .providers
            .borrow()
            .iter()
            .find(|(w, _)| w.upgrade().as_ref() == Some(widget))
            .map(|(_, p)| p.clone());
        match provider {
            Some(provider) => provider(widget),
            None => widget.tooltip_text().map(|text| layer_ui::Tip::text(text)),
        }
        .filter(|tip| !tip.is_empty())
    }

    // A parked mouse must not show a second, unrelated native tooltip while
    // the pen is browsing. Restore native mouse tooltips on mouse input/exit.
    fn suppress_mouse(&self, native: &gtk::Widget) {
        let Some(device) = native.display().default_seat().and_then(|s| s.pointer()) else {
            return;
        };
        let (Some(surface), x, y) = device.surface_at_position() else {
            return;
        };
        let Some(root) = gtk::Native::for_surface(&surface) else {
            return;
        };
        if root.root() != native.root() {
            return;
        }
        let (dx, dy) = root.surface_transform();
        let root = root.dynamic_cast::<gtk::Widget>().unwrap();
        let mut picked = root.pick(x - dx, y - dy, gtk::PickFlags::INSENSITIVE);
        while let Some(widget) = picked {
            if widget.has_tooltip() {
                self.suppressed.borrow_mut().push(widget.downgrade());
                widget.set_has_tooltip(false);
            }
            picked = widget.parent();
        }
        root.trigger_tooltip_query();
    }

    fn restore_mouse(&self) {
        self.pen_active.set(false);
        for widget in self
            .suppressed
            .take()
            .into_iter()
            .filter_map(|w| w.upgrade())
        {
            widget.set_has_tooltip(true);
        }
    }

    pub(crate) fn hover(self: &Rc<Self>, native: &gtk::Widget, point: [f64; 2]) {
        if !self.pen_active.replace(true) {
            self.suppress_mouse(native);
        }
        let mut picked = native.pick(point[0], point[1], gtk::PickFlags::INSENSITIVE);
        let target = loop {
            let Some(widget) = picked else {
                self.hide();
                return;
            };
            let suppressed = self
                .suppressed
                .borrow()
                .iter()
                .any(|w| w.upgrade().as_ref() == Some(&widget));
            if (widget.has_tooltip() || suppressed) && self.content(&widget).is_some() {
                break widget;
            }
            picked = widget.parent();
        };
        if self
            .source
            .borrow()
            .as_ref()
            .is_some_and(|(s, p)| *s == target && s.parent().as_ref() == Some(p))
        {
            return;
        }
        self.hide();
        self.suppress_mouse(native);
        let Some(parent) = target.parent() else {
            return;
        };
        // Retire both pending and visible tooltips when their anchor changes.
        let unmapped = target.connect_unmap(glib::clone!(
            #[weak(rename_to = tips)]
            self,
            move |_| tips.hide()
        ));
        let reparented = target.connect_parent_notify(glib::clone!(
            #[weak(rename_to = tips)]
            self,
            move |_| tips.hide()
        ));
        self.watchers.borrow_mut().extend(
            [unmapped, reparented]
                .into_iter()
                .map(|id| (target.clone(), id)),
        );
        *self.source.borrow_mut() = Some((target, parent));
        *self.pending.borrow_mut() = Some(glib::timeout_add_local_once(
            Duration::from_millis(500),
            glib::clone!(
                #[weak(rename_to = tips)]
                self,
                move || {
                    tips.pending.borrow_mut().take();
                    tips.show();
                }
            ),
        ));
    }

    fn show(self: &Rc<Self>) {
        let Some((source, parent)) = self.source.borrow().clone() else {
            return;
        };
        if !source.is_mapped() || source.parent().as_ref() != Some(&parent) {
            self.hide();
            return;
        }
        let Some(tip) = self.content(&source) else {
            self.hide();
            return;
        };
        let Some(native) = source
            .native()
            .and_then(|n| n.dynamic_cast::<gtk::Widget>().ok())
        else {
            return;
        };
        let Some(bounds) = source.compute_bounds(&native) else {
            return;
        };
        self.suppress_mouse(&native);
        let popup = gtk::Popover::builder()
            .accessible_role(gtk::AccessibleRole::Tooltip)
            .autohide(false)
            .has_arrow(false)
            .position(gtk::PositionType::Bottom)
            .child(&view(&tip))
            .build();
        popup.add_css_class("pen-tooltip");
        popup.set_widget_name("pen-tooltip");
        popup.set_can_target(false);
        popup.set_focusable(false);
        popup.set_offset(0, 4);
        popup.set_pointing_to(Some(&gdk::Rectangle::new(
            bounds.x().floor() as i32,
            bounds.y().floor() as i32,
            bounds.width().ceil() as i32,
            bounds.height().ceil() as i32,
        )));
        popup.set_parent(&native);
        *self.popup.borrow_mut() = Some(popup.clone());
        popup.popup();
    }

    pub(crate) fn hide(&self) {
        if let Some(timer) = self.pending.take() {
            timer.remove();
        }
        for (widget, id) in self.watchers.take() {
            widget.disconnect(id);
        }
        self.source.borrow_mut().take();
        if let Some(popup) = self.popup.take() {
            popup.popdown();
            popup.unparent();
        }
    }
}

impl Drop for PenTooltips {
    fn drop(&mut self) {
        self.hide();
        self.restore_mouse();
    }
}

fn view(tip: &layer_ui::Tip) -> gtk::Widget {
    let text = gtk::Label::new(Some(&tip.text));
    text.set_wrap(true);
    text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    text.set_max_width_chars(40);
    if tip.lines.is_empty() {
        return text.upcast();
    }
    text.set_xalign(0.);
    let grid = gtk::Grid::builder().column_spacing(16).row_spacing(4).build();
    for (row, line) in (0..).zip(&tip.lines) {
        let label = gtk::Label::new(Some(&line.label));
        label.set_xalign(0.);
        label.set_valign(gtk::Align::Start);
        label.add_css_class("tip-key");
        let value = gtk::Label::new(Some(&line.value));
        if tip.figures {
            value.set_xalign(1.);
            value.add_css_class("tip-figure");
        } else {
            value.set_xalign(0.);
            value.set_wrap(true);
            value.set_max_width_chars(38);
        }
        grid.attach(&label, 0, row, 1, 1);
        grid.attach(&value, 1, row, 1, 1);
    }
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    text.set_visible(!tip.text.is_empty());
    content.append(&text);
    content.append(&grid);
    content.upcast()
}
