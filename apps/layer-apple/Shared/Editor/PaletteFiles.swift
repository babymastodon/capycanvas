import SwiftUI
import UniformTypeIdentifiers

struct PaletteFiles: ViewModifier {
    @ObservedObject var controller: PaletteController
    private static let types: [UTType] = {
        let types = ColorPreferencesStore.paletteExtensions().compactMap { UTType(filenameExtension: $0) }
        return types.isEmpty ? [.data] : types
    }()
    func body(content: Content) -> some View {
        content
            .fileImporter(isPresented: $controller.importing, allowedContentTypes: Self.types) { controller.importPalette($0) }
            .fileExporter(isPresented: Binding(get: { controller.export != nil }, set: { if !$0 { controller.export = nil } }),
                document: controller.export?.document, contentType: controller.export?.type ?? .data,
                defaultFilename: controller.export?.name) { controller.exported($0) }
            .editorSheet(intent: controller.view["edit_dialog"], item: Binding(get: { controller.dialog.flatMap { $0.kind == .remove ? nil : $0 } },
                set: { if $0 == nil, controller.dialog?.kind != .remove { controller.dialog = nil } })) { dialog in
                PaletteNameForm(controller: controller, dialog: dialog).modifier(EditorFittedPresentation()).modifier(EditorPopupPresentation())
            }
            .editorAlert(intent: controller.view["delete_dialog"], title: controller.copy["remove_title"].string,
                message: controller.removeCaption, isPresented: Binding(get: { controller.dialog?.kind == .remove },
                    set: { if !$0, controller.dialog?.kind == .remove { controller.dialog = nil } }), responses: [
                    EditorAlertResponse(label: controller.common["cancel"].string, identifier: "palette-remove-cancel", cancel: true, action: { controller.dialog = nil }),
                    EditorAlertResponse(label: controller.common["remove"].string, identifier: "palette-remove-confirm", destructive: true, action: {
                        let id = controller.dialog?.palette; controller.dialog = nil
                        if let id { controller.apply(["op": "remove_palette", "id": id]) }
                    })])
    }
}

private struct PaletteNameForm: View {
    @ObservedObject var controller: PaletteController
    let dialog: PaletteDialog
    @State private var name: String
    @State private var error: String?
    @State private var saving = false
    @FocusState private var focused: Bool
    @Environment(\.editorDialogPage) private var page
    init(controller: PaletteController, dialog: PaletteDialog) {
        self.controller = controller; self.dialog = dialog
        _name = State(initialValue: dialog.name)
    }
    private var action: [String: Any] {
        if dialog.kind == .rename, let id = dialog.palette { return ["op": "rename_palette", "id": id, "name": name] }
        return ["op": "create_palette", "name": name]
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if page {
                EditorPageHeader(intent: controller.view["edit_dialog"],
                    title: dialog.kind == .rename ? controller.copy["rename_title"].string : controller.copy["new_title"].string,
                    identifier: "palette-name", primaryLabel: controller.common["save"].string, primaryEnabled: error == nil && !saving,
                    primaryIdentifier: "palette-name-save", cancel: { controller.dialog = nil }, primary: save)
            } else { Text(dialog.kind == .rename ? controller.copy["rename_title"].string : controller.copy["new_title"].string).font(.headline) }
            TextField(controller.copy["palette_name"].string, text: Binding(get: { name }, set: { name = String($0.prefix(64)) }))
                .textFieldStyle(.roundedBorder).focused($focused).onNativeTextSubmit(name) { name = String($0.prefix(64)); if page { focused = false } else { save() } }
                .accessibilityIdentifier("palette-library-name")
            if let error { Text(error).foregroundStyle(.red).font(.callout).accessibilityIdentifier("palette-name-error") }
            if !page { HStack {
                Spacer()
                Button(controller.common["cancel"].string, role: .cancel) { controller.dialog = nil }.keyboardShortcut(.cancelAction)
                Button(controller.common["save"].string, action: save).keyboardShortcut(.defaultAction).disabled(error != nil || saving)
                    .accessibilityIdentifier("palette-name-save")
            } }
        }.padding(20).modifier(EditorDialogFrame(minWidth: 320))
            .accessibilityElement(children: .contain).accessibilityIdentifier("palette-name-dialog")
            .onAppear { focused = true }
            .task(id: name) { controller.dryRun(action) { error = $0 } }
    }
    private func save() {
        guard !saving else { return }
        saving = true
        controller.apply(action) { failure in
            saving = false
            if let failure { error = failure } else { controller.dialog = nil; controller.chooser = false }
        }
    }
}
