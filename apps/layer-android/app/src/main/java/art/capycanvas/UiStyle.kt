package art.capycanvas

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.hoverable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.IndicationNodeFactory
import androidx.compose.foundation.LocalIndication
import androidx.compose.foundation.interaction.FocusInteraction
import androidx.compose.foundation.interaction.InteractionSource
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsHoveredAsState
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.CornerBasedShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardActionScope
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.*
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.foundation.focusable
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.drawscope.inset
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onKeyEvent
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.isSecondaryPressed
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.disabled
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.setProgress
import androidx.compose.ui.unit.Constraints
import kotlin.math.abs
import kotlin.math.roundToInt
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONObject
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.LocalInputModeManager
import androidx.compose.ui.input.InputMode
import androidx.compose.ui.window.DialogWindowProvider
import androidx.compose.ui.graphics.drawscope.ContentDrawScope
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawOutline
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.node.DrawModifierNode
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.unit.toSize
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntRect
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.PopupPositionProvider

private fun org.json.JSONArray.rgba() = Color(getDouble(0).toFloat(), getDouble(1).toFloat(), getDouble(2).toFloat(), getDouble(3).toFloat())

internal class Palette(val dark: Boolean, private val source: org.json.JSONObject, private val inGlass: Boolean = false) {
    private fun role(name: String) = Color(android.graphics.Color.parseColor(source.getString(name)))
    private fun glass(name: String) = source.getJSONObject("glass").getJSONArray(name).rgba()
    private fun line(surface: String, role: String) = source.getJSONObject("lines").getJSONObject(surface).getJSONArray(role).rgba()
    val surround = role("bg")
    val headerSurface = glass("chip")
    val switcher = glass("switcher")
    val switcherActive = glass("switcher_selection")
    val documentTab = glass("document_tab")
    val panel = role("panel")
    val tabs = role("tabbar")
    val panelFill = if (inGlass) glass("panel") else panel
    val strip = if (inGlass) glass("strip") else tabs
    val sidebar = role("sidebar")
    val input = role("input")
    val view = role("view")
    val text = role("text")
    val secondary = text.copy(alpha = .55f)
    val accent = role("accent")
    val error = role("error")
    val relationship = role("relationship")
    val accentForeground = role("accent_foreground")
    val sliderFill = lerp(panel, text, .5f)
    val active = if (inGlass) glass("selection") else role("selection")
    val headerActive = glass("header_selection")
    val button = role("button").copy(alpha = 13 / 255f)
    val thumb = role("thumb")
    val checkerLight = role("checker_light")
    val checkerDark = role("checker_dark")
    val divider = text.copy(alpha = .12f)
    val dialogField = text.copy(alpha = .1f)
    val settingsBackground = role("settings")
    val settingsCard = role("card")
    val settingsSecondary = role("settings_secondary")
    val rowDivider = line("settings", "card_shade")
    val cardRing = line("settings", "card_ring")
    val cardShadow = line("settings", "card_shadow")
    val cardFarShadow = line("settings", "card_far_shadow")
    val dialogSeparator = line("settings", "border")
    val menuSeparator = line("panel", "border")
    val onGlass by lazy { Palette(dark, source, true) }
}

