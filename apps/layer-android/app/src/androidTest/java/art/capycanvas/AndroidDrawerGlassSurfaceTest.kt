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
