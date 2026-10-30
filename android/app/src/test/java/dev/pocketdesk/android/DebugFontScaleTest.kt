package dev.pocketdesk.android

import android.content.Context
import android.content.ContextWrapper
import android.content.pm.ApplicationInfo
import android.content.res.Configuration
import java.io.File
import java.nio.file.Files
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Test

class DebugFontScaleTest {
    @Test
    fun `reads the scales the checks use`() {
        assertEquals(0.85f, DebugFontScale.parse("0.85\n")!!, 0f)
        assertEquals(1.3f, DebugFontScale.parse(" 1.3 ")!!, 0f)
    }

    @Test
    fun `no file or nonsense keeps the system's`() {
        assertNull(DebugFontScale.parse(null))
        assertNull(DebugFontScale.parse(""))
        assertNull(DebugFontScale.parse("big"))
        assertNull(DebugFontScale.parse("0"))
        assertNull(DebugFontScale.parse("9"))
    }

    @Test
    fun `a release build never reads the file`() {
        val ctx = FontScaleContext(debuggable = false, scale = "1.3")
        assertNull(DebugFontScale.scaleFor(ctx))
        assertSame(ctx, DebugFontScale.wrap(ctx))
        assertNull(ctx.asked)
    }

    @Test
    fun `a debuggable build without the file keeps the system's`() {
        val ctx = FontScaleContext(debuggable = true, scale = null)
        assertNull(DebugFontScale.scaleFor(ctx))
        assertSame(ctx, DebugFontScale.wrap(ctx))
    }

    @Test
    fun `a debuggable build with the file takes its scale`() {
        val ctx = FontScaleContext(debuggable = true, scale = "1.3")
        assertEquals(1.3f, DebugFontScale.scaleFor(ctx)!!, 0f)
        assertSame(ctx.wrapped, DebugFontScale.wrap(ctx))
        assertEquals(1.3f, ctx.asked!!.fontScale, 0f)
    }
}

/** A context with only what the font-scale hook reads. */
private class FontScaleContext(debuggable: Boolean, scale: String?) : ContextWrapper(null) {
    private val dir: File = Files.createTempDirectory("pd-fontscale").toFile().apply { deleteOnExit() }
    private val info = ApplicationInfo().apply { flags = if (debuggable) ApplicationInfo.FLAG_DEBUGGABLE else 0 }
    private val config = Configuration()
    private val res = object : android.content.res.Resources(null, null, null) {
        override fun getConfiguration(): Configuration = config
    }
    val wrapped = ContextWrapper(null)
    var asked: Configuration? = null

    init {
        if (scale != null) File(dir, DebugFontScale.FILE).apply { writeText(scale); deleteOnExit() }
    }

    override fun getApplicationInfo(): ApplicationInfo = info
    override fun getFilesDir(): File = dir
    override fun getResources(): android.content.res.Resources = res
    override fun createConfigurationContext(overrideConfiguration: Configuration): Context {
        asked = overrideConfiguration
        return wrapped
    }
}
