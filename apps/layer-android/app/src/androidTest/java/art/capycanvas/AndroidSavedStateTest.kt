package art.capycanvas

import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

class AndroidSavedStateTest {
    @get:Rule val device = CapyDeviceRule()
    @Test fun damagedWorkspaceStoreIsKeptAndFreshSettingsSurviveRestart() {
        val context = instrumentation.targetContext
        val storage = AppStorage.of(context)
        val bytes = "damaged SQLite store".toByteArray()
        val theme = androidx.test.platform.app.InstrumentationRegistry.getArguments().getString("theme") ?: "dark"
        assertTrue(context.getSharedPreferences(CanvasHost.preferencesName,0).edit().putString("settings",obj("theme" to theme,"language" to obj("Explicit" to "en")).toString()).commit())
        storage.workspaces.mkdirs()
        File(storage.workspaces,"workspaces.sqlite3").writeBytes(bytes)
        wakeDevice()
        androidx.test.core.app.ActivityScenario.launch(MainActivity::class.java).use { scenario ->
            val host = scenario.activity().host
            host.awaitMain("damaged store adopted",120_000) {host.workspaceManager?.optBoolean("ready")==true && host.snapshot?.getJSONObject("state")?.isNull("notice")==false}
            val notice = host.snapshot!!.getJSONObject("state").getJSONObject("notice")
            assertEquals("Your saved workspaces were damaged. Capy Canvas started new ones.",notice.getString("text"))
            host.awaitMain("damaged notice rendered") {findNode(hasLabel(notice.getString("text")))!=null}
            val committed = CountDownLatch(1)
            instrumentation.runOnMainSync {
                val view = findTag("canvas-notice-text")!!.first.view
                view.viewTreeObserver.registerFrameCommitCallback {committed.countDown()}
                view.invalidate()
            }
            assertTrue("The damaged notice committed a frame",committed.await(5,TimeUnit.SECONDS))
            screenshot("validation/saved-data/damaged-$theme.png")
            val copies = storage.workspaces.listFiles()!!.filter {it.name.startsWith("workspaces.damaged.") && it.extension=="sqlite3"}
            assertEquals(1,copies.size)
            assertArrayEquals(bytes,copies.single().readBytes())
            host.awaitReady(120_000)
            host.drain(obj("type" to "set_brush_size","value" to 43.0))
            host.awaitMain("fresh settings saved",30_000) {host.workspaceManager?.let {!it.optBoolean("dirty") && !it.optBoolean("saving")}==true}
        }
        launchCapy(120_000).use { scenario ->
            val host = scenario.activity().host
            host.awaitMain("fresh store reopened",120_000) {host.surfaceReady}
            val state = host.snapshot!!.getJSONObject("state")
            assertTrue(state.isNull("notice"))
            assertEquals(43.0,state.getJSONObject("brush").getDouble("diameter"),0.0)
            assertArrayEquals(bytes,storage.workspaces.listFiles()!!.single {it.name.startsWith("workspaces.damaged.") && it.extension=="sqlite3"}.readBytes())
        }
    }
    @Test fun resetWorkspacePromptRestoresOrKeepsAndDoesNotRepeat() {
        val context = instrumentation.targetContext
        val theme = androidx.test.platform.app.InstrumentationRegistry.getArguments().getString("theme") ?: "dark"
        assertTrue(context.getSharedPreferences(CanvasHost.preferencesName,0).edit().putString("settings",obj("theme" to theme,"language" to obj("Explicit" to "en")).toString()).commit())
        for (restore in listOf(false,true)) {
            AppStorage.directoryForTest = File(device.root,"restore-$restore")
            val storage = AppStorage.of(context)
            storage.workspaces.mkdirs()
            instrumentation.context.assets.open("workspaces.sqlite3").use { input -> File(storage.workspaces,"workspaces.unreadable.sqlite3").outputStream().use {input.copyTo(it)} }
            wakeDevice()
            launchCapy(120_000).use { scenario ->
                val host = scenario.activity().host
                host.awaitMain("reset restore prompt ($restore)",30_000,{"${host.workspaceManager}; files=${storage.workspaces.list()?.toList()}"}) {host.workspaceManager?.objectOrNull("prompt")!=null && findNode(hasLabel("Keep Current"))!=null}
                val prompt = host.workspaceManager!!.getJSONObject("prompt")
                assertEquals("Restore Workspaces?",prompt.getString("title"))
                assertEquals("An older version of Capy Canvas reset your workspaces, palettes and brush settings. Restoring replaces changes you made after the reset.",prompt.getString("message"))
                assertEquals("Restore",prompt.getString("confirm"))
                assertEquals("Keep Current",prompt.getString("cancel"))
                screenshot("validation/saved-data/restore-$restore-$theme.png")
                tap(hasLabel(if(restore)"Restore" else "Keep Current"))
                host.awaitMain("restore choice adopted",120_000) {host.workspaceManager?.isNull("prompt")==true && host.workspaceManager?.optBoolean("ready")==true && host.snapshot?.getJSONObject("state")?.getJSONObject("brush")?.getDouble("diameter")==if(restore)37.0 else 9.0}
                assertTrue(host.workspaceManager!!.isNull("error"))
                val copies = storage.workspaces.listFiles()!!.filter {it.name.startsWith("workspaces.") && it.name!="workspaces.sqlite3" && it.name!="workspaces.unreadable.sqlite3" && it.extension=="sqlite3"}
                assertEquals(if(restore)2 else 1,copies.size)
                host.drain(obj("type" to "set_brush_size","value" to 43.0))
                host.awaitMain("settings after restore saved",30_000) {host.workspaceManager?.let {!it.optBoolean("dirty") && !it.optBoolean("saving")}==true}
            }
            launchCapy(120_000).use { scenario ->
                val host = scenario.activity().host
                assertTrue(host.workspaceManager!!.isNull("prompt"))
                assertEquals(43.0,host.snapshot!!.getJSONObject("state").getJSONObject("brush").getDouble("diameter"),0.0)
            }
        }
    }
    @Test fun damagedWorkspaceStoreOffersEarlierWorkspacesOnce() {
        val damaged = "Your saved workspaces were damaged. Capy Canvas started new ones."
        releasedStore("damaged",true).writeBytes("damaged SQLite store".toByteArray())
        val theme = startupTheme()
        wakeDevice()
        launchCapy(120_000).use { scenario ->
            val host = revealed(scenario)
            host.awaitMain("the damaged store offers its earlier copy",120_000,{"${brief(host)}; ${host.snapshot?.getJSONObject("state")?.opt("notice")}"}) {
                host.workspaceManager?.objectOrNull("prompt")?.optString("title")=="Restore Earlier Workspaces?" && findTag("workspace-submit")?.second?.find(hasLabel("Restore"))!=null
                    && host.snapshot?.getJSONObject("state")?.objectOrNull("notice")?.optString("text")==damaged && findNode(hasLabel(damaged))!=null
            }
            shot("damaged-prompt-$theme")
            tap(hasTag("workspace-submit"))
            host.awaitMain("the earlier workspaces open",120_000,{brief(host)}) {host.workspaceManager?.let {it.optBoolean("ready") && !it.optBoolean("busy") && it.isNull("prompt") && it.isNull("error")}==true && host.snapshot?.getJSONObject("state")?.getJSONObject("brush")?.getDouble("diameter")==37.0}
            shot("damaged-restored-$theme")
            host.awaitMain("the restored workspaces are saved",30_000) {host.workspaceManager?.let {!it.optBoolean("dirty") && !it.optBoolean("saving")}==true}
        }
        launchCapy(120_000).use { scenario ->
            val host = revealed(scenario)
            val view = host.workspaceManager!!
            val state = host.snapshot!!.getJSONObject("state")
            assertTrue("$view",view.isNull("prompt") && view.isNull("error") && state.isNull("notice"))
            assertEquals(37.0,state.getJSONObject("brush").getDouble("diameter"),0.0)
            shot("damaged-relaunch-$theme")
        }
    }
    private fun revealed(scenario: androidx.test.core.app.ActivityScenario<MainActivity>) = scenario.activity().let { activity ->
        activity.host.awaitMain("launch revealed",60_000) {!activity.launchSplash.nativeActive && findTag("launch-splash")==null}
        activity.host
    }
    private fun shot(name: String) {
        android.os.SystemClock.sleep(600)
        screenshot("validation/saved-data/$name.png")
    }
    private fun brief(host: CanvasHost) = host.workspaceManager?.let {view -> listOf("id","ready","busy","page","selected","primary","error","status","prompt").associateWith {view.opt(it)}}.toString()
    private fun releasedStore(name: String, retained: Boolean): File {
        AppStorage.directoryForTest = File(device.root,name)
        val workspaces = AppStorage.of(instrumentation.targetContext).workspaces.apply {mkdirs()}
        val database = File(workspaces,"workspaces.sqlite3")
        instrumentation.context.assets.open("workspaces.sqlite3").use { input -> database.outputStream().use {input.copyTo(it)} }
        val copies = org.json.JSONArray()
        if (retained) {
            val copy = File(workspaces,"workspaces.${BuildConfig.VERSION_NAME}.${java.util.UUID.randomUUID()}.sqlite3")
            database.copyTo(copy)
            copies.put(obj("id" to copy.name,"bytes" to copy.length()))
        }
        File(workspaces,"last-version").writeText(obj("version" to BuildConfig.VERSION_NAME,"unreadable_identity" to null,"copies" to copies).toString())
        return database
    }
    @Test fun unopenableWorkspaceOffersOnlyItsRecovery() {
        val sketch = "builtin:workspace:painter"
        for (retained in listOf(false,true)) {
            val database = releasedStore("recover-$retained",retained)
            val theme = startupTheme()
            android.database.sqlite.SQLiteDatabase.openDatabase(database.path,null,android.database.sqlite.SQLiteDatabase.OPEN_READWRITE or android.database.sqlite.SQLiteDatabase.NO_LOCALIZED_COLLATORS)
                .use { it.execSQL("UPDATE items SET content='{not json' WHERE id=?",arrayOf(sketch)) }
            val label = if(retained)"Restore Earlier Workspaces…" else "Restore Starting Layout…"
            val variant = if(retained)"earlier" else "starting"
            wakeDevice()
            launchCapy(120_000).use { scenario ->
                val host = revealed(scenario)
                val retry = host.catalog.getJSONObject("native_copy").getJSONObject("header").getString("retry")
                val phone = host.snapshot!!.getJSONObject("layout").objectOrNull("phone") != null
                val surface = if(phone)"workspace-status" else "workspace-error"
                fun view() = host.workspaceManager!!
                fun shows(tag: String, text: String) = findTag(tag)?.second?.find(hasLabel(text)) != null
                fun press(name: String, match: (androidx.compose.ui.semantics.SemanticsNode) -> Boolean) {
                    host.awaitMain(name,30_000,{brief(host)}) {findNode(match)!=null}
                    tap(match)
                }
                fun fail() {
                    if (host.snapshot!!.getJSONObject("state").getJSONObject("workspace").optBoolean("zen_mode")) host.drain(obj("type" to "invoke","command" to "zen_mode"))
                    press("Sketch in the switcher",hasTag("workspace-switch-$sketch"))
                    host.awaitMain("Sketch can't be opened",30_000,{brief(host)}) {host.workspaceManager?.let {!it.optBoolean("busy") && it.objectOrNull("status")?.optBoolean("error")==true}==true}
                    val status = view().getJSONObject("status")
                    assertFalse(status.getBoolean("retry"))
                    assertEquals(label,status.getJSONObject("action").getString("label"))
                    host.awaitMain("only $label is offered",30_000,{brief(host)}) {shows("$surface-action",label) && findTag("$surface-retry")==null && findNode(hasLabel(retry))==null}
                }
                assertTrue(view().isNull("error"))
                assertNotEquals(sketch,view().optString("id"))
                fail()
                shot("recover-$variant-error-$theme")
                if (!phone) {
                    press("Keep Open",hasTag("workspace-error-open"))
                    host.awaitMain("the title bar keeps $label",30_000,{brief(host)}) {findTag("workspace-error")==null && shows("workspace-switcher-action",label) && findNode(hasLabel(retry))==null}
                    shot("recover-$variant-header-$theme")
                }
                instrumentation.runOnMainSync {host.workspaceInput(obj("type" to "open","page" to "workspaces"))}
                press("Sketch in Manage Workspaces",hasTag("workspace-row-$sketch"))
                host.awaitMain("selecting Sketch offers $label",30_000,{brief(host)}) {host.workspaceManager?.let {it.optString("selected")==sketch && !it.optBoolean("loading") && !it.optBoolean("busy") && it.optString("primary")==label}==true && shows("workspace-confirm",label)}
                shot("recover-$variant-manager-$theme")
                press("Cancel",hasTag("workspace-cancel"))
                host.awaitMain("Manage Workspaces closes",30_000) {host.workspaceManager?.isNull("page")==true && findTag("workspace-manager")==null}
                fail()
                press(label,hasTag("$surface-action"))
                host.awaitMain("$label asks first",30_000,{brief(host)}) {host.workspaceManager?.objectOrNull("prompt")!=null && shows("workspace-submit","Restore")}
                assertEquals(if(retained)"Restore Earlier Workspaces?" else "Restore Starting Layout",view().getJSONObject("prompt").getString("title"))
                shot("recover-$variant-prompt-$theme")
                press("Restore",hasTag("workspace-submit"))
                host.awaitMain("$label restores the workspaces",120_000,{brief(host)}) {host.workspaceManager?.let {it.optBoolean("ready") && !it.optBoolean("busy") && it.isNull("prompt") && it.isNull("status")}==true}
                host.awaitMain("$label opens Sketch and saves it",60_000,{brief(host)}) {host.workspaceManager?.let {it.optString("id")==sketch && !it.optBoolean("busy") && it.isNull("status") && !it.optBoolean("dirty") && !it.optBoolean("saving")}==true}
                shot("recover-$variant-restored-$theme")
            }
            launchCapy(120_000).use { scenario ->
                val view = revealed(scenario).workspaceManager!!
                assertEquals(sketch,view.optString("id"))
                assertTrue(view.isNull("error") && view.isNull("status"))
                shot("recover-$variant-relaunch-$theme")
            }
        }
    }
    private fun tap(match: (androidx.compose.ui.semantics.SemanticsNode) -> Boolean) {
        lateinit var target: Pair<android.view.View,androidx.compose.ui.geometry.Offset>
        instrumentation.runOnMainSync { findNode(match)!!.let { (owner,node) -> target = owner.view to node.boundsInRoot.center } }
        val at = android.os.SystemClock.uptimeMillis()
        for (action in listOf(android.view.MotionEvent.ACTION_DOWN,android.view.MotionEvent.ACTION_UP)) {
            val event = motion(android.view.MotionEvent.TOOL_TYPE_FINGER,action,target.second,at)
            try {instrumentation.runOnMainSync {target.first.dispatchTouchEvent(event)}} finally {event.recycle()}
            android.os.SystemClock.sleep(40)
        }
    }
    @Test fun releasedStoreKeepsEditingPreferencesAndPresetsAfterRestart() {
        val context = instrumentation.targetContext
        val storage = AppStorage.of(context)
        fun copy(name: String, destination: File) {
            destination.parentFile!!.mkdirs()
            instrumentation.context.assets.open(name).use { input -> destination.outputStream().use { input.copyTo(it) } }
        }
        copy("workspaces.sqlite3", File(storage.workspaces,"workspaces.sqlite3"))
        copy("export-presets",storage.exportPresets)
        val settings = instrumentation.context.assets.open("settings.json").bufferedReader().use { it.readText() }
        assertTrue(context.getSharedPreferences(CanvasHost.preferencesName,0).edit().putString("settings",settings).commit())
        wakeDevice()
        var diameter = 37.0
        for (theme in listOf("light","dark")) {
            launchCapy(120_000).use { scenario ->
                val host = scenario.activity().host
                host.drain(obj("type" to "set_theme","theme" to theme))
                host.awaitMain("first restored canvas",120_000) { host.surfaceReady }
                val state = host.snapshot!!.getJSONObject("state")
                assertEquals(diameter,state.getJSONObject("brush").getDouble("diameter"),0.0)
                assertEquals(1.5,state.getJSONObject("settings").getDouble("pan_speed"),0.0)
                assertTrue(state.isNull("notice"))
                assertTrue(state.getJSONObject("color_library").getJSONArray("palettes").objects().any { palette ->
                    palette.getString("name")=="Release palette" && palette.getJSONArray("swatches").objects().any { it.getString("name")=="Release blue" }
                })
                assertEquals(BuildConfig.VERSION_NAME,File(storage.workspaces,"last-version").readText())
                assertEquals(1,storage.workspaces.listFiles()!!.count { it.name.startsWith("workspaces.earlier.") && it.extension=="sqlite3" })
                assertTrue(host.workspaceManager!!.isNull("error"))
                val presets = runBlocking { ColorPreferencesStore.presets(context,obj("space" to "Srgb","depth" to "U8"),obj("type" to "list")) }
                assertTrue(presets.getJSONArray("names").values().contains("Release delivery"))
                diameter = 43.0
                host.drain(obj("type" to "set_brush_size","value" to diameter))
                host.awaitMain("editing save",30_000) { host.workspaceManager?.let { !it.optBoolean("dirty") && !it.optBoolean("saving") } == true }
            }
        }
    }
}
