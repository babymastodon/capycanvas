import AppKit

@main struct PhoneBarInputChecks {
    @MainActor static func main() async throws {
        _ = NSApplication.shared
        NSApp.setActivationPolicy(.prohibited)
        let directory = URL(fileURLWithPath: ProcessInfo.processInfo.environment["CAPY_STORAGE_DIR"]!)
        let store = EditorStore(platform: 0, persistence: EditorPersistence(root: directory))
        defer { store.native?.detach() }
        let deadline = Date().addingTimeInterval(10)
        while store.state.isNull {
            precondition(Date() < deadline, "The private phone owner must publish its initial state")
            try await Task.sleep(for: .milliseconds(5))
        }
        try await store.workspaces!.started()
        try await store.workspaces!.perform(["type": "switch", "id": "builtin:workspace:painter"])
        for viewport: [Int] in [[375, 667], [667, 375]] {
            store.native?.resize(width: UInt32(viewport[0]), height: UInt32(viewport[1]), scale: 1)
            while store.snapshot["layout"]["viewport"].array.map { Int($0.number) } != viewport {
                precondition(Date() < deadline, "The real phone slider layout must follow its viewport")
                try await Task.sleep(for: .milliseconds(5))
            }
            let phone = store.snapshot["layout"]["phone"]
            for name in ["size", "strength"] {
                let bounds = phone[name].rect, vertical = phone["edge"].string != "top"
                let presentation = ToolbarSliderPresentation(size: bounds.size, vertical: vertical,
                    style: JSON("medium"), sliderStyle: JSON([String: Any]()), phone: true, language: store.interfaceLanguage)
                let scale = ToolbarUI.cached(["type": "slider_scale", "cross": vertical ? bounds.width : bounds.height]).number
                precondition(presentation.layout["track"].rect == CGRect(origin: .zero, size: bounds.size),
                    "Phone sliders use their full gripless shared bounds")
                precondition(abs(presentation.scale - scale) < 0.0001 && presentation.endInset == 0,
                    "Phone slider paint scales with the shared cross extent without end insets")
                let along = vertical ? bounds.height : bounds.width
                precondition(abs(presentation.travel(along) - (along - ToolbarSliderPresentation.paint["thumb"][0].number * scale)) < 0.0001,
                    "Phone thumb travel retains physical coordinates and the scaled shared thumb")
            }
        }
        let desktopSize = CGSize(width: 180, height: 36)
        let desktop = ToolbarSliderPresentation(size: desktopSize, vertical: false, style: JSON("medium"),
            sliderStyle: JSON([String: Any]()), phone: false, language: store.interfaceLanguage)
        precondition(desktop.scale == 1 && desktop.endInset == ToolbarSliderPresentation.paint["inset"].number)
        precondition(desktop.layout.stableKey == ToolbarUI.cached(["type": "slider_layout", "width": desktopSize.width,
            "height": desktopSize.height, "axis": "horizontal", "style": "medium", "slider_style": [String: Any](), "grip": true],
            language: store.interfaceLanguage).stableKey, "Desktop sliders retain their shared gripped layout")
        store.native?.resize(width: 412, height: 891, scale: 1)
        while store.snapshot["layout"]["viewport"].array.map({ Int($0.number) }) != [412, 891] {
            precondition(Date() < deadline, "The shared phone layout must be published without a window")
            try await Task.sleep(for: .milliseconds(5))
        }
        let layout = store.snapshot["layout"]["phone"]
        let tiles = store.state["workspace"]["layout"]["phone"]
        precondition(tiles.array.count > 1)
        let first = layout["tool_hits"][0].rect, second = layout["tool_hits"][1].rect
        let start = CGPoint(x: first.midX, y: first.midY)
        let destination = CGPoint(x: second.midX, y: second.midY)
        var actions: [JSON] = [], menus: [JSON] = [], dismissals = 0
        let input = PhoneBarInput(enabled: { store.workspace.input.enabled },
            action: { actions.append($0) }, menu: { target, _ in menus.append(target) }, dismissMenu: { dismissals += 1 })
        input.update(layout: layout, tiles: tiles)
        precondition(input.begin(at: start, device: .touch))
        input.finish(at: start)
        precondition(input.pointerDown && !input.active)
        input.click(["kind": "tool", "tile": tiles[0]["id"].raw])
        precondition(actions.count == 1, "An ordinary tile tap retains its button action")
        input.endGesture()
        try await Task.sleep(for: .milliseconds(1))
        actions.removeAll()

        precondition(input.begin(at: start, device: .pen))
        input.arm(at: start)
        precondition(input.preview["outcome"]["kind"].string == "menu" && input.suppressed)
        precondition(menus.count == 1 && menus[0]["kind"].string == "phone_tile" && menus[0]["tile"].uint == tiles[0]["id"].uint
            && dismissals == 0, "A held pen tile opens its menu before release")
        input.move(to: destination, pastSlop: true)
        precondition(input.preview["outcome"]["kind"].string == "dragging" && dismissals == 1,
            "The original contact closes its held menu when it starts reordering")
        precondition(!input.preview["geometry"].array.isEmpty && !input.preview["lifted"].isNull
            && input.preview["lifted"].uint == 0)
        input.finish(at: destination); input.finish(at: destination)
        precondition(actions.count == 1 && actions[0]["kind"].string == "move_tool",
            "A native held drag commits exactly one shared move outcome")
        precondition(input.pointerDown && !input.active && !input.begin(at: start, device: .touch))
        input.click(["kind": "tool", "tile": tiles[0]["id"].raw])
        precondition(actions.count == 1, "Suppression must survive logical finish until physical release")
        input.endGesture()
        try await Task.sleep(for: .milliseconds(1))
        precondition(!input.suppressed)

        precondition(input.begin(at: start, device: .touch))
        input.arm(at: start)
        precondition(menus.count == 2 && menus[1]["kind"].string == "phone_tile" && menus[1]["tile"].uint == tiles[0]["id"].uint,
            "A held touch tile opens its menu before release")
        input.finish(at: start)
        precondition(menus.count == 2 && dismissals == 1, "A stationary release keeps the held menu open")
        input.cancel()
        precondition(dismissals == 1, "A released contact no longer owns the menu")
        input.endGesture()
        try await Task.sleep(for: .milliseconds(1))

        weak var cancelling: PhoneBarInput?
        let cancelled = PhoneBarInput(enabled: { true }, action: { actions.append($0) },
            menu: { target, _ in menus.append(target) }, dismissMenu: { dismissals += 1; cancelling?.cancel() })
        cancelling = cancelled
        cancelled.update(layout: layout, tiles: tiles)
        precondition(cancelled.begin(at: start, device: .touch))
        cancelled.arm(at: start); cancelled.cancel(); cancelled.cancel()
        precondition(menus.count == 3 && dismissals == 2 && !cancelled.active && cancelled.suppressed,
            "Reentrant and repeated cancellation dismiss a held menu exactly once")
        cancelled.endGesture()

        let mouse = PhoneBarInput(enabled: { true }, action: { actions.append($0) },
            menu: { target, _ in menus.append(target) }, dismissMenu: { dismissals += 1 })
        mouse.update(layout: layout, tiles: tiles)
        precondition(mouse.begin(at: start, device: .mouse))
        mouse.arm(at: start)
        precondition(mouse.preview["outcome"]["kind"].string == "menu" && mouse.suppressed && menus.count == 3,
            "A mouse hold arms pickup without a menu")
        let beforeMouseDrop = actions.count
        mouse.move(to: destination, pastSlop: true)
        precondition(mouse.preview["outcome"]["kind"].string == "dragging")
        mouse.finish(at: destination)
        precondition(menus.count == 3 && dismissals == 2 && actions.count == beforeMouseDrop + 1
            && actions.last!["kind"].string == "move_tool")
        mouse.endGesture()

        let capy = layout["capy_target"].rect
        precondition(input.begin(at: CGPoint(x: capy.midX, y: capy.midY), device: .touch))
        input.arm(at: start); input.cancel(); input.cancel()
        precondition(actions.suffix(2).map { $0["kind"].string } == ["capy_hold", "capy_drop"])
        precondition(actions.last!["position"].array.map(\.number) == [-1, -1])
        precondition(input.pointerDown && input.suppressed)
        input.endGesture()
        try await Task.sleep(for: .milliseconds(1))

        let popup = UUID()
        precondition(input.begin(at: start, device: .touch))
        input.arm(at: start); input.move(to: destination, pastSlop: true)
        let beforeBlockedDrop = actions.count, snapshot = store.snapshot.json.stableKey
        store.workspace.popover(popup, open: true)
        input.finish(at: destination)
        precondition(actions.count == beforeBlockedDrop && !input.active && input.pointerDown && input.suppressed,
            "A native modal opening without a snapshot update cancels a held tile drop")
        precondition(store.snapshot.json.stableKey == snapshot)
        input.endGesture()
        try await Task.sleep(for: .milliseconds(1))
        precondition(!input.pointerDown && !input.suppressed && !input.acceptsClick)
        input.click(["kind": "tool", "tile": tiles[0]["id"].raw])
        precondition(actions.count == beforeBlockedDrop && !input.begin(at: start, device: .touch),
            "Native admission blocks button and new-contact activation while the modal remains open")
        store.workspace.popover(popup, open: false)
        precondition(input.acceptsClick)

        precondition(input.begin(at: CGPoint(x: capy.midX, y: capy.midY), device: .touch))
        input.arm(at: start)
        let beforeBlockedCapy = actions.count
        store.workspace.popover(popup, open: true)
        input.move(to: start, pastSlop: true); input.finish(at: start)
        precondition(actions.count == beforeBlockedCapy + 1 && actions.last!["kind"].string == "capy_drop"
            && actions.last!["position"].array.map(\.number) == [-1, -1],
            "Closing native admission retires a held Capy contact with its cancellation drop")
        precondition(!input.active && input.pointerDown && input.suppressed)
        store.workspace.popover(popup, open: false)
        input.endGesture()
        try await Task.sleep(for: .milliseconds(1))
        precondition(!input.pointerDown && !input.suppressed && input.acceptsClick)

        let keyboardHold = PhoneBarInput(enabled: { true }, action: { actions.append($0) },
            menu: { target, _ in menus.append(target) }, dismissMenu: { dismissals += 1 })
        keyboardHold.update(layout: layout, tiles: tiles)
        precondition(keyboardHold.begin(at: CGPoint(x: capy.midX, y: capy.midY), device: .touch))
        keyboardHold.arm(at: start)
        keyboardHold.update(layout: layout.replacing("keyboard", with: JSON(300)), tiles: tiles)
        precondition(keyboardHold.active, "A keyboard-only metrics change retains a held Capy contact")
        keyboardHold.finish(at: destination)
        precondition(actions.last!["kind"].string == "capy_drop"
            && actions.last!["position"].array.map(\.number) == [Double(destination.x), Double(destination.y)],
            "A held Capy contact drops where it lifts after keyboard height changes")
        keyboardHold.endGesture()

        precondition(input.begin(at: start, device: .touch))
        var changed = layout["work_area"].object
        changed["x"] = layout["work_area"]["x"].number + 1
        input.update(layout: layout.replacing("work_area", with: JSON(changed)), tiles: tiles)
        precondition(!input.active && input.pointerDown && input.suppressed,
            "A shared geometry invalidation retires decisions without ending the physical stream")
        let count = actions.count
        input.arm(at: start); input.move(to: destination, pastSlop: true); input.finish(at: destination)
        precondition(actions.count == count)
        input.endGesture()
        print("PASS: shared phone begin, held menu, motion, finish, cancellation and physical-stream click suppression")
    }
}
