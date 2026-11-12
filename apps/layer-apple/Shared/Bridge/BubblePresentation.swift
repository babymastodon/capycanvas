import SwiftUI
import QuartzCore

enum BubbleUI {
    static func resolve(_ request: [String: Any]) -> JSON {
        guard let text = try? JSON(request).encoded(),
            let result = text.withCString({ takeNativeText(capy_apple_bubble_ui($0)) }) else { return JSON() }

        return (try? JSON.decode(result)) ?? JSON()
    }
}

@MainActor final class BubblePlacement: ObservableObject {
    @Published var center = CGPoint.zero
    @Published var lifted = false
    @Published var homing = false
    @Published var opacity = 1.0
    @Published var pinch: CGFloat = 1
    @Published private(set) var positioned = false
    var displayOpacity: Double { positioned ? opacity * (homing ? 0.55 : 1) : 0 }
    private var target = 1.0
    private var from = 1.0
    private var start: TimeInterval?
    private var release: TimeInterval?
    private var stroke = false
    @Published private(set) var ignoresInput = false
    func receive(_ value: JSON) {
        if !positioned { positioned = true }
        let next = value["center"].point
        if center != next { center = next }
        if lifted != value["lifted"].bool { lifted = value["lifted"].bool }
        if homing != value["homing"].bool { homing = value["homing"].bool }
    }
    func fade(down: Bool, faded: Bool, now: TimeInterval) {
        if stroke && !down { release = now + 0.180 }
        if down { release = nil }
        stroke = down
        if down { ignoresInput = faded }
        else if release == nil { ignoresInput = false }
        let next = down ? faded ? 0.2 : 1 : release == nil ? 1 : target
        if target != next { from = opacity; target = next; start = nil }
    }
    func tick(_ now: TimeInterval, reduced: Bool) -> Bool {
        if let release, now >= release {
            self.release = nil; ignoresInput = false
            if target != 1 { from = opacity; target = 1; start = nil }
        }
        if opacity != target {
            if start == nil { start = now }
            let t = reduced ? 1 : min(1, max(0, (now - (start ?? now)) / 0.140))
            opacity = t == 1 ? target : from + (target - from) * t * t * (3 - 2 * t)
        }
        return opacity != target || release != nil
    }
}

