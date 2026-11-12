//! Shared `layer_ui::Meter` readouts. The bar is one snapshot of rectangles with
//! a fixed height, so resizing a panel only redraws it.
use crate::tooltips::PenTooltips;
use gtk::{gdk, glib, graphene, gsk, prelude::*, subclass::prelude::*};
use layer_ui::{MeterTone, ThemePalette, Tip};
use std::{cell::RefCell, rc::Rc};

const BAR: f32 = 6.;
const HEIGHT: i32 = 12;

mod imp {
    use super::*;
    #[derive(Default)]
    pub struct Meter {
        pub value: RefCell<Option<(layer_ui::Meter, [gdk::RGBA; 2])>>,
    }
    #[glib::object_subclass]
    impl ObjectSubclass for Meter {
        const NAME: &'static str = "CapyMeter";
        type Type = super::Meter;
        type ParentType = gtk::Widget;
        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("meter");
            klass.set_accessible_role(gtk::AccessibleRole::Presentation);
        }
    }
    impl ObjectImpl for Meter {}
    impl WidgetImpl for Meter {
        fn measure(&self, orientation: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            if orientation == gtk::Orientation::Horizontal { (24, 24, -1, -1) } else { (HEIGHT, HEIGHT, -1, -1) }
        }
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let value = self.value.borrow();
            let Some((meter, [accent, over])) = value.as_ref() else { return };
            let obj = self.obj();
            let (width, height) = (obj.width() as f32, obj.height() as f32);
            let ink = obj.color();
            let tint = |mut color: gdk::RGBA, alpha: f32| { color.set_alpha(color.alpha() * alpha); color };
            let top = ((height - BAR) / 2.).round();
            let span = |from: f32, to: f32| {
                let [from, to] = [from, to].map(|at| at.clamp(0., 1.) * width);
                graphene::Rect::new(from, top, (to - from).max(0.), BAR)
            };
            snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(graphene::Rect::new(0., top, width, BAR), BAR / 2.));
            snapshot.append_color(&tint(ink, 0.12), &span(0., meter.limit));
            snapshot.append_color(&tint(ink, 0.06), &span(meter.limit, 1.));
            for segment in &meter.segments {
                let color = match segment.tone {
                    MeterTone::Fill => *accent,
                    MeterTone::Soft => tint(*accent, 0.4),
                    MeterTone::Rest => tint(ink, 0.32),
                    MeterTone::Over => *over,
                };
                snapshot.append_color(&color, &span(segment.from, segment.to));
            }
            snapshot.pop();
            if meter.limit < 0.999 {
                snapshot.append_color(&tint(ink, 0.6), &graphene::Rect::new((meter.limit * width).round() - 1., 0., 1., height));
            }
        }
    }
}

