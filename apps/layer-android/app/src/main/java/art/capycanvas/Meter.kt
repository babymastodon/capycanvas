package art.capycanvas

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.RoundRect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlin.math.roundToInt
import org.json.JSONObject

internal fun plainTip(tip: JSONObject) = (listOf(tip.getString("text")) + tip.array("lines").objects().map {
    "${it.getString("label")}: ${it.getString("value")}"
}).filter { it.isNotEmpty() }.joinToString("\n")

@Composable internal fun MeterChip(chip: JSONObject?) {
    if (chip == null) return
    val colors = LocalPalette.current
    val warning = chip.getBoolean("warning")
    val ink = if (warning) colors.error else colors.text
    Text(chip.getString("label"), Modifier.background(ink.copy(alpha = if (warning) .16f else .1f), CircleShape).padding(horizontal = 7.dp),
        color = ink, style = LocalTextStyle.current.copy(fontSize = LocalTextStyle.current.fontSize * .85f, lineHeight = LocalTextStyle.current.lineHeight * .85f, fontWeight = FontWeight.SemiBold),
        maxLines = 1, softWrap = false)
}

@Composable internal fun MeterBar(meter: JSONObject, modifier: Modifier = Modifier) {
    val colors = LocalPalette.current
    val limit = meter.number("limit")
    val segments = meter.array("segments").objects()
    Canvas(modifier.fillMaxWidth().height(12.dp)) {
        val height = 6.dp.toPx()
        val top = ((size.height - height) / 2).roundToInt().toFloat()
        val clip = Path().apply { addRoundRect(RoundRect(Rect(0f, top, size.width, top + height), CornerRadius(3.dp.toPx()))) }
        fun part(from: Float, to: Float, color: androidx.compose.ui.graphics.Color) {
            drawRect(color, Offset(from * size.width, top), Size((to - from) * size.width, height))
        }
        clipPath(clip) {
            part(0f, limit, colors.text.copy(alpha = .12f))
            part(limit, 1f, colors.text.copy(alpha = .06f))
            for (segment in segments) part(segment.number("from"), segment.number("to"), when (segment.getString("tone")) {
                "fill" -> colors.accent
                "soft" -> colors.accent.copy(alpha = .4f)
                "rest" -> colors.text.copy(alpha = .32f)
                else -> colors.error
            })
        }
        if (limit < .999f) drawRect(colors.text.copy(alpha = .6f), Offset((limit * size.width).roundToInt() - 1.dp.toPx(), 0f), Size(1.dp.toPx(), size.height))
    }
}

@Composable internal fun MeterRow(row: JSONObject, modifier: Modifier = Modifier, valueId: String? = null, barId: String? = null) {
    val tip = row.getJSONObject("tip")
    val description = if (row.has("description")) row.getString("description") else plainTip(tip)
    val colors = LocalPalette.current
    HoverTip(tip, modifier.fillMaxWidth().semantics { contentDescription = description }) {
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(row.getString("label"), Modifier.weight(1f, fill = false), maxLines = 1, overflow = TextOverflow.Ellipsis)
                    MeterChip(row.objectOrNull("chip"))
                }
                Text(row.getString("value"), if (valueId == null) Modifier else Modifier.testTag(valueId),
                    color = if (row.optBoolean("warning")) colors.error else colors.text,
                    style = LocalTextStyle.current.copy(fontFeatureSettings = "tnum"), maxLines = 1, softWrap = false, textAlign = TextAlign.End)
            }
            row.objectOrNull("meter")?.let { MeterBar(it, if (barId == null) Modifier else Modifier.testTag(barId)) }
        }
    }
}

@Composable internal fun MeterSection(section: JSONObject) {
    val tip = section.getJSONObject("tip")
    val colors = LocalPalette.current
    Column(Modifier.fillMaxWidth().testTag("renderer-memory"), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        HoverTip(tip, Modifier.fillMaxWidth().testTag("renderer-memory-heading").semantics { contentDescription = plainTip(tip) }) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(section.getString("title"), fontWeight = FontWeight.SemiBold, maxLines = 1)
                Text(section.getString("note"), Modifier.weight(1f), color = colors.secondary, maxLines = 1, overflow = TextOverflow.Ellipsis)
                MeterChip(section.objectOrNull("chip"))
            }
        }
        for ((index, row) in section.array("rows").objects().withIndex()) MeterRow(row,
            Modifier.testTag("renderer-memory-row-$index"), "renderer-memory-value-$index", "renderer-memory-bar-$index")
    }
}
