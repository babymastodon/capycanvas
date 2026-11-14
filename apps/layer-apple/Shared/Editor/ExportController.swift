import SwiftUI

@MainActor final class ExportController: ObservableObject {
    @Published private(set) var view = JSON()
    @Published private(set) var copy: JSON
    @Published private(set) var details = JSON()
    @Published private(set) var busy = false
    @Published private(set) var loaded = false
    @Published private(set) var profiles: [JSON] = []
    @Published private(set) var previews: [CGImage] = []
    @Published private(set) var clipped: UInt64 = 0
    @Published private(set) var outputSize = ""
    @Published var error: String?
    let preferences: ColorPreferencesStore
    private let currentCopy: () -> JSON
    private weak var native: NativeOwner?
    private let request: UInt64
    private var task: NativeProjectTask?
    private var operations = 0
    private var previewRunning = false
    private var pendingPreview: UInt64?
    private var pendingChoice = false
    @Published private(set) var working = false { didSet { refreshBusy() } }
    private func refreshBusy() { busy = working || operations > 0 }
    private var closing = false
    private var finished = false
    private let completion: (NativeProjectTask?, JSON, JSON?) -> Void
    init(native: NativeOwner, request: UInt64, preferences: ColorPreferencesStore, copy: @escaping () -> JSON,
        completion: @escaping (NativeProjectTask?, JSON, JSON?) -> Void) {
        self.currentCopy = copy; self.copy = copy()
        self.native = native; self.request = request; self.preferences = preferences; self.completion = completion
    }
    var fields: JSON { view["fields"] }
    func refreshCopy() {
        copy = currentCopy()
        if loaded { query("export_dialog_view") { [weak self] in self?.accept($0) } }
        if !outputSize.isEmpty {
            native?.submit(2, JSON(["type": "native_caption", "caption": ["type": "export_size",
                "width": details["output_extent"][0].uint, "height": details["output_extent"][1].uint]])) { [weak self] value in
                DispatchQueue.main.async { self?.outputSize = value?.string ?? "" }
            }
        }
    }
    private func query(_ type: String, _ values: [String: Any] = [:], failure: (() -> Void)? = nil, completion: @escaping @MainActor (JSON) -> Void) {
        guard !finished, let native else { return }
        var request = values
        request["type"] = type; request["id"] = self.request
        let observation = (values["action"] as? [String: Any])?["type"] as? String == "preview"
        let mutation = !observation && !["export_dialog_view", "export_dialog_profiles", "export_dialog_recipe"].contains(type)
        if mutation { operations += 1; refreshBusy() }
        native.submit(2, JSON(request)) { [weak self] value in
            DispatchQueue.main.async {
                guard let self, !self.finished else { return }
                if mutation { self.operations -= 1; self.refreshBusy() }
                guard let value, value["error"].isNull || !value["dialog"].isNull else { failure?(); self.failed(value?["error"].string ?? "Export is unavailable"); return }
                completion(value)
                if self.closing && !self.busy && !self.previewRunning { self.finish(nil) }
            }
        }
    }
    func load(ready: (() -> Void)? = nil) {
        guard !busy, !closing, !finished, let native else { return }
        working = true; error = nil
        native.exportTask(id: request) { [weak self] task, error in
            DispatchQueue.main.async {
                guard let self else { task?.cancel(); return }
                guard !self.closing else { task?.cancel(); self.working = false; self.finish(nil); return }
                guard let task else { self.failed(error ?? "The drawing is unavailable"); return }
                self.task = task
                let preferences = self.preferences
                NativeProjectTask.io.async { [weak self] in
                    do {
                        let details = try task.details()
                        let presets = try preferences.retainedPresets(JSON(["type": "read"]))["bytes"]
                        DispatchQueue.main.async { [weak self] in
                            guard let self else { return }
                            self.details = details
                            self.query("begin_export_dialog", ["presets": presets.raw, "capabilities": [
                                "formats": ["Png", "Tiff", "Jpeg", "Webp", "PngHdr", "JpegHdr", "AvifHdr", "Exr"], "profile_import": true]]) { [weak self] value in
                                guard let self else { return }
                                self.loaded = true; self.working = false; self.accept(value); ready?()
                            }
                        }
                    } catch { let message = error.localizedDescription
                        DispatchQueue.main.async { [weak self] in self?.failed(message) }
                    }
                }
            }
        }
    }
    private func accept(_ value: JSON) {
        let revision = view["preview_revision"].uint, profilesChanged = view["profile_revision"].uint != value["profile_revision"].uint || !loaded || profiles.isEmpty
        view = value; copy = value["copy"]
        if revision != value["preview_revision"].uint { previews = []; clipped = 0; outputSize = "" }
        if profilesChanged { query("export_dialog_profiles") { [weak self] in self?.profiles = $0.array } }
        if revision != value["preview_revision"].uint { pendingPreview = value["preview_revision"].uint }
        schedulePreview()
    }
    func act(_ action: [String: Any], completion: (@MainActor () -> Void)? = nil) {
        guard loaded, !closing, !finished else { return }
        if action["type"] as? String != "preview" { error = nil }
        query("export_dialog_action", ["action": action]) { [weak self] value in self?.accept(value); completion?() }
    }
    func change(_ type: String, _ value: JSON) { act(["type": "edit", "action": ["type": type, "value": value.raw]]) }
    func imported(_ profile: JSON) { act(["type": "profile", "result": ["Ok": profile.raw]]) }
    func selectProfile(_ index: Int) { guard profiles.indices.contains(index) else { return }; imported(profiles[index]) }
    func usePrintProfile() {
        query("export_dialog_print_profile") { [weak self] profile in
            guard let self, !profile.isNull else { return }
            self.query("profile_channels", ["profile": profile[0].raw]) { [weak self] channels in
                self?.act(["type": "intent", "value": profile[2].raw])
                self?.imported(JSON(["profile": profile[0].raw, "name": profile[1].raw, "channels": channels.raw]))
            }
        }
    }
    func preset(_ operation: String) {
        act(["type": "begin_preset", "operation": operation]) { [weak self] in
            guard let self, self.view["preset_busy"].bool, operation != "save" else { return }
            self.storePreset(operation)
        }
    }
    func storePreset(_ operation: String = "save") {
        guard !working else { return }
        working = true
        query("export_dialog_preset_write", ["operation": operation]) { [weak self] write in
            guard let self else { return }
            let preferences = self.preferences
            NativeProjectTask.io.async { [weak self] in
                let result = Result { try preferences.retainedPresets(JSON(["type": "replace", "expected": write["expected"].raw, "next": write["next"].raw])) }
                DispatchQueue.main.async { [weak self] in
                    guard let self else { return }
                    self.working = false
                    let response: [String: Any]
                    switch result {
                    case .success: response = ["Ok": write["result"].raw]
                    case .failure(let error): response = ["Err": ["Diagnostic": error.localizedDescription]]
                    }
                    self.act(["type": "preset_result", "result": response])
                }
            }
        }
    }
    private func schedulePreview() {
        guard view["error"].isNull else { pendingPreview = nil; return }
        guard pendingPreview != nil, operations == 0, !previewRunning, !working, !closing, !finished else { return }
        prepare(preview: true)
    }
    func preview() { error = nil; pendingPreview = view["preview_revision"].uint; schedulePreview() }
    func choose() {
        if previewRunning { pendingChoice = true } else { prepare(preview: false) }
    }
    private func prepare(preview: Bool) {
        guard loaded, !working, !previewRunning, !closing, !finished, let task else { return }
        let revision = view["preview_revision"].uint
        if preview { previewRunning = true; pendingPreview = nil } else { working = true }
        if !preview { error = nil }
        query("export_dialog_recipe", failure: { [weak self] in self?.previewRunning = false }) { [weak self] recipe in
            guard let self else { return }
            NativeProjectTask.io.async { [weak self] in
                do {
                    try task.configureExport(recipe)
                    if preview || recipe["format"].string.contains("Hdr") { try task.compare() }
                    let details = try task.details()
                    let images = preview ? try (0..<(details["has_sdr_preview"].bool ? 3 : 2)).map { try task.comparison(index: UInt32($0)) } : []
                    DispatchQueue.main.async { [weak self] in
                        guard let self else { return }
                        if preview { self.previewRunning = false } else { self.working = false }
                        if self.closing { self.finish(nil); return }
                        if preview && (revision != self.view["preview_revision"].uint || self.operations > 0) {
                            self.pendingPreview = self.view["preview_revision"].uint; self.schedulePreview(); return
                        }
                        self.details = details; self.previews = images; self.clipped = details["clipped_channels"].uint
                        self.act(["type": "preview", "value": ["ready": preview, "transparent": details["transparent"].raw, "range_exceeded": details["range_blocked"].bool]]) { [weak self] in
                            guard let self else { return }
                            if self.pendingChoice { self.pendingChoice = false; self.choose() }
                        }
                        if !preview && !details["range_blocked"].bool {
                            self.query("export_dialog_remember_write") { [weak self] write in self?.finish(task, recipe: recipe, write: write) }
                        } else if preview {
                            self.native?.submit(2, JSON(["type": "native_caption", "caption": ["type": "export_size",
                                "width": details["output_extent"][0].uint, "height": details["output_extent"][1].uint]])) { [weak self] caption in
                                DispatchQueue.main.async { self?.outputSize = caption?.string ?? "" }
                            }
                        }
                    }
                } catch { let message = error.localizedDescription
                    DispatchQueue.main.async { [weak self] in self?.previewRunning = false; self?.failed(message); self?.schedulePreview() }
                }
            }
        }
    }
    func cancel() {
        guard !finished else { return }
        closing = true; task?.cancel()
        if !busy && !previewRunning { finish(nil) }
    }
    private func failed(_ message: String) { working = false; if closing { finish(nil) } else { error = message } }
    private func finish(_ result: NativeProjectTask?, recipe: JSON = JSON(), write: JSON? = nil) {
        guard !finished else { return }
        finished = true; task = nil
        completion(result, recipe, write)
    }
}
