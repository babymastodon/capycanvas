package art.capycanvas

import androidx.compose.ui.graphics.addOutline
import androidx.compose.ui.graphics.asAndroidPath
import org.json.JSONObject
import kotlin.math.roundToInt

internal fun CanvasHost.useFlatPaper(compose: androidx.compose.ui.test.junit4.ComposeTestRule) {
    val layers = snapshot!!.getJSONObject("state").array("layers").objects()
    val paper = layers.single { !it.isNull("paper") }.getLong("id")
    val ink = layers.first { it.getBoolean("drawing") }.getLong("id")
    drain(obj("type" to "select_layer", "id" to paper))
    drain(obj("type" to "effect", "action" to obj("op" to "set", "layer" to paper, "key" to "texture", "value" to obj("kind" to "choice", "value" to 0))))
    drain(obj("type" to "select_layer", "id" to ink))
    compose.waitForIdle()
}

internal fun CanvasHost.coverChromeWithPaper(compose: androidx.compose.ui.test.junit4.ComposeTestRule) = frameGlassBackdrop(compose, true)

internal fun CanvasHost.frameGlassBackdrop(compose: androidx.compose.ui.test.junit4.ComposeTestRule, paper: Boolean) {
    drain(obj("type" to "invoke", "command" to "fit_canvas"))
    compose.waitForIdle()
    val state = snapshot!!.getJSONObject("state")
    val camera = state.getJSONObject("camera")
    val document = state.array("tabs").objects().single { it.getBoolean("active") }
    val viewport = camera.array("viewport")
    val zoom = camera.number("zoom")
    val target = if (paper) 4f * maxOf(viewport.getDouble(0).toFloat() / document.number("width"), viewport.getDouble(1).toFloat() / document.number("height"))
        else .1f * minOf(viewport.getDouble(0).toFloat() / document.number("width"), viewport.getDouble(1).toFloat() / document.number("height"))
    val translation = camera.array("translation")
    val center = androidx.compose.ui.geometry.Offset(translation.getDouble(0).toFloat() + document.number("width") * zoom / 2,
        translation.getDouble(1).toFloat() + document.number("height") * zoom / 2)
    scroll(center.x, center.y, 0f, -kotlin.math.ln(target / zoom) / (.06f * state.getJSONObject("settings").number("zoom_speed", 1.0)), true, false)
    drain()
    awaitMain("Glass backdrop camera applied", 30_000, { snapshot!!.getJSONObject("state").getJSONObject("camera").toString() }, compose) {
        val current = snapshot!!.getJSONObject("state").getJSONObject("camera")
        if (!paper) return@awaitMain kotlin.math.abs(current.number("zoom") - target) < target * .001f
        val offset = current.array("translation")
        offset.getDouble(0) < -64 && offset.getDouble(1) < -64 &&
            offset.getDouble(0) + document.number("width") * current.number("zoom") > viewport.getDouble(0) + 64 &&
            offset.getDouble(1) + document.number("height") * current.number("zoom") > viewport.getDouble(1) + 64
    }
}

