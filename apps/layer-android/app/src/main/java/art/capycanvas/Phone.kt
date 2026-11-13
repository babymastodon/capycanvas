package art.capycanvas

import androidx.activity.compose.PredictiveBackHandler
import android.view.KeyEvent
import kotlinx.coroutines.CancellationException
import android.os.Build
import android.view.RoundedCorner
import androidx.compose.foundation.systemGestureExclusion
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.input.pointer.*
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.LocalWindowInfo
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.zIndex
import org.json.JSONArray
import org.json.JSONObject
import org.json.JSONTokener
import kotlinx.coroutines.Job
import kotlinx.coroutines.currentCoroutineContext
import kotlin.math.roundToInt

internal fun phoneUi(request: JSONObject): Any? = JSONTokener(Native.phoneUi(request.toString())).nextValue().takeUnless { it == JSONObject.NULL }
internal fun CanvasHost.phone(action: JSONObject) = dispatch(obj("type" to "phone", "action" to action))

internal data class PhoneFace(val anchor: JSONObject, val visual: JSONObject, val hit: JSONObject, val divider: Boolean = false, val lifted: Boolean = false) {
    fun bounds(density: Float): Rect {
        fun offset(axis: String) = (hit.number(axis) * density).roundToInt() + ((visual.number(axis) - hit.number(axis)) * density).roundToInt()
        val x = offset("x").toFloat(); val y = offset("y").toFloat()
        return Rect(x, y, x + (visual.number("width") * density).roundToInt(), y + (visual.number("height") * density).roundToInt())
    }
    fun modifier(density: Float) = Modifier.placedAbsolute(visual.copy().put("x", visual.number("x") - hit.number("x")).put("y", visual.number("y") - hit.number("y")), density)
}

@Composable internal fun PhoneBack(host: CanvasHost, dock: DockInteraction, enabled: Boolean) {
    PredictiveBackHandler(enabled) { progress ->
        host.retirePhoneContacts()
        try {
            progress.collect { }
            val down = KeyEvent(KeyEvent.ACTION_DOWN,KeyEvent.KEYCODE_ESCAPE)
            when {
                dock.contextMenu != null -> dock.closeContext()
                host.numberKeyHandler?.invoke(down) == true -> Unit
                host.palettes.escape() -> Unit
                host.dialogKeyHandler?.invoke(down) == true -> Unit
                else -> {
                    val key = obj("type" to "key","key" to "escape","pressed" to true,"repeat" to false,"editing" to host.editingText,
                        "modifiers" to obj("command" to false,"shift" to false,"alt" to false))
                    host.input(key) { if (!it.optBoolean("handled")) host.drawingTabs.closeSelected() }
                    host.input(key.copy().put("pressed",false))
                }
            }
        } catch (_: CancellationException) { }
    }
}

internal class PhoneInteraction(val host: CanvasHost, val dock: DockInteraction, private val view: android.view.View) {
    var layout = JSONObject()
    var tiles = JSONArray()
    var faces = emptyList<PhoneFace>()
    var contact: JSONObject? = null
    var capy = false
    var geometryKey: String? = null
    var held = false
    var heldContext = false
    var suppressed = false
    var pointerDown = false
    val active get() = capy || contact != null || jobs.isNotEmpty()
    var generation = 0
    val jobs = mutableSetOf<Job>()
    var menu: PhoneMenuInteraction? = null
    var preview by mutableStateOf<JSONObject?>(null)
    fun step(input: JSONObject): JSONObject {
        val value = phoneUi(obj("kind" to "bar_step", "layout" to layout, "tiles" to tiles, "contact" to contact, "input" to input)) as JSONObject
        contact = value.getJSONObject("contact")
        return value.getJSONObject("reply").also { preview = it }
    }
    fun valid() = (!capy || geometryKey == layout.getJSONObject("capy_target").toString()) &&
        (contact?.let { phoneUi(obj("kind" to "bar_valid", "layout" to layout, "tiles" to tiles, "contact" to it)) == true } ?: true)
    fun cancel() {
        generation++
        if (heldContext) { heldContext = false; dock.closeContext() }
        dock.contactHeld = false
        if (capy && held) host.phone(obj("kind" to "capy_drop", "position" to JSONArray(listOf(-1, -1))))
        suppressed = true; capy = false; held = false; contact = null; preview = null
    }
    fun endGesture() { if(!pointerDown)return;pointerDown = false;releaseClick() }
    fun releaseClick() {
        if(pointerDown)return
        val retired = generation
        view.postOnAnimation { view.post { if (generation == retired && !pointerDown) suppressed = false } }
    }
    fun retire() { cancel(); menu?.cancel(); jobs.toList().forEach { it.cancel() }; jobs.clear(); releaseClick() }
    fun click(action: JSONObject) { if (!suppressed) host.phone(action) }
}

