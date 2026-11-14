import SwiftUI
import UIKit

@MainActor private final class SheetState: ObservableObject {
    @Published var sizeClass: UserInterfaceSizeClass = .regular
    var constructions = 0
    var mounts = 0
    var dismissals = 0
    var page = false
    var contentFrame: () -> CGRect = { .zero }
}

private struct ContentMount: UIViewRepresentable {
    let state: SheetState
    var page = false
    final class View: UIView {
        let state: SheetState
        init(state: SheetState) { self.state = state; super.init(frame: .zero) }
        required init?(coder: NSCoder) { fatalError() }
        override func didMoveToWindow() { super.didMoveToWindow(); if window != nil { state.mounts += 1 } }
    }
    func makeUIView(context: Context) -> View { state.constructions += 1; return View(state: state) }
    func updateUIView(_ view: View, context: Context) {
        state.page = page
        state.contentFrame = { [weak view] in
            guard let view, let window = view.window else { return .zero }
            return view.convert(view.bounds, to: window)
        }
    }
}
private struct FittedContent: SwiftUI.View {
    @ObservedObject var state: SheetState
    var body: some SwiftUI.View {
        Color.clear.frame(width: 520, height: 360).background(ContentMount(state: state))
            .modifier(EditorLegacyFittedPresentation()).environment(\.horizontalSizeClass, state.sizeClass)
    }
}
private struct DraftContent: SwiftUI.View {
    @Environment(\.editorDialogPage) private var page
    @ObservedObject var store: EditorStore
    let state: SheetState
    var body: some SwiftUI.View {
        NewDrawingPresentation.dialog(store: store)
            .background(ContentMount(state: state, page: page))
            .modifier(EditorFittedPresentation()).modifier(EditorPopupPresentation())
    }
}
private struct DraftRoot: SwiftUI.View {
    @ObservedObject var store: EditorStore
    let state: SheetState
    var body: some SwiftUI.View {
        MetalCanvas(store: store).editorSheet(store: store, intent: store.state["new_drawing"]["dialog"],
            isPresented: Binding(get: { !store.state["new_drawing"].isNull }, set: { if !$0 { store.projectFiles.newDrawing(["op": "cancel"]) } })) {
                DraftContent(store: store, state: state)
            }.editorSheet(store: store, intent: store.snapshot["toolbar_prompt"]["dialog"],
                isPresented: Binding(get: { !store.snapshot["toolbar_prompt"].isNull }, set: { if !$0 { store.customize(["type": "cancel_toolbar"]) } }),
                onDismiss: { state.dismissals += 1 }) {
                WorkspaceDialog(store: store).background(ContentMount(state: state))
                    .modifier(EditorPopupPresentation())
            }.environment(\.editorPopupStore, store).environment(\.capyCommonCopy, store.bootstrap["common"])
                .environment(\.capyNativeCopy, store.catalog["native_copy"])
                .environment(\.capyProfileCopy, store.catalog["profile_copy"])
                .environment(\.capyInterfaceLanguage, store.interfaceLanguage)
    }
}

@MainActor private final class DisplayFrame: NSObject {
    private var continuation: CheckedContinuation<Void, Never>?
    private var link: CADisplayLink?
    func wait() async {
        await withCheckedContinuation { continuation in
            self.continuation = continuation
            let link = CADisplayLink(target: self, selector: #selector(frame))
            self.link = link
            link.add(to: .main, forMode: .common)
        }
    }
    @objc private func frame() {
        link?.invalidate(); link = nil
        continuation?.resume(); continuation = nil
    }
}

@main final class FittedSheetChecks: UIResponder, UIApplicationDelegate, UIWindowSceneDelegate {
    var window: UIWindow?

    func application(_ application: UIApplication, configurationForConnecting session: UISceneSession,
        options: UIScene.ConnectionOptions) -> UISceneConfiguration {
        let configuration = UISceneConfiguration(name: "Reorder lifecycle", sessionRole: session.role)
        configuration.delegateClass = FittedSheetChecks.self
        return configuration
    }

