export function brushSizeGrid({ app, catalog, state, dispatch, element, button }) {
  const style = app.toolbar_ui({ type: "style", style: "small" }), grid = element("div", "size-grid");
  grid.style.setProperty("--size-tile", `${catalog.brush_size_tile[0]}px`);
  grid.style.setProperty("--size-height", `${catalog.brush_size_tile[1]}px`);
  grid.style.setProperty("--size-dot", `${catalog.brush_size_tile[0]}px`);
  grid.style.setProperty("--tile-radius", `${style.size[0] / 2}px`);
  grid.style.setProperty("--size-gap", `${style.gap}px`);
  const labels = [];
  const buttons = catalog.brush_sizes.map(({ value, label, preview_diameter }) => {
    const choice = button("", () => dispatch({ type: "set_brush_size", value }), "size-button");
    choice.title = `${label} px`;
    choice.setAttribute("aria-label", choice.title);
    choice.onpointerenter = () => { choice.title = app.action_tooltip(`${label} px`, { type: "set_brush_size", value }); };
    choice.dataset.size = label;
    const preview = element("span", "size-preview"), dot = element("span", "size-dot");
    dot.style.width = dot.style.height = `${preview_diameter}px`;
    const text = element("span", "size-label", label);
    labels.push(text);
    preview.append(dot); choice.append(preview, text);
    grid.append(choice);
    return [value, choice];
  });
  const resize = new ResizeObserver(() => {
    let width = 0, height = 0;
    for (const label of labels) {
      const rect = label.getBoundingClientRect();
      width = Math.max(width, rect.width); height = Math.max(height, rect.height);
    }
    const tile = app.toolbar_ui({ type: "brush_size_tile", label: [width, height] });
    grid.style.setProperty("--size-tile", `${tile[0]}px`);
    grid.style.setProperty("--size-height", `${tile[1]}px`);
  });
  for (const label of labels) resize.observe(label);
  grid.disposeEditor = () => resize.disconnect();
  grid.refresh = () => {
    const size = state().brush.diameter;
    for (const [value, choice] of buttons) {
      const pressed = String(value === size);
      if (choice.getAttribute("aria-pressed") !== pressed) choice.setAttribute("aria-pressed", pressed);
    }
  };
  grid.refresh();
  return grid;
}
