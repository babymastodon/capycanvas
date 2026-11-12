package art.capycanvas

import android.view.ViewTreeObserver
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.layout.LayoutInfo
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.unit.IntSize
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import kotlinx.coroutines.launch
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.atomic.AtomicInteger

class AndroidDrawerCompositionTest {
    @get:Rule(order=0) val device = CapyDeviceRule()
    @get:Rule(order=1) val compose = createAndroidComposeRule<MainActivity>()

    @Before fun applyRequestedTheme() {
        val theme=androidx.test.platform.app.InstrumentationRegistry.getArguments().getString("theme") ?: return
        require(theme in listOf("light","dark"))
        val host=compose.activity.host
        host.awaitReady(compose=compose)
        host.drain(obj("type" to "set_theme","theme" to theme))
        compose.waitForIdle()
    }

    @Test fun phoneDrawerFacesShareTheirConnectionPaintAndJoinedCorners() {
        val host=compose.activity.host
        host.awaitReady(compose=compose)
        assumeTrue("Requires the native phone layout",host.snapshot!!.getJSONObject("layout").objectOrNull("phone")!=null)
        host.awaitMain("launch reveal removed",30_000,{ "" },compose) { findTag("launch-splash")==null }
        val activity=compose.activity
        val original=activity.requestedOrientation
        val density=activity.resources.displayMetrics.density
        val failures=mutableListOf<String>()
        try {
            for(orientation in listOf(android.content.pm.ActivityInfo.SCREEN_ORIENTATION_PORTRAIT,android.content.pm.ActivityInfo.SCREEN_ORIENTATION_LANDSCAPE)) {
                compose.runOnUiThread { activity.requestedOrientation=orientation }
                host.awaitMain("phone orientation",30_000,{ "" },compose) {
                    host.workspaceInteraction?.phone!=null && (activity.resources.configuration.orientation==android.content.res.Configuration.ORIENTATION_PORTRAIT)==(orientation==android.content.pm.ActivityInfo.SCREEN_ORIENTATION_PORTRAIT) && host.snapshot!!.getJSONObject("layout").array("viewport").let {
                        (it.getDouble(0)<it.getDouble(1))==(orientation==android.content.pm.ActivityInfo.SCREEN_ORIENTATION_PORTRAIT)
                    }
                }
                val dock=checkNotNull(host.workspaceInteraction)
                fun frame(menu:Boolean)=if(menu)dock.phone?.menu?.frame else dock.drawer
                val tiles=host.snapshot!!.getJSONObject("state").getJSONObject("workspace").getJSONObject("layout").array("phone").objects()
                val selected=host.snapshot!!.array("phone_tools").objects().indexOfFirst { it.optBoolean("selected") }
                assertTrue("Painter has a selected tool",selected>=0)
                for((family,action) in listOf("capy" to obj("kind" to "capy"),"settings" to obj("kind" to "tool_settings"),
                    "selected" to obj("kind" to "tool","tile" to tiles[selected].getInt("id")))) {
                    val menu=family=="capy"
                    host.drain(obj("type" to "phone","action" to action))
                    compose.mainClock.advanceTimeBy(400)
                    host.awaitMain("$family glass presented",30_000,{ "frame=${frame(menu)}" },compose) {
                        frame(menu)?.objectOrNull("connection")!=null && host.glassConnectionsForTest.isNotEmpty()
                    }
                    compose.waitForIdle()
                    val settled=compose.runOnIdle { checkNotNull(frame(menu)).copy() }
                    fun pixels(opaque:Boolean) {
                        val placement=settled.getJSONObject("placement")
                        val anchor=placement.getJSONObject("anchor").rect()
                        val body=placement.getJSONObject("bounds").rect()
                        val bridge=settled.getJSONObject("connection").getJSONObject("bounds").rect()
                        val direction=placement.getString("direction")
                        val vertical=direction in listOf("top","bottom")
                        val sign=if(direction in listOf("top","left"))-1 else 1
                        val tangent=if(vertical)anchor.left+anchor.width*.25f else anchor.top+anchor.height*.25f
                        val edge=when(direction){"top"->anchor.top;"bottom"->anchor.bottom;"left"->anchor.left;else->anchor.right}
                        val bodyEdge=when(direction){"top"->body.bottom;"bottom"->body.top;"left"->body.right;else->body.left}
                        fun point(normal:Float)=if(vertical)androidx.compose.ui.geometry.Offset(tangent*density,normal) else androidx.compose.ui.geometry.Offset(normal,tangent*density)
                        val face=point(edge*density-sign*8)
                        val joint=point((if(vertical)bridge.center.y else bridge.center.x)*density)
                        val inside=point(bodyEdge*density+sign*8)
                        val corners=when(direction){"top"->listOf(anchor.topLeft,anchor.topRight);"bottom"->listOf(anchor.bottomLeft,anchor.bottomRight)
                            "left"->listOf(anchor.topLeft,anchor.bottomLeft);else->listOf(anchor.topRight,anchor.bottomRight)}
                        if(!opaque)compose.runOnIdle {
                            val native=Rect(anchor.topLeft*density+dock.origin-host.surfaceOrigin,anchor.bottomRight*density+dock.origin-host.surfaceOrigin)
                            val chip=host.glassBoxesForTest.single { it[0]<=native.left+1 && it[1]<=native.top+1 && it[0]+it[2]>=native.right-1 && it[1]+it[3]>=native.bottom-1 }
                            val bounds=Rect(chip[0],chip[1],chip[0]+chip[2],chip[1]+chip[3])
                            listOf(bounds.topLeft,bounds.topRight,bounds.bottomRight,bounds.bottomLeft).forEachIndexed { index,corner ->
                                if(corners.any { (it*density+dock.origin-host.surfaceOrigin-corner).getDistance()<1f })assertEquals("Joined chip glass corner",0f,chip[4+index],0f)
                            }
                        }
                        val expected=android.graphics.Color.parseColor(host.snapshot!!.getJSONObject("state").getJSONObject("palette").getString("panel"))
                        val suffix=if(opaque)"opaque" else "rest"
                        val surfaces=compose.runOnIdle { host.glassBoxesForTest.map {
                            Rect(it[0],it[1],it[0]+it[2],it[1]+it[3]).translate(host.surfaceOrigin-dock.origin)
                        } }+listOf(Rect(anchor.topLeft*density,anchor.bottomRight*density),Rect(body.topLeft*density,body.bottomRight*density))
                        var inspected=false
                        screenshot("validation/drawer-faces/$orientation-$family-$suffix.png") { image ->
                            inspected=true
                            fun pixel(at:androidx.compose.ui.geometry.Offset)=image.getPixel(kotlin.math.round(at.x+dock.origin.x).toInt(),kotlin.math.round(at.y+dock.origin.y).toInt())
                            fun equal(label:String,want:Int,actual:Int,tolerance:Int) {
                                val delta=listOf(16,8,0).map { kotlin.math.abs((want shr it and 255)-(actual shr it and 255)) }
                                if(delta.any { it>tolerance })failures+="$family $orientation $suffix $label: expected=${Integer.toHexString(want)} actual=${Integer.toHexString(actual)} delta=$delta"
                            }
                            val reference=if(opaque)expected else pixel(inside)
                            for((label,at) in listOf("face" to face,"bridge" to joint,"body" to inside))equal(label,reference,pixel(at),if(opaque)0 else 3)
                            val connection=settled.getJSONObject("connection")
                            val transform=connection.array("transform")
                            fun project(u:Float,v:Float)=bridge.topLeft*density+androidx.compose.ui.geometry.Offset(
                                transform.getDouble(0).toFloat()*u+transform.getDouble(2).toFloat()*v+transform.getDouble(4).toFloat()*density,
                                transform.getDouble(1).toFloat()*u+transform.getDouble(3).toFloat()*v+transform.getDouble(5).toFloat()*density)
                            val samples=org.json.JSONArray()
                            for(index in 0..1) {
                                val radius=connection.array("radii").getDouble(index).toFloat()*density
                                if(radius<4f)continue
                                val length=connection.number("length")*density
                                val depth=connection.number("depth")*density
                                val u=if(index==0)-radius+2f else length+radius-2f
                                val v=depth-radius+2f
                                val inner=project(u,v)
                                val outside=(0..image.width).asSequence().map { distance ->
                                    project(if(index==0)-radius-4f-distance else length+radius+4f+distance,v)
                                }.firstOrNull { candidate ->
                                    val screen=candidate+dock.origin
                                    screen.x in 1f..(image.width-2f) && screen.y in 1f..(image.height-2f) &&
                                        surfaces.all { it.contains(candidate)==it.contains(inner) }
                                }
                                assertNotNull("$family $orientation $suffix corner $index has a reference on the same surfaces",outside)
                                val outer=checkNotNull(outside)
                                samples.put(obj("corner" to index,"inside" to org.json.JSONArray(listOf(inner.x,inner.y)),
                                    "outside" to org.json.JSONArray(listOf(outer.x,outer.y)),"inside_rgb" to pixel(inner),"outside_rgb" to pixel(outer)))
                                java.io.File(instrumentation.targetContext.getExternalFilesDir(null),"validation/drawer-faces/$orientation-$family-$suffix.json")
                                    .writeText(samples.toString(2))
                                equal("concave corner $index at $inner versus $outer",pixel(outer),pixel(inner),2)
                            }
                            if(opaque) {
                                val start=project(0f,connection.number("depth")*density)+dock.origin
                                val end=project(connection.number("length")*density,connection.number("depth")*density)+dock.origin
                                val near=if(vertical)minOf(start.x,end.x) else minOf(start.y,end.y)
                                val far=if(vertical)maxOf(start.x,end.x) else maxOf(start.y,end.y)
                                val normal=kotlin.math.round(bodyEdge*density+if(vertical)dock.origin.y else dock.origin.x).toInt()
                                for(along in kotlin.math.ceil(near).toInt() until kotlin.math.floor(far).toInt()) {
                                    for(across in normal-1..normal) {
                                        val x=if(vertical)along else across
                                        val y=if(vertical)across else along
                                        equal("body joint band ($x,$y)",reference,image.getPixel(x,y),0)
                                    }
                                }
                                for(inset in 1..8)equal("face joint strip $inset",reference,pixel(point(edge*density-sign*inset)),0)
                                for(corner in corners) {
                                    val at=corner*density+androidx.compose.ui.geometry.Offset(if(corner.x==anchor.left)2f else -2f,if(corner.y==anchor.top)2f else -2f)
                                    equal("joined corner $corner",reference,pixel(at),0)
                                }
                            }
                        }
                        assertTrue("$family $suffix captures pixels",inspected)
                    }
                    pixels(false)
                    compose.mainClock.autoAdvance=false
                    host.drain(if(menu)obj("type" to "phone","action" to obj("kind" to "capy")) else obj("type" to "customize","action" to obj("type" to "close_expanded")))
                    repeat(3) { compose.mainClock.advanceTimeByFrame();instrumentation.waitForIdleSync() }
                    host.awaitMain("$family glass retired",10_000,{ "" }) { host.glassConnectionsForTest.isEmpty() }
                    assertNotNull("Opaque connection remains until the animation advances",frame(menu)?.objectOrNull("connection"))
                    pixels(true)
                    compose.mainClock.autoAdvance=true
                    compose.mainClock.advanceTimeBy(400)
                    host.awaitMain("$family drawer closed",30_000,{ "" },compose) { frame(menu)==null }
                }
            }
            assertTrue(failures.joinToString("\n"),failures.isEmpty())
        } finally {
            compose.mainClock.autoAdvance=true
            compose.runOnUiThread { activity.requestedOrientation=original }
        }
    }