    @MainActor private func settle(_ window: UIWindow) async {
        for _ in 0..<4 {
            window.layoutIfNeeded()
            await DisplayFrame().wait()
        }
    }
    @MainActor private func sizingControllers(_ controller: UIViewController) -> [UIViewController] {
        let matched = String(reflecting: type(of: controller)).contains("EditorFittedSheet.Controller") ? [controller] : []
        return matched + controller.children.flatMap { sizingControllers($0) }
    }
    @MainActor private func textFields(_ view: UIView) -> [UITextField] {
        (view as? UITextField).map { [$0] } ?? view.subviews.flatMap(textFields)
    }
    @MainActor private func wait(_ window: UIWindow, _ label: String, until ready: () -> Bool) async {
        let deadline = Date().addingTimeInterval(15)
        while !ready() {
            precondition(Date() < deadline, label)
            window.layoutIfNeeded()
            await DisplayFrame().wait()
        }
    }
    @MainActor private func draft(_ window: UIWindow) async throws {
        let directory = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent("native-dialog-draft-" + UUID().uuidString)
        let store = EditorStore(platform: 0, persistence: EditorPersistence(root: directory))
        await wait(window, "The real dialog catalog loads") { !store.catalog.isNull }
        try await store.workspaces!.started()
        try await store.workspaces!.perform(["type": "switch", "id": "builtin:workspace:painter"])
        store.native?.resize(width: 800, height: 700, scale: 1)
        try await store.apply(["type": "invoke", "command": "new_document"])
        try await store.projectFiles.newDrawingStep(["op": "name", "text": "Resize draft"])
        try await store.projectFiles.newDrawingStep(["op": "width", "value": 140])
        let expected = store.state["new_drawing"]["values"].stableKey
        let state = SheetState()
        let root = UIHostingController(rootView: DraftRoot(store: store, state: state))
        window.rootViewController = root
        for size in [CGSize(width: 800, height: 700), CGSize(width: 479, height: 700), CGSize(width: 480, height: 700), CGSize(width: 800, height: 700)] {
            window.frame.size = size; root.view.frame = CGRect(origin: .zero, size: size)
            store.native?.resize(width: UInt32(size.width), height: UInt32(size.height), scale: 1)
            await wait(window, "The current shared viewport is published") { store.snapshot["layout"]["viewport"][0].number == size.width }
            let page = !store.snapshot["layout"]["phone"].isNull
            await wait(window, "The shared container and actual accepted draft are shown") {
                guard let controller = root.presentedViewController else { return false }
                return !controller.isBeingPresented && !controller.isBeingDismissed && state.page == page
                    && self.textFields(controller.view).contains { $0.accessibilityIdentifier == "number-entry-new-drawing-width" && $0.text == "140" }
            }
            precondition(store.state["new_drawing"]["name"].string == "Resize draft" && store.state["new_drawing"]["values"].stableKey == expected,
                "Switching native containers preserves the shared accepted draft")
            let controller = root.presentedViewController!
            precondition(page ? !(controller.presentationController is UISheetPresentationController) : controller.presentationController is UISheetPresentationController,
                "Only the shared page uses a cover; floating keeps the native sheet")
            if size.width == 479 {
                let field = textFields(controller.view).first { $0.accessibilityIdentifier == "number-entry-new-drawing-width" }!
                precondition(field.becomeFirstResponder())
                let mounts = state.mounts
                do {
                    let release = DispatchSemaphore(value: 0)
                    defer { release.signal() }
                    let projection: JSON = await withCheckedContinuation { done in
                        let size = CGSize(width: 479, height: 650)
                        window.frame.size = size; root.view.frame = CGRect(origin: .zero, size: size)
                        store.native!.resize(width: 479, height: 650, scale: 1)
                        store.native!.submit(2, JSON(["type": "dialog_presentation", "role": "editor",
                            "visible": ["x": 0, "y": 0, "width": 479, "height": 650]])) { reply in
                            done.resume(returning: reply ?? JSON())
                            precondition(release.wait(timeout: .now() + 15) == .success, "The fixture must release its private query worker")
                        }
                    }
                    await wait(window, "The resized viewport publishes before its held presentation query") { store.snapshot["layout"]["viewport"][1].number == 650 }
                    await settle(window)
                    precondition(root.presentedViewController === controller && state.mounts == mounts && field.isFirstResponder && field.text == "140",
                        "Pending bounds queries retain the native container, field and focused draft")
                    let viewport = store.snapshot["layout"]["viewport"]
                    let visible = CGRect(x: 0, y: 0, width: viewport[0].number,
                        height: max(0, viewport[1].number - store.obscuredBottom))
                    let bounds = projection["bounds"].rect.intersection(visible)
                    let actual = state.contentFrame()
                    precondition(projection["kind"].string == "page" && abs(actual.minX - bounds.minX) < 1
                        && abs(actual.minY - bounds.minY) < 1 && abs(actual.width - bounds.width) < 1
                        && abs(actual.height - bounds.height) < 1,
                        "The mounted page must already match current shared bounds while the owner query remains held: actual=\(actual), expected=\(bounds), viewport=\(store.snapshot["layout"]["viewport"].raw), keyboardInset=\(store.keyboardInset)")
                }
                precondition(root.presentedViewController === controller && state.mounts == mounts,
                    "A bounds reply must retain the mounted native container")
                let keyboard = CGRect(x: window.bounds.minX, y: window.bounds.maxY - 200, width: window.bounds.width, height: 200)
                NotificationCenter.default.post(name: UIResponder.keyboardWillChangeFrameNotification, object: nil,
                    userInfo: [UIResponder.keyboardFrameEndUserInfoKey: NSValue(cgRect: window.convert(keyboard, to: window.screen.coordinateSpace))])
                await wait(window, "Native keyboard end-frame notifications report the owning window's bottom overlap") { store.keyboardInset == 200 && store.state["phone"]["keyboard"].number == 200 }
                precondition(field.isFirstResponder && field.text == "140", "Refreshing native keyboard facts keeps the focused page field")
                let floating = CGRect(x: window.bounds.midX, y: window.bounds.midY, width: window.bounds.width / 2, height: 100)
                NotificationCenter.default.post(name: UIResponder.keyboardWillChangeFrameNotification, object: nil,
                    userInfo: [UIResponder.keyboardFrameEndUserInfoKey: NSValue(cgRect: window.convert(floating, to: window.screen.coordinateSpace))])
                await wait(window, "Floating keyboards do not reserve bottom clearance") { store.keyboardInset == 0 && store.state["phone"]["keyboard"].number == 0 }
                let hidden = CGRect(x: window.bounds.minX, y: window.bounds.maxY, width: window.bounds.width, height: 200)
                NotificationCenter.default.post(name: UIResponder.keyboardWillChangeFrameNotification, object: nil,
                    userInfo: [UIResponder.keyboardFrameEndUserInfoKey: NSValue(cgRect: window.convert(hidden, to: window.screen.coordinateSpace))])
                field.resignFirstResponder()
                await wait(window, "The owning window keyboard dismissal is reported") { store.keyboardInset == 0 && store.state["phone"]["keyboard"].number == 0 }
            }
        }
        try await store.projectFiles.newDrawingStep(["op": "cancel"])
        await wait(window, "The accepted draft closes normally") { root.presentedViewController == nil }
        try await store.apply(["type": "invoke", "command": "new_document"])
        await wait(window, "A closed native editor reopens with its current shared intent") {
            guard let controller = root.presentedViewController else { return false }
            return !controller.isBeingPresented && !controller.isBeingDismissed
                && controller.presentationController is UISheetPresentationController
                && self.textFields(controller.view).contains { $0.accessibilityIdentifier == "number-entry-new-drawing-width" }
        }
        try await store.projectFiles.newDrawingStep(["op": "cancel"])
        await wait(window, "The reopened editor closes normally") { root.presentedViewController == nil }
        print("PASS: accepted controller draft survives 479/480 native container changes")
        try await store.apply(["type": "customize", "action": ["type": "rename_toolbar", "panel": "toolbar"]])
        await wait(window, "The floating toolbar editor uses the native sheet") {
            guard let controller = root.presentedViewController else { return false }
            let prompt = store.snapshot["toolbar_prompt"]
            return !prompt["name"].isNull && controller.presentationController is UISheetPresentationController
                && self.textFields(controller.view).filter { $0.isEnabled && $0.text == prompt["name"].string }.count == 1
        }
        try await store.apply(["type": "customize", "action": ["type": "cancel_toolbar"]])
        await wait(window, "Rename closes before the independent Delete confirmation") {
            root.presentedViewController == nil && store.snapshot["toolbar_prompt"].isNull
        }
        precondition(state.dismissals == 1, "Closing Rename notifies its owner once")
        let mounts = state.mounts
        let size = CGSize(width: 375, height: 667)
        window.frame.size = size; root.view.frame = CGRect(origin: .zero, size: size)
        store.native?.resize(width: 375, height: 667, scale: 1)
        await wait(window, "The phone viewport publishes before its alert") { !store.snapshot["layout"]["phone"].isNull }
        try await store.apply(["type": "customize", "action": ["type": "delete_toolbar", "panel": "toolbar"]])
        await wait(window, "The Delete confirmation uses the compact phone alert") {
            let frame = state.contentFrame()
            return store.snapshot["toolbar_prompt"]["dialog"]["role"].string == "alert"
                && root.presentedViewController == nil && state.mounts > mounts
                && frame.height > 0 && frame.height < size.height / 2
                && window.bounds.contains(frame)
        }
        precondition(state.dismissals == 1, "Opening Delete keeps its confirmation owner active")
        try await store.apply(["type": "customize", "action": ["type": "cancel_toolbar"]])
        await wait(window, "The custom phone alert closes normally") { store.snapshot["toolbar_prompt"].isNull && state.dismissals == 2 }
        print("PASS: native editors close before independent compact phone confirmations")
    }
    func scene(_ scene: UIScene, willConnectTo session: UISceneSession, options: UIScene.ConnectionOptions) {
        guard let scene = scene as? UIWindowScene else { return }
        Task { @MainActor in
            let window = UIWindow(windowScene: scene)
            self.window = window
            let presenter = UIViewController()
            presenter.traitOverrides.horizontalSizeClass = .regular
            window.rootViewController = presenter
            window.makeKeyAndVisible()
            await settle(window)
            let state = SheetState()
            let sheet = UIHostingController(rootView: FittedContent(state: state))
            sheet.modalPresentationStyle = .pageSheet
            presenter.present(sheet, animated: false)
            await settle(window)
            let controllers = sizingControllers(sheet)
            precondition(controllers.count == 1, "The sheet must mount one sizing controller")
            let sizing = controllers[0]
            precondition(state.constructions == 1 && state.mounts == 1, "The sheet must mount its content once")
            precondition(abs(sheet.preferredContentSize.height - 360) < 1, "Regular sheets must use the content's ideal height")
            for sizeClass: UserInterfaceSizeClass in [.compact, .regular] {
                state.sizeClass = sizeClass
                await settle(window)
                precondition(presenter.traitCollection.horizontalSizeClass == .regular, "The presenting size class must remain regular")
                precondition(state.constructions == 1 && state.mounts == 1, "Changing child size class must preserve mounted sheet content")
                let current = sizingControllers(sheet)
                precondition(current.count == 1 && current[0] === sizing, "Changing child size class must preserve the sizing controller")
                precondition(abs(sheet.preferredContentSize.height - 360) < 1, "Child size class must not change the presenting sheet's ideal height")
            }
            for sizeClass: UIUserInterfaceSizeClass in [.compact, .regular] {
                presenter.traitOverrides.horizontalSizeClass = sizeClass
                await settle(window)
                precondition(state.sizeClass == .regular, "The child size class must remain regular")
                precondition(state.constructions == 1 && state.mounts == 1, "Changing presenter traits must preserve mounted sheet content")
                let current = sizingControllers(sheet)
                precondition(current.count == 1 && current[0] === sizing, "Changing presenter traits must preserve the sizing controller")
                precondition(sheet.sheetPresentationController?.prefersPageSizing == (sizeClass == .compact), "Native page sizing must follow the presenter's size class")
                if sizeClass == .regular {
                    precondition(abs(sheet.preferredContentSize.height - 360) < 1, "Returning to regular presentation must restore the ideal height")
                }
            }
            presenter.dismiss(animated: false)
            await settle(window)
            do { try await draft(window) }
            catch { print("DRAFT setup failed: \(error)"); fflush(stdout); exit(1) }
            print("PASS: fitted sheet sizing: stable content and sizing ownership across child size classes")
            fflush(stdout)
            exit(0)
        }
    }
}
