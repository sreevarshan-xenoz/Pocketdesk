package dev.pocketdesk.watchlink

/** What an answer's body holds, written and read in one place. */
object WatchBodies {
    /** The most deck buttons and asks one answer holds: a watch shows a short list. */
    const val MAX_DECK = 8
    const val MAX_ASKS = 3
    const val MAX_LABEL_CHARS = 24
    const val MAX_ASK_CHARS = 120

    class Status(
        /** What the phone calls the computer it works with, or "" for none. */
        val computer: String,
        /** The buttons that are on. */
        val buttons: Set<WatchButton>,
        /** How many asks wait for an answer. */
        val asks: Int,
    )

    fun status(s: Status): ByteArray =
        WatchOut().str(WatchText.clean(s.computer, MAX_LABEL_CHARS))
            .u8(WatchButton.mask(s.buttons))
            .u8(s.asks.coerceIn(0, 255))
            .bytes()

    fun readStatus(body: ByteArray): Status? = runCatching {
        val r = WatchIn(body)
        val name = r.str()
        val mask = r.u8()
        val asks = r.u8()
        r.end()
        Status(name, WatchButton.fromMask(mask), asks)
    }.getOrNull()

    class Deck(val index: Int, val label: String)

    fun deck(list: List<Deck>): ByteArray {
        val shown = list.take(MAX_DECK)
        val out = WatchOut().u8(shown.size)
        for (d in shown) out.u16(d.index).str(WatchText.clean(d.label, MAX_LABEL_CHARS))
        return out.bytes()
    }

    fun readDeck(body: ByteArray): List<Deck>? = runCatching {
        val r = WatchIn(body)
        val n = r.u8()
        if (n > MAX_DECK) throw WatchIn.Malformed()
        val list = List(n) { Deck(r.u16(), r.str()) }
        r.end()
        list
    }.getOrNull()

    fun deckPress(index: Int, label: String): ByteArray = WatchOut().u16(index).str(label).bytes()

    fun readDeckPress(body: ByteArray): Deck? = runCatching {
        val r = WatchIn(body)
        val d = Deck(r.u16(), r.str())
        r.end()
        d
    }.getOrNull()

    class Ask(
        val id: String,
        /** A request that needs the phone's own fingerprint: the watch can only say No to it. */
        val approval: Boolean,
        val text: String,
    )

    fun asks(list: List<Ask>): ByteArray {
        val shown = list.take(MAX_ASKS)
        val out = WatchOut().u8(shown.size)
        for (a in shown) out.str(a.id).u8(if (a.approval) 1 else 0).str(WatchText.clean(a.text, MAX_ASK_CHARS))
        return out.bytes()
    }

    fun readAsks(body: ByteArray): List<Ask>? = runCatching {
        val r = WatchIn(body)
        val n = r.u8()
        if (n > MAX_ASKS) throw WatchIn.Malformed()
        val list = List(n) { Ask(r.str(), r.u8() == 1, r.str()) }
        r.end()
        list
    }.getOrNull()

    fun askDeny(id: String): ByteArray = WatchOut().str(id).bytes()

    fun readAskDeny(body: ByteArray): String? = runCatching {
        val r = WatchIn(body)
        val id = r.str()
        r.end()
        id
    }.getOrNull()

    fun music(command: MusicCommand): ByteArray = byteArrayOf(command.code.toByte())

    fun readMusic(body: ByteArray): MusicCommand? =
        if (body.size == 1) MusicCommand.of(body[0].toInt() and 0xff) else null

    /** A failure's words: a few, plain. */
    fun why(text: String): ByteArray = WatchOut().str(WatchText.clean(text, 48)).bytes()

    fun readWhy(body: ByteArray): String? = runCatching {
        val r = WatchIn(body)
        val s = r.str()
        r.end()
        s
    }.getOrNull()
}