@Composable internal fun Modifier.glass(shape: Shape, color: Color = Color.Transparent, rootOffset: () -> Offset = { Offset.Zero },
    key: Any = remember { Any() }, active: Boolean = true, nativeShape: (() -> Shape)? = null): Modifier {
    val host = LocalCanvasHost.current
    val density = LocalDensity.current
    val opacity=LocalWorkspaceOpacity.current
    val currentShape by rememberUpdatedState(nativeShape)
    var bounds by remember { mutableStateOf<FloatArray?>(null) }
    DisposableEffect(host, key) { onDispose { host.glassBox(key, null) } }
    LaunchedEffect(host,key,active,opacity,density) {
        snapshotFlow {
            val box=bounds?.toList()?.let { box ->
                val corners=currentShape?.invoke() as? CornerBasedShape
                if(corners==null)box else box.take(4)+listOf(corners.topStart,corners.topEnd,corners.bottomEnd,corners.bottomStart)
                    .map { it.toPx(androidx.compose.ui.geometry.Size(box[2],box[3]),density) }
            }
            box to (opacity?.value ?: 1f)
        }.collect { (box,strength) ->
            host.glassBox(key,box?.takeIf { active }?.toFloatArray(),strength)
        }
    }
    return background(color, shape).onGloballyPositioned { coords ->
        val origin = coords.positionInRoot() + rootOffset() - host.surfaceOrigin
        val size = coords.size.toSize()
        val radii = (shape as? CornerBasedShape)?.let { s ->
            listOf(s.topStart, s.topEnd, s.bottomEnd, s.bottomStart).map { it.toPx(size, density) }
        } ?: listOf(0f, 0f, 0f, 0f)
        bounds=floatArrayOf(origin.x, origin.y, size.width, size.height) + radii
    }
}
internal val LocalPalette = staticCompositionLocalOf<Palette> { error("Missing core palette") }
internal val LocalCanvasHost = staticCompositionLocalOf<CanvasHost> { error("Missing native host") }
internal val HeaderTextPadding = 8.dp

/** Tab colors never depend on hover/press. A focus-only indication also avoids
 * Android's native ripple layer changing the rasterization of tab joins. */
@Composable internal fun PanelHeaderFeedback(content: @Composable () -> Unit) {
    CompositionLocalProvider(LocalRippleConfiguration provides null,
        LocalIndication provides rememberChromeFocusIndication(), content = content)
}

@Composable internal fun rememberChromeFocusIndication(): IndicationNodeFactory {
    val accent = LocalPalette.current.accent
    return remember(accent) { ChromeFocusIndication(accent) }
}

private data class ChromeFocusIndication(val color: Color) : IndicationNodeFactory {
    override fun create(interactionSource: InteractionSource): Modifier.Node = object : Modifier.Node(), DrawModifierNode {
        var focused by mutableStateOf(false)
        override fun onAttach() {
            coroutineScope.launch {
                interactionSource.interactions.collect { interaction ->
                    when (interaction) {
                        is FocusInteraction.Focus -> focused = true
                        is FocusInteraction.Unfocus -> focused = false
                    }
                }
            }
        }
        override fun ContentDrawScope.draw() {
            drawContent()
            if (focused) drawOutline(ControlShape.createOutline(size, layoutDirection, this), color, style = Stroke(2.dp.toPx()))
        }
    }
}

@Composable internal fun keyboardInputMode() = LocalInputModeManager.current.inputMode == InputMode.Keyboard

private tailrec fun inputActivity(context: android.content.Context): android.app.Activity = when (context) {
    is android.app.Activity -> context
    is android.content.ContextWrapper -> inputActivity(context.baseContext)
    else -> error("Text input has no native window")
}

