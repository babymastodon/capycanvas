fn main() {
    println!(":root {{ --launch-reveal: {}ms; --launch-mark-size: {}px; --launch-mark-fit: calc({} * min(100vw, 100dvh)); --launch-caption-gap: {}px; --launch-caption-size: {}px; }}",
        layer_ui::LAUNCH_REVEAL_MS, layer_ui::LAUNCH_MARK_SIZE,
        layer_ui::LAUNCH_MARK_MAX_FRACTION, layer_ui::LAUNCH_CAPTION_GAP,
        layer_ui::LAUNCH_CAPTION_SIZE);
}
