package dev.pocketdesk.android.status

/**
 * The phone's mirror of what a computer allows it (M2a): for each of L7's
 * features, the phone's own switch and Android's grant beside the
 * computer's permission for this phone and its own Settings, in the
 * phone's order. A feature works when both sides allow it.
 */
object AllowedRule {
    /** The computer's two sides of one feature, as it answered them. */
    data class Side(val permission: Boolean, val setting: Boolean)

    enum class Computer { ALLOWED, PERMISSION_OFF, SETTING_OFF, UNKNOWN }

    enum class Phone { ON, OFF, ANDROID_REFUSES }

    data class Row(val feature: String, val phone: Phone, val computer: Computer) {
        val works: Boolean get() = phone == Phone.ON && computer == Computer.ALLOWED

        /** On here, but held back on the computer or by Android: the row to look at. */
        val heldBack: Boolean get() = phone != Phone.OFF && !works
    }

    fun rows(allowed: Map<String, Side>, switches: Map<String, SwitchReport.Switch>): List<Row> =
        SwitchReport.FEATURES.map { feature ->
            val side = allowed[feature]
            // The permission first: it is this phone's own, the Settings every phone's.
            val computer = when {
                side == null -> Computer.UNKNOWN
                !side.permission -> Computer.PERMISSION_OFF
                !side.setting -> Computer.SETTING_OFF
                else -> Computer.ALLOWED
            }
            val switch = switches[feature]
            val phone = when {
                switch == null || !switch.on -> Phone.OFF
                switch.android == false -> Phone.ANDROID_REFUSES
                else -> Phone.ON
            }
            Row(feature, phone, computer)
        }
}