    @Test fun closedPhoneDrawerFamiliesRetainFrozenBodiesWithoutInputOrGlass() {
        val host=compose.activity.host
        host.awaitReady(compose=compose)
        assumeTrue("Requires the native phone layout",host.snapshot!!.getJSONObject("layout").objectOrNull("phone") != null)
        host.awaitMain("launch reveal removed",30_000,{ "" },compose) { findTag("launch-splash")==null }
        val dock=checkNotNull(host.workspaceInteraction)
        val color=host.snapshot!!.getJSONObject("state").getJSONObject("workspace").getJSONObject("layout")
            .array("phone").objects().first { it.getJSONObject("control").getString("kind")=="color" }.getInt("id")
        val families=listOf(
            Triple("tool_settings",obj("kind" to "tool_settings"),"ToolSettingsControls"),
            Triple("color",obj("kind" to "tool","tile" to color),"ColorPanelControls"),
            Triple("navigator",obj("kind" to "panel","panel" to "navigator"),"NavigatorPanel"))
        val bodies=mutableMapOf<String,LayoutInfo>()
        val initialGlass=compose.runOnIdle { host.glassBoxesForTest.map { it.take(4) }.toSet() }
        val initialConnections=compose.runOnIdle { host.glassConnectionsForTest.map { it.toString() }.toSet() }
        val initialChrome=compose.runOnIdle { dock.chromeRegions.keys.toSet() }
        val initialSources=compose.runOnIdle { dock.regions.keys.toSet() }
        fun open(action:org.json.JSONObject,panel:String) {
            host.drain(obj("type" to "phone","action" to action))
            host.awaitMain("$panel drawer settled",30_000,{ "drawer=${dock.drawer}" },compose) {
                findTag("drawer-panel-$panel")?.second?.boundsInRoot?.isEmpty==false &&
                    dock.drawer?.objectOrNull("placement")?.objectOrNull("bounds")?.number("height")?.let { it>0f }==true
            }
            compose.mainClock.advanceTimeBy(400)
            compose.waitForIdle()
        }
        fun close() {
            host.drain(obj("type" to "customize","action" to obj("type" to "close_expanded")))
            compose.mainClock.advanceTimeBy(400)
            compose.waitForIdle()
            host.awaitMain("drawer close completed",30_000,{ "drawer=${dock.drawer}" },compose) {
                findTag("tool-drawer")==null
            }
            host.drain()
            compose.runOnIdle {
                for(panel in bodies.keys) {
                    assertNull("$panel exposes no closed semantics",findTag("drawer-panel-$panel"))
                }
                assertTrue("Closed drawer chrome cannot receive native input",dock.chromeRegions.filterKeys { it !in initialChrome }.values.all { it.bounds.isEmpty })
                assertTrue("Closed drawer bodies cannot receive native input",dock.regions.filterKeys { it !in initialSources }.values.all { it.bounds.isEmpty })
                assertTrue("Closed drawer has no native glass",host.glassBoxesForTest.none { it.take(4) !in initialGlass })
                assertTrue("Closed drawer has no bridge glass",host.glassConnectionsForTest.none { it.toString() !in initialConnections })
            }
            val display=kotlinx.coroutines.runBlocking { host.withNative { org.json.JSONObject(Native.displayStatus(it)) } }
            assertEquals("Closed Navigator has no native overview",0,display.getInt("overview_count"))
        }
        val tracedBodies=families.map { it.third }+listOf("ContentDrawer","PalettePanel")
        compose.runOnIdle { CompositionCounts { info ->
            info.substringBefore(" (").substringAfterLast('.').takeIf { it in tracedBodies }
        } }.use { counts ->
            for((panel,action,bodyName) in families) {
                open(action,panel)
                compose.runOnIdle {
                    assertTrue("The tracer sees the real $panel body",counts[bodyName]>0)
                    bodies[panel]=checkNotNull(findTag("drawer-panel-$panel")).second.layoutInfo
                }
                close()
            }
            compose.runOnIdle {
                for(bodyName in tracedBodies)assertTrue("The tracer observes $bodyName while priming",counts[bodyName]>0)
                counts.clear()
            }
            val revision=host.snapshot!!.getJSONObject("state").getJSONObject("document_file").getLong("revision")
            host.drain(obj("type" to "set_color","rgba" to org.json.JSONArray(listOf(.12,.68,.31,1))))
            kotlinx.coroutines.runBlocking { host.withNative { handle ->
                val viewport=host.snapshot!!.getJSONObject("state").getJSONObject("camera").array("viewport")
                val now=System.nanoTime()
                val samples=(0..7).flatMap { step -> listOf(viewport.getDouble(0)*(.45+step*.01),viewport.getDouble(1)*.5,
                    if(step==7)0.0 else .7,0.0,0.0,0.0,0.0,(now+step*10_000_000L).toDouble(),
                    when(step){0->1.0;7->3.0;else->2.0}) }.toDoubleArray()
                Native.pointer(handle,72,0,0,samples,samples.size,false,0)
            } }
            compose.runOnIdle { host.canvasChanged() }
            host.awaitMain("stroke publishes a document revision",30_000,{ "" },compose) {
                host.snapshot!!.getJSONObject("state").getJSONObject("document_file").getLong("revision")>revision
            }
            host.drain()
            compose.waitForIdle()
            compose.runOnIdle {
                for(bodyName in tracedBodies)assertEquals("Closed retained $bodyName ignores stroke and color publications",0,counts[bodyName])
            }
            repeat(2) {
                for((panel,action,_) in families) {
                    open(action,panel)
                    compose.runOnIdle { assertSame("$panel reuses its retained body across family switches",bodies.getValue(panel),checkNotNull(findTag("drawer-panel-$panel")).second.layoutInfo) }
                    close()
                }
            }
        }
    }

