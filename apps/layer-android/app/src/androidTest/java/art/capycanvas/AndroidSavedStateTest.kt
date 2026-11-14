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
            assertTrue("The notice waits for the launch reveal",host.snapshot!!.getJSONObject("launch").getBoolean("finished"))
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
    @Test fun damagedWorkspaceStoreInstallsItsNewestCopy() {
        val restored = "Your saved workspaces were damaged. Capy Canvas restored an earlier copy."
        val database = releasedStore("damaged",true)
        database.writeBytes("damaged SQLite store".toByteArray())
        val workspaces = database.parentFile!!
        fun copies() = workspaces.listFiles()!!.map {it.name}.filter {it.startsWith("workspaces.") && it.endsWith(".sqlite3") && it!="workspaces.sqlite3"}.toSet()
        val before = copies()
        val theme = startupTheme()
        wakeDevice()
        launchCapy(120_000).use { scenario ->
            val host = revealed(scenario)
            host.awaitMain("the damaged store is replaced by its earlier copy",120_000,{"${brief(host)}; ${host.snapshot?.getJSONObject("state")?.opt("notice")}"}) {
                host.snapshot?.getJSONObject("state")?.objectOrNull("notice")?.optString("text")==restored && findNode(hasLabel(restored))!=null
            }
            val view = host.workspaceManager!!
            assertTrue(brief(host),view.isNull("prompt") && view.isNull("error") && view.isNull("status"))
            assertEquals(0,host.snapshot!!.getJSONObject("state").getJSONObject("notice").getJSONArray("actions").length())
            assertEquals(37.0,host.snapshot!!.getJSONObject("state").getJSONObject("brush").getDouble("diameter"),0.0)
            val after = copies()
            assertEquals(1,after.count {it.startsWith("workspaces.damaged.")})
            assertTrue("the installed copy is consumed: $before $after",before.none {it in after})
            shot("damaged-restored-$theme")
            host.awaitMain("the restored workspaces are saved",30_000) {host.workspaceManager?.let {!it.optBoolean("dirty") && !it.optBoolean("saving")}==true}
        }
        val after = copies()
        launchCapy(120_000).use { scenario ->
            val host = revealed(scenario)
            val view = host.workspaceManager!!
            val state = host.snapshot!!.getJSONObject("state")
            assertTrue("$view",view.isNull("prompt") && view.isNull("error") && state.isNull("notice"))
            assertEquals(37.0,state.getJSONObject("brush").getDouble("diameter"),0.0)
            assertEquals(after,copies())
            shot("damaged-relaunch-$theme")
        }
    }
    @Test fun unreadableEditingEntriesStayQuietAcrossLaunches() {
        fun paletteValue(value: Any?, channel: Boolean = false): Any? = when (value) {
            is org.json.JSONObject -> value.keys().asSequence().associateWith {paletteValue(value.get(it),it=="rgba")}
            is org.json.JSONArray -> (0 until value.length()).map {paletteValue(value.get(it),channel)}
            is Number -> if (channel) value.toFloat() else jsonValue(value)
            else -> value
        }
        val database = releasedStore("editing-entries",false)
        val expected = android.database.sqlite.SQLiteDatabase.openDatabase(database.path,null,android.database.sqlite.SQLiteDatabase.OPEN_READWRITE or android.database.sqlite.SQLiteDatabase.NO_LOCALIZED_COLLATORS).use { db ->
            val editing = db.rawQuery("SELECT json FROM editing WHERE id=1",null).use { cursor -> cursor.moveToFirst(); org.json.JSONObject(cursor.getString(0)) }
            val palettes = editing.getJSONObject("color_library").getJSONArray("palettes")
            val expected = palettes.objects().associate {it.getString("id") to paletteValue(it)}
            palettes.put(obj("id" to "bad"))
            editing.getJSONObject("tool_slots").getJSONObject("choices").put("unknown-slot","unknown-choice")
            db.execSQL("UPDATE editing SET json=? WHERE id=1",arrayOf(editing.toString()))
            expected
        }
        assertTrue("released palettes exist",expected.isNotEmpty())
        val theme = startupTheme()
        wakeDevice()
        for (launch in 1..3) {
            launchCapy(120_000).use { scenario ->
                val host = revealed(scenario)
                val state = host.snapshot!!.getJSONObject("state")
                val view = host.workspaceManager!!
                assertTrue("launch $launch: $view",view.isNull("prompt") && view.isNull("error") && view.isNull("status"))
                assertTrue("launch $launch: ${state.opt("notice")}",state.isNull("notice"))
                val palettes = state.getJSONObject("color_library").getJSONArray("palettes").objects().associate {it.getString("id") to paletteValue(it)}
                for ((id,palette) in expected) assertEquals("launch $launch palette $id",palette,palettes[id])
                assertFalse("bad palette dropped",palettes.containsKey("bad"))
                shot("editing-entries-launch-$launch-$theme")
            }
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
    @Test fun releasedWorkspacesOpenWithoutAsking() {
        val workspaces = listOf("builtin:workspace:painter" to "sketch","builtin:workspace:illustrator" to "paint","builtin:workspace:photographer" to "photo")
        val database = releasedStore("released",false)
        val released = org.json.JSONObject(instrumentation.context.assets.open("web-store-1.0.12.json").bufferedReader().use {it.readText()}).getJSONObject("items")
        fun stored() = android.database.sqlite.SQLiteDatabase.openDatabase(database.path,null,android.database.sqlite.SQLiteDatabase.OPEN_READONLY or android.database.sqlite.SQLiteDatabase.NO_LOCALIZED_COLLATORS).use { db ->
            workspaces.map { (id,_) -> db.rawQuery("SELECT content FROM items WHERE id=?",arrayOf(id)).use {it.moveToFirst();it.getString(0)} }
        }
        android.database.sqlite.SQLiteDatabase.openDatabase(database.path,null,android.database.sqlite.SQLiteDatabase.OPEN_READWRITE or android.database.sqlite.SQLiteDatabase.NO_LOCALIZED_COLLATORS).use { db ->
            for ((id,_) in workspaces) db.execSQL("UPDATE items SET content=? WHERE id=?",arrayOf(released.getJSONObject(id).getJSONObject("entity").getJSONObject("content").toString(),id))
        }
        val before = stored()
        fun toolbars(layout: org.json.JSONObject) = layout.getJSONArray("panels").let { panels -> (0 until panels.length()).map {panels.getJSONObject(it)} }
            .filter {it.getJSONObject("content").getString("kind")=="toolbar"}.associate { panel ->
                val tiles = panel.getJSONObject("content").getJSONArray("tiles").let { tiles -> (0 until tiles.length()).map {tiles.getJSONObject(it)} }
                panel.get("id").toString() to tiles.filter { val control = it.getJSONObject("control"); control.getString("kind") !in listOf("opacity","brush_opacity_slider") && control.optString("command")!="decoration" }.map {it.getLong("id")}
            }
        val theme = startupTheme()
        wakeDevice()
        launchCapy(120_000).use { scenario ->
            val host = revealed(scenario)
            fun quiet(step: String) {
                val view = host.workspaceManager!!
                assertTrue("$step: ${brief(host)}",view.isNull("error") && view.isNull("status") && view.isNull("prompt"))
                assertNull("$step asks nothing",findTag("workspace-error"))
            }
            quiet("startup")
            if (host.snapshot!!.getJSONObject("state").getJSONObject("workspace").optBoolean("zen_mode")) host.drain(obj("type" to "invoke","command" to "zen_mode"))
            for ((id,name) in workspaces) {
                host.awaitMain("$name in the switcher",30_000,{brief(host)}) {findNode(hasTag("workspace-switch-$id"))!=null}
                tap(hasTag("workspace-switch-$id"))
                val history = released.getJSONObject(id).getJSONObject("entity").getJSONObject("content").getJSONObject("history")
                val current = toolbars(history.getJSONObject("revisions").getJSONObject(history.getString("current")).getJSONObject("layout"))
                fun shown() = host.snapshot?.getJSONObject("state")?.getJSONObject("workspace")?.getJSONObject("layout")?.let {toolbars(it)}
                host.awaitMain("$name opens with every released tile this build reads",60_000,{"$current != ${shown()}; ${brief(host)}"}) {
                    host.workspaceManager?.let {it.optString("id")==id && it.optBoolean("ready") && !it.optBoolean("busy")}==true && shown()==current
                }
                quiet(name)
                shot("released-$name-$theme")
            }
            host.awaitMain("opening saves nothing pending",30_000) {host.workspaceManager?.let {!it.optBoolean("dirty") && !it.optBoolean("saving")}==true}
        }
        assertEquals("opening released workspaces rewrites none of them",before,stored())
        launchCapy(120_000).use { scenario ->
            val view = revealed(scenario).workspaceManager!!
            assertEquals(workspaces.last().first,view.optString("id"))
            assertTrue("$view",view.isNull("error") && view.isNull("status") && view.isNull("prompt"))
            shot("released-relaunch-$theme")
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
                assertEquals(BuildConfig.VERSION_NAME,org.json.JSONObject(File(storage.workspaces,"last-version").readText()).getString("version"))
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
