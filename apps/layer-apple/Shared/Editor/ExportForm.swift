import SwiftUI

struct ExportForm: View {
    @ObservedObject var editor: ExportController
    @Environment(\.capyInterfaceLanguage) private var language
    @Environment(\.editorDialogPage) private var phonePage
    @State private var drafts: [String: String] = [:]
    @State private var readingProfile = false
    @FocusState private var focused: String?
    private var view: JSON { editor.view }
    private var copy: JSON { editor.copy }
    private var fields: JSON { editor.fields }
    private var page: String { view["page"].string }
    private var title: String {
        switch page {
        case "size": copy["image_size"].string
        case "color": copy["color_transparency"].string
        case "presets": copy["presets"].string
        case "save_preset": copy["save_preset_title"].string
        default: copy["title"].string
        }
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            if phonePage {
                EditorPageHeader(intent: page == "save_preset" ? copy["preset_dialog"] : view["dialog"], title: title,
                    identifier: "export", subpage: page != "root", primaryLabel: copy[page == "save_preset" ? "save_preset" : "choose_file"].string,
                    primaryEnabled: editor.loaded && !editor.busy && !readingProfile && (page == "save_preset" ? drafts["name"] != nil || view["name_valid"].bool : view["can_export"].bool),
                    primaryIdentifier: page == "save_preset" ? "export-preset-confirm" : "export-choose-file",
                    backIdentifier: page == "save_preset" ? "export-preset-cancel" : "export-back",
                    cancel: { editor.cancel() }, back: back, primary: {
                        if commit() { if page == "save_preset" { editor.storePreset() } else { editor.choose() } }
                    })
            } else { HStack {
                if page != "root" && page != "save_preset" && !page.isEmpty {
                    Button { commit(); editor.act(["type": "back"]) } label: { SharedIcon(name: "go-previous") }
                        .accessibilityLabel(copy["common"]["back"].string).accessibilityIdentifier("export-back")
                }
                Text(title).font(.headline)
            } }
            EditorScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    if editor.loaded { choices.disabled(editor.working || readingProfile) }
                    if editor.working { ProgressView(copy["preparing"].string) }
                    if let error = editor.error ?? (view["error"].isNull ? nil : view["error"].string) {
                        Text(error).foregroundStyle(.red).accessibilityIdentifier("export-error")
                    }
                }.frame(maxWidth: .infinity, alignment: .leading)
            }
            if !phonePage && page != "save_preset" { HStack {
                Button(copy["common"]["cancel"].string) { editor.cancel() }.keyboardShortcut(.cancelAction).disabled(readingProfile)
                    .accessibilityIdentifier("export-cancel")
                Spacer()
                Button(copy["choose_file"].string) { if commit() { editor.choose() } }.keyboardShortcut(.defaultAction)
                    .disabled(!editor.loaded || editor.busy || readingProfile || !view["can_export"].bool).accessibilityIdentifier("export-choose-file")
            } }
        }.padding(phonePage ? 8 : 24).modifier(EditorDialogFrame(idealWidth: 520, maxWidth: 620, idealHeight: 660, maxHeight: 760))
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("export-dialog")
            .onNativeTextKey(.escape) {
                let next = DialogUI.resolve(["kind": "escape", "subpage": page != "root", "search_focused": false, "search_has_text": false]).string
                if next == "back" { back() } else { editor.cancel() }
                return .handled
            }
            .onChange(of: language, initial: true) { _, _ in editor.refreshCopy() }
            .onChange(of: page) { previous, next in if previous == "save_preset" || next == "save_preset" { drafts["name"] = nil } }
            .onChange(of: focused) { previous, _ in if let previous { commit(previous) } }
            .interactiveDismissDisabled(readingProfile)
    }
    @ViewBuilder private var choices: some View {
        switch page {
        case "size":
            Toggle(copy["fit_bounds"].string, isOn: actionBool(view["show_size"], type: "fit")).accessibilityIdentifier("export-fit")
            if view["show_size"].bool {
                number("width", copy["maximum_width"].string, view["bounds"][0], control: "dimension")
                number("height", copy["maximum_height"].string, view["bounds"][1], control: "dimension")
                Toggle(copy["enlarge"].string, isOn: actionBool(view["enlarge"], type: "enlarge"))
            }
            FormPicker(copy["resolution"].string, selection: Binding(get: { fields["resolution"].stableKey }, set: { key in
                if let choice = view["resolutions"].array.first(where: { $0["value"].stableKey == key }) { editor.change("resolution", choice["value"]) }
            })) { ForEach(view["resolutions"].array, id: \.stableKey) { Text($0["label"].string).tag($0["value"].stableKey) } }
            if view["show_ppi"].bool { number("ppi", copy["ppi"].string, view["ppi"], control: "ppi") }
            Text(view["size_summary"].string)
        case "color":
            if view["show_conversion"].bool {
                FormPicker(copy["profile"].string, selection: Binding(get: { Int(fields["profile"].uint) }, set: editor.selectProfile)) {
                    ForEach(editor.profiles.indices, id: \.self) { Text(editor.profiles[$0]["name"].string).tag($0) }
                }.accessibilityIdentifier("export-profile")
                ProfileChooserButtons(preferences: editor.preferences, busy: $readingProfile, onProfile: editor.imported)
            }
            if view["show_print_profile"].bool {
                Button(copy["use_print_profile"].string) { editor.usePrintProfile() }.disabled(!view["profile_ready"].bool)
                Text(view["print_profile_name"].string).font(.caption)
            }
            if view["show_depth"].bool { choice("depth", copy["depth"].string, view["depths"], fields["depth"]).disabled(!view["depth_enabled"].bool) }
            if view["show_background"].bool { choice("background", copy["background"].string, view["backgrounds"], fields["background"]) }
            if view["show_dither"].bool { Toggle(copy["reduce_banding"].string, isOn: actionBool(view["dither"], type: "dither")) }
            if view["show_conversion"].bool {
                FormPicker(copy["intent"].string, selection: Binding(get: { fields["encoding"]["conversion"]["intent"].string }, set: { editor.act(["type": "intent", "value": $0]) })) {
                    ForEach(view["intents"].array, id: \.stableKey) { Text($0["label"].string).tag($0["value"].string) }
                }
            }
            if view["jpeg_note"].bool { Text(copy["jpeg_note"].string).font(.caption) }
            if !view["precision_note"].isNull { Text(view["precision_note"].string).font(.caption) }
        case "presets":
            FormPicker(copy["preset"].string, selection: Binding(get: { Int(view["preset"].uint) }, set: { editor.act(["type": "preset", "index": $0]) })) {
                ForEach(view["preset_names"].array.indices, id: \.self) { Text(view["preset_names"][$0].string).tag($0).disabled(!view["preset_enabled"][$0].bool) }
            }.accessibilityIdentifier("export-destination")
            ForEach(Array(["save", "update", "remove", "reset"].enumerated()), id: \.offset) { index, operation in
                if view["preset_\(operation)"].bool {
                    Button(view["preset_labels"][index].string) { if commit() { editor.preset(operation) } }
                        .disabled(!editor.preferences.canSave || view["preset_busy"].bool)
                        .accessibilityIdentifier("export-preset-\(operation)")
                }
            }
            if !view["preset_status"].isNull { Text(view["preset_status"].string).foregroundStyle(view["preset_error"].bool ? .red : .secondary) }
        case "save_preset":
            TextField(copy["preset_name"].string, text: Binding(get: { drafts["name"] ?? view["name"].string }, set: { drafts["name"] = $0 }))
                .focused($focused, equals: "name").onAppear { if phonePage { focused = "name" } }
                .onNativeTextSubmit(drafts["name"] ?? view["name"].string) { text in
                    drafts["name"] = text
                    if commit("name") { if phonePage { focused = nil } else { editor.storePreset() } }
                }
                .textFieldStyle(.roundedBorder).accessibilityIdentifier("export-preset-name")
            if !phonePage { HStack {
                Button(copy["common"]["cancel"].string) { drafts["name"] = nil; editor.act(["type": "preset_cancel"]) }.keyboardShortcut(.cancelAction)
                    .accessibilityIdentifier("export-preset-cancel")
                Button(copy["common"]["save"].string) { if commit("name") { editor.storePreset() } }.keyboardShortcut(.defaultAction)
                    .disabled(drafts["name"] == nil && !view["name_valid"].bool).accessibilityIdentifier("export-preset-confirm")
            } }
        default:
            if view["show_output"].bool {
                FormPicker(copy["output"].string, selection: Binding(get: { view["output"].string }, set: { editor.act(["type": "output", "value": $0]) })) {
                    ForEach(view["outputs"].array, id: \.stableKey) { Text($0["label"].string).tag($0["value"].string) }
                }.accessibilityIdentifier("export-range")
            }
            if view["show_conversion"].bool { choice("format", copy["format"].string, view["formats"], fields["format"]) }
            if view["show_flatten"].bool { Toggle(copy["flatten"].string, isOn: actionBool(view["flatten"], type: "flatten")) }
            if view["show_quality"].bool { number("quality", copy["quality"].string, fields["jpeg_quality"], control: "quality") }
            if view["show_metadata"].bool {
                FormPicker(view["metadata"]["label"].string, selection: Binding(get: { fields["metadata"]["keep"].string }, set: { editor.act(["type": "metadata_keep", "value": $0]) })) {
                    ForEach(view["metadata"]["choices"].array, id: \.stableKey) { Text($0["label"].string).tag($0["value"].string) }
                }.accessibilityIdentifier("export-metadata")
            }
            if view["show_location"].bool { Toggle(view["metadata"]["remove_location"].string, isOn: actionBool(fields["metadata"]["remove_location"], type: "remove_location")) }
            if view["show_metadata_note"].bool { Text(view["metadata"]["note"].string).font(.caption) }
            Button(copy["preview"].string) { if commit() { editor.preview() } }.accessibilityIdentifier("export-preview")
            ForEach(0..<min(2, editor.previews.count), id: \.self) { index in
                Text(view["comparison_labels"][index].string).font(.headline)
                Image(decorative: editor.previews[index == 1 && view["preview_sdr"].bool && editor.previews.count == 3 ? 2 : index], scale: 1)
                    .resizable().aspectRatio(contentMode: .fit).frame(maxWidth: .infinity, maxHeight: 180)
            }
            if view["show_rendition"].bool {
                FormPicker(copy["preview_rendition"].string, selection: Binding(get: { view["preview_sdr"].bool }, set: { editor.act(["type": "rendition", "sdr": $0]) })) {
                    ForEach(view["renditions"].array, id: \.stableKey) { Text($0["label"].string).tag($0["value"].bool) }
                }
            }
            Text(editor.outputSize).accessibilityIdentifier("export-output-size")
            if editor.clipped > 0 { Text(copy[editor.details["range_blocked"].bool ? "outside_range" : "outside_gamut"].string) }
            if view["compression_note"].bool { Text(copy["compression_note"].string).font(.caption) }
            navigation("size", copy["size"].string, view["size_subtitle"].string)
            if view["show_color"].bool { navigation("color", view["color_title"].string, view["color_subtitle"].string) }
            navigation("presets", copy["preset"].string, view["preset_names"][Int(view["preset"].uint)].string)
            if view["show_clip"].bool {
                Toggle(copy["clip_hdr"].string, isOn: Binding(get: { view["clip"].bool }, set: { editor.change("clip_hdr_range", JSON($0)) })).accessibilityIdentifier("export-hdr-clip")
            }
        }
    }
    private func navigation(_ page: String, _ title: String, _ detail: String) -> some View {
        Button { if commit() { editor.act(["type": "navigate", "page": page]) } } label: {
            HStack { VStack(alignment: .leading) { Text(title); Text(detail).font(.caption).foregroundStyle(.secondary) }; Spacer(); SharedIcon(name: "go-next") }
        }.accessibilityIdentifier("export-page-\(page)")
    }
    private func back() {
        if page == "save_preset" { drafts["name"] = nil; editor.act(["type": "preset_cancel"]) }
        else if commit() { editor.act(["type": "back"]) }
    }
    private func actionBool(_ value: JSON, type: String) -> Binding<Bool> { Binding(get: { value.bool }, set: { editor.act(["type": type, "enabled": $0]) }) }
    private func choice(_ type: String, _ label: String, _ choices: JSON, _ selected: JSON) -> some View {
        FormPicker(label, selection: Binding(get: { selected.string }, set: { editor.change(type, JSON($0)) })) {
            ForEach(choices.array, id: \.stableKey) { Text($0["label"].string).tag($0["value"].string) }
        }.accessibilityIdentifier("export-\(type)")
    }
    private func number(_ id: String, _ label: String, _ value: JSON, control: String) -> some View {
        HStack {
            Text(label); Spacer()
            TextField(label, text: Binding(get: { drafts[id] ?? String(value.uint) }, set: { drafts[id] = $0 }))
                .focused($focused, equals: id).onSubmit {
                    if phonePage { commit(id) } else if commit() { editor.choose() }
                }.textFieldStyle(.roundedBorder).frame(width: 120)
                .accessibilityLabel(label).accessibilityIdentifier("export-\(id)")
        }
    }
    @discardableResult private func commit(_ id: String? = nil) -> Bool {
        for key in id.map({ [$0] }) ?? Array(drafts.keys) {
            guard let text = drafts[key] else { continue }
            if key == "name" { editor.act(["type": "name", "value": text]); drafts[key] = nil; continue }
            if key == "quality" && !view["show_quality"].bool || ["width", "height"].contains(key) && !view["show_size"].bool || key == "ppi" && !view["show_ppi"].bool { continue }
            do {
                let spec = key == "quality" ? "quality" : key == "ppi" ? "ppi" : "dimension"
                let value = try NativeTextContext.number(view["numeric"][spec], value: 0, operation: ["type": "expression", "text": text], language: language)["value"].uint
                switch key {
                case "width", "height": editor.act(["type": "bound", "axis": key == "width" ? 0 : 1, "value": value])
                case "ppi": editor.act(["type": "ppi", "value": value])
                default: editor.change("quality", JSON(value))
                }
                drafts[key] = nil
            } catch { editor.error = error.localizedDescription; return false }
        }
        return true
    }
}
