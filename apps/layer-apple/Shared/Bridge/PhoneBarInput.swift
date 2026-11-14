import SwiftUI

@MainActor final class PhoneBarInput: ObservableObject {
    private(set) var layout = JSON()
    private var tiles = JSON([])
    private var contact = JSON()
    private var capy = false
    private var held = false
    private var heldMenu = false
    private var geometryKey = ""
    private var generation = 0
    private var device = ReorderDevice.touch
    private(set) var pointerDown = false
    private(set) var suppressed = false
    @Published private(set) var preview = JSON()
    private let enabled: () -> Bool
    private let action: (JSON) -> Void
    private let menu: (JSON, CGPoint) -> Void
    private let dismissMenu: () -> Void
    var active: Bool { capy || !contact.isNull }

    var acceptsClick: Bool { enabled() && !suppressed }

    init(enabled: @escaping () -> Bool, action: @escaping (JSON) -> Void, menu: @escaping (JSON, CGPoint) -> Void,
        dismissMenu: @escaping () -> Void) {
        self.enabled = enabled; self.action = action; self.menu = menu; self.dismissMenu = dismissMenu
    }
    func update(layout: JSON, tiles: JSON) {
        self.layout = layout; self.tiles = tiles
        if active && !valid() { cancel() }
    }
    @discardableResult func begin(at point: CGPoint, device: ReorderDevice) -> Bool {
        guard enabled(), !pointerDown, !layout.isNull else { return false }
        let capy = layout["capy_target"].rect.contains(point)
        let contact = capy ? JSON() : PhoneUI.resolve(["kind": "bar_begin", "layout": layout.raw,
            "tiles": tiles.raw, "point": [point.x, point.y]])
        guard capy || !contact.isNull && contact["error"].isNull else { return false }
        generation += 1; suppressed = false; pointerDown = true
        self.capy = capy; self.contact = contact; self.device = device
        geometryKey = layout["capy_target"].stableKey; held = false; preview = JSON()
        return true
    }
    func valid() -> Bool {
        guard enabled() else { return false }
        guard !layout.isNull else { return !active }
        if capy { return geometryKey == layout["capy_target"].stableKey }
        return contact.isNull || PhoneUI.resolve(["kind": "bar_valid", "layout": layout.raw,
            "tiles": tiles.raw, "contact": contact.raw]).bool
    }
    func contextTarget(at point: CGPoint) -> JSON? {
        guard !layout.isNull else { return nil }
        let contact = PhoneUI.resolve(["kind": "bar_begin", "layout": layout.raw,
            "tiles": tiles.raw, "point": [point.x, point.y]])
        guard layout["capy_target"].rect.contains(point) || !contact.isNull && contact["error"].isNull else { return nil }
        if let index = layout["tool_hits"].array.firstIndex(where: { $0.rect.contains(point) }), index < tiles.array.count {
            return JSON(["kind": "phone_tile", "tile": tiles[index]["id"].raw])
        }
        return JSON(["kind": "phone_toolbar"])
    }
    private func step(_ input: [String: Any]) -> JSON {
        let value = PhoneUI.resolve(["kind": "bar_step", "layout": layout.raw,
            "tiles": tiles.raw, "contact": contact.raw, "input": input])
        contact = value["contact"]; preview = value["reply"]
        return value["reply"]["outcome"]
    }
    func arm(at point: CGPoint) {
        guard active else { return }
        guard valid() else { cancel(); return }
        if capy { held = true; suppressed = true; action(JSON(["kind": "capy_hold"])) }
        else {
            let outcome = step(["kind": "arm"])
            switch outcome["kind"].string {
            case "menu":
                held = true; suppressed = true
                guard device != .mouse else { break }
                heldMenu = true
                menu(outcome["value"].isNull ? JSON(["kind": "phone_toolbar"])
                    : JSON(["kind": "phone_tile", "tile": outcome["value"].raw]), point)
            case "cancel": cancel()
            default: break
            }
        }
    }
    func move(to point: CGPoint, pastSlop: Bool) {
        guard active else { return }
        guard valid() else { cancel(); return }
        if capy { if pastSlop && !held { cancel() }; return }
        switch step(["kind": "motion", "position": [point.x, point.y], "past_slop": pastSlop])["kind"].string {
        case "dragging":
            if heldMenu { heldMenu = false; dismissMenu() }
            suppressed = true
        case "held": suppressed = true
        case "window_move", "cancel": cancel()
        default: break
        }
    }
    func finish(at point: CGPoint) {
        guard active else { return }
        guard valid() else { cancel(); return }
        if capy {
            if held { action(JSON(["kind": "capy_drop", "position": [point.x, point.y]])) }
        } else {
            let outcome = step(["kind": "finish", "position": [point.x, point.y]])
            switch outcome["kind"].string {
            case "move": action(outcome["value"].replacing("kind", with: JSON("move_tool")))
            case "cancel": suppressed = true
            default: break
            }
        }
        capy = false; held = false; heldMenu = false; contact = JSON(); preview = JSON()
    }
    func cancel() {
        guard active || pointerDown else { return }
        generation += 1
        if heldMenu { heldMenu = false; dismissMenu() }
        if capy && held { action(JSON(["kind": "capy_drop", "position": [-1, -1]])) }
        suppressed = true; capy = false; held = false; contact = JSON(); preview = JSON()
    }
    func endGesture() {
        guard pointerDown else { return }
        pointerDown = false
        let retired = generation
        DispatchQueue.main.async { [weak self] in
            guard let self, generation == retired, !pointerDown else { return }
            suppressed = false
        }
    }
    func click(_ value: JSON) { if acceptsClick { action(value) } }
    func click(_ value: [String: Any]) { click(JSON(value)) }
}
