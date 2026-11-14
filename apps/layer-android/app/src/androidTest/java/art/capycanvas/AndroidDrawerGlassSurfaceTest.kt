package art.capycanvas

import androidx.compose.foundation.layout.*
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.addOutline
import androidx.compose.ui.graphics.asAndroidPath
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.unit.dp
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Rule
import org.junit.Test
import kotlin.math.roundToInt

class AndroidDrawerGlassSurfaceTest {
    @get:Rule(order=0) val device = CapyDeviceRule()
    @get:Rule(order=1) val compose = createAndroidComposeRule<MainActivity>()

    @Test fun selectedHeaderFacesKeepTheirClosedColour() {
        val host = compose.activity.host
        host.awaitReady(compose = compose)
        assumeTrue("Requires the tablet workspace", host.snapshot!!.getJSONObject("layout").objectOrNull("phone") == null)
        host.awaitMain("launch revealed", 30_000, { "" }, compose) { findTag("launch-splash") == null && !host.documentInputBlocked }
        host.useFlatPaper(compose)
        host.coverChromeWithPaper(compose)
        val failures = mutableListOf<String>()
        for (theme in listOf("light", "dark")) for ((level, value) in listOf("low" to 1, "high" to 3)) {
            host.drain(obj("type" to "set_theme", "theme" to theme))
            host.drain(obj("type" to "preferences", "action" to obj("type" to "edit", "id" to "transparency", "value" to value)))
            compose.waitForIdle()
            val palette = Palette(theme == "dark", host.snapshot!!.getJSONObject("state").getJSONObject("palette"))
            lateinit var view: ComposeView
            compose.runOnIdle {
                view = ComposeView(compose.activity)
                compose.activity.addContentView(view, android.widget.FrameLayout.LayoutParams(-1, -1))
                view.setContent {
                    CompositionLocalProvider(LocalCanvasHost provides host, LocalPalette provides palette) {
                        Box(Modifier.fillMaxSize()) {
                            for ((row, inBar) in listOf(false, true).withIndex()) for ((column, open) in listOf(false, true).withIndex()) {
                                val bounds = Modifier.offset((80 + column * 160).dp, (140 + row * 80).dp).size(80.dp, 48.dp)
                                Box(bounds.then(if (inBar) Modifier.glass(TileShape, palette.headerSurface) else Modifier)) {
                                    HeaderButton("Selected tool", true, true, if (open) "bottom" else null,
                                        Modifier.fillMaxSize().testTag("selected-header-$row-$column"), inBar = inBar,
                                        drawerPresented = true, onClick = {}) {}
                                }
                            }
                        }
                    }
                }
            }
            try {
                host.awaitMain("Selected header faces", 30_000, { "" }, compose) {
                    findTag("selected-header-1-1", view.descendant<androidx.compose.ui.platform.ViewRootForTest>()) != null
                }
                compose.waitForIdle()
                val points = compose.runOnIdle {
                    val root = checkNotNull(view.descendant<androidx.compose.ui.platform.ViewRootForTest>())
                    val origin = IntArray(2).also(root.view::getLocationOnScreen)
                    (0..1).flatMap { row -> (0..1).map { column ->
                        checkNotNull(findTag("selected-header-$row-$column", root)).second.boundsInRoot.center + androidx.compose.ui.geometry.Offset(origin[0].toFloat(), origin[1].toFloat())
                    } }
                }
                val receipt = obj("theme" to theme, "transparency" to level)
                var inspected = false
                screenshot("validation/drawer-faces/selected-header-$theme-$level.png") { image ->
                    inspected = true
                    assertTrue("Selected header reference points exist", points.size == 4)
                    val colors = points.map { at -> listOf(16, 8, 0).map { shift ->
                        (-1..1).sumOf { dy -> (-1..1).sumOf { dx -> image.getPixel(at.x.roundToInt() + dx, at.y.roundToInt() + dy) shr shift and 255 } } / 9.0
                    } }
                    assertTrue("Selected header samples are visible", colors.all { rgb -> rgb.any { it > 0.0 } })
                    receipt.put("closed_open_rgb", org.json.JSONArray(colors.map { org.json.JSONArray(it) }))
                    for (row in 0..1) {
                        val delta = colors[row * 2 + 1].zip(colors[row * 2]).map { (open, closed) -> open - closed }
                        if (delta.any { kotlin.math.abs(it) > 1.0 }) failures += "$theme/$level/inBar=${row == 1}: selected open−closed=$delta"
                    }
                }
                assertTrue("Selected header screenshot inspected", inspected)
                java.io.File(instrumentation.targetContext.getExternalFilesDir(null), "validation/drawer-faces/selected-header-$theme-$level.json").writeText(receipt.toString(2))
            } finally { compose.runOnIdle { view.disposeComposition(); (view.parent as android.view.ViewGroup).removeView(view) } }
        }
        assertTrue(failures.joinToString("\n"), failures.isEmpty())
    }