@Composable internal fun CoreTextField(value: TextFieldValue, onValueChange: (TextFieldValue) -> Unit,
    modifier: Modifier = Modifier, enabled: Boolean = true, singleLine: Boolean = true,
    textStyle: TextStyle = LocalTextStyle.current, cursorBrush: Brush = SolidColor(LocalPalette.current.accent),
    keyboardOptions: KeyboardOptions = KeyboardOptions.Default, keyboardActions: KeyboardActions = KeyboardActions.Default,
    decorationBox: @Composable (@Composable () -> Unit) -> Unit = { it() }) {
    val host = LocalCanvasHost.current
    val owner = remember { Any() }
    val endEntry = rememberEndTextEntry()
    val inDialog = LocalDialogFocus.current != null
    var focused by remember { mutableStateOf(false) }
    val view = LocalView.current
    val window = remember(view) { (view.parent as? DialogWindowProvider)?.window ?: inputActivity(view.context).window }
    DisposableEffect(window, owner) {
        val release = host.textComposition.window(window)
        onDispose { release(); host.textComposition.clear(owner); if (focused) host.editingText = false }
    }
    var latest by remember { mutableStateOf(value) }
    val change by rememberUpdatedState(onValueChange)
    fun update(next: TextFieldValue) {
        latest = next
        host.textComposition.update(owner, next, focused)
        change(next)
    }
    val commit = { if (latest.composition != null) update(latest.copy(composition = null)) }
    SideEffect { latest = value; host.textComposition.update(owner, value, focused, commit) }
    fun action(ime: ImeAction, callback: (KeyboardActionScope.() -> Unit)?): KeyboardActionScope.() -> Unit = {
        commit()
        if (callback != null) callback() else if (ime == ImeAction.Done && inDialog) endEntry() else defaultKeyboardAction(ime)
    }
    BasicTextField(value, ::update,
        Modifier.onFocusChanged {
            if (focused != it.isFocused) host.editingText = it.isFocused
            focused = it.isFocused
            host.textComposition.update(owner, latest, focused, commit)
        }.then(modifier), enabled = enabled, singleLine = singleLine, textStyle = textStyle, cursorBrush = cursorBrush,
        keyboardOptions = keyboardOptions, keyboardActions = KeyboardActions(
            onDone = action(ImeAction.Done, keyboardActions.onDone), onGo = action(ImeAction.Go, keyboardActions.onGo),
            onNext = action(ImeAction.Next, keyboardActions.onNext), onPrevious = action(ImeAction.Previous, keyboardActions.onPrevious),
            onSearch = action(ImeAction.Search, keyboardActions.onSearch), onSend = action(ImeAction.Send, keyboardActions.onSend)),
        decorationBox = decorationBox)
}

@Composable internal fun CoreTextField(value: String, onChange: (String) -> Unit,
    modifier: Modifier = Modifier, label: (@Composable () -> Unit)? = null,
    placeholder: (@Composable () -> Unit)? = null, leadingIcon: (@Composable () -> Unit)? = null,
    trailingIcon: (@Composable () -> Unit)? = null,
    height: Dp = 36.dp, enabled: Boolean = true, focusRequest: Long = 0,
    textStyle: TextStyle = LocalTextStyle.current, shape: Shape = RoundedCornerShape(6.dp),
    keyboardOptions: androidx.compose.foundation.text.KeyboardOptions = androidx.compose.foundation.text.KeyboardOptions.Default,
    maxLength: Int = Int.MAX_VALUE, selectAllOnFocusRequest: Boolean = false, background: Color? = null, onCommit: ((String) -> Unit)? = null) {
    var text by remember { mutableStateOf(TextFieldValue(value, TextRange(value.length))) }
    var focused by remember { mutableStateOf(false) }
    val requester = remember { FocusRequester() }
    val endEntry = rememberEndTextEntry()
    LaunchedEffect(value, focused) { if (!focused) text = TextFieldValue(value, TextRange(value.length)) }
    // A core type-to-search request carries text as well as focus. Apply it
    // even if another key arrived before the native field gained focus.
    LaunchedEffect(focusRequest) {
        if (focusRequest != 0L) {
            text = TextFieldValue(value, if (selectAllOnFocusRequest) TextRange(0, value.length) else TextRange(value.length))
            requester.requestFocus()
        }
    }
    val colors = LocalPalette.current
    Column(modifier.edgeControl().background(background ?: colors.input, shape)) {
        label?.let { Box(Modifier.padding(start = 10.dp, top = 6.dp)) { it() } }
        CoreTextField(text, { next ->
            val previous = text.text
            text = next
            if (next.composition == null && next.text.length > maxLength) text = TextFieldValue(next.text.take(maxLength))
            if (text.text != previous && onCommit == null) onChange(text.text)
        },
            Modifier.fillMaxWidth().height(height).textCursorRegion(enabled).focusRequester(requester).onFocusChanged {
                if (focused && !it.isFocused) onCommit?.invoke(text.text)
                focused = it.isFocused
            },
            enabled = enabled, singleLine = true, textStyle = textStyle.copy(color = colors.text),
            cursorBrush = SolidColor(colors.accent), keyboardOptions = keyboardOptions.copy(imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = { endEntry() }),
            decorationBox = { field ->
                Row(Modifier.fillMaxSize().padding(horizontal = 6.dp), verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    leadingIcon?.invoke()
                    Box(Modifier.weight(1f)) {
                        if (text.text.isEmpty()) ProvideTextStyle(textStyle.copy(color = colors.secondary)) { placeholder?.invoke() }
                        field()
                    }
                    trailingIcon?.invoke()
                }
            })
    }
}

