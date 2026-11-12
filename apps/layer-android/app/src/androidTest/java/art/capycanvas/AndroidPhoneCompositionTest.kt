package art.capycanvas

import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.captureToImage
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.semantics.SemanticsProperties
import android.os.ParcelFileDescriptor
import android.view.Choreographer
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Rule
import org.junit.Test

class AndroidPhoneCompositionTest {
    @get:Rule(order=0) val device = CapyDeviceRule()
    @get:Rule(order=1) val compose = createAndroidComposeRule<MainActivity>()

    @Test fun phoneSliderShowsAndDispatchesOnlyTheLatestBlockedMove() {
        val host=compose.activity.host
        host.awaitReady(compose=compose)
        assumeTrue("Requires the native phone layout",host.snapshot!!.getJSONObject("layout").objectOrNull("phone") != null)
        host.awaitMain("launch reveal removed",30_000,{ "" },compose) { findTag("launch-splash")==null }
        host.drain(obj("type" to "set_brush_size","value" to 40))
        compose.waitForIdle()
        val slider=compose.onNodeWithTag("component-slider-size")
        val reached=CountDownLatch(1)
        val release=CountDownLatch(1)
        var frames=0
        val clock=object:Choreographer.FrameCallback {
            override fun doFrame(time:Long) { frames++;Choreographer.getInstance().postFrameCallback(this) }
        }
        fun trace(command:String)=ParcelFileDescriptor.AutoCloseInputStream(
            instrumentation.uiAutomation.executeShellCommand(command)).bufferedReader().use { it.readText() }
        compose.runOnIdle {
            CoroutineScope(Dispatchers.Main.immediate).launch {
                host.withNative { reached.countDown();check(release.await(15,TimeUnit.SECONDS)) }
            }
        }
        assertTrue("The native owner is blocked",reached.await(5,TimeUnit.SECONDS))
        trace("atrace --async_start -b 16384 -a ${instrumentation.targetContext.packageName} view")
        var lifted=false
        var captured=""
        try {
            slider.performTouchInput { down(center) }
            repeat(24) { index -> slider.performTouchInput {
                val fraction=.55f+.45f*(index+1)/24
                moveTo(if(height>width)Offset(centerX,height*(1-fraction)) else Offset(width*fraction,centerY))
            } }
            val progress=slider.fetchSemanticsNode().config[SemanticsProperties.ProgressBarRangeInfo].current
            assertEquals("Semantics follows the latest pointer while the owner is blocked",1f,progress,.001f)
            val pixels=slider.captureToImage().toPixelMap()
            val state=host.snapshot!!.getJSONObject("state")
            val thumb=Palette(state.getString("theme")=="dark",state.getJSONObject("palette")).thumb
            val inset=(8*compose.density.density).toInt()
            val pixel=if(pixels.height>pixels.width)pixels[pixels.width/2,inset] else pixels[pixels.width-inset-1,pixels.height/2]
            assertEquals("The drawn thumb follows the latest pointer while the owner is blocked",thumb,pixel)
            compose.runOnUiThread { Choreographer.getInstance().postFrameCallback(clock) }
            release.countDown()
            host.drain()
            compose.runOnUiThread { android.os.Trace.beginSection("capy.test.slider.blocked.end");android.os.Trace.endSection() }
            val report=host.measurementReport()
            val before=report.getLong("snapshot_attempts")
            val beforeFrames=frames
            repeat(8) { index -> slider.performTouchInput {
                moveTo(if(height>width)Offset(centerX,height*(if(index%2==0).1f else 0f)) else Offset(width*(if(index%2==0).9f else 1f),centerY))
            } }
            host.drain()
            val after=host.measurementReport().getLong("snapshot_attempts")
            assertTrue("Moving publications are bounded by display frames: ${after-before} / ${frames-beforeFrames}",after-before<=frames-beforeFrames)
            slider.performTouchInput { up() };lifted=true
            host.drain()
            val model=host.snapshot!!.getJSONArray("phone_sliders").getJSONObject(0).getJSONObject("numeric")
            assertEquals("Release publishes the latest Rust-resolved value",model.getJSONObject("numeric").getDouble("max"),model.getDouble("value"),0.0)
        } finally {
            release.countDown()
            if(!lifted)slider.performTouchInput { cancel() }
            host.drain()
            compose.runOnUiThread { Choreographer.getInstance().removeFrameCallback(clock) }
            captured=trace("atrace --async_stop")
            compose.activity.getExternalFilesDir(null)!!.resolve("slider-coalescing.atrace").writeText(captured)
        }
        assertTrue("The trace includes the blocked-move boundary",captured.contains("capy.test.slider.blocked.end"))
        val edits=Regex("B\\|[0-9]+\\|capy.action.toolbar_edit\\.").findAll(captured.substringBefore("capy.test.slider.blocked.end")).count()
        assertTrue("Blocked moves dispatch one edit plus the initial bookmark snap: $edits",edits in 1..2)
        val phases=Regex("B\\|[0-9]+\\|capy.action.phone\\.").findAll(captured).count()
        assertEquals("Only Down and Up reach native; no slider Move",2,phases)
    }

    @Test fun sizeChangesRetainUnchangedPhoneTileCompositions() {
        val host=compose.activity.host
        host.awaitReady(compose=compose)
        assumeTrue("Requires the native phone layout",host.snapshot!!.getJSONObject("layout").objectOrNull("phone") != null)
        host.awaitMain("launch reveal removed",30_000,{ "${host.snapshot?.opt("launch")}" },compose) { findTag("launch-splash") == null }
        compose.onNodeWithTag("phone-frame").assertExists()
        compose.runOnIdle { CompositionCounts { info ->
            when {
                info.startsWith("art.capycanvas.PhoneTile") -> "PhoneTile"
                info.startsWith("art.capycanvas.HeaderButton") -> "HeaderButton"
                else -> null
            }
        } }.use { counts ->
            for (command in listOf("eraser","drawing_brush")) {
                host.drain(obj("type" to "invoke","command" to command))
                compose.waitForIdle()
            }
            host.awaitReady(compose=compose)
            compose.waitForIdle()
            val tools=compose.runOnIdle { jsonValue(host.snapshot!!.getJSONArray("phone_tools")) }
            compose.runOnIdle {
                assertTrue("Changing the selected tool composes its real phone tiles",counts["PhoneTile"]>0)
                assertTrue("Changing the selected tool composes its real buttons",counts["HeaderButton"]>0)
                counts.clear()
            }
            for (size in listOf(40,48,64)) {
                host.drain(obj("type" to "set_brush_size","value" to size))
                compose.waitForIdle()
                compose.runOnIdle {
                    assertEquals("Size does not change the tool tile models",tools,jsonValue(host.snapshot!!.getJSONArray("phone_tools")))
                    assertEquals(size.toDouble(),host.snapshot!!.getJSONArray("phone_sliders").getJSONObject(0)
                        .getJSONObject("numeric").getDouble("value"),0.0)
                }
            }
            compose.runOnIdle {
                assertEquals("Size publications retain unchanged phone tiles",0,counts["PhoneTile"])
                assertEquals("Size publications retain unchanged phone buttons",0,counts["HeaderButton"])
            }
        }
    }
}
