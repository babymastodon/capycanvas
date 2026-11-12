import XCTest
import UIKit
import ImageIO

struct PhoneCaptureID {
    fileprivate let row: [String: Any]
    var family: String { row["family"] as! String }
    var state: String { row["state"] as! String }
    var menu: [String: Any]? { row["menu"] as? [String: Any] }
    var inventory: [String: Any]? { row["inventory"] as? [String: Any] }
}

private enum PhoneCaptureRegistry {
    static let rows: [[String: Any]]? = {
        guard let url = Bundle(for: EditorLaunchTests.self).url(forResource: "phone-capture-ids", withExtension: "json"),
            let data = try? Data(contentsOf: url),
            let value = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            value["schema_version"] as? Int == 1 else { return nil }
        return value["states"] as? [[String: Any]]
    }()
}

extension XCTestCase {
    @MainActor private func phoneCaptureID(required: Bool = true, where matches: ([String: Any]) -> Bool) -> PhoneCaptureID? {
        guard let rows = PhoneCaptureRegistry.rows else {
            XCTFail("The phone capture identity resource must be packaged in the test bundle"); return nil
        }
        let found = rows.filter(matches)
        if found.isEmpty && !required { return nil }
        guard found.count == 1, let row = found.first, row["family"] is String, row["state"] is String else {
            XCTFail("A capture must resolve exactly one published phone identity"); return nil
        }
        return PhoneCaptureID(row: row)
    }

    @MainActor func phoneCaptureID(_ family: String, _ state: String) -> PhoneCaptureID? {
        phoneCaptureID { $0["family"] as? String == family && $0["state"] as? String == state }
    }

    @MainActor func phoneMenuID(route: [String: Any], swapped: Bool) -> PhoneCaptureID? {
        phoneCaptureID {
            guard let menu = $0["menu"] as? [String: Any], let expected = menu["route"] as? [String: Any] else { return false }
            return menu["swapped"] as? Bool == swapped && NSDictionary(dictionary: route).isEqual(to: expected)
        }
    }

    @MainActor func phoneMenuID(title: String) -> PhoneCaptureID? {
        phoneCaptureID {
            guard let menu = $0["menu"] as? [String: Any] else { return false }
            return menu["swapped"] as? Bool == false && menu["title"] as? String == title
        }
    }

    @MainActor func phoneMenuParentID(_ id: PhoneCaptureID) -> PhoneCaptureID? {
        guard var route = id.menu?["route"] as? [String: Any], var path = route["path"] as? [[Int]] else { return nil }
        if path.isEmpty { return phoneMenuID(route: ["path": [[Int]]()], swapped: false) }
        path.removeLast(); route["path"] = path
        return phoneMenuID(route: route, swapped: false)
    }

    @MainActor func phoneInventoryID(workspace: String, control: [String: Any], variant: String, required: Bool = true) -> PhoneCaptureID? {
        phoneCaptureID(required: required) {
            guard let inventory = $0["inventory"] as? [String: Any], let expected = inventory["control"] as? [String: Any] else { return false }
            return inventory["variant"] as? String == variant
                && (inventory["preset"] == nil || inventory["preset"] as? String == workspace)
                && NSDictionary(dictionary: control).isEqual(to: expected)
        }
    }

    @MainActor func phoneInventoryIDs(variant: String) -> [PhoneCaptureID] {
        guard let rows = PhoneCaptureRegistry.rows else {
            XCTFail("The phone capture identity resource must be packaged in the test bundle"); return []
        }
        return rows.filter { ($0["inventory"] as? [String: Any])?["variant"] as? String == variant }.map(PhoneCaptureID.init(row:))
    }

    @MainActor func phoneInventoryTileID(in app: XCUIApplication, tile: XCUIElement, variant: String, required: Bool = true) -> PhoneCaptureID? {
        guard let workspace = phoneObservedValue("persistence-workspace", in: app),
            let tools = phoneObservedTools(in: app),
            let tileID = Int(tile.identifier.replacingOccurrences(of: "phone-tool-", with: "")),
            let observed = tools.first(where: { ($0["id"] as? Int) == tileID }) else {
            XCTFail("The captured tile must have an observed shared control"); return nil
        }
        let source = variant == "settings" ? tools.first { $0["selected"] as? Bool == true } : observed
        let key = variant == "settings" ? "resolved_control" : "control"
        guard let control = source?[key] as? [String: Any] else {
            XCTFail("The captured drawer must have an observed shared control"); return nil
        }
        return phoneInventoryID(workspace: workspace, control: control, variant: variant, required: required)
    }

