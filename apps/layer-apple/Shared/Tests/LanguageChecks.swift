import XCTest

extension XCTestCase {
    @MainActor func checkLiveInterfaceLanguage(in app: XCUIApplication, theme: String) {
        app.launchEnvironment["CAPY_INITIAL_ACTIONS"] = #"[{"type":"set_theme","theme":"\#(theme)"},{"type":"workspace_manager","command":{"type":"switch","id":"builtin:workspace:illustrator"}},{"type":"invoke","command":"settings"},{"type":"preferences","action":{"type":"page","page":"appearance"}}]"#
        app.launch()
        waitForEditorInteraction(in: app)
        let done = app.buttons["settings-done"], layers = app.buttons["panel-tab-layers"]
        #if os(macOS)
        let language = app.popUpButtons["preference-language"]
        #else
        let language = app.buttons["preference-language"]
        #endif
        func expectLabel(_ element: XCUIElement, _ label: String) {
            expectation(for: NSPredicate(format: "label == %@", label), evaluatedWith: element); waitForExpectations(timeout: 15)
        }
        func choose(_ label: String) {
            XCTAssertTrue(language.waitForExistence(timeout: 20))
            workspaceActivate(language)
            #if os(macOS)
            let option = language.menuItems[label].firstMatch
            XCTAssertTrue(option.waitForExistence(timeout: 5)); option.click()
            #else
            let option = app.buttons[label].firstMatch
            XCTAssertTrue(option.waitForExistence(timeout: 5)); workspaceActivate(option)
            #endif
        }
        choose("日本語")
        expectLabel(done, "完了")
        attachEditor(in: app, name: "language-settings-ja-" + theme)
        workspaceActivate(done)
        XCTAssertTrue(done.waitForNonExistence(timeout: 10))
        expectLabel(layers, "レイヤー")
        attachEditor(in: app, name: "language-editor-ja-" + theme)
        workspaceActivate(app.buttons["settings-button"])
        choose("English")
        expectLabel(done, "Done")
        workspaceActivate(done)
        expectLabel(layers, "Layers")
        capturePaintEditor(in: app, theme: theme)
        editorTool("Pencil", in: app)
        let sketch = app.buttons["workspace-switch-builtin:workspace:painter"]
        let paint = app.buttons["workspace-switch-builtin:workspace:illustrator"]
        workspaceActivate(sketch)
        let sizeCap = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH %@ AND label == %@", "slider-cap-", "Brush size")).firstMatch
        XCTAssertTrue(sizeCap.waitForExistence(timeout: 15))
        let capID = sizeCap.identifier
        let preview = app.descendants(matching: .any)["brush-slider-preview"].firstMatch
        func bookmarkLabels(_ add: String, _ remove: String, _ tag: String) {
            workspaceActivate(app.buttons[capID])
            XCTAssertTrue(preview.waitForExistence(timeout: 10))
            let bookmark = app.buttons["slider-bookmark"]
            let initial = bookmark.label == remove ? remove : add
            let opposite = initial == remove ? add : remove
            expectLabel(bookmark, initial)
            workspaceActivate(bookmark); expectLabel(bookmark, opposite)
            workspaceActivate(bookmark); expectLabel(bookmark, initial)
            attachEditor(in: app, name: "language-bookmark-\(tag)-" + theme)
            workspaceActivate(sketch)
            XCTAssertTrue(preview.waitForNonExistence(timeout: 10))
        }
        bookmarkLabels("Bookmark this value", "Remove bookmark", "en-before")
        for (label, doneLabel, add, remove, tag) in [
            ("日本語", "完了", "この値をブックマーク", "ブックマークを削除", "ja"),
            ("English", "Done", "Bookmark this value", "Remove bookmark", "en-after")
        ] {
            workspaceActivate(paint)
            expectation(for: NSPredicate(format: "selected == YES"), evaluatedWith: paint)
            waitForExpectations(timeout: 10)
            workspaceActivate(app.buttons["settings-button"])
            choose(label); expectLabel(done, doneLabel); workspaceActivate(done)
            XCTAssertTrue(done.waitForNonExistence(timeout: 10))
            workspaceActivate(sketch)
            expectation(for: NSPredicate(format: "selected == YES"), evaluatedWith: sketch)
            waitForExpectations(timeout: 10)
            bookmarkLabels(add, remove, tag)
        }
        workspaceActivate(paint)
        expectation(for: NSPredicate(format: "selected == YES"), evaluatedWith: paint)
        waitForExpectations(timeout: 10)
        let untouched = settledEditorPixels(in: app)
        #if os(macOS)
        let canvas = workspaceViewport(in: app)
        let start = canvas.coordinate(withNormalizedOffset: CGVector(dx: 0.45, dy: 0.6))
        let end = canvas.coordinate(withNormalizedOffset: CGVector(dx: 0.55, dy: 0.6))
        start.click(forDuration: 0.05, thenDragTo: end)
        #else
        editorMenu(in: app, menu: "Select", id: "select_all", label: "Select all pixels")
        editorMenu(in: app, menu: "Edit", id: "fill_selection", label: "Fill selection")
        #endif
        expectation(for: NSPredicate { _, _ in self.editorPixels(in: app) != untouched }, evaluatedWith: app)
        waitForExpectations(timeout: 15)
        let painted = settledEditorPixels(in: app)
        for (label, doneLabel, title, formatField, previewLabel, chooseFile, cancel, save, discard, fileMenu, export, close, closeTitle, undo, redo, tag) in [
            ("日本語", "完了", "画像を書き出す", "形式", "出力をプレビュー", "ファイルを選択…", "キャンセル", "保存", "変更を破棄", "ファイル", "書き出し…", "閉じる", "「無題」の変更を保存しますか？", "元に戻す", "やり直す", "ja"),
            ("English", "Done", "Export image", "Format", "Preview Output", "Choose File…", "Cancel", "Save", "Discard Changes", "File", "Export…", "Close", "Save changes to “Untitled”?", "Undo", "Redo", "en")
        ] {
            workspaceActivate(app.buttons["settings-button"])
            choose(label); expectLabel(done, doneLabel); workspaceActivate(done)
            XCTAssertTrue(done.waitForNonExistence(timeout: 10))
            func fileCommand(_ id: String, _ caption: String) {
                editorMenu(in: app, menu: fileMenu, id: id, label: caption)
            }
            fileCommand("export_document", export)
            let exportCancel = app.buttons["export-cancel"], destination = app.buttons["export-choose-file"]
            XCTAssertTrue(destination.waitForExistence(timeout: 30))
            expectLabel(destination, chooseFile)
            expectLabel(app.buttons["export-preview"], previewLabel)
            expectLabel(exportCancel, cancel)
            XCTAssertTrue(app.staticTexts[title].exists)
            XCTAssertTrue(app.staticTexts[formatField].exists)
            let colorPage = app.buttons["export-page-color"]
            revealEditorControl(colorPage, in: app.scrollViews.containing(.button, identifier: colorPage.identifier).firstMatch)
            workspaceActivate(colorPage)
            XCTAssertTrue(app.staticTexts[tag == "ja" ? "色と透明度" : "Color & transparency"].exists)
            expectLabel(app.buttons["source-profile-import"], tag == "ja" ? "ICCプロファイルを読み込む…" : "Import ICC Profile…")
            let profileLibrary = app.buttons["color-profile-library"]
            expectLabel(profileLibrary, tag == "ja" ? "保存したプロファイル…" : "Saved Profiles…")
            attachEditor(in: app, name: "language-export-" + tag + "-" + theme)
            workspaceActivate(profileLibrary)
            let libraryDone = app.buttons["profile-library-done"]
            XCTAssertTrue(libraryDone.waitForExistence(timeout: 20))
            expectLabel(libraryDone, doneLabel)
            expectation(for: NSPredicate(format: "enabled == YES"), evaluatedWith: libraryDone)
            waitForExpectations(timeout: 15)
            XCTAssertTrue(app.staticTexts[tag == "ja" ? "カラープロファイルライブラリ" : "Color Profile Library"].exists)
            XCTAssertTrue(app.staticTexts[tag == "ja" ? "読み込んだプロファイルはありません" : "No imported profiles"].exists)
            attachEditor(in: app, name: "language-profile-library-" + tag + "-" + theme)
            workspaceActivate(libraryDone)
            XCTAssertTrue(libraryDone.waitForNonExistence(timeout: 10))
            workspaceActivate(app.buttons["export-back"])
            XCTAssertTrue(colorPage.waitForExistence(timeout: 10))
            workspaceActivate(exportCancel)
            XCTAssertTrue(exportCancel.waitForNonExistence(timeout: 10))
            expectPixels(painted, in: app)
            fileCommand("close_document", close)
            let confirmation = app.staticTexts[closeTitle].firstMatch
            XCTAssertTrue(confirmation.waitForExistence(timeout: 15))
            #if os(macOS)
            let dialogButtons = app.windows.sheets.containing(.staticText, identifier: closeTitle).buttons
            #else
            let dialogButtons = app.buttons
            #endif
            XCTAssertTrue(dialogButtons[save].exists)
            XCTAssertTrue(dialogButtons[discard].exists)
            let closeCancel = dialogButtons[cancel].firstMatch
            XCTAssertTrue(closeCancel.exists)
            attachEditor(in: app, name: "language-close-" + tag + "-" + theme)
            #if os(macOS)
            app.typeKey(.return, modifierFlags: [])
            let savePicker = app.windows.buttons["OKButton"].firstMatch
            XCTAssertTrue(savePicker.waitForExistence(timeout: 15))
            XCTAssertTrue(app.textFields["saveAsNameTextField"].exists, "Return must choose Save rather than discard the drawing")
            let pickerCancel = app.windows.buttons["CancelButton"].firstMatch
            XCTAssertTrue(pickerCancel.isHittable)
            pickerCancel.click()
            XCTAssertTrue(savePicker.waitForNonExistence(timeout: 15))
            XCTAssertTrue(confirmation.waitForNonExistence(timeout: 10))
            expectPixels(painted, in: app)
            fileCommand("close_document", close)
            XCTAssertTrue(confirmation.waitForExistence(timeout: 15))
            #endif
            workspaceActivate(closeCancel)
            XCTAssertTrue(confirmation.waitForNonExistence(timeout: 10))
            XCTAssertTrue(app.state == .runningForeground)
            expectPixels(painted, in: app)
            editorHistory(undo, in: app); expectPixels(untouched, in: app)
            editorHistory(redo, in: app); expectPixels(painted, in: app)
        }
        XCTAssertFalse(app.staticTexts["Canvas error"].exists)
    }
}