@Composable internal fun Modifier.phoneGestures(host: CanvasHost, dock: DockInteraction): Modifier {
    val focused = LocalWindowInfo.current.isWindowFocused
    SideEffect { dock.windowFocus(focused) }
    return pointerInput(host, host.phoneContactEpoch, focused) {
    if (!focused) return@pointerInput
    val job = currentCoroutineContext()[Job]!!
    awaitEachGesture {
        val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
        val input = dock.phone ?: return@awaitEachGesture
        if (!dock.enabled || dock.popupOpen || down.isConsumed) return@awaitEachGesture
        val start = down.position / dock.density
        input.generation++
        input.suppressed = false
        input.capy = input.layout.getJSONObject("capy_target").rect().contains(start)
        input.geometryKey = input.layout.getJSONObject("capy_target").toString()
        input.contact = if (input.capy) null else phoneUi(obj("kind" to "bar_begin", "layout" to input.layout,
            "tiles" to input.tiles, "point" to JSONArray(listOf(start.x, start.y)))) as? JSONObject
        if (!input.capy && input.contact == null) return@awaitEachGesture
        input.pointerDown = true
        input.jobs.add(job)
        val generation = input.generation
        dock.beginContact(down.position, down.type)
        var released = false
        var cancelled = false
        var remaining = viewConfiguration.longPressTimeoutMillis
        var eventTime = down.uptimeMillis
        fun menu(tile: Any?) {
            val target = if (tile == null || tile == JSONObject.NULL) obj("kind" to "phone_toolbar") else obj("kind" to "phone_tile", "tile" to tile)
            dock.context(target)
        }
        if (currentEvent.buttons.isSecondaryPressed) {
            input.suppressed = true; down.consume(); cancelled = true
            val index = input.layout.array("tool_hits").objects().indexOfFirst { it.rect().contains(start) }
            val tile = input.tiles.optJSONObject(index)
            menu(tile?.getInt("id"))
        }
        try {
            while (true) {
                val event = if (!input.held && !cancelled) withTimeoutOrNull(remaining) { awaitPointerEvent(PointerEventPass.Initial) }
                    else awaitPointerEvent(PointerEventPass.Initial)
                if (generation != input.generation || !dock.enabled || dock.popupOpen) {
                    if (!cancelled) input.cancel()
                    cancelled = true; input.suppressed = true
                }
                if (event == null) {
                    if (cancelled) continue
                    if (input.capy) { input.held = true; input.suppressed = true; host.phone(obj("kind" to "capy_hold")) }
                    else {
                        val outcome = input.step(obj("kind" to "arm")).getJSONObject("outcome")
                        when (outcome.getString("kind")) {
                            "menu" -> {
                                input.held = true; input.suppressed = true; dock.contactHeld = true
                                if(down.type != PointerType.Mouse) { input.heldContext = true; menu(outcome.opt("value")) }
                            }
                            "cancel" -> { input.cancel(); cancelled = true }
                            else -> cancelled = true
                        }
                    }
                    continue
                }
                val change = event.changes.firstOrNull { it.id == down.id } ?: break
                remaining = (remaining - (change.uptimeMillis - eventTime)).coerceAtLeast(1)
                eventTime = change.uptimeMillis
                val point = change.position / dock.density
                val pastSlop = (change.position - down.position).getDistance() > viewConfiguration.touchSlop
                if (!input.valid() || event.changes.count { it.pressed } > 1 || change.isConsumed && !change.pressed) {
                    input.cancel(); cancelled = true
                }
                if (!cancelled && change.pressed) {
                    if (input.capy) {
                        if (pastSlop && !input.held) { input.cancel(); cancelled = true }
                    } else when (input.step(obj("kind" to "motion", "position" to JSONArray(listOf(point.x, point.y)), "past_slop" to pastSlop)).getJSONObject("outcome").getString("kind")) {
                        "dragging" -> {
                            if(input.heldContext) { input.heldContext = false; dock.closeContext() }
                            input.suppressed = true
                        }
                        "held" -> input.suppressed = true
                        "window_move", "cancel" -> { input.cancel(); cancelled = true }
                    }
                }
                if (input.suppressed || cancelled) change.consume()
                if (!change.pressed) {
                    released = true
                    if (!cancelled) {
                        if (input.capy) {
                            if (input.held) host.phone(obj("kind" to "capy_drop", "position" to JSONArray(listOf(point.x, point.y))))
                        } else {
                            val outcome = input.step(obj("kind" to "finish", "position" to JSONArray(listOf(point.x, point.y)))).getJSONObject("outcome")
                            when (outcome.getString("kind")) {
                                "move" -> host.phone(outcome.getJSONObject("value").copy().put("kind", "move_tool"))
                                "cancel" -> input.suppressed = true
                            }
                        }
                    }
                    break
                }
            }
        } finally {
            if (!released) input.cancel()
            input.heldContext = false; dock.contactHeld = false
            input.capy = false; input.held = false; input.contact = null; input.preview = null
            input.jobs.remove(job);input.releaseClick()
        }
    }
}
}

