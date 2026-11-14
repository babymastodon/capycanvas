import SwiftUI

struct NavigatorPanel: View {
    @ObservedObject var store: EditorStore
    private var aspect: CGFloat {
        let document = store.state["tabs"][0]
        guard !store.camera.value.isNull,
            let source = try? JSON([store.camera.value.raw, [document["width"].uint, document["height"].uint], [100, 100]]).encoded(),
            let pointer = source.withCString({ takeNativeText(capy_apple_navigator_geometry($0)) }) else { return 1 }

        return (try? JSON.decode(pointer))?["overview_aspect"].number ?? 1
    }
    var body: some View {
        let aspect = aspect
        NavigatorPanelLayout(aspect: aspect, spacing: store.panelMetrics.spacing) {
            NavigatorDrawing(store: store)
                .modifier(PanelBodyMeasurement(panel: "navigator", part: "overview", kind: .scroll,
                    naturalHeight: { [aspect] width in max(0, width) * aspect }))
            NavigationButtons(store: store, prefix: "navigator")
                .modifier(PanelBodyMeasurement(panel: "navigator", part: "commands"))
        }.padding(8)
            .modifier(PanelBodyMeasurement(panel: "navigator", part: "insets",
                intrinsicHeight: 16 + store.panelMetrics.spacing))
    }
}


private struct NavigatorPanelLayout: Layout {
    let aspect: CGFloat
    let spacing: CGFloat
    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width.flatMap { $0.isFinite ? $0 : nil } ?? subviews[1].sizeThatFits(.unspecified).width
        let commands = subviews[1].sizeThatFits(ProposedViewSize(width: width, height: nil)).height
        return CGSize(width: width, height: proposal.height.flatMap { $0.isFinite ? $0 : nil }
            ?? max(0, width) * aspect + commands + spacing)
    }
    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let commands = subviews[1].sizeThatFits(ProposedViewSize(width: bounds.width, height: nil)).height
        let overview = max(0, bounds.height - commands - spacing)
        subviews[0].place(at: bounds.origin, anchor: .topLeading,
            proposal: ProposedViewSize(width: bounds.width, height: overview))
        subviews[1].place(at: CGPoint(x: bounds.minX, y: bounds.minY + overview + spacing), anchor: .topLeading,
            proposal: ProposedViewSize(width: bounds.width, height: commands))
    }
}

struct NavigationButtons: View {
    @ObservedObject var store: EditorStore
    let prefix: String
    var body: some View {
        HStack(spacing: store.panelMetrics.spacing) {
            ForEach(store.catalog["navigator_commands"].array.map(\.string), id: \.self) { id in
                let command = store.command(id)
                IconTile(icon: command["icon"].string, label: command["tooltip"].string,
                    selected: command["selected"].bool, enabled: command["enabled"].bool) { store.invoke(id) }
                    .frame(height: store.panelMetrics.height).accessibilityIdentifier(prefix + "-" + id)
            }
        }
    }
}

private struct NavigatorDrawing: View {
    @ObservedObject var store: EditorStore
    @Environment(\.workspaceClip) private var clip
    @Environment(\.workspaceLayer) private var order
    @Environment(\.workspaceChromeOpacity) private var opacity
    @State private var identity = UUID()
    @State private var started = false
    @GestureState private var contact = false
    private var palette: EditorPalette { EditorPalette(source: store.state["palette"]) }
    private func geometry(_ size: CGSize) -> JSON {
        let document = store.state["tabs"][0]
        guard size.width > 8, size.height > 8, !store.camera.value.isNull,
            let source = try? JSON([store.camera.value.raw, [document["width"].uint, document["height"].uint], [size.width, size.height]]).encoded(),
            let pointer = source.withCString({ takeNativeText(capy_apple_navigator_geometry($0)) }) else { return JSON() }

        return (try? JSON.decode(pointer)) ?? JSON()
    }
    private func send(_ phase: String, _ point: CGPoint = .zero, _ size: CGSize = .zero) {
        store.dispatch(["type": "navigator", "phase": phase, "position": [point.x, point.y], "viewport": [size.width, size.height]])
    }
    var body: some View {
        GeometryReader { allocation in
            let model = geometry(allocation.size)
            let bounds = allocation.frame(in: .named("editor-workspace"))
            let visible = bounds.intersection(clip)
            Canvas { context, size in
                context.fill(Path(CGRect(origin: .zero, size: size)), with: .color(palette["bg"]))
            }.contentShape(Rectangle())
                .preference(key: NavigatorPlacements.self, value: model.isNull || visible.isEmpty || opacity <= 0 ? [:] : [identity:
                    NavigatorPlacement(bounds: bounds, clip: visible,
                        image: model["image"].rect.offsetBy(dx: bounds.minX, dy: bounds.minY), order: order, opacity: opacity)])
                .gesture(DragGesture(minimumDistance: 0).updating($contact) { _, value, _ in value = true }.onChanged { event in
                    if !started { started = true; send("down", event.startLocation, allocation.size) }
                    if event.translation != .zero { send("move", event.location, allocation.size) }
                }.onEnded { event in
                    if started { send("up", event.location, allocation.size); started = false }
                })
                .onChange(of: contact) { _, active in if !active && started { send("cancel"); started = false } }
                .onDisappear { if started { send("cancel"); started = false } }
                .accessibilityElement().accessibilityLabel(store.catalog["native_copy"]["color"]["navigator"].string)
                .accessibilityAddTraits(.isImage)
                .accessibilityValue(store.catalog["native_copy"]["color"][store.canvasReady ? "navigator_live" : "navigator_preparing"].string)
                .accessibilityHint(store.catalog["native_copy"]["color"]["navigator_hint"].string)
                .accessibilityIdentifier("navigator-overview")
        }
    }
}