internal fun CanvasHost.captureDrawerGlass(name: String, frame: JSONObject, failures: MutableList<String>, paper: Boolean = true) {
    val dock = checkNotNull(workspaceInteraction)
    val connection = frame.getJSONObject("connection")
    val bounds = connection.getJSONObject("bounds").rect()
    val transform = connection.array("transform")
    val scale = dock.density
    val length = connection.number("length") * scale
    val depth = connection.number("depth") * scale
    val screen = IntArray(2)
    instrumentation.runOnMainSync { checkNotNull(findTag("workspace")).first.view.getLocationOnScreen(screen) }
    val origin = dock.origin + androidx.compose.ui.geometry.Offset(screen[0].toFloat(), screen[1].toFloat())
    fun point(u: Float, v: Float) = bounds.topLeft * scale + origin + androidx.compose.ui.geometry.Offset(
        transform.getDouble(0).toFloat() * u + transform.getDouble(2).toFloat() * v + transform.getDouble(4).toFloat() * scale,
        transform.getDouble(1).toFloat() * u + transform.getDouble(3).toFloat() * v + transform.getDouble(5).toFloat() * scale)
    val state = snapshot!!.getJSONObject("state")
    val camera = state.getJSONObject("camera")
    val document = state.array("tabs").objects().single { it.getBoolean("active") }
    val translation = camera.array("translation")
    val paperBounds = androidx.compose.ui.geometry.Rect(origin.x + translation.getDouble(0).toFloat(), origin.y + translation.getDouble(1).toFloat(),
        origin.x + translation.getDouble(0).toFloat() + document.number("width") * camera.number("zoom"),
        origin.y + translation.getDouble(1).toFloat() + document.number("height") * camera.number("zoom"))
    org.junit.Assert.assertEquals("Glass backdrop has no camera rotation", 0f, camera.number("rotation"), .001f)
    val receipt = obj("name" to name, "backdrop" to if (paper) "paper" else "surround", "camera" to camera.copy(), "frame" to frame)
    var inspected = false
    screenshot("validation/drawer-faces/$name.png") { image ->
        inspected = true
        val patches = listOf("face" to point(length * .25f, -2f), "bridge" to point(length * .25f, depth / 2), "body" to point(length * .25f, depth + 8f))
        val means = patches.map { (label, at) ->
            val rect = when (label) {
                "face" -> frame.getJSONObject("placement").getJSONObject("anchor").rect()
                "body" -> frame.getJSONObject("placement").getJSONObject("bounds").rect()
                else -> bounds
            }.let { androidx.compose.ui.geometry.Rect(it.topLeft * scale + origin, it.bottomRight * scale + origin) }
            org.junit.Assert.assertTrue("$name $label patch lies inside its surface: $at in $rect",
                rect.contains(at - androidx.compose.ui.geometry.Offset(1f, 1f)) && rect.contains(at + androidx.compose.ui.geometry.Offset(1f, 1f)))
            val patch = androidx.compose.ui.geometry.Rect(at - androidx.compose.ui.geometry.Offset(1f, 1f), at + androidx.compose.ui.geometry.Offset(1f, 1f))
            org.junit.Assert.assertTrue("$name $label patch has the requested backdrop: $patch against $paperBounds",
                if (paper) paperBounds.contains(patch.topLeft) && paperBounds.contains(patch.bottomRight) else !paperBounds.inflate(64f).overlaps(patch))
            val x = at.x.roundToInt(); val y = at.y.roundToInt()
            val channels = listOf(16, 8, 0).map { shift ->
                (-1..1).sumOf { dy -> (-1..1).sumOf { dx -> (image.getPixel(x + dx, y + dy) shr shift) and 255 } } / 9.0
            }
            org.junit.Assert.assertTrue("$name $label samples visible glass", channels.any { it > 0.0 })
            receipt.put(label, obj("x" to x, "y" to y, "mean_rgb" to org.json.JSONArray(channels)))
            channels
        }
        val deltas = listOf(0 to 1, 2 to 1, 0 to 2).map { (first, second) -> means[first].zip(means[second]).map { (a, b) -> a - b } }
        receipt.put("face_minus_bridge", org.json.JSONArray(deltas[0])).put("body_minus_bridge", org.json.JSONArray(deltas[1])).put("face_minus_body", org.json.JSONArray(deltas[2]))
        if (deltas.flatten().any { kotlin.math.abs(it) > 1.0 }) failures += "$name: face−bridge=${deltas[0]}, body−bridge=${deltas[1]}, face−body=${deltas[2]}"
        if (paper && dock.phone != null) capturePhoneGlassCurve(image, name, frame, origin, means[0], receipt, failures)
    }
    org.junit.Assert.assertTrue("$name screenshot captured", inspected)
    java.io.File(instrumentation.targetContext.getExternalFilesDir(null), "validation/drawer-faces/$name.json").writeText(receipt.toString(2))
}