@Composable internal fun PhoneFrame(host: CanvasHost, snapshot: JSONObject, dock: DockInteraction) {
    val layout = snapshot.getJSONObject("layout").getJSONObject("phone")
    val state = snapshot.getJSONObject("state")
    val tiles = state.getJSONObject("workspace").getJSONObject("layout").array("phone")
    val view = LocalView.current
    val input = remember(host, dock, view) { PhoneInteraction(host, dock, view) }
    val density = LocalDensity.current.density
    val colors = LocalPalette.current
    val toolbarContext = remember { obj("kind" to "phone_toolbar") }
    val settingsAction = remember { obj("kind" to "tool_settings") }
    val preview = input.preview
    val faces = remember(layout, tiles, preview) { listOf(PhoneFace(obj("kind" to "capy"), layout.getJSONObject("capy"), layout.getJSONObject("capy_target")),
        PhoneFace(settingsAction, layout.getJSONObject("settings"), layout.getJSONObject("settings_hit"))) + tiles.objects().mapIndexed { index, tile ->
        val lifted = preview?.optInt("lifted", -1) == index
        val visual = preview?.array("geometry")?.optJSONObject(index) ?: layout.array("tools").getJSONObject(index)
        val divider = tile.getJSONObject("control").getString("kind") == "divider"
        PhoneFace(obj("kind" to "tool", "id" to tile.getInt("id")), visual,
            if (lifted || divider) visual else layout.array("tool_hits").getJSONObject(index), divider, lifted)
    } }
    SideEffect { input.layout = layout; input.tiles = tiles; input.faces = faces; dock.phone = input; if (!input.valid()) input.cancel() }
    DisposableEffect(input) { onDispose { input.cancel(); if (dock.phone === input) dock.phone = null } }
    PhoneMetrics(host, state)
    Box(Modifier.fillMaxSize().zIndex(300f).testTag("phone-frame"), contentAlignment = androidx.compose.ui.AbsoluteAlignment.TopLeft) {
        layout.array("bars").objects().forEachIndexed { index, rect ->
            key(index) {
            if (rect.number("width") > 0 && rect.number("height") > 0) Box(Modifier.placedAbsolute(rect, density)
                .chromeRegion(dock).contextAnchor(dock, toolbarContext).testTag("phone-bar-$index"))
            }
        }
        layout.array("chips").objects().forEachIndexed { index, rect ->
            key(index) {
            val radius=minOf(rect.number("width"),rect.number("height"))/2
            if (rect.number("width") > 0 && rect.number("height") > 0) Box(Modifier.placedAbsolute(rect, density)
                .glass(dock.drawerContainerShape(rect,radius), colors.headerSurface,
                    nativeShape={dock.drawerContainerShape(rect,radius,sources=dock.drawerSources.values.filter { it.glass?.active==true })}).testTag("phone-chip-$index"))
            }
        }
        val views = snapshot.array("phone_tools")
        tiles.objects().forEachIndexed { index, tile ->
            val id = tile.getInt("id")
            key(id) {
                val (action, context) = remember(id) { obj("kind" to "tool", "tile" to id) to obj("kind" to "phone_tile", "tile" to id) }
                PhoneTile(host, dock, input, views.getJSONObject(index), faces[index + 2], "phone-tool-$id", action, context, layout.number("icon"))
            }
        }
        PhoneTile(host, dock, input, snapshot.getJSONObject("phone_settings"), faces[1],
            "phone-tool-settings", settingsAction, null, layout.number("icon"))
        val capyFocus = remember { FocusRequester() }
        val capyModifier = remember(host,capyFocus) { Modifier.fillMaxSize().phoneFocus(host,capyFocus).testTag("phone-capy") }
        val capy = snapshot.getJSONObject("header").getJSONObject("capy")
        val capyFace = faces[0]
        val source = dock.anchorSource(obj("kind" to "phone", "anchor" to capyFace.anchor))
        Box(Modifier.placedAbsolute(capyFace.hit, density).chromeRegion(dock).testTag("phone-capy-target"), contentAlignment = androidx.compose.ui.AbsoluteAlignment.TopLeft) {
            key(host.phoneContactEpoch) {
            HeaderButton(snapshot.getString("capy_caption"), false, true, source?.direction,
                capyModifier, inBar = true, drawerPresented = source?.presented == true,
                face = capyFace.modifier(density),
                onClick = { input.click(obj("kind" to "capy")) }) {
                SharedIcon(capy.getString("icon"), null, Modifier.size(layout.number("capy_icon").dp))
            }
            }
        }
        for ((index, entry) in listOf("size" to "brush_size_slider", "strength" to "brush_strength_slider").withIndex()) {
            val (name, kind) = entry
            val bounds = layout.getJSONObject(name)
            if (bounds.number("width") > 0 && bounds.number("height") > 0) {
                val control = remember(kind) { obj("kind" to kind) }
                snapshot.array("phone_sliders").optJSONObject(index)?.let {
                    ToolbarComponent(host, control, it, bounds, remember { obj("tile_style" to "medium") }, dock, layout.getString("edge") != "top",
                        Modifier.placedAbsolute(bounds, density).chromeRegion(dock).systemGestureExclusion().testTag("phone-slider-$name"),
                        remember { obj("show_value" to false, "expand" to false) }, name, phone = layout)
                }
            }
        }
    }
}

