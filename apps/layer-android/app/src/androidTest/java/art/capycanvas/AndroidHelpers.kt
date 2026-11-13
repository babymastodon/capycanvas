package art.capycanvas

import org.json.JSONObject
import kotlin.math.roundToInt

internal fun CanvasHost.frameGlassBackdrop(compose: androidx.compose.ui.test.junit4.ComposeTestRule) {
    drain(obj("type" to "invoke", "command" to "fit_canvas"))
    compose.waitForIdle()
    val state = snapshot!!.getJSONObject("state")
    val camera = state.getJSONObject("camera")
    val document = state.array("tabs").objects().single { it.getBoolean("active") }
    val viewport = camera.array("viewport")
    val zoom = camera.number("zoom")
    val target = .1f * minOf(viewport.getDouble(0).toFloat() / document.number("width"), viewport.getDouble(1).toFloat() / document.number("height"))
    val translation = camera.array("translation")
    val center = androidx.compose.ui.geometry.Offset(translation.getDouble(0).toFloat() + document.number("width") * zoom / 2,
        translation.getDouble(1).toFloat() + document.number("height") * zoom / 2)
    scroll(center.x, center.y, 0f, -kotlin.math.ln(target / zoom) / (.06f * state.getJSONObject("settings").number("zoom_speed", 1.0)), true, false)
    drain()
    awaitMain("Glass backdrop camera applied", 30_000, { snapshot!!.getJSONObject("state").getJSONObject("camera").toString() }, compose) {
        val current = snapshot!!.getJSONObject("state").getJSONObject("camera")
        kotlin.math.abs(current.number("zoom") - target) < target * .001f
    }
}

internal fun CanvasHost.captureDrawerGlass(name: String, frame: JSONObject, failures: MutableList<String>) {
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
    val receipt = obj("name" to name, "backdrop" to "surround", "camera" to camera.copy(), "frame" to frame)
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
                !paperBounds.inflate(64f).overlaps(patch))
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
    }
    org.junit.Assert.assertTrue("$name screenshot captured", inspected)
    java.io.File(instrumentation.targetContext.getExternalFilesDir(null), "validation/drawer-faces/$name.json").writeText(receipt.toString(2))
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