glib::wrapper! {
    pub struct Meter(ObjectSubclass<imp::Meter>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Meter {
    pub fn new() -> Self {
        let meter: Self = glib::Object::new();
        meter.set_hexpand(true);
        meter
    }
    pub fn set(&self, meter: &layer_ui::Meter, colors: [gdk::RGBA; 2]) {
        if self.imp().value.borrow().as_ref().is_some_and(|(shown, shown_colors)| shown == meter && *shown_colors == colors) { return; }
        *self.imp().value.borrow_mut() = Some((meter.clone(), colors));
        self.queue_draw();
    }
}

pub fn colors(palette: &ThemePalette) -> [gdk::RGBA; 2] {
    [palette.accent, palette.error].map(|color| {
        let [r, g, b] = color.0.map(|channel| f32::from(channel) / 255.);
        gdk::RGBA::new(r, g, b, 1.)
    })
}

pub fn badge() -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class("badge");
    label.set_visible(false);
    label
}

pub fn show_badge(label: &gtk::Label, chip: Option<&layer_ui::Chip>) {
    label.set_visible(chip.is_some());
    if let Some(chip) = chip {
        if label.label().as_str() != &*chip.label { label.set_label(&chip.label); }
        set_class(label, "alert", chip.warning);
    }
}

fn set_class(widget: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if widget.has_css_class(class) != on {
        if on { widget.add_css_class(class); } else { widget.remove_css_class(class); }
    }
}

struct Row {
    root: gtk::Box,
    name: gtk::Label,
    chip: gtk::Label,
    value: gtk::Label,
    meter: Meter,
    tip: Rc<RefCell<Tip>>,
}

impl Row {
    fn new(tooltips: &PenTooltips) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 3);
        root.add_css_class("meter-row");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, crate::panel_controls::CONTROL_SPACING);
        let lead = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        lead.set_hexpand(true);
        let name = gtk::Label::new(None);
        name.set_xalign(0.);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let chip = badge();
        chip.set_valign(gtk::Align::Center);
        lead.append(&name);
        lead.append(&chip);
        let value = gtk::Label::new(None);
        value.set_xalign(1.);
        value.add_css_class("numeric");
        line.append(&lead);
        line.append(&value);
        let meter = Meter::new();
        root.append(&line);
        root.append(&meter);
        let tip = Rc::new(RefCell::new(Tip::default()));
        tooltips.bind_tip(&root, {
            let tip = tip.clone();
            move |_| Some(tip.borrow().clone())
        });
        Self { root, name, chip, value, meter, tip }
    }

    fn update(&self, row: &layer_ui::MeterRow, colors: [gdk::RGBA; 2]) {
        if self.name.label().as_str() != row.label { self.name.set_label(&row.label); }
        if self.value.label().as_str() != row.value { self.value.set_label(&row.value); }
        set_class(&self.value, "alert", row.warning);
        show_badge(&self.chip, row.chip.as_ref());
        self.meter.set_visible(row.meter.is_some());
        if let Some(meter) = &row.meter { self.meter.set(meter, colors); }
        if *self.tip.borrow() != row.tip {
            *self.tip.borrow_mut() = row.tip.clone();
            self.root.update_property(&[gtk::accessible::Property::Description(&row.tip.plain())]);
        }
    }
}

pub struct MeterRows {
    pub root: gtk::Box,
    rows: RefCell<Vec<Row>>,
}

impl MeterRows {
    pub fn new(spacing: i32) -> Self {
        Self { root: gtk::Box::new(gtk::Orientation::Vertical, spacing), rows: RefCell::default() }
    }

    pub fn update(&self, rows: &[layer_ui::MeterRow], colors: [gdk::RGBA; 2], tooltips: &PenTooltips) {
        let mut views = self.rows.borrow_mut();
        while views.len() < rows.len() {
            let view = Row::new(tooltips);
            self.root.append(&view.root);
            views.push(view);
        }
        for view in views.drain(rows.len()..) { self.root.remove(&view.root); }
        for (view, row) in views.iter().zip(rows) { view.update(row, colors); }
    }
}

pub struct Section {
    pub root: gtk::Box,
    heading: gtk::Box,
    title: gtk::Label,
    note: gtk::Label,
    chip: gtk::Label,
    tip: Rc<RefCell<Tip>>,
    rows: MeterRows,
}

impl Section {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class("meter-section");
        let heading = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let title = gtk::Label::new(None);
        title.set_xalign(0.);
        title.add_css_class("meter-section-title");
        let note = gtk::Label::new(None);
        note.add_css_class("dim-label");
        note.set_hexpand(true);
        note.set_xalign(0.);
        note.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let chip = badge();
        heading.append(&title);
        heading.append(&note);
        heading.append(&chip);
        let rows = MeterRows::new(8);
        root.append(&heading);
        root.append(&rows.root);
        Self { root, heading, title, note, chip, tip: Rc::default(), rows }
    }

    pub fn update(&self, section: &layer_ui::MeterSection, colors: [gdk::RGBA; 2], tooltips: &PenTooltips) {
        if !self.heading.has_tooltip() {
            let tip = self.tip.clone();
            tooltips.bind_tip(&self.heading, move |_| Some(tip.borrow().clone()));
        }
        if self.title.label().as_str() != section.title { self.title.set_label(&section.title); }
        if self.note.label().as_str() != section.note { self.note.set_label(&section.note); }
        show_badge(&self.chip, section.chip.as_ref());
        if *self.tip.borrow() != section.tip { *self.tip.borrow_mut() = section.tip.clone(); }
        self.rows.update(&section.rows, colors, tooltips);
    }
}
