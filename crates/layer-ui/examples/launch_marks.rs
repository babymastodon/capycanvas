fn main() {
    for (icon, _) in layer_ui::ZenIcon::CHOICES {
        let [x, y, radius] = icon.launch_system_circle();
        println!("{} {x} {y} {radius}", icon.icon());
    }
}