@Composable private fun PhoneTile(host: CanvasHost, dock: DockInteraction, input: PhoneInteraction, view: JSONObject,
    geometry: PhoneFace, tag: String, action: JSONObject, context: JSONObject?, icon: Float) {
    if (geometry.hit.number("width") <= 0 || geometry.hit.number("height") <= 0) return
    val density = LocalDensity.current.density
    val focus = remember { FocusRequester() }
    val source = dock.anchorSource(obj("kind" to "phone", "anchor" to geometry.anchor))
    Box(Modifier.placedAbsolute(geometry.hit, density).zIndex(if (geometry.lifted) 1f else 0f).chromeRegion(dock)
        .then(if (context != null) Modifier.contextAnchor(dock, context) else Modifier), contentAlignment = androidx.compose.ui.AbsoluteAlignment.TopLeft) {
        val face = geometry.modifier(density).testTag(tag)
        if (geometry.divider) ToolbarDivider(face, horizontal = input.layout.getString("edge") != "top", length = null)
        else key(host.phoneContactEpoch) { HeaderButton(view.getString("tooltip"), view.optBoolean("selected"), view.optBoolean("enabled") || view.optBoolean("has_variants"), source?.direction,
            Modifier.fillMaxSize().phoneFocus(host,focus).semantics { selected = view.optBoolean("selected") }.testTag("$tag-hit"), inBar = true, face = face,
            drawerPresented = source?.presented == true,
            reason = view.nullableString("disabled_reason"),
            onDoubleClick = if (view.optBoolean("double_click")) ({ host.dispatch(obj("type" to "double_click_tool", "control" to view.getJSONObject("resolved_control"))) }) else null,
            onClick = { input.click(action) }) {
            LiveIcon(host, view.getString("icon"), null, Modifier.size(icon.dp))
            if (view.optBoolean("has_variants")) Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomEnd) { ToolGroupIndicator() }
        } }
    }
}