    @Test fun warmPhoneDrawerStartsMovingWithinSixNativeFrames() {
        val host=compose.activity.host
        host.awaitReady(compose=compose)
        assumeTrue("Requires the native phone layout",host.snapshot!!.getJSONObject("layout").objectOrNull("phone") != null)
        host.awaitMain("launch reveal removed",30_000,{ "" },compose) { findTag("launch-splash")==null }
        val dock=checkNotNull(host.workspaceInteraction)
        val color=host.snapshot!!.getJSONObject("state").getJSONObject("workspace").getJSONObject("layout")
            .array("phone").objects().first { it.getJSONObject("control").getString("kind")=="color" }.getInt("id")
        val frames=mutableMapOf<String,Int>()
        val root=compose.activity.window.decorView
        for((family,action) in listOf("tool" to obj("kind" to "tool_settings"),
            "color" to obj("kind" to "tool","tile" to color),"navigator" to obj("kind" to "panel","panel" to "navigator"))) {
            host.drain(obj("type" to "phone","action" to action))
            compose.mainClock.advanceTimeBy(400)
            compose.waitForIdle()
            host.awaitMain("$family primer visible",30_000,{ "" },compose) { findTag("tool-drawer")?.second?.boundsInRoot?.isEmpty==false }
            host.drain(obj("type" to "customize","action" to obj("type" to "close_expanded")))
            compose.mainClock.advanceTimeBy(400)
            compose.waitForIdle()
            host.awaitMain("$family primer closed",30_000,{ "" },compose) { findTag("tool-drawer")==null }
            var count=0
            var firstMoving:Int?=null
            val observer=ViewTreeObserver.OnPreDrawListener {
                count++
                val bounds=dock.drawer?.objectOrNull("placement")?.objectOrNull("bounds")
                val pane=findTag("tool-drawer")?.second
                if(firstMoving==null && bounds!=null && bounds.number("width")>0f && bounds.number("height")>0f &&
                    pane?.layoutInfo?.isPlaced==true && !pane.boundsInRoot.isEmpty)firstMoving=count
                true
            }
            compose.runOnUiThread {
                root.viewTreeObserver.addOnPreDrawListener(observer)
                host.dispatch(obj("type" to "phone","action" to action))
            }
            try {
                host.awaitMain("$family warm first moving native frame",30_000,{ "frames=$count; drawer=${dock.drawer}" },compose) { firstMoving!=null }
                compose.runOnIdle { frames[family]=checkNotNull(firstMoving) }
            } finally {
                compose.runOnUiThread { root.viewTreeObserver.removeOnPreDrawListener(observer) }
            }
            compose.mainClock.advanceTimeBy(400)
            compose.waitForIdle()
            host.drain(obj("type" to "customize","action" to obj("type" to "close_expanded")))
            compose.mainClock.advanceTimeBy(400)
            compose.waitForIdle()
            host.awaitMain("$family warm drawer fully closed",30_000,{ "" },compose) { findTag("tool-drawer")==null }
        }
        compose.activity.getExternalFilesDir(null)!!.resolve("drawer-warm-native-frames.json")
            .writeText(org.json.JSONObject(frames as Map<*,*>).toString())
        assertTrue("Warm action to first moving native frame: $frames",frames.values.all { it<=6 })
    }