private fun CanvasHost.capturePhoneGlassCurve(image: android.graphics.Bitmap, name: String, frame: JSONObject,
    origin: androidx.compose.ui.geometry.Offset, faceRgb: List<Double>, receipt: JSONObject, failures: MutableList<String>) {
    val dock = checkNotNull(workspaceInteraction)
    val scale = dock.density
    lateinit var facePath: android.graphics.Path
    lateinit var chipPath: android.graphics.Path
    lateinit var faceBounds: androidx.compose.ui.geometry.Rect
    lateinit var glyphBounds: androidx.compose.ui.geometry.Rect
    var variants = false
    instrumentation.runOnMainSync {
        val phone = checkNotNull(dock.phone)
        val center = frame.getJSONObject("placement").getJSONObject("anchor").rect().center * scale
        val face = phone.faces.single { it.bounds(scale).contains(center) &&
            dock.anchorSource(obj("kind" to "phone", "anchor" to it.anchor))?.presented == true }
        val source = checkNotNull(dock.anchorSource(obj("kind" to "phone", "anchor" to face.anchor)))
        val view = checkNotNull(findTag("workspace")).first.view
        val direction = if (view.layoutDirection == android.view.View.LAYOUT_DIRECTION_RTL)
            androidx.compose.ui.unit.LayoutDirection.Rtl else androidx.compose.ui.unit.LayoutDirection.Ltr
        val density = androidx.compose.ui.unit.Density(scale)
        faceBounds = face.bounds(scale).translate(origin)
        val tag = when (face.anchor.optString("kind")) {
            "tool_settings" -> "phone-tool-settings"
            "tool" -> "phone-tool-${face.anchor.getInt("id")}"
            else -> null
        }
        if (tag != null) {
            val actual = checkNotNull(findTag(tag))
            val at = IntArray(2).also(actual.first.view::getLocationOnScreen)
            val actualBounds = actual.second.boundsInRoot.translate(androidx.compose.ui.geometry.Offset(at[0].toFloat(), at[1].toFloat()))
            org.junit.Assert.assertEquals("$name native PhoneFace bounds match actual rendered face", faceBounds, actualBounds)
        }

        facePath = DrawerFace(faceBounds, source.direction).outline(density, direction).asAndroidPath()
        val chip = phone.layout.array("chips").objects().single { it.rect().contains(face.bounds(scale).center / scale) }
        val chipOrigin = androidx.compose.ui.geometry.Offset((chip.number("x") * scale).roundToInt().toFloat(),
            (chip.number("y") * scale).roundToInt().toFloat())
        val size = androidx.compose.ui.geometry.Size((chip.number("width") * scale).roundToInt().toFloat(),
            (chip.number("height") * scale).roundToInt().toFloat())
        chipPath = androidx.compose.ui.graphics.Path().apply {
            addOutline(dock.drawerContainerShape(chip, minOf(chip.number("width"), chip.number("height")) / 2).createOutline(size, direction, density))
            translate(chipOrigin + origin)
        }.asAndroidPath()
        val icon = phone.layout.number(if (face.anchor.optString("kind") == "capy") "capy_icon" else "icon") * scale + 4f
        glyphBounds = androidx.compose.ui.geometry.Rect(faceBounds.center - androidx.compose.ui.geometry.Offset(icon / 2, icon / 2),
            faceBounds.center + androidx.compose.ui.geometry.Offset(icon / 2, icon / 2))
        val index = phone.faces.indexOf(face) - 2
        variants = index >= 0 && snapshot!!.array("phone_tools").getJSONObject(index).optBoolean("has_variants")
    }
    val clip = android.graphics.Region(0, 0, image.width, image.height)
    fun region(path: android.graphics.Path) = android.graphics.Region().apply { setPath(path, clip) }
    fun stroke(path: android.graphics.Path, width: Float): android.graphics.Region {
        val paint = android.graphics.Paint().apply { style = android.graphics.Paint.Style.STROKE; strokeWidth = width }
        val outline = android.graphics.Path()
        paint.getFillPath(path, outline)
        return region(outline)
    }
    fun rectangle(bounds: androidx.compose.ui.geometry.Rect) = android.graphics.Region(kotlin.math.ceil(bounds.left).toInt(),
        kotlin.math.ceil(bounds.top).toInt(), kotlin.math.floor(bounds.right).toInt(), kotlin.math.floor(bounds.bottom).toInt())
    val chipInterior = region(chipPath).apply { op(stroke(chipPath, 8f), android.graphics.Region.Op.DIFFERENCE) }
    val edge = stroke(facePath, 3f).apply {
        op(chipInterior, android.graphics.Region.Op.INTERSECT)
        op(rectangle(faceBounds.deflate(3f)), android.graphics.Region.Op.INTERSECT)
        op(rectangle(glyphBounds), android.graphics.Region.Op.DIFFERENCE)
        if (variants) op(rectangle(androidx.compose.ui.geometry.Rect(faceBounds.right - 16f * scale,
            faceBounds.bottom - 16f * scale, faceBounds.right, faceBounds.bottom)), android.graphics.Region.Op.DIFFERENCE)
    }
    val reference = android.graphics.Region(chipInterior).apply {
        op(rectangle(faceBounds), android.graphics.Region.Op.INTERSECT)
        op(region(facePath), android.graphics.Region.Op.DIFFERENCE)
        op(stroke(facePath, 8f), android.graphics.Region.Op.DIFFERENCE)
    }
    fun pixels(area: android.graphics.Region): List<Pair<androidx.compose.ui.unit.IntOffset, Int>> {
        val result = mutableListOf<Pair<androidx.compose.ui.unit.IntOffset, Int>>()
        val iterator = android.graphics.RegionIterator(area)
        val rect = android.graphics.Rect()
        while (iterator.next(rect)) for (y in rect.top until rect.bottom) for (x in rect.left until rect.right)
            result += androidx.compose.ui.unit.IntOffset(x, y) to image.getPixel(x, y)
        return result
    }
    val edgePixels = pixels(edge)
    val referencePixels = pixels(reference)
    org.junit.Assert.assertTrue("$name has interior face curves to inspect: $faceBounds", edgePixels.isNotEmpty())
    org.junit.Assert.assertTrue("$name has adjacent chip reference pixels: $faceBounds", referencePixels.isNotEmpty())
    val chipRgb = listOf(16, 8, 0).map { shift -> referencePixels.map { (_, color) -> (color shr shift) and 255 }.sorted().let { it[it.size / 2] } }
    fun excess(color: Int) = listOf(16, 8, 0).mapIndexed { index, shift ->
        val value = (color shr shift) and 255
        maxOf(minOf(faceRgb[index], chipRgb[index].toDouble()) - 1 - value,
            value - maxOf(faceRgb[index], chipRgb[index].toDouble()) - 1, 0.0)
    }.max()
    val worst = edgePixels.maxBy { excess(it.second) }
    val maximum = excess(worst.second)
    receipt.put("curves", obj("samples" to edgePixels.size, "reference_samples" to referencePixels.size,
        "chip_rgb" to org.json.JSONArray(chipRgb), "maximum_excess" to maximum,
        "worst" to obj("x" to worst.first.x, "y" to worst.first.y, "rgb" to org.json.JSONArray(listOf(16, 8, 0).map { (worst.second shr it) and 255 }))))
    if (maximum > 0) failures += "$name: curve exceeds adjacent face/chip RGB by $maximum beyond tolerance 1 at ${worst.first}"
}

internal fun snapshotSource(kind: String, occurrenceToken: Long): String {
    require(kind == "EffectInput" || kind == "EffectChannels")
    require(occurrenceToken in 1..0xffffffffL)
    return obj(kind to (occurrenceToken - 1)).toString()
}

internal fun snapshotSource(target: JSONObject): String {
    require(target.length() == 1)
    val kind = target.keys().next()
    require(kind in listOf("Paint", "Coverage", "Selection"))
    require(target.getLong(kind) in 0..0xfffffffeL)
    return obj("Source" to target).toString()
}