private fun Modifier.phoneFocus(host: CanvasHost,focus: FocusRequester) = focusRequester(focus).onFocusChanged {
    if(it.isFocused)host.workspaceOpener = focus
    else if(host.workspaceOpener === focus)host.workspaceOpener = null
}

@OptIn(ExperimentalLayoutApi::class)
@Composable private fun PhoneMetrics(host: CanvasHost, state: JSONObject) {
    val density = LocalDensity.current
    val view = LocalView.current
    val safe = WindowInsets.displayCutout.union(WindowInsets.waterfall)
    val keyboard = WindowInsets.imeAnimationTarget.getBottom(density) / density.density
    val insets = listOf(safe.getLeft(density, androidx.compose.ui.unit.LayoutDirection.Ltr), safe.getTop(density),
        safe.getRight(density, androidx.compose.ui.unit.LayoutDirection.Ltr), safe.getBottom(density)).map { it / density.density }
    val report by rememberUpdatedState(newValue = {
        val nativeInsets = host.windowInsets ?: view.rootWindowInsets
        val corners = if (Build.VERSION.SDK_INT >= 31) listOf(RoundedCorner.POSITION_TOP_LEFT, RoundedCorner.POSITION_TOP_RIGHT,
            RoundedCorner.POSITION_BOTTOM_RIGHT, RoundedCorner.POSITION_BOTTOM_LEFT).map { position ->
            nativeInsets?.getRoundedCorner(position)?.let { corner ->
                val left = position == RoundedCorner.POSITION_TOP_LEFT || position == RoundedCorner.POSITION_BOTTOM_LEFT
                val top = position == RoundedCorner.POSITION_TOP_LEFT || position == RoundedCorner.POSITION_TOP_RIGHT
                val x = if (left) corner.center.x else view.width - corner.center.x
                val y = if (top) corner.center.y else view.height - corner.center.y
                if (x in 0..corner.radius && y in 0..corner.radius) corner.radius / density.density else 0f
            } ?: 0f
        } else List(4) { 0f }
        val currentInsets = listOf(safe.getLeft(density, androidx.compose.ui.unit.LayoutDirection.Ltr), safe.getTop(density),
            safe.getRight(density, androidx.compose.ui.unit.LayoutDirection.Ltr), safe.getBottom(density)).map { it / density.density }
        if (state.optBoolean("workspace_ready")) host.reportPhoneMetrics(JSONArray(currentInsets), JSONArray(corners), keyboard)
    })
    DisposableEffect(host) {
        val listener = { report() }
        host.windowMetricsChanged = listener
        onDispose { if (host.windowMetricsChanged === listener) host.windowMetricsChanged = null }
    }
    LaunchedEffect(insets, density, keyboard, state.optBoolean("workspace_ready")) { report() }
}