    @Test fun closedPhoneDrawerReleasesItsBodyAfterMemoryPressureLayoutAndWorkspaceChanges() {
        val host=compose.activity.host
        host.awaitReady(compose=compose)
        assumeTrue("Requires the native phone layout",host.snapshot!!.getJSONObject("layout").objectOrNull("phone") != null)
        host.awaitMain("launch reveal removed",30_000,{ "" },compose) { findTag("launch-splash")==null }
        fun open():LayoutInfo {
            host.drain(obj("type" to "phone","action" to obj("kind" to "tool_settings")))
            host.awaitMain("Tool Settings visible",30_000,{ "" },compose) {
                findTag("drawer-panel-tool_settings")?.second?.boundsInRoot?.isEmpty==false
            }
            compose.mainClock.advanceTimeBy(400)
            return compose.runOnIdle { checkNotNull(findTag("drawer-panel-tool_settings")).second.layoutInfo }
        }
        fun close() {
            host.drain(obj("type" to "customize","action" to obj("type" to "close_expanded")))
            host.awaitMain("Tool Settings closed",30_000,{ "" },compose) {
                findTag("tool-drawer")==null
            }
        }
        val original=open()
        close()
        compose.runOnIdle { compose.activity.application.onTrimMemory(android.content.ComponentCallbacks2.TRIM_MEMORY_RUNNING_LOW) }
        compose.waitForIdle()
        val afterPressure=open()
        assertNotSame("Memory pressure releases the closed body",original,afterPressure)
        close()
        val surface=compose.runOnIdle { checkNotNull(compose.activity.window.decorView.descendant<CanvasSurfaceView>()) }
        val originalSize=compose.runOnIdle { surface.holder.surfaceFrame.let { IntSize(it.width(),it.height()) } }
        val density=surface.resources.displayMetrics.density
        try {
            compose.runOnIdle { host.resize((1200*density).toInt(),(480*density).toInt(),density) }
            host.awaitMain("Expanded viewport uses desktop layout",30_000,{ "" },compose) {
                host.snapshot!!.getJSONObject("layout").objectOrNull("phone")==null
            }
        } finally {
            compose.runOnIdle { host.resize(originalSize.width,originalSize.height,density) }
            host.awaitMain("Actual viewport restores phone layout",30_000,{ "" },compose) {
                host.snapshot!!.getJSONObject("layout").objectOrNull("phone")!=null
            }
        }
        val afterLayoutChange=open()
        assertNotSame("Leaving the phone layout releases its closed body",afterPressure,afterLayoutChange)
        close()
        val workspace=host.workspaceManager!!.getString("id")
        compose.runOnIdle { host.workspaceInput(obj("type" to "switch","id" to "builtin:workspace:photographer")) }
        host.awaitMain("Photo workspace",30_000,{ "${host.workspaceManager}" },compose) {
            host.workspaceManager?.optString("id")=="builtin:workspace:photographer" && host.workspaceManager?.optBoolean("busy")==false
        }
        compose.runOnIdle { host.workspaceInput(obj("type" to "switch","id" to workspace)) }
        host.awaitMain("Starting workspace restored",30_000,{ "${host.workspaceManager}" },compose) {
            host.workspaceManager?.optString("id")==workspace && host.workspaceManager?.optBoolean("busy")==false
        }
        assertNotSame("Changing workspace releases the prior body's layout nodes",afterLayoutChange,open())
        close()
    }