struct RendererStatsPanel: View {
    @ObservedObject var store: EditorStore
    @ObservedObject var stats: RendererStats
    @State private var viewer = UUID()
    private var palette: EditorPalette { EditorPalette(source: store.state["palette"]) }
    var body: some View {
        let colors = ReadoutColors(palette)
        let textSize = CGFloat(store.catalog["text_size_pt"].number * 4 / 3)
        VStack(alignment: .leading, spacing: 6) {
            ForEach(stats.view["rows"].array.indices, id: \.self) { index in
                MeterRowView(row: ReadoutRow(stats.view["rows"][index]), colors: colors, textSize: textSize,
                    rowID: "renderer-stat-row-\(index)", valueID: "stats-value-\(index)", barID: "").equatable()
                if index + 1 == Int(stats.view["chart_after_rows"].number) {
                    RendererStatsChart(samples: stats.view["samples"].array.map(\.number),
                        budget: stats.view["budget_ms"].number, color: colors.ink,
                        label: stats.view["chart_label"].string).equatable()
                }
            }
            if !stats.view["memory"].isNull {
                EditorLine(color: palette.panelBorder)
                MeterSectionView(section: ReadoutSection(stats.view["memory"]), colors: colors, textSize: textSize).equatable()
                EditorLine(color: palette.panelBorder)
            }
            RecordingButton(recording: store.strokeRecording, height: store.panelMetrics.height)
        }.accessibilityElement(children: .contain).accessibilityIdentifier("renderer-stats")
            .onAppear { stats.show(viewer); store.strokeRecording.refresh() }.onDisappear { stats.hide(viewer) }
    }
}

private extension JSON {
    var readoutTip: String {
        ([self["text"].string] + self["lines"].array.map { "\($0["label"].string): \($0["value"].string)" })
            .filter { !$0.isEmpty }.joined(separator: "\n")
    }
}

private struct ReadoutColors: Equatable {
    let ink: Color, accent: Color, error: Color
    init(_ palette: EditorPalette) { ink = palette["text"]; accent = palette.accent; error = palette["error"] }
}

private struct ReadoutChip: Equatable {
    let label: String, warning: Bool
    init(_ source: JSON) { label = source["label"].string; warning = source["warning"].bool }
}

private struct ReadoutMeter: Equatable {
    struct Segment: Equatable {
        let from: Double, to: Double
        let tone: String
    }
    let limit: Double
    let segments: [Segment]
    init(_ source: JSON) {
        limit = source["limit"].number
        segments = source["segments"].array.map { Segment(from: $0["from"].number, to: $0["to"].number, tone: $0["tone"].string) }
    }
}

private struct ReadoutRow: Equatable {
    let label: String, value: String, description: String
    let warning: Bool
    let chip: ReadoutChip?
    let meter: ReadoutMeter?
    init(_ source: JSON) {
        label = source["label"].string; value = source["value"].string; warning = source["warning"].bool
        description = source["description"].isNull ? source["tip"].readoutTip : source["description"].string
        chip = source["chip"].isNull ? nil : ReadoutChip(source["chip"])
        meter = source["meter"].isNull ? nil : ReadoutMeter(source["meter"])
    }
}

private struct ReadoutSection: Equatable {
    let title: String, note: String, description: String
    let chip: ReadoutChip?
    let rows: [ReadoutRow]
    init(_ source: JSON) {
        title = source["title"].string; note = source["note"].string; description = source["tip"].readoutTip
        chip = source["chip"].isNull ? nil : ReadoutChip(source["chip"])
        rows = source["rows"].array.map(ReadoutRow.init)
    }
}

private struct MeterBadge: View {
    let chip: ReadoutChip?
    let colors: ReadoutColors
    let textSize: CGFloat
    var body: some View {
        if let chip {
            let ink = chip.warning ? colors.error : colors.ink
            Text(chip.label).font(.system(size: textSize * 0.85, weight: .semibold)).lineLimit(1).fixedSize()
                .foregroundStyle(ink).padding(.horizontal, 7)
                .background(ink.opacity(chip.warning ? 0.16 : 0.10), in: Capsule())
        }
    }
}

