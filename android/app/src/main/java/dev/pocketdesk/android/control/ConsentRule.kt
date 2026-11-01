package dev.pocketdesk.android.control

/**
 * Hands-free: accepting Android's own screen-sharing question for the
 * phone's own computer, when the owner turned that on ("Accept screen
 * sharing for my computers", off by default). Pure, for JVM tests;
 * [ConsentPress] does it through the control service.
 *
 * What it may press is narrow on purpose. Only while Pocketdesk itself has
 * just launched the question for an open its own computer asked (and for
 * [APPEAR_MS] after); only in the one window whose activity is exactly the
 * one Android resolved Pocketdesk's question intent to (its package and
 * class, named by the window-state event, and then only that window by its
 * id); only when that window's text names Pocketdesk as a whole word; and
 * only its one confirm button, whose words must be a known confirm word of
 * the phone's language. Where the window shows a one-app-or-entire-screen
 * chooser, only when it reads "entire screen". It never opens a chooser or
 * picks a row, and presses nothing in any other window or app; when no ask
 * is waiting, nothing at all.
 */
object ConsentRule {
    /** How long after Pocketdesk launches the question it may show up. */
    const val APPEAR_MS = 5_000L

    /** How long, once it has shown, the confirm may take. */
    const val STEPS_MS = 8_000L

    /** How often the window is looked at. */
    const val POLL_MS = 250L

    /** The most views looked at in a window. */
    const val MAX_NODES = 150

    /** What became of an arming. */
    const val PRESSED = "pressed"
    const val NOT_FOUND = "not_found"
    const val NOT_ALLOWED = "not_allowed"

    /**
     * Whether the question may be accepted for an open: the owner's opt-in
     * is on ([autoOn]), so is "Let the computer open my apps" ([appsOn]),
     * the computer that asked is the phone's own ([own]) and the control
     * service is bound ([service]).
     */
    fun mayArm(autoOn: Boolean, appsOn: Boolean, own: Boolean, service: Boolean): Boolean =
        autoOn && appsOn && own && service

    /**
     * Whether a window-state event is the question's own window: its
     * package and class are exactly the activity Android resolved the
     * question's intent to ([expectedPackage], [expectedClass]), none of
     * them blank, and not Pocketdesk's own ([own]).
     */
    fun isQuestionWindow(
        eventPackage: String?,
        eventClass: String?,
        expectedPackage: String?,
        expectedClass: String?,
        own: String,
    ): Boolean = !expectedPackage.isNullOrBlank() && !expectedClass.isNullOrBlank() &&
        expectedPackage != own && eventPackage == expectedPackage && eventClass == expectedClass

    /**
     * Whether the window is live at [now]: armed at [armedAt] and, until
     * the question has been seen ([seenAt] null), within [APPEAR_MS]; after
     * it, within [STEPS_MS] of its first sight.
     */
    fun live(armedAt: Long, seenAt: Long?, now: Long): Boolean =
        if (seenAt == null) now - armedAt <= APPEAR_MS else now - seenAt <= STEPS_MS

    /**
     * Whether any of [texts] names the app by its exact [label] as a whole
     * word or phrase (not inside a longer word), case as the system wrote it.
     */
    fun namesApp(texts: List<String>, label: String): Boolean {
        val name = label.trim()
        if (name.length < 3) return false
        val whole = Regex("(?<![\\p{L}\\p{N}])" + Regex.escape(name) + "(?![\\p{L}\\p{N}])")
        return texts.any { whole.containsMatchIn(it) }
    }

    /** A view of the window as the control service saw it. */
    data class Node(
        val id: String?,
        val text: String?,
        val description: String?,
        val className: String?,
        /** It, or a parent within three, takes a click. */
        val clickable: Boolean,
    )

    /** What to do next. */
    sealed class Step {
        /** Nothing yet (or nothing safe): look again; the window ends and the one-tap notification takes over. */
        object Wait : Step()

        /** Press the question's confirm button (the node at [index]). */
        data class Confirm(val index: Int) : Step()
    }

    /** What a chooser reads when "entire screen" is selected. */
    private val ENTIRE = listOf(
        "entire screen", "whole screen", "full screen",
        "pantalla completa", "toda la pantalla", "ganzen bildschirm", "gesamten bildschirm",
        "écran entier", "tout l'écran", "tela inteira", "toda a tela", "schermo intero", "intero schermo",
        "整个屏幕", "整個螢幕", "画面全体", "전체 화면",
    )

    /**
     * The words on the question's confirm button, as stock Android words it,
     * in the languages it is shipped in most. A button that says anything
     * else (an unknown language, "Next", "Cancel") is never pressed: the
     * one-tap notification stands in.
     */
    val CONFIRM_WORDS: Set<String> = setOf(
        "start now", "start", "share screen", "share", "allow", "start recording", "start casting",
        "iniciar ahora", "iniciar", "compartir pantalla", "permitir", "comenzar ahora", "comenzar",
        "jetzt starten", "starten", "bildschirm freigeben", "teilen", "zulassen",
        "démarrer maintenant", "démarrer", "commencer", "commencer maintenant", "partager l'écran", "autoriser",
        "iniciar agora", "compartilhar tela", "permitir",
        "avvia adesso", "avvia", "condividi schermo", "consenti",
        "立即开始", "开始", "共享屏幕", "立即開始", "開始", "分享螢幕", "今すぐ開始", "開始", "画面を共有",
        "지금 시작", "시작", "화면 공유",
        "தொடங்கு", "இப்போது தொடங்கு", "திரையைப் பகிர்",
    )

    private fun words(node: Node): String =
        (node.text ?: node.description).orEmpty().trim().lowercase()

    private fun isChooser(node: Node): Boolean =
        node.className?.contains("Spinner") == true || node.id?.contains("screen_share_mode", ignoreCase = true) == true

    private fun isConfirm(node: Node): Boolean {
        val byId = node.id == "android:id/button1" || node.id?.endsWith(":id/button1") == true ||
            node.id?.endsWith(":id/button_start") == true
        val button = byId || node.className?.contains("Button") == true
        return button && words(node) in CONFIRM_WORDS
    }

    /**
     * What to do next in the question's window [nodes] (already known to be
     * that window, naming Pocketdesk). With a chooser, only a confirm when
     * it reads "entire screen"; one that reads one app, or something not
     * known, is waited on. Without a chooser, the confirm button, when its
     * words are a known confirm word and it can be clicked. Anything else:
     * [Step.Wait].
     */
    fun next(nodes: List<Node>): Step {
        val chooser = nodes.firstOrNull { isChooser(it) }
        if (chooser != null && ENTIRE.none { words(chooser).contains(it) }) return Step.Wait
        val button = nodes.withIndex().firstOrNull { it.value.clickable && isConfirm(it.value) }
            ?: return Step.Wait
        return Step.Confirm(button.index)
    }
}