@Composable internal fun PanelGrip(description: String, vertical: Boolean = false) {
    SharedIcon("grip", description, Modifier.alpha(.65f)
        .offset(x = if (vertical) 0.dp else (-2).dp, y = if (vertical) (-2).dp else 0.dp)
        .rotate(if (vertical) 90f else 0f))
}

/** Slider gestures/semantics with a 4dp track and an optional 16dp thumb. */
@Composable internal fun EditorSlider(value: Float, onChange: (Float) -> Unit,
    modifier: Modifier = Modifier, enabled: Boolean = true, label: String = "", height: Dp = 28.dp,
    inactiveTrackColor: Color = LocalPalette.current.input, showThumb: Boolean = true,
    activeTrackColor: Color = LocalPalette.current.accent, onPress: () -> Unit = {}, onCancel: () -> Unit = {}) {
    val colors = LocalPalette.current
    val endEntry = rememberEndTextEntry()
    val fill = value.coerceIn(0f, 1f)
    val current by rememberUpdatedState(fill)
    val change by rememberUpdatedState(onChange)
    val press by rememberUpdatedState(onPress)
    val cancel by rememberUpdatedState(onCancel)
    val thumb = if (showThumb) 16.dp else 0.dp
    Layout({ if (showThumb) Box(Modifier.size(thumb).shadow(2.dp, CircleShape).background(colors.thumb, CircleShape)) },
        modifier.height(height).semantics {
            contentDescription = label
            progressBarRangeInfo = ProgressBarRangeInfo(fill, 0f..1f)
            if (enabled) setProgress { change(it.coerceIn(0f, 1f)); true } else disabled()
        }.onKeyEvent { event ->
            val next = when (event.key) {
                Key.DirectionRight, Key.DirectionUp -> current + .01f
                Key.DirectionLeft, Key.DirectionDown -> current - .01f
                Key.PageUp -> current + .1f
                Key.PageDown -> current - .1f
                Key.MoveHome -> 0f
                Key.MoveEnd -> 1f
                else -> null
            }
            if (!enabled || next == null) false
            else { if (event.type == KeyEventType.KeyDown) change(next.coerceIn(0f, 1f)); true }
        }.focusable(enabled).pointerInput(enabled, thumb) {
            if (!enabled) return@pointerInput
            awaitEachGesture {
                val down = awaitFirstDown()
                if (currentEvent.buttons.isSecondaryPressed) return@awaitEachGesture
                down.consume(); endEntry(); press()
                val inset = thumb.toPx() / 2
                var picked = Float.NaN
                fun pick(x: Float) = ((x - inset) / maxOf(1f, size.width - 2 * inset)).coerceIn(0f, 1f)
                    .let { if (it != picked) { picked = it; change(it) } }
                pick(down.position.x)
                var owned = false
                while (true) {
                    val contact = awaitPointerEvent().changes.firstOrNull { it.id == down.id }
                    if (contact == null || contact.isConsumed) { cancel(); break }
                    if (!contact.pressed) { contact.consume(); break }
                    val moved = contact.position - down.position
                    owned = owned || abs(moved.x) > viewConfiguration.touchSlop && abs(moved.x) >= abs(moved.y)
                    if (owned) contact.consume()
                    pick(contact.position.x)
                    if (!owned && awaitPointerEvent(PointerEventPass.Final).changes.firstOrNull { it.id == down.id }?.isConsumed != false) {
                        cancel(); break
                    }
                }
            }
        }.drawBehind {
            val track = 4.dp.roundToPx(); val knob = thumb.roundToPx(); val top = ((size.height.roundToInt() - track) / 2).toFloat()
            inset((knob / 2).toFloat(), top, (knob - knob / 2).toFloat(), size.height - top - track) {
                editorSliderTrack(inactiveTrackColor, activeTrackColor.copy(alpha = if (enabled) 1f else .4f), size.width * fill)
            }
        }) { measurables, constraints ->
        val width = if (constraints.hasBoundedWidth) constraints.maxWidth else constraints.minWidth
        val knobs = measurables.map { it.measure(Constraints()) }
        layout(width, constraints.maxHeight) {
            knobs.forEach { it.place(((width - it.width) * fill).roundToInt(), (constraints.maxHeight - it.height) / 2) }
        }
    }
}