private struct MeterBar: View, Equatable {
    let meter: ReadoutMeter
    let colors: ReadoutColors
    var body: some View {
        Canvas { context, size in
            let top = ((size.height - 6) / 2).rounded()
            var bar = context
            bar.clip(to: Path(roundedRect: CGRect(x: 0, y: top, width: size.width, height: 6), cornerRadius: 3))
            func span(_ from: Double, _ to: Double) -> Path {
                Path(CGRect(x: CGFloat(from) * size.width, y: top, width: CGFloat(to - from) * size.width, height: 6))
            }
            bar.fill(span(0, meter.limit), with: .color(colors.ink.opacity(0.12)))
            bar.fill(span(meter.limit, 1), with: .color(colors.ink.opacity(0.06)))
            for segment in meter.segments {
                let color: Color
                switch segment.tone {
                case "fill": color = colors.accent
                case "soft": color = colors.accent.opacity(0.4)
                case "rest": color = colors.ink.opacity(0.32)
                default: color = colors.error
                }
                bar.fill(span(segment.from, segment.to), with: .color(color))
            }
            if meter.limit < 0.999 {
                context.fill(Path(CGRect(x: (CGFloat(meter.limit) * size.width).rounded() - 1, y: 0, width: 1, height: size.height)),
                    with: .color(colors.ink.opacity(0.6)))
            }
        }.frame(height: 12)
    }
}

private struct MeterRowView: View, Equatable {
    let row: ReadoutRow
    let colors: ReadoutColors
    let textSize: CGFloat
    let rowID: String, valueID: String, barID: String
    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                HStack(spacing: 6) {
                    Text(row.label).lineLimit(1).truncationMode(.tail)
                    MeterBadge(chip: row.chip, colors: colors, textSize: textSize)
                }.frame(maxWidth: .infinity, alignment: .leading)
                Text(row.value).monospacedDigit().fixedSize().foregroundStyle(row.warning ? colors.error : colors.ink)
                    .accessibilityIdentifier(valueID)
            }
            if let meter = row.meter {
                MeterBar(meter: meter, colors: colors).equatable()
                    .accessibilityElement().accessibilityLabel(row.label).accessibilityIdentifier(barID)
            }
        }.help(row.description).accessibilityHint(row.description)
            .accessibilityElement(children: .contain).accessibilityIdentifier(rowID)
    }
}

private struct MeterSectionView: View, Equatable {
    let section: ReadoutSection
    let colors: ReadoutColors
    let textSize: CGFloat
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Text(section.title).fontWeight(.semibold)
                Text(section.note).lineLimit(1).truncationMode(.tail).foregroundStyle(colors.ink.opacity(0.55))
                    .frame(maxWidth: .infinity, alignment: .leading)
                MeterBadge(chip: section.chip, colors: colors, textSize: textSize)
            }.help(section.description).accessibilityHint(section.description)
                .accessibilityElement(children: .combine).accessibilityIdentifier("renderer-memory-heading")
            ForEach(section.rows.indices, id: \.self) { index in
                MeterRowView(row: section.rows[index], colors: colors, textSize: textSize,
                    rowID: "renderer-memory-row-\(index)", valueID: "renderer-memory-value-\(index)",
                    barID: "renderer-memory-bar-\(index)").equatable()
            }
        }.accessibilityElement(children: .contain).accessibilityIdentifier("renderer-memory")
    }
}

private struct RendererStatsChart: View, Equatable {
    let samples: [Double]
    let budget: Double
    let color: Color
    let label: String
    var body: some View {
        Canvas { context, size in
            let maximum = max(0.001, max(budget, samples.max() ?? 0) * 1.1)
            // Match the shared chart's centered 200×46 SVG view box.
            let scale = min(size.width / 200, size.height / 46)
            context.translateBy(x: (size.width - 200 * scale) / 2, y: (size.height - 46 * scale) / 2)
            context.scaleBy(x: scale, y: scale)
            let y = 46 * (1 - budget / maximum)
            var line = Path(); line.move(to: CGPoint(x: 0, y: y)); line.addLine(to: CGPoint(x: 200, y: y))
            context.stroke(line, with: .color(color.opacity(0.3)), style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
            var chart = Path()
            for (index, value) in samples.enumerated() {
                let point = CGPoint(x: Double(index) * 200 / 119, y: 46 * (1 - value / maximum))
                if index == 0 { chart.move(to: point) } else { chart.addLine(to: point) }
            }
            context.stroke(chart, with: .color(color), lineWidth: 1.5)
        }.frame(height: 46).help(label).accessibilityElement().accessibilityLabel(label).accessibilityIdentifier("renderer-stats-chart")
    }
}

private struct RecordingButton: View {
    @Environment(\.capyNativeCopy) private var nativeCopy
    @ObservedObject var recording: StrokeRecording
    @Environment(\.editorPalette) private var palette
    let height: CGFloat
    var body: some View {
        Button { recording.activate() } label: {
            Text(recording.label).fontWeight(.bold).lineLimit(1).padding(.horizontal, 6)
                .frame(minHeight: height).contentShape(Rectangle())
        }.buttonStyle(EditorControlButtonStyle(background: palette.controlFill)).disabled(recording.busy)
            .help(nativeCopy["color"]["record_tablet"].string).accessibilityIdentifier("stroke-recording")
    }
}