    @Test fun selectedPanelTileKeepsItsClosedColour() {
        val host = compose.activity.host
        host.awaitReady(compose = compose)
        assumeTrue("Requires the tablet workspace", host.snapshot!!.getJSONObject("layout").objectOrNull("phone") == null)
        host.awaitMain("launch revealed", 30_000, { "" }, compose) { findTag("launch-splash") == null && !host.documentInputBlocked }
        val panelId = host.snapshot!!.array("panels").objects().first { it.array("tiles").length() > 0 }.getString("id")
        val workspace = host.snapshot!!.getJSONObject("state").getJSONObject("workspace").copy()
        val layout = workspace.getJSONObject("layout")
        layout.array("panels").objects().single { it.getString("id") == panelId }.getJSONObject("content")
            .put("tiles", org.json.JSONArray(listOf(obj("id" to 900, "control" to obj("kind" to "brush", "id" to host.snapshot!!.getJSONObject("state").getJSONObject("brush").getInt("preset"))))))
        layout.put("next_tile_id", 901)
        host.drain(obj("type" to "restore_workspace", "workspace" to workspace))
        host.drain(obj("type" to "select_brush", "id" to host.snapshot!!.getJSONObject("state").getJSONObject("brush").getInt("preset")))
        compose.waitForIdle()
        val failures = mutableListOf<String>()
        for (theme in listOf("light", "dark")) for ((level, value) in listOf("low" to 1, "high" to 3)) {
            host.drain(obj("type" to "set_theme", "theme" to theme))
            host.drain(obj("type" to "preferences", "action" to obj("type" to "edit", "id" to "transparency", "value" to value)))
            compose.waitForIdle()
            val selected = host.snapshot!!.array("panels").objects().flatMap { panel -> panel.array("tiles").objects().map { panel.getString("id") to it } }
                .firstOrNull { (panel, tile) -> tile.optBoolean("selected") && !tile.has("component") && findTag("tile-$panel-${tile.getInt("id")}") != null }
            assertNotNull("Selected visible tile: ${host.snapshot!!.array("panels")}", selected)
            val (panel, tile) = checkNotNull(selected)
            val tag = "tile-$panel-${tile.getInt("id")}"
            val colors = mutableListOf<List<Double>>()
            for (open in listOf(false, true)) {
                if (open) {
                    host.drain(obj("type" to "customize", "action" to obj("type" to "toggle_tool_drawer", "anchor" to obj("panel" to panel, "tile" to tile.getInt("id")))))
                    compose.mainClock.advanceTimeBy(400)
                    host.awaitMain("Selected panel drawer presented", 30_000, { "${host.workspaceInteraction?.drawer}" }, compose) {
                        host.workspaceInteraction?.anchorSource(obj("kind" to "tile", "panel" to panel, "tile" to tile.getInt("id")))?.presented == true
                    }
                }
                val bounds = compose.runOnIdle {
                    val actual = checkNotNull(findTag(tag))
                    val origin = IntArray(2).also(actual.first.view::getLocationOnScreen)
                    actual.second.boundsInRoot.translate(androidx.compose.ui.geometry.Offset(origin[0].toFloat(), origin[1].toFloat()))
                }
                var inspected = false
                screenshot("validation/drawer-faces/selected-panel-$theme-$level-$open.png") { image ->
                    inspected = true
                    val x = bounds.left.roundToInt() + 3
                    val y = bounds.center.y.roundToInt()
                    assertTrue("Selected panel patch lies inside tile", bounds.contains(androidx.compose.ui.geometry.Offset(x - 1f, y - 1f)) && bounds.contains(androidx.compose.ui.geometry.Offset(x + 1f, y + 1f)))
                    colors += listOf(16, 8, 0).map { shift -> (-1..1).sumOf { dy -> (-1..1).sumOf { dx -> image.getPixel(x + dx, y + dy) shr shift and 255 } } / 9.0 }
                }
                assertTrue("Selected panel screenshot inspected", inspected)
                assertTrue("Selected panel samples are visible", colors.last().any { it > 0.0 })
            }
            val delta = colors[1].zip(colors[0]).map { (open, closed) -> open - closed }
            if (delta.any { kotlin.math.abs(it) > 1.0 }) failures += "$theme/$level selected panel open−closed=$delta"
            java.io.File(instrumentation.targetContext.getExternalFilesDir(null), "validation/drawer-faces/selected-panel-$theme-$level.json")
                .writeText(obj("closed_rgb" to org.json.JSONArray(colors[0]), "open_rgb" to org.json.JSONArray(colors[1]), "delta" to org.json.JSONArray(delta)).toString(2))
            host.drain(obj("type" to "customize", "action" to obj("type" to "close_expanded")))
            compose.mainClock.advanceTimeBy(400)
            host.awaitMain("Selected panel drawer closed", 30_000, { "" }, compose) { host.workspaceInteraction?.drawer == null }
        }
        assertTrue(failures.joinToString("\n"), failures.isEmpty())
    }