internal fun DrawScope.editorSliderTrack(inactive: Color, active: Color, fillWidth: Float) {
    val radius = CornerRadius(size.height / 2f)
    drawRoundRect(inactive, cornerRadius = radius)
    if (fillWidth > 0f) drawRoundRect(active, size = Size(fillWidth, size.height), cornerRadius = radius)
}

internal val LocalCompactPanel = staticCompositionLocalOf { false }
internal class PanelMetrics(view: JSONObject) {
    val inset = view.getDouble("inset").toFloat().dp
    val spacing = view.getDouble("control_spacing").toFloat().dp
    val group = view.getDouble("group_spacing").toFloat().dp
    val compound = view.getDouble("compound_spacing").toFloat().dp
    val height = view.getDouble("control_height").toFloat().dp
    val sliderHeight = view.getDouble("slider_height").toFloat().dp
    val padding = view.getDouble("field_padding").toFloat().dp
    val icon = view.getDouble("icon_size").toFloat().dp
}
@Composable internal fun panelMetrics(): PanelMetrics {
    val host = LocalCanvasHost.current
    return remember(host.catalog) { PanelMetrics(host.catalog.getJSONObject("panel_metrics")) }
}
@Composable internal fun panelLabelWidth(labels: List<String>): androidx.compose.ui.unit.Dp {
    val measurer = androidx.compose.ui.text.rememberTextMeasurer()
    val style = LocalTextStyle.current
    return with(androidx.compose.ui.platform.LocalDensity.current) {
        labels.maxOfOrNull { measurer.measure(it, style).size.width }?.toDp() ?: 0.dp
    }.coerceAtMost(120.dp)
}

@Composable internal fun EditorCheck(checked: Boolean, label: String, modifier: Modifier = Modifier,
    enabled: Boolean = true, onChange: (Boolean) -> Unit) {
    val colors = LocalPalette.current
    val compact=LocalCompactPanel.current
    Box(modifier.size(if (compact) 18.dp else 22.dp, if (compact) panelMetrics().height else 28.dp).toggleable(checked, enabled = enabled, role = Role.Checkbox, onValueChange = onChange)
        .semantics { contentDescription = label }, contentAlignment = Alignment.Center) {
        Box(Modifier.size(16.dp).clip(SquircleShape(4.dp))
            .then(if (checked) Modifier.background(colors.accent) else Modifier.border(2.dp, colors.text.copy(alpha = .35f), SquircleShape(4.dp)))) {
            if (checked) SharedIcon("check", null, tint = colors.accentForeground)
        }
    }
}
/** GTK's south/north anchor: center below the tile, flip up and slide at edges. */
private class TileTooltipPositionProvider(private val gap: Int) : PopupPositionProvider {
    override fun calculatePosition(anchorBounds: IntRect, windowSize: IntSize,
        layoutDirection: LayoutDirection, popupContentSize: IntSize): IntOffset {
        val x = (anchorBounds.left + (anchorBounds.width - popupContentSize.width) / 2)
            .coerceIn(0, (windowSize.width - popupContentSize.width).coerceAtLeast(0))
        val below = anchorBounds.bottom + gap
        val y = if (below + popupContentSize.height <= windowSize.height) below
            else anchorBounds.top - gap - popupContentSize.height
        return IntOffset(x, y.coerceIn(0, (windowSize.height - popupContentSize.height).coerceAtLeast(0)))
    }
}