    @MainActor func phoneMenuTitle(for row: XCUIElement) -> String? {
        if row.identifier == "phone-drawing" {
            return PhoneCaptureRegistry.rows?.compactMap { $0["menu"] as? [String: Any] }.first {
                guard let route = $0["route"] as? [String: Any], let page = route["page"] as? [String: Any] else { return false }
                return page["kind"] as? String == "drawings"
            }?["title"] as? String
        }
        let text = row.staticTexts.firstMatch
        let title = (text.exists ? text.label : row.label).trimmingCharacters(in: CharacterSet(charactersIn: ".…"))
        guard PhoneCaptureRegistry.rows?.contains(where: { ($0["menu"] as? [String: Any])?["title"] as? String == title }) == true else {
            XCTFail("The shared registry must identify the submenu title: \(title)"); return nil
        }
        return title
    }

    @MainActor func phoneMenuChildID(parent: PhoneCaptureID, row: XCUIElement, swapped: Bool) -> PhoneCaptureID? {
        guard let title = phoneMenuTitle(for: row), let menu = parent.menu, var route = menu["route"] as? [String: Any] else { return nil }
        if let target = PhoneCaptureRegistry.rows?.first(where: {
            guard let candidate = $0["menu"] as? [String: Any], let path = (candidate["route"] as? [String: Any])?["path"] as? [[Int]] else { return false }
            return candidate["swapped"] as? Bool == swapped && candidate["title"] as? String == title && path.isEmpty
        }) { return PhoneCaptureID(row: target) }
        let position = row.identifier.split(separator: "-").suffix(2).compactMap { Int($0) }
        guard position.count == 2, var path = route["path"] as? [[Int]] else {
            XCTFail("A submenu must expose its shared row position"); return nil
        }
        path.append(position); route["path"] = path
        return phoneMenuID(route: route, swapped: swapped)
    }

    @MainActor func phoneObservedValue(_ identifier: String, in app: XCUIApplication) -> String? {
        let element = app.staticTexts[identifier]
        guard element.exists, let value = element.value as? String, !value.isEmpty else {
            XCTFail("The app must publish its observed capture value: \(identifier)"); return nil
        }
        return value
    }

