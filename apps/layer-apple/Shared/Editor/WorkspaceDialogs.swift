import SwiftUI

struct WorkspaceDialogs: ViewModifier {
    @ObservedObject var store: EditorStore
    private var open: Bool {
        ["picker", "toolbar_prompt", "toolbar_manager"].contains { !store.snapshot[$0].isNull }
            || !store.state["customization"]["control"].isNull
    }
    private var intent: JSON {
        for key in ["toolbar_prompt", "picker", "toolbar_manager"] where !store.snapshot[key].isNull {
            return store.snapshot[key]["dialog"]
        }
        return store.catalog["native_copy"]["color"]["control_dialog"]
    }
    func body(content: Content) -> some View {
        content.editorSheet(store: store, intent: intent, isPresented: Binding(get: { open }, set: { _ in })) {
            WorkspaceDialog(store: store).interactiveDismissDisabled().modifier(EditorPopupPresentation())
        }
    }
}

struct WorkspaceDialog: View {
    @ObservedObject var store: EditorStore
    @Environment(\.editorDialogPage) private var page
    @Environment(\.editorDialogAvailableHeight) private var availableHeight
    #if os(iOS)
    @State private var searchFocused = false
    #else
    @FocusState private var searchFocused: Bool
    #endif
    @FocusState private var nameFocused: Bool
    @State private var resultsHeight: CGFloat = 0
    @State private var firstRowHeight: CGFloat = 0
    @State private var countHeight: CGFloat = 0
    @State private var showsCount = true
    private var snapshot: SnapshotProjection { store.snapshot }
    private var common: JSON { store.bootstrap["common"] }
    private var palette: EditorPalette { EditorPalette(source: store.state["palette"]) }
    private var alert: Bool { snapshot["toolbar_prompt"]["dialog"]["role"].string == "alert" }
    var body: some View {
        Group {
            if !snapshot["toolbar_prompt"].isNull { prompt(snapshot["toolbar_prompt"]) }
            else if !snapshot["picker"].isNull { picker(snapshot["picker"]) }
            else if !snapshot["toolbar_manager"].isNull { manager(snapshot["toolbar_manager"]) }
            else { colorPopup }
        }.padding(alert || page ? 0 : 24).frame(minWidth: alert ? nil : page ? 0 : 320,
            idealWidth: alert || page ? nil : 500, maxWidth: alert ? nil : page ? .infinity : 560)
            .buttonStyle(.bordered).modifier(EditorFittedPresentation())
            .id(!snapshot["toolbar_prompt"].isNull ? snapshot["toolbar_prompt"]["title"].string : "workspace")
            .onNativeTextKey(.escape) {
                escape()
                return .handled
            }
    }
    private func escape() {
        let reply = DialogUI.resolve(["kind": "escape", "subpage": false,
            "search_focused": searchFocused, "search_has_text": !snapshot["picker"]["query"].string.isEmpty]).string
        if reply == "clear_search" { store.customize(["type": "picker_search", "query": ""]) }
        else if !snapshot["toolbar_prompt"].isNull { action("cancel_toolbar") }
        else if !snapshot["picker"].isNull { action("cancel_tools") }
        else if !snapshot["toolbar_manager"].isNull { action("close_toolbar_manager") }
        else { action("close_control") }
    }
    private func action(_ name: String) { store.customize(["type": name]) }
    private func error(_ value: JSON) -> some View {
        Group { if !value["error"].isNull { Text(value["error"].string).foregroundStyle(.red).accessibilityIdentifier("workspace-validation") } }
    }
    private func fitCount() {
        guard firstRowHeight > 0, countHeight > 0 else { return }
        showsCount = !page || (resultsHeight + (showsCount ? countHeight + 14 : 0)) - countHeight - 14 >= firstRowHeight
    }
    private func picker(_ view: JSON) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            if page {
                EditorPageHeader(intent: view["dialog"], title: view["title"].string, identifier: "tool-picker",
                    primaryLabel: view["confirm_label"].string, primaryEnabled: view["can_confirm"].bool,
                    primaryIdentifier: "tool-picker-confirm", cancelsOnEscape: false, cancel: { action("cancel_tools") }, primary: { action("confirm_tools") })
            } else { Text(view["title"].string).font(.headline) }
            if !view["name"].isNull {
                EditorTextField(view["name_label"].string, value: view["name"].string) { store.customize(["type": "picker_name", "name": $0]) }
                    .focused($nameFocused).onAppear { nameFocused = page }
                    .textFieldStyle(.roundedBorder).accessibilityIdentifier("toolbar-name")
            }
            EditorTextField(view["search_hint"].string, value: view["query"].string) { store.customize(["type": "picker_search", "query": $0]) }
                #if os(iOS)
                .editorFocus($searchFocused, identifier: "tool-picker-search")
                #else
                .focused($searchFocused)
                #endif
                .editorSearchInput()
                .textFieldStyle(.roundedBorder).accessibilityIdentifier("tool-picker-search")
                .onAppear {
                    #if os(iOS)
                    searchFocused = NativeTextContext.keyboardInput && view["name"].isNull
                    #endif
                }
            EditorScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(view["choices"].array, id: \.workspaceChoiceKey) { choice in
                        VStack(spacing: 0) {
                            Button {
                                store.customize(["type": "picker_select", "control": choice["control"].raw, "selected": !choice["selected"].bool])
                            } label: {
                                HStack(spacing: 12) {
                                    SharedIcon(name: choice["icon"].string)
                                    VStack(alignment: .leading, spacing: 3) {
                                        Text(choice["label"].string).fontWeight(.medium)
                                        Text(choice["description"].string).font(.caption).foregroundStyle(.secondary)
                                    }.frame(maxWidth: .infinity, alignment: .leading)
                                    SharedIcon(name: choice["selected"].bool ? "selection-checked" : "selection-empty")
                                }.frame(minHeight: 50).padding(.horizontal, 8).contentShape(Rectangle())
                            }.buttonStyle(.plain)
                                .disabled(view["disabled"].array.contains { $0.stableKey == choice["control"].stableKey })
                                .accessibilityIdentifier("tool-choice-" + choice["label"].string)
                                .accessibilityAddTraits(choice["selected"].bool ? .isSelected : [])
                            EditorLine(color: palette.settingsBorder)
                        }.onGeometryChange(for: CGFloat.self, of: { $0.size.height }) { height in
                            if choice.workspaceChoiceKey == view["choices"].array.first?.workspaceChoiceKey {
                                firstRowHeight = height; fitCount()
                            }
                        }
                    }
                }
            }.frame(minHeight: page ? 0 : 120, idealHeight: page ? nil : 340, maxHeight: page ? .infinity : 400)
                .accessibilityIdentifier("tool-picker-results")
                .onGeometryChange(for: CGFloat.self, of: { $0.size.height }) { height in
                    resultsHeight = height; fitCount()
                }
                .onChange(of: page) { _, _ in fitCount() }
            error(view)
            if showsCount { HStack {
                Text(view["selected_label"].string).foregroundStyle(.secondary).accessibilityIdentifier("tool-picker-count")
                Spacer()
                if !page {
                Button(common["cancel"].string, role: .cancel) { escape() }.keyboardShortcut(.cancelAction)
                Button(view["confirm_label"].string) { action("confirm_tools") }
                    .buttonStyle(.borderedProminent).foregroundStyle(.white)
                    .keyboardShortcut(.defaultAction).disabled(!view["can_confirm"].bool)
                    .accessibilityIdentifier("tool-picker-confirm")
                }
            }.onGeometryChange(for: CGFloat.self, of: { $0.size.height }) { height in
                    countHeight = height; fitCount()
                }
            }
        }.padding(page ? 8 : 0).accessibilityElement(children: .contain).accessibilityIdentifier("tool-picker")
    }
    @ViewBuilder private func prompt(_ view: JSON) -> some View {
        Group {
            if alert {
                EditorAlertLayout(viewport: CGSize(width: snapshot["layout"]["viewport"][0].number,
                    height: availableHeight ?? max(0, snapshot["layout"]["viewport"][1].number - store.obscuredBottom)),
                    preferWide: view["dialog"]["frame"]["prefer_wide"].bool,
                    fontPixels: CGFloat(store.catalog["text_size_pt"].number * 4 / 3)) {
                    Text(view["title"].string).font(.headline).multilineTextAlignment(.center)
                    EditorScrollView { promptBody(view) }
                    promptCancel(view)
                    promptConfirm(view)
                }
            } else {
                VStack(alignment: .leading, spacing: 16) {
                    ZStack {
                        if page {
                            EditorPageHeader(intent: view["dialog"], title: view["title"].string, identifier: "toolbar-prompt",
                                primaryLabel: view["confirm_label"].string, primaryEnabled: view["can_confirm"].bool,
                                primaryIdentifier: "toolbar-prompt-confirm", cancel: { action("cancel_toolbar") }, primary: { action("confirm_toolbar") })
                        } else { Text(view["title"].string).font(.headline) }
                    }
                    EditorScrollView { promptBody(view).padding(page ? 16 : 0) }
                    if !page { HStack { Spacer(); promptCancel(view); promptConfirm(view) } }
                }
            }
        }.accessibilityElement(children: .contain).accessibilityIdentifier("toolbar-prompt")
    }
    private func promptBody(_ view: JSON) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            if !view["message"].string.isEmpty { Text(view["message"].string) }
            if !view["name"].isNull {
                EditorTextField(view["name_label"].string, value: view["name"].string) { store.customize(["type": "toolbar_name", "name": $0]) }
                    .focused($nameFocused).onAppear { nameFocused = page }
                    .textFieldStyle(.roundedBorder).accessibilityIdentifier("toolbar-name")
            }
            error(view)
        }
    }
    private func promptCancel(_ view: JSON) -> some View {
        Button(view["cancel_label"].string, role: .cancel) { action("cancel_toolbar") }
            .keyboardShortcut(.cancelAction).accessibilityIdentifier("toolbar-prompt-cancel")
    }
    private func promptConfirm(_ view: JSON) -> some View {
        Button(view["confirm_label"].string, role: view["destructive"].bool ? .destructive : nil) { action("confirm_toolbar") }
            .buttonStyle(.borderedProminent)
            .foregroundStyle(view["destructive"].bool ? .white : palette.accentForeground)
            .tint(view["destructive"].bool ? .red : palette.accent)
            .keyboardShortcut(.defaultAction).disabled(!view["can_confirm"].bool)
            .accessibilityIdentifier("toolbar-prompt-confirm")
    }
    private func manager(_ view: JSON) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            if page {
                EditorPageHeader(intent: view["dialog"], title: view["title"].string, identifier: "toolbar-manager",
                    cancel: { action("close_toolbar_manager") })
            } else { Text(view["title"].string).font(.headline) }
            Text(view["description"].string).foregroundStyle(.secondary)
            EditorScrollView {
                VStack(spacing: 0) {
                    if view["toolbars"].array.isEmpty { Text(view["empty_label"].string).padding(40) }
                    ForEach(view["toolbars"].array, id: \.workspacePanelKey) { toolbar in
                        let selected = toolbar["panel"].string == view["selected"].string
                        Button { store.customize(["type": "select_managed_toolbar", "panel": toolbar["panel"].raw]) } label: {
                            HStack(spacing: 12) {
                                SharedIcon(name: toolbar["icon"].string)
                                VStack(alignment: .leading, spacing: 3) {
                                    Text(toolbar["title"].string)
                                    Text(toolbar["subtitle"].string).foregroundStyle(.secondary).font(.caption)
                                }.frame(maxWidth: .infinity, alignment: .leading)
                            }.padding(10).contentShape(Rectangle())
                        }.buttonStyle(EditorControlButtonStyle(selected: selected))
                            .accessibilityAddTraits(selected ? .isSelected : [])
                            .accessibilityIdentifier("managed-toolbar-" + toolbar["panel"].string)
                        EditorLine(color: palette.settingsBorder)
                    }
                }
            }.frame(minHeight: page ? 0 : 120, idealHeight: page ? nil : 250, maxHeight: page ? .infinity : 340)
            HStack {
                if !page { Button(view["close_label"].string, role: .cancel) { action("close_toolbar_manager") }.keyboardShortcut(.cancelAction) }
                Spacer()
                Button(view["delete_label"].string, role: .destructive) { store.customize(view["delete_action"].object) }
                    .buttonStyle(.borderedProminent).tint(.red).foregroundStyle(.white)
                    .disabled(view["delete_action"].isNull).accessibilityIdentifier("delete-managed-toolbar")
            }
        }.padding(page ? 8 : 0).accessibilityElement(children: .contain).accessibilityIdentifier("toolbar-manager")
    }
    private var colorPopup: some View {
        VStack(alignment: .leading, spacing: 14) {
            if store.state["customization"]["control"].string == "brush_color" {
                ColorPanel(store: store).frame(maxWidth: 320)
                PaintColorControls(store: store)
            }
            HStack { Spacer(); Button(common["done"].string, role: .cancel) { action("close_control") }.keyboardShortcut(.cancelAction) }
        }.accessibilityElement(children: .contain).accessibilityIdentifier("toolbar-control-popup")
    }
}

private extension JSON {
    var workspaceChoiceKey: String { self["control"].stableKey }
    var workspacePanelKey: String { self["panel"].string }
}