/** Hover only: touch holds remain available for editing and context menus. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable internal fun HoverTip(label: String, modifier: Modifier = Modifier,
    resolve: (((String) -> Unit) -> Unit)? = null, enabled: Boolean = true, reveal: Int = 0, tipContent: (@Composable (String) -> Unit)? = null, content: @Composable () -> Unit) {
    val interaction = remember { MutableInteractionSource() }
    val hovered by interaction.collectIsHoveredAsState()
    val state = rememberTooltipState()
    val gap = with(LocalDensity.current) { 4.dp.roundToPx() }
    val position = remember(gap) { TileTooltipPositionProvider(gap) }
    var text by remember(label) { mutableStateOf(label) }
    val currentResolve by rememberUpdatedState(resolve)
    LaunchedEffect(hovered && enabled, label) {
        if (hovered && enabled) currentResolve?.invoke { text = it }
    }
    LaunchedEffect(hovered && enabled) {
        if (hovered && enabled) {
            delay(500)
            state.show()
        } else state.dismiss()
    }
    LaunchedEffect(reveal) {
        if (reveal > 0) {
            currentResolve?.invoke { text = it }
            state.show()
        }
    }
    Box(modifier, propagateMinConstraints = true) {
        Box(Modifier.hoverable(interaction)) {
            // Keep interactive content and pointer capture stable. Only the
            // hovered item needs popup infrastructure. Its outer Box owns the
            // match-parent constraint; TooltipBox applies modifiers internally.
            content()
            if ((hovered && enabled) || state.isVisible) Box(Modifier.matchParentSize()) {
                TooltipBox(modifier = Modifier.fillMaxSize(),
                    positionProvider = position,
                    tooltip = {
                        val shape = RoundedCornerShape(9.dp)
                        Box(Modifier.testTag("hover-tooltip").widthIn(max = if (tipContent == null) 400.dp else Dp.Infinity).background(Color(0xcc000006), shape)
                            .border(1.dp, Color.White.copy(alpha = .1f), shape).padding(horizontal = 11.dp, vertical = 7.dp)) {
                            if (tipContent == null) Text(text, color = Color.White, style = MaterialTheme.typography.bodyMedium) else tipContent(text)
                        }
                    }, state = state,
                    focusable = false, enableUserInput = false,
                    content = { Box(Modifier.fillMaxSize()) })
            }
        }
    }
}

@Composable internal fun HoverTip(tip: JSONObject, modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    if (tip.array("lines").length() == 0) HoverTip(tip.getString("text"), modifier, content = content)
    else HoverTip(plainTip(tip), modifier, tipContent = { RichTip(tip) }, content = content)
}

@Composable private fun RichTip(tip: JSONObject) {
    val text = tip.getString("text")
    val lines = tip.array("lines").objects()
    val style = MaterialTheme.typography.bodyMedium
    val figures = tip.getBoolean("figures")
    val keyWidth = panelLabelWidth(lines.map { it.getString("label") })
    val valueStyle = style.copy(fontWeight = if (figures) androidx.compose.ui.text.font.FontWeight.Medium else style.fontWeight,
        fontFeatureSettings = if (figures) "tnum" else null)
    val measurer = androidx.compose.ui.text.rememberTextMeasurer()
    val valueWidth = with(LocalDensity.current) { measurer.measure("0".repeat(38), style).size.width.toDp() }
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        if (text.isNotEmpty()) Text(text, Modifier.widthIn(max = 400.dp), color = Color.White, style = style)
        Layout(content = {
            for (line in lines) {
                Text(line.getString("label"), color = Color.White.copy(alpha = .75f), style = style)
                Text(line.getString("value"), color = Color.White, style = valueStyle,
                    textAlign = if (figures) androidx.compose.ui.text.style.TextAlign.End else androidx.compose.ui.text.style.TextAlign.Start)
            }
        }) { children, constraints ->
            val gap = 16.dp.roundToPx(); val rowGap = 4.dp.roundToPx()
            val key = keyWidth.roundToPx().coerceAtMost((constraints.maxWidth - gap).coerceAtLeast(0) / 2)
            val available = (constraints.maxWidth - key - gap).coerceAtLeast(0)
            val value = if (figures) children.filterIndexed { i, _ -> i % 2 == 1 }.maxOf { it.maxIntrinsicWidth(Constraints.Infinity) }.coerceAtMost(available)
                else valueWidth.roundToPx().coerceAtMost(available)
            val rows = children.chunked(2).map { pair -> pair[0].measure(Constraints(maxWidth = key)) to pair[1].measure(Constraints(maxWidth = value)) }
            val height = rows.sumOf { maxOf(it.first.height, it.second.height) } + rowGap * (rows.size - 1)
            layout(key + gap + value, height) {
                var y = 0
                for ((label, reading) in rows) {
                    label.placeRelative(0, y); reading.placeRelative(key + gap + if (figures) value - reading.width else 0, y)
                    y += maxOf(label.height, reading.height) + rowGap
                }
            }
        }
    }
}

@Composable internal fun ActionTip(host: CanvasHost, label: String, action: JSONObject,
    modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    HoverTip(label, modifier, resolve = { reply ->
        host.query(obj("type" to "action_tooltip", "label" to label, "action" to action)) { reply(it as? String ?: label) }
    }, content = content)
}

@Composable internal fun IconTile(name: String, label: String, selected: Boolean = false,
    enabled: Boolean = true, modifier: Modifier = Modifier, onLongClick: (() -> Unit)? = null, fill: Color? = null, iconSize: Dp = 16.dp, selectedColor: Color? = null, tileSize: Dp = 36.dp, onClick: () -> Unit) {
    val colors = LocalPalette.current
    HoverTip(label, modifier) {
    Box(Modifier.size(tileSize).alpha(if (enabled) 1f else 0.4f).background(if (selected) selectedColor ?: colors.active else Color.Transparent, TileShape)
        .combinedClickable(enabled = enabled, role = Role.Button, onClickLabel = label, onLongClick = onLongClick, onClick = onClick), contentAlignment = Alignment.Center) {
        SharedIcon(name, label, modifier = Modifier.size(iconSize), fill = fill)
    }
    }
}

/** A centered, reusable choice grid. Content and selection come from the core. */
@Composable internal fun ImageSelector(options: List<String>, icons: List<String>, columns: Int,
    selected: Int, enabled: Boolean, modifier: Modifier = Modifier, onSelect: (Int) -> Unit) {
    val colors = LocalPalette.current
    Column(modifier.fillMaxWidth().selectableGroup().padding(vertical = 6.dp),
        horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp)) {
        options.indices.toList().chunked(columns).forEach { indices ->
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                indices.forEach { index ->
                    val active = index == selected
                    Box(Modifier.size(64.dp).testTag("image-choice-$index").clip(TileShape)
                        .background(if (active) colors.active else colors.text.copy(alpha = .05f))
                        .then(if (active) Modifier.border(2.dp, colors.accent, TileShape) else Modifier)
                        .alpha(if (enabled) 1f else .4f)
                        .selectable(active, enabled = enabled, role = Role.RadioButton) { onSelect(index) },
                        contentAlignment = Alignment.Center) {
                        SharedIcon(icons[index], options[index], Modifier.size(48.dp))
                    }
                }
            }
        }
    }
}

/** Compact GTK panel choice: input surface, ellipsized value and trailing arrow. */
@Composable internal fun PanelChoiceButton(label:String,modifier:Modifier=Modifier,enabled:Boolean=true,fill:Color=LocalPalette.current.input,onClick:()->Unit) {
    val colors=LocalPalette.current
    Row(modifier.fillMaxWidth().heightIn(min=panelMetrics().height).clip(ControlShape).background(fill)
        .clickable(enabled=enabled,role=Role.Button,onClick=onClick).padding(horizontal=6.dp),
        verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(6.dp)) {
        Text(label,Modifier.weight(1f),color=colors.text.copy(alpha=if(enabled)1f else .5f),maxLines=1,overflow=androidx.compose.ui.text.style.TextOverflow.Ellipsis)
        SharedIcon("chevron-down",null,Modifier.size(12.dp),tint=colors.text.copy(alpha=if(enabled)1f else .5f))
    }
}