    @Test fun emptyDrawerGlassHidesWithoutSuspending() {
        val host=compose.activity.host
        host.awaitReady(compose=compose)
        val dock=checkNotNull(host.workspaceInteraction)
        var job:kotlinx.coroutines.Job?=null
        try {
            compose.runOnIdle {
                val glass=DrawerGlass(host,dock,androidx.compose.ui.unit.Density(dock.density),compose.activity.window.decorView)
                var completed=false
                job=kotlinx.coroutines.CoroutineScope(kotlinx.coroutines.Dispatchers.Main.immediate)
                    .launch(start=kotlinx.coroutines.CoroutineStart.UNDISPATCHED) { glass.hide();completed=true }
                assertTrue("An empty glass hides without a native presentation handoff",completed)
            }
        } finally {
            instrumentation.runOnMainSync { job?.cancel() }
        }
    }

    @Test fun closingColorDrawerRetainsPaletteCompositionWhileBoundsMove() {
        val host=compose.activity.host
        host.awaitReady(compose=compose)
        assumeTrue("Requires the native phone layout",host.snapshot!!.getJSONObject("layout").objectOrNull("phone") != null)
        host.awaitMain("launch reveal removed",30_000,{ "${host.snapshot?.opt("launch")}" },compose) { findTag("launch-splash") == null }
        val root=compose.activity.window.decorView
        assertTrue("The private window owns the drawer",root.isAttachedToWindow && root.hasWindowFocus())
        val dock=checkNotNull(host.workspaceInteraction)
        val initialChrome=compose.runOnIdle { dock.chromeRegions.keys.toSet() }
        val initialSources=compose.runOnIdle { dock.regions.keys.toSet() }
        val initialGlass=compose.runOnIdle { host.glassBoxesForTest.map { it.take(4) }.toSet() }
        val initialConnections=compose.runOnIdle { host.glassConnectionsForTest.map { it.toString() }.toSet() }
        val counts=compose.runOnIdle { CompositionCounts { info ->
            if(info.substringBefore(" (")=="art.capycanvas.ContentDrawer")"DrawerContent"
            else "PalettePanel".takeIf { info.startsWith("art.capycanvas.PalettePanel") }
        } }
        data class Frame(val order:Int,val size:IntSize?,val placed:Boolean,val outer:Rect?,val published:Boolean,val contentCompositions:Int,
            val chrome:List<Rect>,val sources:List<Rect>,val glass:List<Rect>,val connections:List<String>)
        data class Draw(val order:Int,val size:IntSize)
        val sequence=AtomicInteger()
        val frames=mutableListOf<Frame>()
        val draws=ConcurrentLinkedQueue<Draw>()
        val callbacks=mutableListOf<Runnable>()
        fun content(): LayoutInfo? = findTag("tool-drawer")?.second?.layoutInfo?.let {
            (it.javaClass.getMethod("getChildrenInfo").invoke(it) as List<*>).singleOrNull() as? LayoutInfo
        }
        val observer=ViewTreeObserver.OnPreDrawListener {
            val outer=findTag("tool-drawer")?.second
            val child=content()
            val placed=outer?.layoutInfo?.isPlaced == true && child?.isPlaced == true && child.coordinates.isAttached
            val size=child?.coordinates?.takeIf { it.isAttached }?.size
            val bounds=dock.drawer?.objectOrNull("placement")?.objectOrNull("bounds")
            frames.add(Frame(sequence.incrementAndGet(),size,placed,outer?.boundsInRoot?.translate(-dock.origin),
                bounds != null && bounds.number("width")>0f && bounds.number("height")>0f,counts["DrawerContent"],
                dock.chromeRegions.filterKeys { it !in initialChrome }.values.map { it.bounds },
                dock.regions.filterKeys { it !in initialSources }.values.map { it.bounds },
                host.glassBoxesForTest.filter { it.take(4) !in initialGlass }.map { Rect(it[0],it[1],it[0]+it[2],it[1]+it[3]) },
                host.glassConnectionsForTest.map { it.toString() }.filter { it !in initialConnections }))
            if(placed && size != null && size.width>0 && size.height>0) {
                val callback=Runnable { draws.add(Draw(sequence.incrementAndGet(),size)) }
                callbacks.add(callback)
                root.viewTreeObserver.registerFrameCommitCallback(callback)
            }
            true
        }
        val color=host.snapshot!!.getJSONObject("state").getJSONObject("workspace").getJSONObject("layout")
            .array("phone").objects().first { it.getJSONObject("control").getString("kind")=="color" }.getInt("id")
        counts.use {
            compose.runOnIdle { root.viewTreeObserver.addOnPreDrawListener(observer) }
            try {
                compose.mainClock.autoAdvance=false
                host.drain(obj("type" to "phone","action" to obj("kind" to "tool","tile" to color)))
                repeat(30) {
                    host.drain()
                    compose.mainClock.advanceTimeByFrame()
                    compose.waitForIdle()
                }
                host.awaitMain("Color drawer composed and placed",30_000,{
                    "drawer=${dock.drawer}; frame=${frames.lastOrNull()}; draws=${draws.toList().takeLast(2)}; model=${host.snapshot?.getJSONObject("state")?.getJSONObject("customization")?.opt("drawer")}" },compose) {
                    val bounds=dock.drawer?.objectOrNull("placement")?.objectOrNull("bounds")
                    findTag("drawer-panel-palettes") != null && bounds != null && bounds.number("height")>0f
                }
                compose.onNodeWithTag("drawer-panel-palettes").assertExists()
                compose.waitForIdle()
                val target=compose.runOnIdle { checkNotNull(content()).coordinates.size }
                compose.runOnIdle {
                    assertTrue("The tracer observes the real palette",counts["PalettePanel"]>0)
                    assertTrue("The tracer observes the drawer content body",counts["DrawerContent"]>0)
                    val moving=frames.zipWithNext().filter { (before,after) ->
                        before.published && after.published && before.outer?.isEmpty==false && after.outer?.isEmpty==false && before.outer!=after.outer
                    }.map { it.second }
                    assertTrue("The opening has multiple actual moving frames",moving.size>=2)
                    val opening=moving.filter { it.outer!!.width<target.width-1 || it.outer.height<target.height-1 }
                    assertTrue("Opening frames have no native drawer glass: ${opening.map { it.glass }}",opening.isNotEmpty() && opening.all { it.glass.isEmpty() })
                    assertTrue("Opening frames have no native bridge glass: ${opening.map { it.connections }}",opening.all { it.connections.isEmpty() })
                    assertEquals("Moving frames retain the drawer content body",0,moving.last().contentCompositions-moving.first().contentCompositions)
                }
                host.awaitMain("Settled drawer glass matches the pane",30_000,{ "frame=${frames.lastOrNull()}" },compose) {
                    val pane=findTag("tool-drawer")?.second?.boundsInRoot?.translate(-host.surfaceOrigin) ?: return@awaitMain false
                    host.glassBoxesForTest.any { box ->
                        kotlin.math.abs(box[0]-pane.left)<=1f && kotlin.math.abs(box[1]-pane.top)<=1f &&
                            kotlin.math.abs(box[2]-pane.width)<=1f && kotlin.math.abs(box[3]-pane.height)<=1f
                    }
                }
                val closeStart=compose.runOnIdle { frames.size }
                host.drain(obj("type" to "customize","action" to obj("type" to "close_expanded")))
                compose.mainClock.advanceTimeByFrame()
                compose.waitForIdle()
                val before=compose.onNodeWithTag("tool-drawer").fetchSemanticsNode().boundsInRoot
                compose.runOnIdle { counts.clear() }
                repeat(4) {
                    compose.mainClock.advanceTimeByFrame()
                    compose.waitForIdle()
                }
                val after=compose.onNodeWithTag("tool-drawer").fetchSemanticsNode().boundsInRoot
                assertNotEquals("The retained drawer actually moves",before,after)
                compose.runOnIdle {
                    val visible=frames.filter { it.published && it.outer?.isEmpty == false }
                    assertTrue("Native draws observe the moving drawer",visible.map { it.outer }.distinct().size>1)
                    assertTrue("Visible drawer content stays at $target: ${visible.map { it.size }}",visible.all { it.placed && it.size==target })
                    val first=frames.first { it.published }
                    assertTrue("A full-target content frame commits before geometry ${first.order}: target=$target; draws=$draws",
                        draws.any { it.order<first.order && it.size==target })
                    assertTrue("Hidden drawer input bounds are empty",frames.filter { !it.published }.all { frame ->
                        (frame.chrome+frame.sources).all { it.isEmpty }
                    })
                    assertTrue("Content input bounds stay inside the animated viewport",visible.all { frame ->
                        val bounds=checkNotNull(frame.outer)
                        frame.sources.all { it.isEmpty || it.left>=bounds.left-1 && it.top>=bounds.top-1 && it.right<=bounds.right+1 && it.bottom<=bounds.bottom+1 }
                    })
                    assertEquals("Geometry-only frames retain palette content",0,counts["PalettePanel"])
                    val moving=frames.drop(closeStart).zipWithNext().filter { (before,after) -> before.outer!=after.outer }.map { it.second }
                    assertTrue("Closing frames have no native drawer glass: ${moving.map { it.glass }}",moving.isNotEmpty() && moving.all { it.glass.isEmpty() })
                    assertTrue("Closing frames have no native bridge glass: ${moving.map { it.connections }}",moving.all { it.connections.isEmpty() })
                }
            } finally {
                instrumentation.runOnMainSync {
                    root.viewTreeObserver.removeOnPreDrawListener(observer)
                    callbacks.forEach { root.viewTreeObserver.unregisterFrameCommitCallback(it) }
                }
                compose.mainClock.autoAdvance=true
                compose.waitForIdle()
            }
        }
    }

