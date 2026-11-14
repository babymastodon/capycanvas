import AppKit
import SwiftUI

/// Hidden format-specific drafts must not block a different output format.
/// Native typing and the default Choose File action exercise the production form.
@main final class ExportFormChecks: NativeWorkspaceInputFixture {
    @MainActor static func run() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("capy-export-form-\(UUID())")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false)
        defer { try? FileManager.default.removeItem(at: root) }
        let store = EditorStore(platform: 1, persistence: EditorPersistence(root: root), managedWorkspaces: false)
        let native = store.native!, surface = attachSurface(store, CGSize(width: 128, height: 128))
        defer { native.detach(); withExtendedLifetime(surface) {} }
        func wait(_ label: String, _ ready: () -> Bool) async throws {
            try await CapyTest.wait(label, failure: { store.failure }, step: { await frame(native) }, ready)
        }
        try await wait("Metal startup") { store.snapshot["shaders_ready"].bool }
        var destinations: [String] = []
        var acceptedRecipe = JSON()
        store.projectFiles = ProjectFiles(store: store, dialogs: .init(open: { _, done in done([]) }, save: { _, type, done in
            let id = store.state["requests"].array.first { $0["kind"]["request"]["type"].string == "export" }!["id"].uint
            store.query(["type": "export_dialog_recipe", "id": id]) { recipe in
                acceptedRecipe = recipe; destinations.append(type.identifier); done(nil)
            }
        }, create: { _, done in done(newSheet([64, 48], space: "DisplayP3", depth: "U16")) }, exportOptions: { _ in }))
        try await store.apply(["type": "invoke", "command": "new_document"])
        try await wait("New drawing") { !store.projectFiles.busy && store.state["requests"].array.isEmpty }
        let window = NSWindow(contentRect: CGRect(x: 100, y: 100, width: 600, height: 720), styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.contentView = nil; window.close() }
        func fields(_ view: NSView) -> [NSTextField] {
            (view as? NSTextField).map { [$0] } ?? view.subviews.flatMap(fields)
        }
        // Return invokes SwiftUI's native default action, including when its
        // button is virtual rather than backed by an NSButton instance.
        func choose() async throws {
            try key("\r", code: 36, window: window); try await drain()
        }
        for draft in ["1 +", "not a number"] { for format in ["Png", "Tiff"] {
            try await store.apply(["type": "invoke", "command": "export_document"])
            try await wait("Export loaded") { store.projectFiles.exportEditor?.loaded == true }
            let editor = store.projectFiles.exportEditor!
            let host = NSHostingView(rootView: ExportForm(editor: editor)); window.contentView = host
            window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
            try await drain(0.4)
            func changeFormat(_ format: String) async throws {
                editor.change("format", JSON(format)); try await wait("Format change") { !editor.busy }
                try require(editor.error == nil, editor.error ?? ""); try await drain(0.2)
            }
            try await changeFormat("Jpeg")
            guard let field = fields(host).first(where: { $0.isEditable && $0.placeholderString == editor.copy["quality"].string }) else {
                throw HostFailure(message: "Missing native JPEG quality field")
            }
            field.selectText(nil); try await drain()
            for character in draft { try key(String(character), code: 0, window: window); try await drain(0.01) }
            try require((window.firstResponder as? NSTextView)?.string == draft, "Typed quality must reach the real field")
            let before = destinations.count
            try await choose(); try await wait("JPEG validation") { editor.error != nil || destinations.count > before }
            try require(editor.error != nil && destinations.count == before, "Visible invalid JPEG quality must be rejected")
            try await changeFormat(format)
            try require(!fields(host).contains { $0.isEditable && $0.placeholderString == editor.copy["quality"].string }, "Quality field must be hidden")
            try await choose(); try await wait("Export choice") { editor.error != nil || destinations.count > before }
            try require(editor.error == nil && destinations.count == before + 1,
                "Hidden JPEG draft must not block \(format): \(editor.error ?? "missing destination")")
            try require(acceptedRecipe["jpeg_quality"].uint == 90, "Other formats retain a valid stored JPEG quality")
            try await wait("Destination cancellation") { !store.projectFiles.busy && store.state["requests"].array.isEmpty }
            try require(!store.state["document_file"]["modified"].bool, "Export cancellation must not edit the drawing")
            window.contentView = nil
        } }
        try await store.apply(["type": "invoke", "command": "export_document"])
        try await wait("JPEG export loaded") { store.projectFiles.exportEditor?.loaded == true }
        let editor = store.projectFiles.exportEditor!
        let host = NSHostingView(rootView: ExportForm(editor: editor)); window.contentView = host
        editor.act(["type": "navigate", "page": "size"])
        try await wait("Export size") { !editor.busy && editor.view["page"].string == "size" }
        editor.act(["type": "fit", "enabled": true])
        try await wait("Export bounds") { !editor.busy && editor.view["show_size"].bool }
        try await drain(0.2)
        guard let width = fields(host).first(where: { $0.placeholderString == editor.copy["maximum_width"].string }),
            let height = fields(host).first(where: { $0.placeholderString == editor.copy["maximum_height"].string }) else {
            throw HostFailure(message: "Missing native export bounds fields")
        }
        width.selectText(nil); try await drain()
        for character in "37" { try key(String(character), code: 0, window: window); try await drain(0.01) }
        let releaseCommit = DispatchSemaphore(value: 0)
        do {
            defer { releaseCommit.signal() }
            await withCheckedContinuation { continuation in
                native.submit(2, JSON(["type": "native_caption", "caption": ["type": "export_size", "width": 64, "height": 48]])) { _ in
                    continuation.resume(); releaseCommit.wait()
                }
            }
            height.selectText(nil); try await drain(0.2)
            try require(editor.busy, "The previous field commit must remain in flight")
            try require(width.isEnabled && height.isEnabled && height.currentEditor() === window.firstResponder,
                "Committing width must keep the newly focused height field enabled and focused")
        }
        try await wait("Committed export width") { !editor.busy && editor.view["bounds"][0].uint == 37 }
        editor.act(["type": "back"])
        try await wait("Export root") { !editor.busy && editor.view["page"].string == "root" }
        try await drain(0.2)
        editor.change("format", JSON("Jpeg")); try await wait("JPEG format") { !editor.busy }; try await drain(0.2)
        guard let field = fields(host).first(where: { $0.isEditable && $0.placeholderString == editor.copy["quality"].string }) else {
            throw HostFailure(message: "Missing native JPEG quality field")
        }
        field.selectText(nil); try await drain()
        for character in "73" { try key(String(character), code: 0, window: window); try await drain(0.01) }
        let before = destinations.count
        try await choose(); try await wait("Valid JPEG export") { editor.error != nil || destinations.count > before }
        try require(editor.error == nil && destinations.count == before + 1 && acceptedRecipe["jpeg_quality"].uint == 73,
            "JPEG must still use its visible quality draft")
        try await wait("JPEG destination cancellation") { !store.projectFiles.busy && store.state["requests"].array.isEmpty }
        try require(!store.state["document_file"]["modified"].bool, "Valid JPEG export must preserve artwork")
        try await store.apply(["type": "invoke", "command": "export_document"])
        try await wait("Preset export loaded") { store.projectFiles.exportEditor?.loaded == true }
        let presetEditor = store.projectFiles.exportEditor!
        let presetHost = NSHostingView(rootView: ExportForm(editor: presetEditor)); window.contentView = presetHost
        presetEditor.act(["type": "navigate", "page": "presets"])
        try await wait("Export presets") { !presetEditor.busy && presetEditor.view["page"].string == "presets" }
        presetEditor.preset("save")
        try await wait("Preset name") { !presetEditor.busy && presetEditor.view["page"].string == "save_preset" }
        try await drain(0.2)
        guard let name = fields(presetHost).first(where: { $0.isEditable && $0.placeholderString == presetEditor.copy["preset_name"].string }) else {
            throw HostFailure(message: "Missing native preset name field")
        }
        name.selectText(nil); try await drain()
        for character in "Canceled preset" { try key(String(character), code: 0, window: window); try await drain(0.01) }
        try key("\u{1b}", code: 53, window: window)
        try await wait("Canceled preset name") { !presetEditor.busy && presetEditor.view["page"].string == "presets" }
        presetEditor.preset("save")
        try await wait("Reopened preset name") { !presetEditor.busy && presetEditor.view["page"].string == "save_preset" }
        try await drain(0.2)
        guard let reopenedName = fields(presetHost).first(where: { $0.isEditable && $0.placeholderString == presetEditor.copy["preset_name"].string }) else {
            throw HostFailure(message: "Missing reopened native preset name field")
        }
        try require(reopenedName.stringValue.isEmpty, "Canceled native name draft retires with the shared preset dialog")
        let previousName = presetEditor.view["name"].string
        reopenedName.selectText(nil); try await drain()
        let draftedName = "Native preset typing"
        for character in draftedName { try key(String(character), code: 0, window: window); try await drain(0.01) }
        try require((window.firstResponder as? NSTextView)?.string == draftedName && presetEditor.view["name"].string == previousName,
            "Preset typing stays in the native draft until commit")
        try key("\r", code: 36, window: window)
        try await wait("Committed preset name") { !presetEditor.busy && presetEditor.view["page"].string == "presets" }
        try require(presetEditor.view["preset_names"].array.contains { $0.string == draftedName }, "Submitting commits and saves the complete native preset name")
        presetEditor.cancel()
        try await wait("Preset export cancellation") { !store.projectFiles.busy && store.state["requests"].array.isEmpty }
        note("PASS: native JPEG validation, hidden format drafts and committed preset-name typing preserve artwork")
    }
    @MainActor static func main() {
        _ = NSApplication.shared; NSApp.setActivationPolicy(.accessory)
        Task { @MainActor in
            do { try await run(); exit(0) } catch { note("FAIL: \(error)"); exit(1) }
        }
        NSApp.run()
    }
}
