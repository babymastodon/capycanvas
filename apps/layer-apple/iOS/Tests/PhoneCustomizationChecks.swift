import XCTest
import UIKit

extension EditorLaunchTests {
    @MainActor func testPhoneCustomizationLight() throws { try checkPhoneCustomization(theme: "light") }
    @MainActor func testPhoneCustomizationDark() throws { try checkPhoneCustomization(theme: "dark") }
    @MainActor func testPhoneFramePresetsLight() throws { try checkPhoneFramePresets(theme: "light") }
    @MainActor func testPhoneFramePresetsDark() throws { try checkPhoneFramePresets(theme: "dark") }

    @MainActor private func checkPhoneFramePresets(theme: String) throws {
        let app = try phoneCaptureApplication(theme: theme)
        defer { app.terminate() }
        let initial = try XCTUnwrap(app.launchEnvironment["CAPY_INITIAL_ACTIONS"]?.data(using: .utf8))
        let baseline = try XCTUnwrap(try JSONSerialization.jsonObject(with: initial) as? [[String: Any]])
        let ink = 46.0 / 255.0
        for preset in ["painter", "illustrator", "photographer"] {
            for swapped in [false, true] {
                var actions: [[String: Any]] = [["type": "workspace_manager", "command": ["type": "switch", "id": "builtin:workspace:" + preset]],
                    ["type": "select_brush", "id": 2], ["type": "reset_tool_setting", "id": "size"],
                    ["type": "reset_tool_setting", "id": "flow"], ["type": "set_color", "rgba": [ink, ink, ink, 1]]]
                if swapped {
                    actions += [["type": "set_brush_size", "value": 40], ["type": "set_color", "rgba": [0, 0, 0, 1]]]
                }
                try phoneRelaunch(app, theme: theme, actions: baseline + actions)
                phoneSetSwapped(swapped, in: app)
                let tools = phoneTools(in: app)
                XCTAssertGreaterThan(tools.count, 0)
                let bounds = workspaceViewport(in: app).frame
                let withinWindow = tools.allSatisfy {
                    let frame = $0.frame
                    return frame.width > 0 && frame.height > 0 && frame.minX.isFinite && frame.minY.isFinite && bounds.contains(frame)
                }
                if !withinWindow {
                    attachEditor(in: app, name: "phone-frame-tools-outside-window")
                    let tree = XCTAttachment(string: "Preset: \(preset), swapped: \(swapped)\n\(app.debugDescription)")
                    tree.name = "phone-frame-tools-outside-window-hierarchy"; tree.lifetime = .keepAlways; add(tree)
                }
                XCTAssertTrue(withinWindow)
                phoneCapture(in: app, id: phoneCaptureID("frame", swapped ? "frame-swapped" : "frame-rest"), theme: theme, fixture: actions, extra: ["swapped": swapped])
                for kind in ["size", "strength"] {
                    let slider = app.descendants(matching: .any)["phone-slider-" + kind].firstMatch
                    XCTAssertTrue(slider.waitForExistence(timeout: 10))
                    let start = slider.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
                    let vertical = slider.frame.height > slider.frame.width
                    let end = start.withOffset(CGVector(dx: vertical ? 70 : 0, dy: vertical ? 0 : -70))
                    start.press(forDuration: 0.05, thenDragTo: end, withVelocity: .slow, thenHoldForDuration: 0.2)
                    let menu = app.descendants(matching: .any)["phone-menu"].firstMatch
                    workspaceActivate(app.buttons["phone-capy"])
                    phoneWaitForMenuTitle(app.buttons["phone-capy"].label, in: app)
                    workspaceActivate(app.buttons["phone-capy"])
                    XCTAssertTrue(menu.waitForNonExistence(timeout: 10))
                }
            }
        }
        let actions: [[String: Any]] = [["type": "reset_tool_setting", "id": "size"], ["type": "reset_tool_setting", "id": "flow"],
            ["type": "set_color", "rgba": [ink, ink, ink, 1]], ["type": "phone", "action": ["kind": "insert_tools", "before": NSNull()]]]
            + ["drawing_brush", "eraser"].map { command -> [String: Any] in ["type": "customize", "action": ["type": "picker_select", "control": ["kind": "command", "command": command], "selected": true]] }
            + [["type": "customize", "action": ["type": "confirm_tools"]]]
        try phoneRelaunch(app, theme: theme, actions: baseline + actions)
        phoneSetSwapped(false, in: app)
        XCTAssertEqual(phoneTools(in: app).count, 10)
        phoneCapture(in: app, id: phoneCaptureID("frame", "frame-ten-tools"), theme: theme, fixture: actions)
    }