@MainActor final class BubblePresentation: ObservableObject, Identifiable {
    struct Choice: Identifiable {
        let id: String
        var value: JSON
        var index: Int
        var ramp = BubbleRamp(value: 0)
        var alpha: Double { ramp.value }
        var leaving: Bool { ramp.target == 0 }
    }
    let id: UInt64
    let placement = BubblePlacement()
    @Published private(set) var model: JSON
    @Published var hover = JSON()
    @Published var pressed = JSON()
    @Published private(set) var stamp: CGImage?
    @Published private(set) var card = JSON()
    @Published private(set) var cardProgress = 0.0
    @Published private(set) var faceProgress = 1.0
    @Published private(set) var choices: [Choice] = []
    @Published private(set) var rings = JSON()
    @Published private(set) var ringGeometry = JSON()
    @Published private(set) var haloProgress = 0.0
    @Published private(set) var beads: [JSON] = []
    private var cardRamp = BubbleRamp(value: 0)
    private var faceRamp = BubbleRamp(value: 1)
    private var haloRamp = BubbleRamp(value: 0)
    private var beadRamp = BubbleRamp(value: 1)
    private var beadFrom: [JSON] = []
    private var beadTo: [JSON] = []
    private(set) var stampExtent = 1.0
    private var stampRequest: UInt64?
    private var stampPending = false
    private var stampWaiting = false
    private var stampDirty = true
    private var stampGeneration: UInt64 = 0
    var stampNeedsFrame: Bool { !view["stamp"].isNull && (stampDirty || stampWaiting || stampPending) }
    var view: JSON { model["view"] }
    var source: String { view["source"].stableKey }
    init(_ model: JSON) {
        id = model["view"]["id"].uint; self.model = model
        if !view["card"].isNull { card = view["card"]; cardRamp.set(1, duration: 0.2); faceRamp = BubbleRamp(value: 0); faceProgress = 0 }
        receiveParts(model, initial: true)
    }
    private func receiveParts(_ next: JSON, initial: Bool = false) {
        let views = next["view"]["chooser"].array
        var used = Set<String>()
        var shown: [Choice] = []
        for (index, value) in views.enumerated() {
            let key = value["kind"].string + ":" + value["label"].string
            used.insert(key)
            var choice = choices.first { $0.id == key } ?? Choice(id: key, value: value, index: index)
            choice.value = value; choice.index = index
            choice.ramp.set(1, duration: 0.18, delay: choice.alpha == 0 ? Double(index) * (value["kind"].string == "brush" ? 0.016 : 0.014) : 0)
            shown.append(choice)
        }
        for var choice in choices where !used.contains(choice.id) {
            choice.ramp.set(0, duration: 0.12)
            if choice.alpha > 0 { shown.append(choice) }
        }
        if initial || next["view"]["chooser"].stableKey != view["chooser"].stableKey { choices = shown }
        let incoming = next["view"]["rings"]
        if !incoming.array.isEmpty {
            if rings.stableKey != incoming.stableKey { rings = incoming }
            if ringGeometry.stableKey != next["rings"].stableKey { ringGeometry = next["rings"] }
        }
        haloRamp.set(incoming.array.isEmpty ? 0 : 1, duration: incoming.array.isEmpty ? 0.12 : 0.16)
        let target = next["view"]["beads"].array
        if initial || view["diameter"].number != next["view"]["diameter"].number || pressed.bubbleHit == "bead" {
            beadFrom = target; beadTo = target; beads = target; beadRamp = BubbleRamp(value: 1)
        } else if next["view"]["beads"].stableKey != view["beads"].stableKey {
            beadFrom = beads; beadTo = target; beadRamp = BubbleRamp(value: 0); beadRamp.set(1, duration: 0.22)
        }
    }
    static func tweenBeads(_ from: [JSON], _ to: [JSON], _ progress: Double) -> [JSON] {
        func key(_ value: JSON) -> String { value["placement"]["bead"].stableKey }
        func interpolate(_ start: JSON?, _ end: JSON) -> JSON {
            let a = start?["placement"] ?? end["placement"], b = end["placement"]
            func lerp(_ x: Double, _ y: Double) -> Double { x + (y - x) * progress }
            var value = end.object, placement = b.object
            placement["center"] = [lerp(a["center"][0].number, b["center"][0].number), lerp(a["center"][1].number, b["center"][1].number)]
            placement["size"] = lerp(a["size"].number, b["size"].number)
            placement["alpha"] = lerp(start == nil ? 0 : a["alpha"].number, b["alpha"].number)
            value["placement"] = placement
            return JSON(value)
        }
        return to.map { end in interpolate(from.first { key($0) == key(end) }, end) } + from.filter { old in !to.contains { key($0) == key(old) } }.map { old in
            var value = old.object, placement = old["placement"].object
            placement["alpha"] = old["placement"]["alpha"].number * (1 - progress); value["placement"] = placement
            return JSON(value)
        }
    }
    func receive(_ next: JSON) {
        receiveParts(next)
        let nextCard = next["view"]["card"]
        if !nextCard.isNull {
            if card.stableKey != nextCard.stableKey { card = nextCard }
            cardRamp.set(1, duration: 0.2)
            faceRamp = BubbleRamp(value: 0); if faceProgress != 0 { faceProgress = 0 }
        } else if !view["card"].isNull { cardRamp.set(0, duration: 0.15) }
        if view["diameter"].number != next["view"]["diameter"].number { placement.pinch = 1 }
        if model.stableKey != next.stableKey { model = next }
        stampDirty = true
        if view["stamp"].isNull {
            stampGeneration &+= 1; stampPending = false; stampWaiting = false; stampRequest = nil
            if stamp != nil { stamp = nil }
        }
    }
    func tick(_ now: TimeInterval, reduced: Bool) -> Bool {
        let next = cardRamp.tick(now, reduced: reduced)
        if next != cardProgress { cardProgress = next }
        if cardRamp.target == 0 && next == 0 && !card.isNull {
            card = JSON(); faceRamp.set(1, duration: 0.18)
        }
        let face = faceRamp.tick(now, reduced: reduced)
        if faceProgress != face { faceProgress = face }
        let halo = haloRamp.tick(now, reduced: reduced)
        if haloProgress != halo { haloProgress = halo }
        if halo == 0 && haloRamp.target == 0 && !rings.array.isEmpty { rings = JSON(); ringGeometry = JSON() }
        if choices.contains(where: { $0.ramp.active }) {
            var next = choices
            for index in next.indices { _ = next[index].ramp.tick(now, reduced: reduced) }
            next.removeAll { $0.leaving && $0.alpha == 0 }
            choices = next
        }
        if beadRamp.active {
            let progress = beadRamp.tick(now, reduced: reduced)
            beads = progress == 1 ? beadTo : Self.tweenBeads(beadFrom, beadTo, progress)
        }
        return cardRamp.active || faceRamp.active || haloRamp.active || beadRamp.active || choices.contains { $0.ramp.active }
    }
    func hit(_ point: CGPoint) -> JSON {
        BubbleUI.resolve(["type": "hit", "view": view.raw,
            "point": [point.x - placement.center.x, point.y - placement.center.y]])
    }
    func requestStamp(_ store: EditorStore) {
        guard !view["stamp"].isNull, !stampPending, stampDirty || stampWaiting else { return }
        stampPending = true; stampDirty = false
        let source = source, drawing = store.drawingIdentity, generation = stampGeneration
        var query: [String: Any] = ["type": "bubble_stamp"]
        if let stampRequest { query["known_request"] = stampRequest }
        store.query(query) { [weak self, weak store] reply in
            guard let self, self.stampGeneration == generation else { return }
            guard let store, self.source == source, drawing == store.drawingIdentity, !self.view["stamp"].isNull else {
                self.stampPending = false; return
            }
            self.stampWaiting = reply.isNull || reply["pending"].bool
            guard !reply["alpha"][0].isNull else {
                self.stampPending = false
                if self.stampWaiting { store.wake?() }
                return
            }
            Task { @MainActor [weak self, weak store] in
                let bitmap = await Task.detached(priority: .utility) { SliderStampImage(image: SliderPreviewSession.image(reply)) }.value
                guard let self, self.stampGeneration == generation else { return }
                self.stampPending = false
                guard let store, self.source == source, drawing == store.drawingIdentity,
                    !self.view["stamp"].isNull else { return }
                if let image = bitmap.image {
                    self.stampExtent = reply["extent"].number; self.stampRequest = reply["request_id"].uint
                    self.stamp = image
                } else { self.stampWaiting = true }
                store.wake?()
            }
        }
    }
}

