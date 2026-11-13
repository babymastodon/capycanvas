import SwiftUI

struct RecoveryPresentation: ViewModifier {
    @ObservedObject var store: EditorStore
    @ObservedObject var recovery: ArtworkRecovery
    private var phone: Bool { !store.snapshot["layout"]["phone"].isNull }
    private var responses: [EditorAlertResponse] {
        [EditorAlertResponse(label: recovery.copy["retry"].string, identifier: "recovery-retry", accent: true,
            enabled: !recovery.restoring && !recovery.saving, action: { recovery.retry() }),
         EditorAlertResponse(label: recovery.copy["later"].string, identifier: "recovery-later", cancel: true,
            action: { recovery.later() })]
    }
    private var recoveryBody: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(recovery.copy["explanation"].string)
            if recovery.restoring || recovery.saving { ProgressView().controlSize(.small) }
            if recovery.error == nil && !recovery.failedDrawings.isEmpty {
                ForEach(recovery.failedDrawings, id: \.failedDrawingID) { drawing in
                    VStack(alignment: .leading, spacing: 8) {
                        Text(drawing["error"].string).textSelection(.enabled)
                        Button(recovery.copy["discard"].string, role: .destructive) { recovery.discard(drawing["id"].uint) }
                            .disabled(recovery.restoring || recovery.saving)
                            .accessibilityIdentifier("recovery-discard-\(drawing["id"].uint)")
                    }
                }
            } else { Text(recovery.visibleError ?? "").textSelection(.enabled) }
        }
    }
    func body(content: Content) -> some View {
        content
            #if DEBUG
            .overlay(alignment: .topLeading) {
                if ProcessInfo.processInfo.environment["CAPY_PERSISTENCE_PROBE"] == "1" {
                    Text(recovery.hasCurrentCopy ? "Recovery ready" : "Recovery pending")
                        .foregroundStyle(.clear).frame(width: 1, height: 1)
                        .accessibilityIdentifier("recovery-status")
                }
            }
            #endif
            .overlay(alignment: .bottom) {
                if !store.launchVisible {
                    GeometryReader { geometry in
                        let notices = VStack(spacing: 8) {
                            if store.failure != nil || !store.snapshot["error"].isNull {
                                EditorNotice(message: store.canvasDiagnostic,
                                    title: store.bootstrap[store.snapshot["gpu_ready"].bool ? "action_failed" : "canvas_init_failed"].string,
                                    titleIdentifier: "Canvas error", working: store.restartingCanvas, detailIdentifier: "canvas-error-detail") {
                                    if !store.snapshot["gpu_ready"].bool {
                                        Button(store.bootstrap["restart_canvas"].string) { store.restartCanvas() }.disabled(store.restartingCanvas).accessibilityIdentifier("Restart Canvas")
                                        Button(store.command("save_document_as")["label"].string) { store.invoke("save_document_as") }
                                            .disabled(!store.command("save_document_as")["enabled"].bool)
                                    } else {
                                        Button(store.bootstrap["ok"].string) { store.failure = nil }
                                    }
                                }
                            }
                            if let workspaces = store.workspaces { WorkspaceStartupNotice(workspaces: workspaces) }
                            ProjectProgressNotice(files: store.projectFiles, copy: recovery.copy)
                            if !phone, let error = recovery.visibleError {
                                if recovery.error == nil && !recovery.failedDrawings.isEmpty {
                                    ForEach(recovery.failedDrawings, id: \.failedDrawingID) { drawing in
                                        EditorNotice(message: drawing["error"].string, working: recovery.restoring || recovery.saving) {
                                            recoveryActions
                                            Button(recovery.copy["discard"].string, role: .destructive) { recovery.discard(drawing["id"].uint) }
                                                .disabled(recovery.restoring || recovery.saving)
                                                .accessibilityIdentifier("recovery-discard-\(drawing["id"].uint)")
                                        }
                                    }
                                } else {
                                    EditorNotice(message: error, working: recovery.restoring || recovery.saving) { recoveryActions }
                                }
                            }
                        }
                        ViewThatFits(in: .vertical) {
                            notices.fixedSize(horizontal: false, vertical: true)
                            EditorScrollView { notices }
                        }.padding(12).frame(maxWidth: .infinity, maxHeight: geometry.size.height, alignment: .bottom)
                    }
                }
            }.editorAlert(store: store, intent: recovery.copy["dialog"], title: recovery.copy["title"].string,
                isPresented: Binding(get: { phone && recovery.visibleError != nil }, set: { if !$0 { recovery.later() } }),
                responses: responses, bodyContent: AnyView(recoveryBody))
            .modifier(HostSurfacePresence(store: store, shown: recovery.visibleError != nil))
    }
    private var recoveryActions: some View {
        ForEach(responses) { response in
            Button(response.label, action: response.action).disabled(!response.enabled).accessibilityIdentifier(response.identifier)
        }
    }
}

private extension JSON { var failedDrawingID: UInt64 { self["id"].uint } }

struct EditorNotice<Actions: View>: View {
    let message: String
    var title: String?
    var titleIdentifier = ""
    var working = false
    var detailIdentifier = ""
    @ViewBuilder let actions: () -> Actions
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let title { Text(title).font(.headline).accessibilityIdentifier(titleIdentifier) }
            let detail = Text(message).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                .accessibilityIdentifier(detailIdentifier)
            ViewThatFits(in: .vertical) {
                detail.fixedSize(horizontal: false, vertical: true)
                ScrollView { detail }
            }.frame(maxHeight: 180)
            HStack(spacing: 12) {
                if working { ProgressView().controlSize(.small) }
                actions()
            }.buttonStyle(.bordered).controlSize(.regular)
        }.padding(16).frame(maxWidth: 580, alignment: .leading)
            .modifier(EditorPopupSurface(shape: RoundedRectangle(cornerRadius: 12)))
    }
}

struct RecoveryInteraction: ViewModifier {
    @ObservedObject var recovery: ArtworkRecovery
    func body(content: Content) -> some View {
        content.disabled(recovery.restoring).allowsHitTesting(!recovery.restoring)
    }
}