    @MainActor private func checkPhoneCustomization(theme: String) throws {
        let app = try phoneCaptureApplication(theme: theme)
        defer { app.terminate() }
        let initial = phoneTools(in: app)
        XCTAssertGreaterThan(initial.count, 2)
        let identifiers = initial.map(\.identifier)
        func editing(_ tile: XCUIElement) {
            let selected = phoneObservedTools(in: app)?.filter { $0["selected"] as? Bool == true }.compactMap { $0["id"] as? Int }
            workspacePoint(tile)?.press(forDuration: 1)
            XCTAssertTrue(app.descendants(matching: .any)["workspace-context-menu"].firstMatch.waitForExistence(timeout: 10))
            let menu = app.descendants(matching: .any)["workspace-context-menu"].firstMatch
            let rows = menu.buttons.allElementsBoundByIndex.sorted { $0.frame.minY < $1.frame.minY }
            let title = menu.staticTexts["editor-menu-title"]
            if let first = rows.first, let last = rows.last {
                let top = title.exists ? title.frame.minY - (first.frame.minY - title.frame.maxY) : first.frame.minY
                XCTAssertGreaterThanOrEqual(top - menu.frame.minY, 5)
                XCTAssertGreaterThanOrEqual(menu.frame.maxY - last.frame.maxY, 5)
                XCTAssertEqual(top - menu.frame.minY, menu.frame.maxY - last.frame.maxY, accuracy: 1)
            } else { XCTFail("The held tool menu must expose its action rows") }
            XCTAssertEqual(phoneObservedTools(in: app)?.filter { $0["selected"] as? Bool == true }.compactMap { $0["id"] as? Int }, selected)
        }
        func menu(_ label: String) {
            workspaceActivate(app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH %@ AND label == %@", "menu-action-", label)).firstMatch)
        }
        func history(_ label: String, restoring sequence: [String]) {
            phoneAlertCommand("Window", label: label, in: app)
            expectation(for: NSPredicate { _, _ in self.phoneTools(in: app).map(\.identifier) == sequence }, evaluatedWith: app)
            waitForExpectations(timeout: 10)
            XCTAssertTrue(app.descendants(matching: .any)["phone-menu"].firstMatch.waitForNonExistence(timeout: 10))
        }
        phoneCapture(in: app, id: phoneCaptureID("customize", "rest"), theme: theme, fixture: [])
        guard phoneObservedTools(in: app)?.contains(where: {
            ($0["id"] as? Int).map { "phone-tool-" + String($0) } == identifiers[0] && $0["selected"] as? Bool == false
        }) == true else { return XCTFail("Hold the actual unselected first tool without activating it") }
        editing(app.buttons[identifiers[0]])
        phoneCapture(in: app, id: phoneCaptureID("customize", "customize-tile-menu"), theme: theme, fixture: [])
        menu("Remove Tool")
        let removed = Array(identifiers.dropFirst())
        expectation(for: NSPredicate { _, _ in self.phoneTools(in: app).map(\.identifier) == removed }, evaluatedWith: app)
        waitForExpectations(timeout: 10)
        history("Undo Layout Change", restoring: identifiers)
        history("Redo Layout Change", restoring: removed)
        let picker = app.descendants(matching: .any)["tool-picker"].firstMatch
        let confirm = app.buttons["tool-picker-confirm"], cancel = app.buttons["tool-picker-cancel"]
        for count in [2, 1] {
            editing(app.buttons[removed[0]]); menu("Insert Tools…")
            XCTAssertTrue(cancel.waitForExistence(timeout: 10)); XCTAssertFalse(confirm.isEnabled)
            let choices = picker.buttons.matching(NSPredicate(format: "identifier BEGINSWITH %@ AND identifier != %@", "tool-choice-", "tool-choice-Divider"))
                .allElementsBoundByIndex.prefix(count)
            XCTAssertEqual(choices.count, count)
            for choice in choices {
                revealEditorControl(choice, in: picker.scrollViews.firstMatch); workspaceActivate(choice)
                XCTAssertTrue(choice.isSelected)
            }
            XCTAssertTrue(confirm.isEnabled)
            if count == 2 { phoneCapture(in: app, id: phoneCaptureID("customize", "customize-insert-two"), theme: theme, fixture: []) }
            workspaceActivate(confirm)
            XCTAssertTrue(cancel.waitForNonExistence(timeout: 10))
            XCTAssertEqual(phoneTools(in: app).count, count == 2 ? 9 : 10)
        }
        let sequence = phoneTools(in: app).map(\.identifier)
        editing(app.buttons[removed[0]]); menu("Insert Tools…")
        XCTAssertTrue(cancel.waitForExistence(timeout: 10))
        let choices = picker.buttons.matching(NSPredicate(format: "identifier BEGINSWITH %@", "tool-choice-")).allElementsBoundByIndex
        XCTAssertFalse(choices.isEmpty)
        for choice in choices { XCTAssertEqual(choice.isEnabled, choice.identifier == "tool-choice-Divider") }
        phoneCapture(in: app, id: phoneCaptureID("customize", "customize-capacity"), theme: theme, fixture: [])
        let firstChoice = choices[0].identifier, search = app.textFields["tool-picker-search"]
        XCTAssertFalse(app.keyboards.firstMatch.exists)
        XCTAssertFalse(app.textFields.matching(NSPredicate(format: "identifier == %@ AND hasKeyboardFocus == YES", search.identifier)).firstMatch.exists)
        replaceEditorText(search, with: "Divider", in: app)
        let divider = picker.buttons["tool-choice-Divider"]
        XCTAssertTrue(divider.waitForExistence(timeout: 10))
        let results = app.descendants(matching: .any)["tool-picker-results"].firstMatch
        let countLine = app.descendants(matching: .any)["tool-picker-count"].firstMatch
        var measuredCount: CGFloat?
        func fits() {
            XCTAssertTrue(divider.isHittable)
            XCTAssertGreaterThanOrEqual(divider.frame.minY, results.frame.minY - 1)
            XCTAssertLessThanOrEqual(divider.frame.maxY + 1, results.frame.maxY + 1)
            XCTAssertLessThanOrEqual(search.frame.maxY, divider.frame.minY + 1)
            XCTAssertTrue(cancel.isHittable); XCTAssertTrue(confirm.isHittable)
            if countLine.exists { measuredCount = countLine.frame.height }
            if let height = measuredCount {
                let available = results.frame.height + (countLine.exists ? height + 14 : 0)
                XCTAssertEqual(countLine.exists, available - height - 14 >= divider.frame.height + 1)
            }
            if app.keyboards.firstMatch.exists {
                XCTAssertLessThanOrEqual(results.frame.maxY, app.keyboards.firstMatch.frame.minY + 1)
                if countLine.exists { XCTAssertLessThanOrEqual(countLine.frame.maxY, app.keyboards.firstMatch.frame.minY + 1) }
            }
        }
        search.typeText("\n")
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 10))
        let originalOrientation = XCUIDevice.shared.orientation
        for orientation in phoneCaptureOrientations + [originalOrientation] {
            XCUIDevice.shared.orientation = orientation
            expectation(for: NSPredicate { _, _ in
                let bounds = app.windows.firstMatch.frame
                return orientation.isLandscape ? bounds.width > bounds.height : bounds.height > bounds.width
            }, evaluatedWith: app)
            waitForExpectations(timeout: 10)
            fits()
            workspaceActivate(search)
            XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 10))
            fits()
            search.typeText("\n")
            XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 10))
            fits()
        }
        workspaceActivate(divider); XCTAssertTrue(divider.isSelected)
        replaceEditorText(search, with: "", in: app)
        dismissEditorKeyboard(in: app)
        XCTAssertTrue(picker.buttons[firstChoice].waitForExistence(timeout: 10)); XCTAssertTrue(confirm.isEnabled)
        phoneCapture(in: app, id: phoneCaptureID("customize", "customize-capacity-divider"), theme: theme, fixture: [])
        workspaceActivate(cancel)
        XCTAssertTrue(cancel.waitForNonExistence(timeout: 10))
        XCTAssertEqual(phoneTools(in: app).map(\.identifier), sequence)
        let from = app.buttons[sequence[0]].coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        let third = app.buttons[sequence[2]], horizontal = app.buttons[sequence[0]].frame.midY == third.frame.midY
        let to = third.coordinate(withNormalizedOffset: CGVector(dx: horizontal ? 0.95 : 0.5, dy: horizontal ? 0.5 : 0.95))
        from.press(forDuration: 1, thenDragTo: to, withVelocity: .slow, thenHoldForDuration: 0.2)
        XCTAssertTrue(app.descendants(matching: .any)["workspace-context-menu"].firstMatch.waitForNonExistence(timeout: 10))
        XCTAssertEqual(phoneTools(in: app).map(\.identifier), [sequence[1], sequence[2], sequence[0]] + Array(sequence.dropFirst(3)))
        history("Undo Layout Change", restoring: sequence)
        let picked = app.buttons[sequence[0]].coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        picked.press(forDuration: 1, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)),
            withVelocity: .slow, thenHoldForDuration: 0.2)
        XCTAssertEqual(phoneTools(in: app).map(\.identifier), sequence)
        XCTAssertTrue(app.descendants(matching: .any)["workspace-context-menu"].firstMatch.waitForNonExistence(timeout: 10))
        phoneCapture(in: app, id: phoneCaptureID("customize", "customize-offbar-cancelled"), theme: theme, fixture: [])
        for identifier in sequence {
            let count = phoneTools(in: app).count
            editing(app.buttons[identifier]); menu("Remove Tool")
            expectation(for: NSPredicate { _, _ in self.phoneTools(in: app).count == count - 1 }, evaluatedWith: app)
            waitForExpectations(timeout: 10)
        }
        XCTAssertTrue(phoneTools(in: app).isEmpty)
        phoneCapture(in: app, id: phoneCaptureID("frame", "frame-empty"), theme: theme, fixture: [])
        let bar = app.descendants(matching: .any)["phone-bar-0"].firstMatch
        XCTAssertTrue(bar.exists)
        bar.press(forDuration: 1)
        XCTAssertTrue(app.descendants(matching: .any)["workspace-context-menu"].firstMatch.waitForExistence(timeout: 10))
        phoneCapture(in: app, id: phoneCaptureID("customize", "customize-empty-toolbar"), theme: theme, fixture: [])
        menu("Add Tools…")
        XCTAssertTrue(cancel.waitForExistence(timeout: 10))
        workspaceActivate(cancel)
        XCTAssertTrue(phoneTools(in: app).isEmpty)
        phoneAlertCommand("Window", label: "Restore Starting Layout…", in: app)
        XCTAssertTrue(app.buttons["workspace-form-confirm"].waitForExistence(timeout: 30))
        phoneCapture(in: app, id: phoneCaptureID("customize", "customize-phone-restore-confirm"), theme: theme, fixture: [])
        workspaceActivate(app.buttons["workspace-form-confirm"])
        XCTAssertTrue(app.buttons["workspace-form-confirm"].waitForNonExistence(timeout: 30))
        XCTAssertGreaterThan(phoneTools(in: app).count, 0)
        phoneCapture(in: app, id: phoneCaptureID("customize", "customize-phone-restored"), theme: theme, fixture: [])
    }
}

extension XCTestCase {
    @MainActor func phoneTools(in app: XCUIApplication) -> [XCUIElement] {
        app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH %@ AND identifier != %@", "phone-tool-", "phone-tool-settings"))
            .allElementsBoundByIndex.sorted {
                abs($0.frame.minY - $1.frame.minY) < 1 ? $0.frame.minX < $1.frame.minX : $0.frame.minY < $1.frame.minY
            }
    }
}