    @Test fun partiallyOpenedToolSettingsRetargetsToBothColorColumns() {
        val host=compose.activity.host
        host.awaitReady(compose=compose)
        assumeTrue("Requires the native phone layout",host.snapshot!!.getJSONObject("layout").objectOrNull("phone") != null)
        host.awaitMain("launch reveal removed",30_000,{ "${host.snapshot?.opt("launch")}" },compose) { findTag("launch-splash") == null }
        assertTrue("The private window owns the drawer",compose.activity.window.decorView.hasWindowFocus())
        val dock=checkNotNull(host.workspaceInteraction)
        val color=host.snapshot!!.getJSONObject("state").getJSONObject("workspace").getJSONObject("layout")
            .array("phone").objects().first { it.getJSONObject("control").getString("kind")=="color" }.getInt("id")
        val counts=compose.runOnIdle { CompositionCounts { info -> "PalettePanel".takeIf { info.startsWith("art.capycanvas.PalettePanel") } } }
        val initialGlass=compose.runOnIdle { host.glassBoxesForTest.map { it.take(4) }.toSet() }
        val initialConnections=compose.runOnIdle { host.glassConnectionsForTest.map { it.toString() }.toSet() }
        val frames=mutableListOf<Triple<Rect?,List<List<Float>>,List<String>>>()
        val root=compose.activity.window.decorView
        val observer=ViewTreeObserver.OnPreDrawListener {
            frames.add(Triple(findTag("tool-drawer")?.second?.boundsInRoot,host.glassBoxesForTest
                .filter { it.take(4) !in initialGlass }.map { it.toList() },
                host.glassConnectionsForTest.map { it.toString() }.filter { it !in initialConnections }))
            true
        }
        val originalAdvance=compose.mainClock.autoAdvance
        counts.use {
            try {
                compose.runOnIdle { root.viewTreeObserver.addOnPreDrawListener(observer) }
                compose.mainClock.autoAdvance=false
                host.drain(obj("type" to "phone","action" to obj("kind" to "tool_settings")))
                var partial:Rect?=null
                var targetHeight=0f
                for(frame in 0 until 30) {
                    host.drain();compose.mainClock.advanceTimeByFrame();compose.waitForIdle()
                    compose.runOnIdle {
                        val placement=dock.drawer?.objectOrNull("placement")
                        val node=findTag("tool-drawer")?.second
                        val actual=node?.boundsInRoot
                        val published=placement?.objectOrNull("bounds")
                        val target=placement?.array("columns")?.objects()?.maxOfOrNull { it.number("height") }?.times(dock.density) ?: 0f
                        if(node?.layoutInfo?.isPlaced==true && published!=null && published.number("width")>0f && published.number("height")>0f &&
                            actual!=null && actual.width>1f && actual.height>1f && actual.height<target-1f) {partial=actual;targetHeight=target}
                    }
                    if(partial!=null)break
                }
                assertNotNull("Tool Settings is actually partially open before retarget: ${dock.drawer}",partial)
                assertTrue("The partial viewport is smaller than its measured content",partial!!.height<targetHeight-1f)
                val retargetStart=compose.runOnIdle { frames.size }
                host.drain(obj("type" to "phone","action" to obj("kind" to "tool","tile" to color)))
                repeat(30) {host.drain();compose.mainClock.advanceTimeByFrame();compose.waitForIdle()}
                compose.runOnIdle {
                    val model=host.snapshot!!.getJSONObject("state").getJSONObject("customization").getJSONObject("drawer")
                    val panels=host.snapshot!!.array("panels").objects().map { it.getString("id") }
                    assertEquals("Color requests both columns",2,model.array("columns").length())
                    assertTrue("Both new panel bodies are available: $panels",panels.containsAll(listOf("color","palettes")))
                    val placement=dock.drawer?.objectOrNull("placement")
                    val palette=findTag("drawer-panel-palettes")
                    assertTrue("Retarget composes both new columns: partial=$partial; targetHeight=$targetHeight; drawer=$placement; model=$model; palette=${palette!=null}; compositions=${counts["PalettePanel"]}",
                        findTag("drawer-panel-color")!=null && palette!=null && counts["PalettePanel"]>0 && placement?.array("columns")?.length()==2)
                    val moving=frames.drop(retargetStart).zipWithNext().filter { (before,after) -> before.first!=after.first }.map { it.second }
                    assertTrue("Retarget changes the actual drawer bounds",moving.isNotEmpty())
                    val settled=findTag("tool-drawer")!!.second.boundsInRoot
                    val retargeting=frames.drop(retargetStart).filter { it.first?.isEmpty==false && it.first!=settled }
                    assertTrue("Retarget frames have no native drawer glass: $retargeting",retargeting.all { it.second.isEmpty() })
                    assertTrue("Retarget frames have no native bridge glass: $retargeting",retargeting.all { it.third.isEmpty() })
                }
                host.awaitMain("Retargeted glass matches the settled pane",30_000,{ "drawer=${dock.drawer}; glass=${host.glassBoxesForTest.map { it.toList() }}" },compose) {
                    val pane=findTag("tool-drawer")?.second?.boundsInRoot?.translate(-host.surfaceOrigin) ?: return@awaitMain false
                    host.glassBoxesForTest.any { box ->
                        kotlin.math.abs(box[0]-pane.left)<=1f && kotlin.math.abs(box[1]-pane.top)<=1f &&
                            kotlin.math.abs(box[2]-pane.width)<=1f && kotlin.math.abs(box[3]-pane.height)<=1f
                    }
                }
            } finally {
                instrumentation.runOnMainSync { root.viewTreeObserver.removeOnPreDrawListener(observer) }
                compose.mainClock.autoAdvance=originalAdvance
                compose.waitForIdle()
            }
        }
    }
}