    @Test fun standaloneHeaderFaceMatchesPanelGlassOverPaper() {
        val host = compose.activity.host
        host.awaitReady(compose = compose)
        assumeTrue("Requires the tablet workspace", host.snapshot!!.getJSONObject("layout").objectOrNull("phone") == null)
        host.awaitMain("launch reveal removed", 30_000, { "" }, compose) { findTag("launch-splash") == null }
        host.awaitMain("Drawing accepts input", 30_000, { "" }, compose) { !host.documentInputBlocked }
        host.useFlatPaper(compose)
        host.coverChromeWithPaper(compose)
        val density = compose.activity.resources.displayMetrics.density
        val failures = mutableListOf<String>()
        for (theme in listOf("light", "dark")) for ((level, value) in listOf("low" to 1, "high" to 3)) {
            host.drain(obj("type" to "set_theme", "theme" to theme))
            host.drain(obj("type" to "preferences", "action" to obj("type" to "edit", "id" to "transparency", "value" to value)))
            compose.waitForIdle()
            val palette = Palette(theme == "dark", host.snapshot!!.getJSONObject("state").getJSONObject("palette"))
            lateinit var view: ComposeView
            compose.runOnIdle {
                view = ComposeView(compose.activity)
                compose.activity.addContentView(view, android.widget.FrameLayout.LayoutParams(-1, -1))
                view.setContent {
                    CompositionLocalProvider(LocalCanvasHost provides host, LocalPalette provides palette) {
                        Box(Modifier.fillMaxSize()) {
                            HeaderButton("Open drawer", false, true, "bottom", Modifier.offset(80.dp, 140.dp).size(80.dp, 48.dp)
                                .testTag("standalone-glass-face"), drawerPresented = true, onClick = {}) {}
                            HeaderButton("Reference drawer", false, true, "bottom", Modifier.offset(240.dp, 140.dp).size(80.dp, 48.dp)
                                .glass(drawerButtonShape("bottom"), androidx.compose.ui.graphics.Color.Transparent).testTag("standalone-glass-panel"),
                                surface = false, drawerPresented = true, onClick = {}) {}
                        }
                    }
                }
            }
            try {
                host.awaitMain("Standalone native glass regions", 30_000, { "" }, compose) {
                    findTag("standalone-glass-face", view.descendant<androidx.compose.ui.platform.ViewRootForTest>()) != null && host.glassBoxesForTest.any { it[2] == 80f * density && it[3] == 48f * density }
                }
                compose.waitForIdle()
                val bounds = compose.runOnIdle {
                    val root = checkNotNull(view.descendant<androidx.compose.ui.platform.ViewRootForTest>())
                    val screen = IntArray(2)
                    root.view.getLocationOnScreen(screen)
                    listOf("standalone-glass-face", "standalone-glass-panel").map {
                        checkNotNull(findTag(it, root)).second.boundsInRoot.translate(androidx.compose.ui.geometry.Offset(screen[0].toFloat(), screen[1].toFloat()))
                    }
                }
                val points = bounds.map { it.center }
                val name = "standalone-$theme-$level"
                val receipt = obj("name" to name, "reference" to "HeaderButton surface=false with transparent native glass",
                    "bounds" to org.json.JSONArray(bounds.map { obj("x" to it.left, "y" to it.top, "width" to it.width, "height" to it.height) }))
                var inspected = false
                screenshot("validation/drawer-faces/$name.png") { image ->
                    inspected = true
                    val means = points.map { at -> listOf(16, 8, 0).map { shift ->
                        (-1..1).sumOf { dy -> (-1..1).sumOf { dx -> (image.getPixel(at.x.roundToInt() + dx, at.y.roundToInt() + dy) shr shift) and 255 } } / 9.0
                    } }
                    val delta = means[0].zip(means[1]).map { (a, b) -> a - b }
                    receipt.put("face_rgb", org.json.JSONArray(means[0])).put("panel_rgb", org.json.JSONArray(means[1])).put("face_minus_panel", org.json.JSONArray(delta))
                    assertEquals(bounds[0].size, bounds[1].size)
                    val support = android.graphics.Bitmap.createBitmap(bounds[0].width.roundToInt(), bounds[0].height.roundToInt(), android.graphics.Bitmap.Config.ARGB_8888)
                    val outline = drawerButtonShape("bottom").createOutline(bounds[0].size,
                        androidx.compose.ui.unit.LayoutDirection.Ltr, androidx.compose.ui.unit.Density(density))
                    val path = androidx.compose.ui.graphics.Path().apply { addOutline(outline) }.asAndroidPath()
                    android.graphics.Canvas(support).drawPath(path, android.graphics.Paint(android.graphics.Paint.ANTI_ALIAS_FLAG).apply {
                        color = android.graphics.Color.WHITE
                        style = android.graphics.Paint.Style.FILL_AND_STROKE
                        strokeWidth = 2f
                    })
                    var compared = 0
                    var maximum = 0
                    var worst = obj()
                    for (y in 0 until bounds[0].height.roundToInt()) for (x in 0 until bounds[0].width.roundToInt()) {
                        if (android.graphics.Color.alpha(support.getPixel(x, y)) == 0) continue
                        compared++
                        val first = image.getPixel(bounds[0].left.roundToInt() + x, bounds[0].top.roundToInt() + y)
                        val second = image.getPixel(bounds[1].left.roundToInt() + x, bounds[1].top.roundToInt() + y)
                        val difference = listOf(16, 8, 0).maxOf { shift -> kotlin.math.abs(((first shr shift) and 255) - ((second shr shift) and 255)) }
                        if (difference > maximum) {
                            maximum = difference
                            worst = obj("x" to x, "y" to y, "face_argb" to first, "reference_argb" to second)
                        }
                    }
                    support.recycle()
                    assertTrue("$name painted face pixels compared", compared > 0)
                    receipt.put("compared_pixels", compared).put("maximum_pixel_delta", maximum).put("worst_pixel", worst)
                    if (maximum > 1) failures += "$name: identical HeaderButton clip/paint geometry differs by $maximum at $worst"

                    if (delta.any { kotlin.math.abs(it) > 1.0 }) failures += "$name: face−panel=$delta"
                }
                assertTrue("$name screenshot captured", inspected)
                java.io.File(instrumentation.targetContext.getExternalFilesDir(null), "validation/drawer-faces/$name.json").writeText(receipt.toString(2))
            } finally {
                compose.runOnIdle { view.disposeComposition(); (view.parent as android.view.ViewGroup).removeView(view) }
            }
        }
        assertTrue(failures.joinToString("\n"), failures.isEmpty())
    }
}
