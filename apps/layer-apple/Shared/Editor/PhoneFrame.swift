import SwiftUI

struct PhoneFrame: View {
    @ObservedObject var store: EditorStore
    @ObservedObject var input: PhoneBarInput
    private var layout: JSON { store.snapshot["layout"]["phone"] }
    private var tiles: [JSON] { store.state["workspace"]["layout"]["phone"].array }
    private var palette: EditorPalette { EditorPalette(source: store.state["palette"]) }
    private var sources: [DrawerSource] { Array(store.contentDrawers.sources.values).filter { $0.anchor["kind"].string == "phone" } }
    var body: some View {
        ZStack(alignment: .topLeading) {
            ForEach(layout["bars"].array.indices, id: \.self) { index in
                Color.clear.contentShape(Rectangle()).placed(layout["bars"][index])
                    .accessibilityIdentifier("phone-bar-\(index)")
            }
            ForEach(layout["chips"].array.indices, id: \.self) { index in
                let bounds = layout["chips"][index]
                Color.clear.glassSurface(SquircleShape(layout["tile"].number / 2,
                    square: DrawerSource.square(bounds.rect, sources: sources)), fill: palette.chromeSurface)
                    .placed(bounds).allowsHitTesting(false).accessibilityIdentifier("phone-chip-\(index)")
            }
            ForEach(Array(tiles.enumerated()), id: \.element["id"].uint) { index, tile in
                let lifted = !input.preview["lifted"].isNull && input.preview["lifted"].number == Double(index)
                let visual = input.preview["geometry"][index].isNull ? layout["tools"][index] : input.preview["geometry"][index]
                let divider = tile["control"]["kind"].string == "divider"
                let hit = lifted || divider ? visual : layout["tool_hits"][index]
                if divider {
                    ToolbarDivider(vertical: layout["edge"].string != "top", color: palette["text"].opacity(0.3))
                        .placed(visual).allowsHitTesting(false)
                } else {
                    tileButton(store.snapshot["phone_tools"][index], visual: visual, hit: hit,
                        anchor: JSON(["kind": "tool", "id": tile["id"].raw]), id: "phone-tool-\(tile["id"].uint)") {
                        guard input.acceptsClick else { return }
                        let model = store.snapshot["phone_tools"][index]
                        ToolActivation.activate(model["resolved_control"], anchor: ["kind": "phone", "anchor": ["kind": "tool", "id": tile["id"].raw]],
                            doubleClick: model["double_click"].bool, store: store) {
                            input.click(["kind": "tool", "tile": tile["id"].raw])
                        }
                    }.tracksToolActivation().zIndex(lifted ? 1 : 0)
                }
            }
            tileButton(store.snapshot["phone_settings"], visual: layout["settings"], hit: layout["settings_hit"],
                anchor: JSON(["kind": "tool_settings"]), id: "phone-tool-settings") { input.click(["kind": "tool_settings"]) }
            tileButton(JSON(["label": store.snapshot["capy_caption"].raw, "icon": store.snapshot["header"]["capy"]["icon"].raw,
                "enabled": true]), visual: layout["capy"], hit: layout["capy_target"], anchor: JSON(["kind": "capy"]),
                id: "phone-capy", icon: layout["capy_icon"].number) { input.click(["kind": "capy"]) }
            ForEach(0..<2) { index in
                let name = index == 0 ? "size" : "strength", bounds = layout[name]
                if bounds["width"].number > 0 && bounds["height"].number > 0 {
                    let control = JSON(["kind": index == 0 ? "brush_size_slider" : "brush_strength_slider"])
                    ToolbarComponentView(store: store, panel: JSON(["id": "phone", "tile_style": "medium"]),
                        tile: JSON(["id": index, "control": control.raw, "component": store.snapshot["phone_sliders"][index].raw,
                            "slider_style": ["show_value": false, "expand": false]]), size: bounds.rect.size,
                        vertical: layout["edge"].string != "top", phone: layout)
                        .environment(\.workspaceGesturesEnabled, false).placed(bounds)
                        .accessibilityIdentifier("phone-slider-\(name)")
                }
            }
        }.frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .onChange(of: JSON([layout.raw, tiles.map(\.raw)]).stableKey, initial: true) { _, _ in
                input.update(layout: layout, tiles: JSON(tiles.map(\.raw)))
            }.onDisappear { input.cancel() }
    }
    @ViewBuilder private func tileButton(_ model: JSON, visual: JSON, hit: JSON, anchor: JSON, id: String,
        icon: CGFloat? = nil, action: @escaping () -> Void) -> some View {
        if hit["width"].number > 0 && hit["height"].number > 0 {
            let opened = store.state["customization"]["drawer"]["anchor"]
            let isOpen = (opened["kind"].string == "phone" && opened["anchor"].stableKey == anchor.stableKey)
                || anchor["kind"].string == "capy" && store.state["phone"]["menu"].bool
            let face = visual.rect.offsetBy(dx: -hit["x"].number, dy: -hit["y"].number)
            HeaderTileButton(model: model, iconSize: icon ?? layout["icon"].number,
                enabled: model["enabled"].bool || model["has_variants"].bool,
                style: HeaderButtonStyle(selected: model["selected"].bool, inBar: true, drawerOpen: isOpen,
                    face: face, square: DrawerSource.square(visual.rect, sources: sources), radius: layout["tile"].number / 2),
                reasonIdentifier: id + "-reason", action: action)
                .overlay(alignment: .bottomTrailing) { if model["has_variants"].bool { ToolGroupMarker().frame(width: 12, height: 12).offset(x: face.maxX-hit["width"].number, y: face.maxY-hit["height"].number) } }
                .placed(hit).accessibilityIdentifier(id)
        }
    }
}