    @MainActor func phoneObservedTools(in app: XCUIApplication) -> [[String: Any]]? {
        guard let text = phoneObservedValue("persistence-phone-tools", in: app), let data = text.data(using: .utf8),
            let tools = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]] else {
            XCTFail("The app must publish its actual phone tile controls"); return nil
        }
        return tools
    }

    @MainActor private func phoneSettledScreenshot(in app: XCUIApplication) -> XCUIScreenshot? {
        let window = workspaceViewport(in: app), canvas = app.descendants(matching: .any)["canvas"].firstMatch
        let field = app.descendants(matching: .any).matching(NSPredicate(format: "hasKeyboardFocus == YES")).firstMatch
        func screenshot() -> (screen: XCUIScreenshot, pixels: Data) {
            let screen = canvas.exists && canvas.frame == window.frame ? editorScreenshot(in: app) : window.screenshot()
            let image = editorImage(from: screen), scale = CGFloat(image.width) / window.frame.width
            var pixels = Data(count: image.width * image.height * 4)
            pixels.withUnsafeMutableBytes { bytes in
                let context = CGContext(data: bytes.baseAddress, width: image.width, height: image.height, bitsPerComponent: 8,
                    bytesPerRow: image.width * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
                context.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
                guard field.exists else { return }
                let caret = field.frame.offsetBy(dx: -window.frame.minX, dy: -window.frame.minY).insetBy(dx: -4, dy: -4)
                context.clear(CGRect(x: caret.minX * scale, y: CGFloat(image.height) - caret.maxY * scale,
                    width: caret.width * scale, height: caret.height * scale))
            }
            return (screen, pixels)
        }
        var previous = screenshot(), settled: XCUIScreenshot?
        let unchanged = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
            let next = screenshot()
            if next.pixels == previous.pixels { settled = next.screen; return true }
            previous = next; return false
        }, object: app)
        guard XCTWaiter.wait(for: [unchanged], timeout: 10) == .completed, let settled else {
            XCTFail("Two consecutive editor window screenshots must settle before capture"); return nil
        }
        return settled
    }

    @MainActor func phoneCapture(in app: XCUIApplication, id: PhoneCaptureID?, theme: String,
        fixture: [[String: Any]], extra: [String: Any] = [:]) {
        guard let id, let workspace = phoneObservedValue("persistence-workspace", in: app),
            let observedTheme = phoneObservedValue("persistence-status", in: app),
            let category = phoneObservedValue("persistence-content-size-category", in: app) else { return }
        guard category == UIContentSizeCategory.large.rawValue else {
            return XCTFail("A text_size 11 capture requires the app's observed default Large content category: \(category)")
        }
        guard observedTheme == theme else { return XCTFail("The app must display the captured theme: \(observedTheme)") }
        guard let screen = phoneSettledScreenshot(in: app) else { return }
        let image = editorImage(from: screen)
        let bounds = workspaceViewport(in: app).frame, scale = UIScreen.main.scale
        XCTAssertGreaterThan(scale, 0)
        XCTAssertEqual(image.width, Int((bounds.width * scale).rounded()))
        XCTAssertEqual(image.height, Int((bounds.height * scale).rounded()))
        let suffix = scale == 1 ? "" : "@" + String(format: "%g", Double(scale))
        let captureCase = "\(Int(bounds.width))x\(Int(bounds.height))-\(observedTheme)" + suffix
        let name = "phone-\(id.family)-\(id.state)-\(captureCase)-" + workspace.replacingOccurrences(of: ":", with: "_")
        let png = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(png, "public.png" as CFString, 1, nil) else {
            return XCTFail("The normalized screenshot must create a PNG destination")
        }
        CGImageDestinationAddImage(destination, image, nil)
        guard CGImageDestinationFinalize(destination) else { return XCTFail("The normalized screenshot must encode as PNG") }
        let capture = XCTAttachment(data: png as Data, uniformTypeIdentifier: "public.png")
        capture.name = name; capture.lifetime = .keepAlways; add(capture)
        let initial = (app.launchEnvironment["CAPY_INITIAL_ACTIONS"].flatMap { $0.data(using: .utf8) }
            .flatMap { try? JSONSerialization.jsonObject(with: $0) } as? [[String: Any]]) ?? []
        var metadata: [String: Any] = ["schema_version": 1, "host": "apple", "family": id.family, "state": id.state,
            "theme": observedTheme, "viewport": [bounds.width, bounds.height], "dpr": scale,
            "fixture": workspace, "preset": workspace, "case": captureCase, "png_attachment": name,
            "text_size": 11, "text_scale": 1, "status": "captured", "phone_active": app.buttons["phone-capy"].exists,
            "fixture_actions": initial, "requested_actions": fixture, "launch_arguments": app.launchArguments,
            "orientation": bounds.width > bounds.height ? "landscape" : "portrait",
            "app_content_size_category": category, "screenshot_source": "settled editor window",
            "native_only": id.row["native_only"] as? Bool ?? false,
            "document_fixture": app.launchEnvironment["CAPY_STORAGE_DIR"].flatMap { phoneDrawingFixtures[$0] }
                ?? ["status": "STATE", "reason": "The current drawing has no canonical document proof"],
            "required_unverified": extra["required_unverified"] as? [String] ?? []]
        for (key, value) in extra where key != "required_unverified" {
            guard metadata[key] == nil else { return XCTFail("Capture details must not override observed identity: \(key)") }
            metadata[key] = value
        }
        if let recorded = ProcessInfo.processInfo.environment["CAPY_PHONE_CAPTURE_CONTENT_SIZE_CATEGORY"] {
            metadata["simulator_content_size_category"] = recorded
            metadata["simulator_content_size_category_source"] = "simctl ui content_size recorded before invocation"
        }
        let attachment = XCTAttachment(data: try! JSONSerialization.data(withJSONObject: metadata, options: [.sortedKeys]), uniformTypeIdentifier: "public.json")
        attachment.name = name + "-metadata"; attachment.lifetime = .keepAlways; add(attachment)
    }
}