@MainActor final class WorkspaceBubbles: ObservableObject {
    enum TakeResult { case taken, rejected, retired }
    @Published private(set) var items: [BubblePresentation] = []
    @Published private(set) var menu = JSON()
    @Published private(set) var menuPoint = CGPoint.zero
    let popup = UUID()
    private weak var store: EditorStore?
    private var drawing: String?
    private var motionPending = false
    private var moving = false
    private var animating = false
    private var request = UUID()
    private var menuOwner: (id: UInt64, source: String)?
    lazy var input = BubbleInteraction(bubbles: self, store: store)
    var reducedMotion = false
    var needsFrame: Bool { moving || animating }
    init(store: EditorStore) { self.store = store }
    func refresh() {
        guard let store else { return }
        let nextDrawing = store.drawingIdentity
        if drawing != nextDrawing { input.retire(); closeMenu(); drawing = nextDrawing; items = [] }
        let previous = Dictionary(uniqueKeysWithValues: items.map { ($0.id, $0) })
        let next = store.snapshot["bubbles"].array.map { model -> BubblePresentation in
            let id = model["view"]["id"].uint
            if let current = previous[id], current.source == model["view"]["source"].stableKey {
                current.receive(model); return current
            }
            return BubblePresentation(model)
        }
        if items.map(ObjectIdentifier.init) != next.map(ObjectIdentifier.init) { items = next }
        input.validate()
        if let owner = menuOwner, !next.contains(where: { $0.id == owner.id && $0.source == owner.source }) { closeMenu() }
        moving = !items.isEmpty
        store.wake?()
    }
    func tick(_ now: TimeInterval) {
        animating = false
        for item in items {
            animating = item.placement.tick(now, reduced: reducedMotion) || animating
            animating = item.tick(now, reduced: reducedMotion) || animating
            if let store { item.requestStamp(store) }
            animating = item.stampNeedsFrame || animating
        }
        guard !items.isEmpty, !motionPending, let store else { return }
        motionPending = true
        let drawing = drawing
        store.query(["type": "bubble_motion", "now_ns": UInt64(max(0, now) * 1_000_000_000), "reduced": reducedMotion]) { [weak self] reply in
            guard let self else { return }; self.motionPending = false
            guard self.drawing == drawing, !reply.isNull else { return }
            self.moving = reply["moving"].bool
            let down = reply["fade"][0].bool, faded = Set(reply["fade"][1].array.map(\.uint))
            let time = CACurrentMediaTime()
            for placement in reply["placements"].array {
                guard let item = self.items.first(where: { $0.id == placement["id"].uint }) else { continue }
                item.placement.receive(placement)
                item.placement.fade(down: down, faded: faded.contains(item.id), now: time)
                self.animating = item.placement.tick(time, reduced: self.reducedMotion) || self.animating
            }
            if self.needsFrame { store.wake?() }
        }
    }
    func action(_ action: [String: Any]) { store?.dispatch(["type": "bubble", "action": action], moving: action["phase"] as? String == "move") }
    func take(_ grab: JSON, at point: CGPoint, valid: @escaping () -> Bool, completion: @escaping (TakeResult) -> Void) {
        guard let store else { completion(.retired); return }
        let drawing = drawing, token = request
        store.query(["type": "bubble_grab", "grab": grab.raw, "held": true]) { [weak self] result in
            guard let self, self.drawing == drawing, self.request == token, valid() else { completion(.retired); return }
            guard !result.isNull else { completion(.rejected); return }
            store.query(["type": "can_take_bubble", "grab": result.raw, "position": [point.x, point.y],
                "viewport": store.snapshot["layout"]["viewport"].raw]) { [weak self] accepted in
                guard let self, self.drawing == drawing, self.request == token, valid() else { completion(.retired); return }
                guard accepted.bool else { completion(.rejected); return }
                for phase in ["down", "up"] {
                    self.action(["type": "drag", "grab": result.raw, "phase": phase, "position": [point.x, point.y],
                        "viewport": store.snapshot["layout"]["viewport"].raw])
                }
                completion(.taken)
            }
        }
    }
    func showMenu(_ item: BubblePresentation, at point: CGPoint, palettes: Bool = false) {
        guard let store else { return }
        let token = UUID(), source = item.source, drawing = drawing
        request = token; menuOwner = (item.id, source); menuPoint = point
        store.query(["type": "context", "target": ["kind": palettes ? "bubble_palettes" : "bubble", "id": item.id]]) { [weak self] reply in
            guard let self, self.request == token, self.drawing == drawing,
                self.input.enabled,
                self.items.contains(where: { $0.id == item.id && $0.source == source }) else { return }
            self.menu = reply
        }
    }
    func contextMenu() -> AppleContextMenu {
        let owner = menuOwner, drawing = drawing, point = menuPoint
        return AppleContextMenu(menu) { [weak self] value in
            guard let self, let store = self.store, let owner, self.drawing == drawing,
                let item = self.items.first(where: { $0.id == owner.id && $0.source == owner.source }) else { return }
            store.dispatch(value)
            if !self.menu.isNull { self.showMenu(item, at: point) }
        }
    }
    func closeMenu() { request = UUID(); menuOwner = nil; if !menu.isNull { menu = JSON() } }
    func hit(_ point: CGPoint) -> (BubblePresentation, JSON)? {
        for item in items.reversed() where item.placement.positioned && !item.placement.ignoresInput {
            let hit = item.hit(point)
            if !hit.isNull && hit["error"].isNull { return (item, hit) }
        }
        return nil
    }
}

extension JSON {
    var point: CGPoint { CGPoint(x: self[0].number, y: self[1].number) }
    var bubbleHit: String { string.isEmpty ? object.keys.sorted().first ?? "" : string }
    var bubbleBeadKey: String { self["placement"]["bead"].stableKey }
}
